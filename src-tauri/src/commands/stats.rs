use tauri::State;

use crate::commands::DbState;
use crate::models::DailyStat;

use super::util::today_iso;
use super::pool;

#[tauri::command]
pub async fn list_daily_stats(state: State<'_, DbState>, limit: Option<i64>) -> Result<Vec<DailyStat>, String> {
    let lim = limit.unwrap_or(30).max(0);
    let rows = sqlx::query_as::<_, DailyStat>(
        "SELECT date, requests, tokens, cache_hit_tokens, completion_tokens, cost, avg_latency_ms FROM daily_stats ORDER BY date DESC LIMIT ?1",
    )
    .bind(lim)
    .fetch_all(pool(&state))
    .await
    .map_err(|e| e.to_string())?;
    Ok(rows)
}

#[tauri::command]
pub async fn today_stats(state: State<'_, DbState>) -> Result<DailyStat, String> {
    let date = today_iso();
    let row: Option<DailyStat> = sqlx::query_as::<_, DailyStat>("SELECT date, requests, tokens, cache_hit_tokens, completion_tokens, cost, avg_latency_ms FROM daily_stats WHERE date = ?1")
        .bind(&date)
        .fetch_optional(pool(&state))
        .await
        .map_err(|e| e.to_string())?;
    Ok(row.unwrap_or(DailyStat { date, requests: 0, tokens: 0, cache_hit_tokens: 0, completion_tokens: 0, cost: 0.0, avg_latency_ms: 0.0 }))
}
