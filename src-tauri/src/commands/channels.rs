use tauri::State;

use crate::commands::DbState;
use crate::models::Channel;

use super::util::{mask, new_id, now_iso};
use super::pool;

#[tauri::command]
pub async fn list_channels(state: State<'_, DbState>) -> Result<Vec<Channel>, String> {
    let rows = sqlx::query_as::<_, (String, String, String, String, String, String, String, Option<i64>, i64, i64, f64, f64, f64, String)>(
        "SELECT id, name, type, base_url, model, status, api_key_encrypted, latency_ms, priority, weight, price_per_1k_tokens, cache_price_per_1k_tokens, reasoning_price_per_1k_tokens, created_at FROM channels ORDER BY created_at DESC",
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
            priority: r.8,
            weight: r.9,
            price_per_1k_tokens: r.10,
            cache_price_per_1k_tokens: r.11,
            reasoning_price_per_1k_tokens: r.12,
            created_at: r.13,
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
    priority: i64,
    weight: i64,
    price_per_1k_tokens: f64,
    cache_price_per_1k_tokens: f64,
    reasoning_price_per_1k_tokens: f64,
) -> Result<Channel, String> {
    let id = new_id("ch");
    let created = now_iso();
    sqlx::query(
        "INSERT INTO channels (id, name, type, base_url, model, status, api_key_encrypted, latency_ms, priority, weight, price_per_1k_tokens, cache_price_per_1k_tokens, reasoning_price_per_1k_tokens, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,NULL,?8,?9,?10,?11,?12,?13,?13)",
    )
    .bind(&id).bind(&name).bind(&type_).bind(&base_url).bind(&model).bind(&status).bind(&api_key).bind(priority).bind(weight).bind(price_per_1k_tokens).bind(cache_price_per_1k_tokens).bind(reasoning_price_per_1k_tokens).bind(&created)
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
        priority,
        weight,
        price_per_1k_tokens,
        cache_price_per_1k_tokens,
        reasoning_price_per_1k_tokens,
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
    priority: i64,
    weight: i64,
    price_per_1k_tokens: f64,
    cache_price_per_1k_tokens: f64,
    reasoning_price_per_1k_tokens: f64,
) -> Result<(), String> {
    // 前端编辑时提交完整表单，直接全量更新（api_key 传空字符串表示不改动密钥）。
    sqlx::query(
        "UPDATE channels SET name = ?2, type = ?3, base_url = ?4, model = ?5, status = ?6,
                api_key_encrypted = CASE WHEN ?7 = '' THEN api_key_encrypted ELSE ?7 END,
                priority = ?8, weight = ?9, price_per_1k_tokens = ?10, cache_price_per_1k_tokens = ?11, reasoning_price_per_1k_tokens = ?12, updated_at = ?13
         WHERE id = ?1",
    )
    .bind(id)
    .bind(name)
    .bind(type_)
    .bind(base_url)
    .bind(model)
    .bind(status)
    .bind(api_key)
    .bind(priority)
    .bind(weight)
    .bind(price_per_1k_tokens)
    .bind(cache_price_per_1k_tokens)
    .bind(reasoning_price_per_1k_tokens)
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
pub async fn test_channel(state: State<'_, DbState>, id: String) -> Result<serde_json::Value, String> {
    // 1. 从数据库读取渠道配置
    let row: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT type, base_url, api_key_encrypted, model FROM channels WHERE id = ?1",
    )
    .bind(&id)
    .fetch_optional(pool(&state))
    .await
    .map_err(|e| e.to_string())?;

    let Some((ctype, base_url, api_key, model)) = row else {
        return Err("渠道不存在".into());
    };

    // 2. 创建适配器并真实测试
    let adapter = crate::adapters::create_adapter(&ctype, &base_url, &api_key)
        .map_err(|e| e.to_string())?;
    let result = adapter.test().await.map_err(|e| e.to_string())?;

    // 3. 更新渠道状态与延迟
    if result.ok {
        let latency: Option<i64> = result.latency_ms.map(|v| v as i64);
        sqlx::query("UPDATE channels SET status = 'active', latency_ms = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(&id)
            .bind(latency)
            .bind(now_iso())
            .execute(pool(&state))
            .await
            .map_err(|e| e.to_string())?;
    } else {
        sqlx::query("UPDATE channels SET status = 'error', latency_ms = NULL, updated_at = ?2 WHERE id = ?1")
            .bind(&id)
            .bind(now_iso())
            .execute(pool(&state))
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(serde_json::json!({
        "ok": result.ok,
        "latencyMs": result.latency_ms,
        "message": result.message,
        "model": model,
        "protocol": adapter.channel_type(),
    }))
}
