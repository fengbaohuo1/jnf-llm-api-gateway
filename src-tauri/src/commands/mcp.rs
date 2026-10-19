//! MCP 服务管理命令：状态、工具清单与开关、令牌、运行配置、调用记录、调试台。
//!
//! 设计要点：
//! - 配置全部落在 settings 键（`settings.json` 双写，文件权威），host/port 改动
//!   需重启服务，其余键（令牌/只读/单工具开关）**即时生效**；
//! - 令牌明文只在轮换时返回一次，之后只给掩码（与渠道密钥同一处理习惯）；
//! - 调试台走后端同一个 `Tools::run_tool`，因此只读模式/单工具开关/审计/调用
//!   记录与外部客户端完全一致——所见即所得。

use serde_json::Value;
use tauri::State;

use crate::commands::DbState;
use crate::services::mcp::config::{McpConfig, ALLOWED_HOSTS};
use crate::services::mcp::protocol::ToolSet;
use crate::services::mcp::tools::{self, McpToolInfo};
use crate::services::mcp::{list_calls, record_call, McpCall, McpService};
use crate::services::ServiceRegistry;

use super::pool;
use super::settings::persist_setting;

/// MCP 服务状态快照（MCP 服务页顶部）
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    /// 服务启用开关（services.mcp.enabled）
    pub enabled: bool,
    /// 进程内 HTTP 是否在跑（running）
    pub running: bool,
    pub host: String,
    /// 配置端口（0 = 随机）
    pub configured_port: u16,
    /// 实际监听端口
    pub port: u16,
    pub endpoint: String,
    pub token_set: bool,
    pub token_masked: Option<String>,
    pub read_only: bool,
    pub tool_total: usize,
    pub tool_enabled: usize,
    /// 服务 init 失败原因
    pub error: Option<String>,
}

