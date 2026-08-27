mod adapters;
mod commands;
mod db;
mod models;
mod proxy;
mod scheduler;
mod security;

use std::sync::Arc;

use tauri::Manager;
use tauri_plugin_autostart::MacosLauncher;

use commands::DbState;

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--flag1", "--flag2"])))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            // 数据库放在应用数据目录，保证跨平台可写。
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir).ok();
            let db_path = dir.join("llm_gateway.db");
            let pool = tauri::async_runtime::block_on(db::init(&db_path))?;
            app.manage(DbState(pool.clone()));

            // 启动转发代理（本地 HTTP 服务，端口取 settings server.port）
            // 安全扫描器：从 DB 加载规则并定时刷新（规则改动 30s 内生效）
            let scanner_pool = pool.clone();
            tauri::async_runtime::spawn(async move {
                proxy::serve(pool, Arc::new(security::RefreshScanner::new(scanner_pool))).await;
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // channels
            commands::channels::list_channels,
            commands::channels::create_channel,
            commands::channels::update_channel,
            commands::channels::delete_channel,
            commands::channels::toggle_channel_status,
            commands::channels::test_channel,
            // keys
            commands::keys::list_keys,
            commands::keys::create_key,
            commands::keys::update_key,
            commands::keys::rotate_key,
            commands::keys::delete_key,
            commands::keys::toggle_key_status,
            // requests
            commands::requests::list_requests,
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
            // daily stats
            commands::stats::list_daily_stats,
            commands::stats::today_stats,
            // demo
            greet
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
