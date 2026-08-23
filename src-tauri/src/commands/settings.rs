use tauri::State;

use crate::commands::DbState;
use crate::models::Setting;

use super::util::now_iso;
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
pub async fn set_setting(state: State<'_, DbState>, key: String, value: String) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1,?2,?3)
         ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at = ?3",
    )
    .bind(key).bind(value).bind(now_iso())
    .execute(pool(&state)).await.map_err(|e| e.to_string())?;
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
