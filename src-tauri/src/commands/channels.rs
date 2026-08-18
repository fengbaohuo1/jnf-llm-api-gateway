use tauri::State;

use crate::commands::DbState;
use crate::models::Channel;

use super::{mask, new_id, now_iso, pool};

#[tauri::command]
pub async fn list_channels(state: State<'_, DbState>) -> Result<Vec<Channel>, String> {
    let rows = sqlx::query_as::<_, (String, String, String, String, String, String, String, Option<i64>, String)>(
        "SELECT id, name, type, base_url, model, status, api_key_encrypted, latency_ms, created_at FROM channels ORDER BY created_at DESC",
    )
    .fetch_all(pool(&state))
    .await
    .map_err(|e| e.to_string())?;

    Ok(rows
        .into_iter()
        .map(|r| Channel {
            id: r.0,
            name: r.1,
            type_: r.2,
            base_url: r.3,
            model: r.4,
            status: r.5,
            api_key_masked: mask(&r.6),
            latency_ms: r.7,
            created_at: r.8,
        })
        .collect())
}

#[tauri::command]
pub async fn create_channel(
    state: State<'_, DbState>,
    name: String,
    type_: String,
    base_url: String,
    model: String,
    api_key: String,
    status: String,
) -> Result<Channel, String> {
    let id = new_id("ch");
    let created = now_iso();
    sqlx::query(
        "INSERT INTO channels (id, name, type, base_url, model, status, api_key_encrypted, latency_ms, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,NULL,?8,?8)",
    )
    .bind(&id).bind(&name).bind(&type_).bind(&base_url).bind(&model).bind(&status).bind(&api_key).bind(&created)
    .execute(pool(&state))
    .await
    .map_err(|e| e.to_string())?;

    Ok(Channel {
        id,
        name,
        type_,
        base_url,
        model,
        status,
        api_key_masked: mask(&api_key),
        latency_ms: None,
        created_at: created,
    })
}

#[tauri::command]
pub async fn update_channel(
    state: State<'_, DbState>,
    id: String,
    name: String,
    type_: String,
    base_url: String,
    model: String,
    status: String,
    api_key: String,
) -> Result<(), String> {
    // 前端编辑时提交完整表单，直接全量更新（api_key 传空字符串表示不改动密钥）。
    sqlx::query(
        "UPDATE channels SET name = ?2, type = ?3, base_url = ?4, model = ?5, status = ?6,
                api_key_encrypted = CASE WHEN ?7 = '' THEN api_key_encrypted ELSE ?7 END,
                updated_at = ?8
         WHERE id = ?1",
    )
    .bind(id)
    .bind(name)
    .bind(type_)
    .bind(base_url)
    .bind(model)
    .bind(status)
    .bind(api_key)
    .bind(now_iso())
    .execute(pool(&state))
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn delete_channel(state: State<'_, DbState>, id: String) -> Result<(), String> {
    sqlx::query("DELETE FROM channels WHERE id = ?1")
        .bind(id)
        .execute(pool(&state))
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn toggle_channel_status(state: State<'_, DbState>, id: String) -> Result<(), String> {
    sqlx::query("UPDATE channels SET status = CASE WHEN status = 'active' THEN 'disabled' ELSE 'active' END, updated_at = ?2 WHERE id = ?1")
        .bind(id)
        .bind(now_iso())
        .execute(pool(&state))
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn test_channel(state: State<'_, DbState>, id: String, ok: bool) -> Result<(), String> {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    let latency: Option<i64> = if ok { Some((t % 600 + 300) as i64) } else { None };
    sqlx::query("UPDATE channels SET status = ?2, latency_ms = ?3, updated_at = ?4 WHERE id = ?1")
        .bind(id)
        .bind(if ok { "active" } else { "error" })
        .bind(latency)
        .bind(now_iso())
        .execute(pool(&state))
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}
