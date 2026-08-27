use std::sync::atomic::Ordering;

use tauri::Manager;
use tauri::State;

use crate::commands::DbState;
use crate::models::Setting;
use crate::{commands::AppState, config};

use super::util::{now_iso, record_admin_audit};
use super::pool;

#[tauri::command]
pub async fn list_settings(state: State<'_, DbState>) -> Result<Vec<Setting>, String> {
    let rows = sqlx::query_as::<_, Setting>("SELECT key, value FROM settings ORDER BY key")
        .fetch_all(pool(&state))
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

#[tauri::command]
pub async fn set_setting(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    key: String,
    value: String,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1,?2,?3)
         ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at = ?3",
    )
    .bind(&key).bind(&value).bind(now_iso())
    .execute(pool(&state)).await.map_err(|e| e.to_string())?;

    // 同步内存标志位（窗口关闭事件用）
    if key == "general.close_to_tray" {
        let st = app.state::<AppState>();
        st.close_to_tray.store(value == "true", Ordering::Relaxed);
    }

    // 双写：设置修改同步写回配置文件（settings.json）
    if let Ok(dir) = app.path().app_config_dir() {
        config::dump_settings_file(pool(&state), &dir).await;
    }

    record_admin_audit(pool(&state), "set", "setting", Some(&key), Some(&value)).await;
    Ok(())
}

#[tauri::command]
pub async fn get_setting(state: State<'_, DbState>, key: String) -> Result<Option<String>, String> {
    let v: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?1")
        .bind(key)
        .fetch_optional(pool(&state))
        .await
        .map_err(|e| e.to_string())?;
    Ok(v)
}

/// 配置文件信息（路径 + 是否存在），设置页展示用
#[tauri::command]
pub async fn settings_file_info(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let path = config::settings_path(&dir);
    Ok(serde_json::json!({
        "path": path.to_string_lossy().to_string(),
        "exists": path.exists(),
    }))
}

/// 从配置文件重新加载：文件覆盖 DB（改完文件点此按钮生效，无需重启）
#[tauri::command]
pub async fn reload_settings_file(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
) -> Result<usize, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let n = config::import_settings_file(pool(&state), &dir).await?;
    // 刷新内存标志位
    let st = app.state::<AppState>();
    if let Ok(Some(v)) = get_setting(state.clone(), "general.close_to_tray".to_string()).await {
        st.close_to_tray.store(v == "true", Ordering::Relaxed);
    }
    record_admin_audit(pool(&state), "reload", "setting", None, Some(&format!("{n} keys"))).await;
    Ok(n)
}
