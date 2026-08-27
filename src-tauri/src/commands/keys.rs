use tauri::State;

use crate::commands::DbState;
use crate::models::{ApiKey, CreatedKey};

use super::util::{gen_key_secret, hash_key, new_id, now_iso};
use super::pool;

#[tauri::command]
pub async fn list_keys(state: State<'_, DbState>) -> Result<Vec<ApiKey>, String> {
    let rows = sqlx::query_as::<_, (String, String, String, String, i64, i64, Option<String>, String)>(
        "SELECT id, name, key_prefix, status, quota_limit, used_tokens, expires_at, created_at FROM keys ORDER BY created_at DESC",
    )
    .fetch_all(pool(&state))
    .await
    .map_err(|e| e.to_string())?;

    Ok(rows
        .into_iter()
        .map(|r| ApiKey {
            id: r.0,
            name: r.1,
            key_prefix: r.2,
            status: r.3,
            quota_limit: r.4,
            used_tokens: r.5,
            expires_at: r.6,
            created_at: r.7,
        })
        .collect())
}

#[tauri::command]
pub async fn create_key(
    state: State<'_, DbState>,
    name: String,
    quota_limit: i64,
    expires_at: Option<String>,
) -> Result<CreatedKey, String> {
    let id = new_id("k");
    let key_secret = gen_key_secret();
    let key_hash = hash_key(&key_secret);
    let prefix = key_secret.chars().take(12).collect::<String>();
    let created = now_iso();
    sqlx::query(
        "INSERT INTO keys (id, name, key_hash, key_prefix, status, quota_limit, used_tokens, expires_at, created_at) VALUES (?1,?2,?3,?4,'active',?5,0,?6,?7)",
    )
    .bind(&id).bind(&name).bind(&key_hash).bind(&prefix).bind(quota_limit).bind(&expires_at).bind(&created)
    .execute(pool(&state))
    .await
    .map_err(|e| e.to_string())?;

    Ok(CreatedKey {
        id,
        name,
        key: key_secret,
        key_prefix: prefix,
        quota_limit,
        expires_at,
        created_at: created,
    })
}

#[tauri::command]
pub async fn delete_key(state: State<'_, DbState>, id: String) -> Result<(), String> {
    sqlx::query("DELETE FROM keys WHERE id = ?1")
        .bind(id)
        .execute(pool(&state))
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn toggle_key_status(state: State<'_, DbState>, id: String) -> Result<(), String> {
    sqlx::query("UPDATE keys SET status = CASE WHEN status = 'active' THEN 'disabled' ELSE 'active' END WHERE id = ?1")
        .bind(id)
        .execute(pool(&state))
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 编辑密钥：修改名称 / 配额限制 / 过期时间（密钥本身与用量累计不变）
#[tauri::command]
pub async fn update_key(
    state: State<'_, DbState>,
    id: String,
    name: String,
    quota_limit: i64,
    expires_at: Option<String>,
) -> Result<(), String> {
    sqlx::query("UPDATE keys SET name = ?1, quota_limit = ?2, expires_at = ?3 WHERE id = ?4")
        .bind(&name)
        .bind(quota_limit)
        .bind(&expires_at)
        .bind(&id)
        .execute(pool(&state))
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 轮换密钥：生成新密钥替换旧密钥（旧密钥立即失效），用量累计保留；明文仅此一次返回
#[tauri::command]
pub async fn rotate_key(state: State<'_, DbState>, id: String) -> Result<CreatedKey, String> {
    let key_secret = gen_key_secret();
    let key_hash = hash_key(&key_secret);
    let prefix = key_secret.chars().take(12).collect::<String>();
    let row: Option<(String, String, i64, Option<String>, String)> = sqlx::query_as(
        "SELECT name, status, quota_limit, expires_at, created_at FROM keys WHERE id = ?1",
    )
    .bind(&id)
    .fetch_optional(pool(&state))
    .await
    .map_err(|e| e.to_string())?;
    let Some((name, _status, quota_limit, expires_at, created_at)) = row else {
        return Err("密钥不存在".into());
    };
    sqlx::query("UPDATE keys SET key_hash = ?1, key_prefix = ?2 WHERE id = ?3")
        .bind(&key_hash)
        .bind(&prefix)
        .bind(&id)
        .execute(pool(&state))
        .await
        .map_err(|e| e.to_string())?;
    Ok(CreatedKey {
        id,
        name,
        key: key_secret,
        key_prefix: prefix,
        quota_limit,
        expires_at,
        created_at,
    })
}
