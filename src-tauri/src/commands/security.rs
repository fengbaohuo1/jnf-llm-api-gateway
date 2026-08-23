use tauri::State;

use crate::commands::DbState;
use crate::models::SecurityLog;

use super::pool;

/// 查询安全事件日志（可按请求 id 过滤）
#[tauri::command]
pub async fn list_security_logs(
    state: State<'_, DbState>,
    request_id: Option<String>,
) -> Result<Vec<SecurityLog>, String> {
    let mut sql = String::from(
        "SELECT id, request_id, stage, rule_id, category, risk, snippet_masked, created_at FROM security_logs WHERE 1=1",
    );
    let mut rid: Option<String> = None;
    if let Some(r) = request_id {
        let r = r.trim();
        if !r.is_empty() {
            rid = Some(r.to_string());
            sql.push_str(" AND request_id = ?");
        }
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT 500");

    let mut q = sqlx::query_as::<_, SecurityLog>(&sql);
    if let Some(ref r) = rid {
        q = q.bind(r.clone());
    }
    let rows = q.fetch_all(pool(&state)).await.map_err(|e| e.to_string())?;
    Ok(rows)
}
