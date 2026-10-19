mod adapters;
mod commands;
mod config;
mod db;
mod models;
mod proxy;
mod scheduler;
mod security;
mod services;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;

use commands::DbState;

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 已有实例时：聚焦主窗口（防止多开导致多个托盘图标/代理端口冲突）
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        // 开机自启时附加 --hidden：启动后直接隐藏到托盘
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            // 数据目录：用户主目录/.llm-api-gateway（常规点目录，配置与数据库同处）
            let dir = config::data_dir(app.handle())?;
            std::fs::create_dir_all(&dir).ok();

            // 一次性迁移：旧版数据在 %APPDATA%/<identifier>/，新位置无数据时自动复制
            if let Ok(old_dir) = app.path().app_data_dir() {
                if !dir.join("llm_gateway.db").exists() && old_dir.join("llm_gateway.db").exists() {
                    let _ = std::fs::copy(old_dir.join("llm_gateway.db"), dir.join("llm_gateway.db"));
                    if old_dir.join("settings.json").exists() {
                        let _ = std::fs::copy(old_dir.join("settings.json"), dir.join("settings.json"));
                    }
                }
            }

            let db_path = dir.join("llm_gateway.db");
            let pool = tauri::async_runtime::block_on(db::init(&db_path))?;

            // 配置文件（settings.json）：存在则导入覆盖 DB（改文件重启生效），
            // 不存在则把当前设置导出生成模板。
            let imported =
                tauri::async_runtime::block_on(config::import_settings_file(&pool, &dir))?;
            if imported == 0 {
                tauri::async_runtime::block_on(config::dump_settings_file(&pool, &dir));
            }
            // 旧版本只有 log.content_enabled（布尔）；补齐新的内容存储级别键并写回文件
            if tauri::async_runtime::block_on(config::ensure_log_store_level(&pool)) {
                tauri::async_runtime::block_on(config::dump_settings_file(&pool, &dir));
            }
            app.manage(DbState(pool.clone()));

            // 扩展服务注册表：装配（开关 services.<id>.enabled，存 settings /
            // settings.json 双写）后由 AppState 托管——保留活实例与运行时状态，
            // 命令可热启停/重启/查询（见 commands::services）。
            let registry = services::ServiceRegistry::new(pool.clone(), dir.clone());
            let boots = tauri::async_runtime::block_on(registry.start());
            for b in &boots {
                if !b.enabled {
                    eprintln!("[services] {}（{}）已停用，跳过装配", b.name, b.id);
                } else if b.ok {
                    eprintln!("[services] {}（{}）装配完成", b.name, b.id);
                } else {
                    eprintln!(
                        "[services] {}（{}）装配失败: {}，已跳过（不影响应用启动）",
                        b.name,
                        b.id,
                        b.error.as_deref().unwrap_or("未知错误")
                    );
                }
            }
            app.manage(registry);

            // 关闭到托盘开关（从设置读取，set_setting 时同步更新）
            let close_to_tray = tauri::async_runtime::block_on(async {
                sqlx::query_scalar::<_, String>(
                    "SELECT value FROM settings WHERE key = 'general.close_to_tray'",
                )
                .fetch_optional(&pool)
                .await
                .ok()
                .flatten()
                .map(|v| v == "true")
                .unwrap_or(true)
            });
            app.manage(commands::AppState {
                close_to_tray: std::sync::atomic::AtomicBool::new(close_to_tray),
            });

            // 系统托盘：左键单击显示窗口，菜单含显示/退出
            let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &quit_item])?;
            let tray = TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().expect("缺少应用图标").clone())
                .tooltip("LLM API 网关")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build(app)?;
            // 托盘图标必须保活（drop 会移除图标），交给 App 状态托管
            app.manage(tray);

            // 开机自启（--hidden）：启动后隐藏主窗口，代理照常运行
            if std::env::args().any(|a| a == "--hidden") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }

            // 启动转发代理（本地 HTTP 服务，端口取 settings server.port）
            // 安全扫描器：从 DB 加载规则并定时刷新（规则改动 30s 内生效）
            let scanner_pool = pool.clone();
            tauri::async_runtime::spawn(async move {
                proxy::serve(pool, Arc::new(security::RefreshScanner::new(scanner_pool))).await;
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            // 关闭按钮 → 隐藏到托盘（进程继续提供代理服务）
            if let WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<commands::AppState>();
                if state.close_to_tray.load(Ordering::Relaxed) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            // channels
            commands::channels::list_channels,
            commands::channels::create_channel,
            commands::channels::update_channel,
            commands::channels::delete_channel,
            commands::channels::toggle_channel_status,
            commands::channels::test_channel,
            commands::channels::fetch_channel_models,
            // keys
            commands::keys::list_keys,
            commands::keys::create_key,
            commands::keys::update_key,
            commands::keys::rotate_key,
            commands::keys::delete_key,
            commands::keys::toggle_key_status,
            // requests
            commands::requests::list_requests,
            commands::requests::get_request_detail,
            commands::requests::insert_request,
            // security
            commands::security::list_security_logs,
            commands::security::list_security_rules,
            commands::security::add_security_rule,
            commands::security::update_security_rule,
            commands::security::toggle_security_rule,
            commands::security::set_security_rules_enabled,
            commands::security::delete_security_rule,
            commands::security::list_auth_events,
            commands::security::list_admin_audits,
            commands::security::security_stats,
            // settings
            commands::settings::list_settings,
            commands::settings::set_setting,
            commands::settings::get_setting,
            commands::settings::reload_settings_file,
            commands::settings::settings_file_info,
            // services（扩展服务状态管理）
            commands::services::list_services,
            commands::services::set_service_enabled,
            commands::services::restart_service,
            // knowledge base（数据模型 / CRUD / 导入）
            commands::kb::kb_list_bases,
            commands::kb::kb_create_base,
            commands::kb::kb_update_base,
            commands::kb::kb_delete_base,
            commands::kb::kb_list_documents,
            commands::kb::kb_import_text,
            commands::kb::kb_import_file,
            commands::kb::kb_delete_document,
            commands::kb::kb_list_chunks,
            // knowledge base（向量化 / HNSW 索引）
            commands::kb::kb_bind_embedding,
            commands::kb::kb_unbind_embedding,
            commands::kb::kb_build_index,
            commands::kb::kb_list_tasks,
            commands::kb::kb_search,
            // knowledge base（RAG 会话问答）
            commands::kb::kb_bind_answer,
            commands::kb::kb_unbind_answer,
            commands::kb::kb_create_session,
            commands::kb::kb_list_sessions,
            commands::kb::kb_delete_session,
            commands::kb::kb_list_messages,
            commands::kb::kb_chat,
            // knowledge base（检索评测）
            commands::kb::kb_eval,
            // MCP 服务管理
            commands::mcp::mcp_status,
            commands::mcp::mcp_list_tools,
            commands::mcp::mcp_set_tool_enabled,
            commands::mcp::mcp_save_config,
            commands::mcp::mcp_rotate_token,
            commands::mcp::mcp_clear_token,
            commands::mcp::mcp_list_calls,
            commands::mcp::mcp_call_tool,
            // daily stats
            commands::stats::list_daily_stats,
            commands::stats::today_stats,
            // demo
            greet
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
