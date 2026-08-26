use sqlx::SqlitePool;
use tauri::State;

use crate::commands::DbState;
use crate::models::RequestLog;

use super::util::{new_id, now_iso, today_iso};
use super::pool;

#[tauri::command]
pub async fn list_requests(
    state: State<'_, DbState>,
    keyword: Option<String>,
    channel: Option<String>,
    model: Option<String>,
) -> Result<Vec<RequestLog>, String> {
    let mut sql = String::from(
        "SELECT id, timestamp, channel_id, channel_name, key_id, key_name, model, status, latency_ms, prompt_tokens, completion_tokens, total_tokens, cache_hit_tokens, reasoning_tokens, cost, error FROM requests WHERE 1=1",
    );
    let mut kw: Option<String> = None;
    if let Some(k) = keyword {
        let k = k.trim();
        if !k.is_empty() {
            kw = Some(format!("%{k}%"));
            sql.push_str(" AND (id LIKE ?1 OR channel_name LIKE ?1 OR key_name LIKE ?1 OR model LIKE ?1)");
        }
    }
    let mut ch: Option<String> = None;
    if let Some(c) = channel {
        if c != "全部" {
            ch = Some(c);
            sql.push_str(" AND channel_name = ?");
        }
    }
    let mut md: Option<String> = None;
    if let Some(m) = model {
        if m != "全部" {
            md = Some(m);
            sql.push_str(" AND model = ?");
        }
    }
    sql.push_str(" ORDER BY timestamp DESC LIMIT 500");

    let mut q = sqlx::query_as::<_, RequestLog>(&sql);
    if let Some(ref k) = kw {
        q = q.bind(k.clone());
    }
    if let Some(ref c) = ch {
        q = q.bind(c.clone());
    }
    if let Some(ref m) = md {
        q = q.bind(m.clone());
    }
    let rows = q.fetch_all(pool(&state)).await.map_err(|e| e.to_string())?;
    Ok(rows)
}

#[tauri::command]
pub async fn insert_request(
    state: State<'_, DbState>,
    channel_id: String,
    channel_name: String,
    key_id: Option<String>,
    key_name: Option<String>,
    model: String,
    status: i64,
    latency_ms: i64,
    prompt_tokens: i64,
    completion_tokens: i64,
    cache_hit_tokens: i64,
    reasoning_tokens: i64,
    cost: f64,
    error: Option<String>,
) -> Result<(), String> {
    log_request(
        pool(&state),
        &channel_id,
        &channel_name,
        key_id.as_deref(),
        key_name.as_deref(),
        &model,
        status,
        latency_ms,
        prompt_tokens,
        completion_tokens,
        cache_hit_tokens,
        reasoning_tokens,
        cost,
        error.as_deref(),
    )
    .await
    .map(|_| ())
}

/// 记账：写 requests 日志 + 累加 daily_stats + 扣减 keys.used_tokens。
/// 转发代理（proxy 模块）与 `insert_request` 命令共用，返回新请求 id 供安全日志关联。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn log_request(
    pool: &SqlitePool,
    channel_id: &str,
    channel_name: &str,
    key_id: Option<&str>,
    key_name: Option<&str>,
    model: &str,
    status: i64,
    latency_ms: i64,
    prompt_tokens: i64,
    completion_tokens: i64,
    cache_hit_tokens: i64,
    reasoning_tokens: i64,
    cost: f64,
    error: Option<&str>,
) -> Result<String, String> {
    let id = new_id("req");
    let ts = now_iso();
    let total = prompt_tokens + completion_tokens;
    let p = pool;
    sqlx::query(
        "INSERT INTO requests (id, timestamp, channel_id, channel_name, key_id, key_name, model, status, latency_ms, prompt_tokens, completion_tokens, total_tokens, cache_hit_tokens, reasoning_tokens, cost, error) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
    )
    .bind(&id).bind(&ts).bind(channel_id).bind(channel_name).bind(key_id).bind(key_name).bind(model).bind(status).bind(latency_ms).bind(prompt_tokens).bind(completion_tokens).bind(total).bind(cache_hit_tokens).bind(reasoning_tokens).bind(cost).bind(error)
    .execute(p)
    .await
    .map_err(|e| e.to_string())?;

    upsert_daily(p, &today_iso(), 1, total, cost, latency_ms).await?;
    if let Some(kid) = key_id {
        sqlx::query("UPDATE keys SET used_tokens = used_tokens + ?1 WHERE id = ?2")
            .bind(total)
            .bind(kid)
            .execute(p)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(id)
}

pub(crate) async fn upsert_daily(
    pool: &SqlitePool,
    date: &str,
    reqs: i64,
    tokens: i64,
    cost: f64,
    latency: i64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO daily_stats (date, requests, tokens, cost, avg_latency_ms) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(date) DO UPDATE SET
           requests = requests + ?2,
           tokens = tokens + ?3,
           cost = cost + ?4,
           avg_latency_ms = (avg_latency_ms * requests + ?5) / (requests + ?2)",
    )
    .bind(date).bind(reqs).bind(tokens).bind(cost).bind(latency)
    .execute(pool).await.map_err(|e| e.to_string())?;
    Ok(())
}