/// 调试台一次调用的结果
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCallResult {
    pub ok: bool,
    pub result: Option<String>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

fn mcp_service(registry: &ServiceRegistry) -> Result<std::sync::Arc<dyn crate::services::Service>, String> {
    registry.get("mcp").ok_or_else(|| "MCP 服务不可用".to_string())
}

/// 从服务实例取具体类型（调用方需先 clone Arc 保活）
fn as_mcp(svc: &std::sync::Arc<dyn crate::services::Service>) -> Option<&McpService> {
    svc.as_any().downcast_ref::<McpService>()
}

/// MCP 服务状态与配置（含实际监听端口）
#[tauri::command]
pub async fn mcp_status(
    state: State<'_, DbState>,
    registry: State<'_, ServiceRegistry>,
) -> Result<McpStatus, String> {
    let cfg = McpConfig::load(pool(&state)).await;
    let info = registry.list().into_iter().find(|s| s.id == "mcp");
    let svc = mcp_service(&registry).ok();
    let port = svc
        .as_ref()
        .and_then(|s| as_mcp(s))
        .and_then(|s| s.bound_port())
        .unwrap_or(cfg.port);
    let running = info.as_ref().map(|i| i.status == "running").unwrap_or(false);
    let tools = tools::tool_infos(&cfg);
    Ok(McpStatus {
        enabled: info.as_ref().map(|i| i.enabled).unwrap_or(false),
        running,
        host: cfg.host.clone(),
        configured_port: cfg.port,
        port,
        endpoint: cfg.endpoint(port),
        token_set: cfg.token.is_some(),
        token_masked: cfg.token_masked(),
        read_only: cfg.read_only,
        tool_total: tools.len(),
        tool_enabled: tools.iter().filter(|t| t.enabled).count(),
        error: info.and_then(|i| i.error),
    })
}

/// 工具清单（含类别、参数说明、启用状态与原始 inputSchema）
#[tauri::command]
pub async fn mcp_list_tools(state: State<'_, DbState>) -> Result<Vec<McpToolInfo>, String> {
    let cfg = McpConfig::load(pool(&state)).await;
    Ok(tools::tool_infos(&cfg))
}

/// 单工具开关（即时生效，无需重启服务）
#[tauri::command]
pub async fn mcp_set_tool_enabled(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    if !tools::known_names().contains(&name.as_str()) {
        return Err(format!("未知工具: {name}"));
    }
    let key = format!("services.mcp.tools.{name}.enabled");
    persist_setting(&app, pool(&state), "mcp_tool", &key, if enabled { "true" } else { "false" }).await
}

/// 保存监听地址 / 端口 / 只读模式；host、port 变更需重启服务生效
#[tauri::command]
pub async fn mcp_save_config(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    registry: State<'_, ServiceRegistry>,
    host: String,
    port: i64,
    read_only: bool,
) -> Result<bool, String> {
    let host = host.trim().to_string();
    if !ALLOWED_HOSTS.contains(&host.as_str()) {
        return Err(format!("监听地址只支持 {}（仅本机）或 0.0.0.0（局域网）", ALLOWED_HOSTS[0]));
    }
    if !(0..=65535).contains(&port) {
        return Err("端口需在 0~65535 之间（0 = 随机端口）".to_string());
    }
    let db = pool(&state);
    let before = McpConfig::load(db).await;
    // 与服务启动时同一安全基线：开 0.0.0.0 必须先设令牌
    if host == "0.0.0.0" && before.token.is_none() {
        return Err("监听 0.0.0.0（局域网可访问）前请先生成 MCP 访问令牌".to_string());
    }
    persist_setting(&app, db, "mcp", "services.mcp.host", &host).await?;
    persist_setting(&app, db, "mcp", "services.mcp.port", &port.to_string()).await?;
    persist_setting(
        &app,
        db,
        "mcp",
        "services.mcp.readOnly",
        if read_only { "true" } else { "false" },
    )
    .await?;
    // host/port 是 init 时绑定的一次性参数 → 变更需要重启
    let restart_required = before.host != host || before.port != port as u16;
    let _ = registry;
    Ok(restart_required)
}

/// 生成新访问令牌（返回明文一次；此后只显示掩码）。空字符串或 null = 关闭鉴权。
#[tauri::command]
pub async fn mcp_rotate_token(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
) -> Result<String, String> {
    let token = format!("mcp_{}", uuid::Uuid::new_v4().simple());
    persist_setting(&app, pool(&state), "mcp", "services.mcp.token", &token).await?;
    Ok(token)
}

/// 清除访问令牌（此后不再校验 Bearer；仅建议在回环监听时使用）
#[tauri::command]
pub async fn mcp_clear_token(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
) -> Result<(), String> {
    persist_setting(&app, pool(&state), "mcp", "services.mcp.token", "").await
}

/// 最近调用记录（倒序；不包含调用参数）
#[tauri::command]
pub async fn mcp_list_calls(
    state: State<'_, DbState>,
    limit: Option<i64>,
) -> Result<Vec<McpCall>, String> {
    list_calls(pool(&state), limit.unwrap_or(50)).await
}

/// 调试台：按外部客户端同样的路径执行工具（只读模式/开关/审计/记录一致）
#[tauri::command]
pub async fn mcp_call_tool(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    registry: State<'_, ServiceRegistry>,
    name: String,
    args: Option<Value>,
) -> Result<McpCallResult, String> {
    let db = pool(&state);
    let live_tools = mcp_service(&registry)
        .ok()
        .and_then(|svc| as_mcp(&svc).and_then(|s| s.tools()));
    let tools = match live_tools {
        Some(t) => t,
        None => {
            // 服务未运行也能调试：按同一索引目录临时构造（共享库与索引文件）
            let dir = crate::config::data_dir(&app)?.join("kb_index");
            std::sync::Arc::new(tools::Tools::new(db.clone(), dir))
        }
    };
    let args = args.unwrap_or_else(|| serde_json::json!({}));
    let started = std::time::Instant::now();
    let outcome = tools.run_tool(&name, &args).await;
    let duration_ms = started.elapsed().as_millis() as u64;
    record_call(
        db,
        "tools/call",
        Some(&name),
        outcome.is_ok(),
        outcome.as_ref().err().map(String::as_str),
        duration_ms as i64,
        Some("app/ui-playground"),
    )
    .await;
    Ok(match outcome {
        Ok(text) => McpCallResult {
            ok: true,
            result: Some(text),
            error: None,
            duration_ms,
        },
        Err(e) => McpCallResult {
            ok: false,
            result: None,
            error: Some(e),
            duration_ms,
        },
    })
}
