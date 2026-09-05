//! 服务管理命令（list_services / set_service_enabled / restart_service）。
//! 运行时状态存于进程级 ServiceRegistry（AppState 托管）；启用开关走既有设置
//! 持久化链路（settings 表 + settings.json 双写 + admin 审计）。

use tauri::State;

use crate::services::{enabled_key, ServiceInfo, ServiceRegistry};

use super::pool;
use super::settings::persist_setting;
use super::DbState;

/// 全部服务及运行时状态（UI 服务列表）。
#[tauri::command]
pub async fn list_services(
    registry: State<'_, ServiceRegistry>,
) -> Result<Vec<ServiceInfo>, String> {
    Ok(registry.list())
}

/// 启用/停用服务：先持久化开关（settings 表 + settings.json 双写 + 审计），
/// 再热启停运行态（启用立即 init；重启应用后按键重放，状态一致）。
#[tauri::command]
pub async fn set_service_enabled(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    registry: State<'_, ServiceRegistry>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let key = enabled_key(&id);
    let value = if enabled { "true" } else { "false" };
    persist_setting(&app, pool(&state), "service", &key, value).await?;
    registry.apply_enabled(&id, enabled).await
}

/// 热重启服务（重跑 init；失败后修复可用，无需重启应用）。
#[tauri::command]
pub async fn restart_service(
    registry: State<'_, ServiceRegistry>,
    id: String,
) -> Result<(), String> {
    registry.restart(&id).await
}
