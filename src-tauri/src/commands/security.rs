//! 安全审计命令：安全事件查询、规则管理（CRUD）、鉴权事件、管理操作审计、统计聚合。

use serde_json::Value;
use tauri::State;

use crate::commands::DbState;
use crate::commands::util::{new_id, now_iso, record_admin_audit};
use crate::models::{AdminAuditLog, AuthEvent, SecurityLog, SecurityRule};

use super::pool;

/// 查询安全事件日志（可按请求 id / 风险等级 / 阶段过滤）
#[tauri::command]
pub async fn list_security_logs(
    state: State<'_, DbState>,
    request_id: Option<String>,
    risk: Option<String>,
    stage: Option<String>,
) -> Result<Vec<SecurityLog>, String> {
    let mut sql = String::from(
        "SELECT id, request_id, stage, rule_id, category, risk, snippet_masked, action, created_at FROM security_logs WHERE 1=1",
    );
    let mut rid: Option<String> = None;
    if let Some(r) = request_id {
        let r = r.trim();
        if !r.is_empty() {
            rid = Some(r.to_string());
            sql.push_str(" AND request_id = ?");
        }
    }
    let mut rk: Option<String> = None;
    if let Some(r) = risk {
        let r = r.trim();
        if !r.is_empty() && r != "全部" {
            rk = Some(r.to_string());
            sql.push_str(" AND risk = ?");
        }
    }
    let mut st: Option<String> = None;
    if let Some(s) = stage {
        let s = s.trim();
        if !s.is_empty() && s != "全部" {
            st = Some(s.to_string());
            sql.push_str(" AND stage = ?");
        }
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT 500");

    let mut q = sqlx::query_as::<_, SecurityLog>(&sql);
    if let Some(ref r) = rid {
        q = q.bind(r.clone());
    }
    if let Some(ref r) = rk {
        q = q.bind(r.clone());
    }
    if let Some(ref s) = st {
        q = q.bind(s.clone());
    }
    let rows = q.fetch_all(pool(&state)).await.map_err(|e| e.to_string())?;
    Ok(rows)
}

// ---------------- 规则管理 ----------------

/// 规则列表（全部，含停用）
#[tauri::command]
pub async fn list_security_rules(state: State<'_, DbState>) -> Result<Vec<SecurityRule>, String> {
    let rows = sqlx::query_as::<_, SecurityRule>(
        "SELECT id, category, risk, pattern, match_type, enabled, action, created_at FROM security_rules ORDER BY created_at, id",
    )
    .fetch_all(pool(&state))
    .await
    .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// 新增规则（id 自动生成；regex 类型必须可编译）
#[tauri::command]
pub async fn add_security_rule(
    state: State<'_, DbState>,
    category: String,
    risk: String,
    pattern: String,
    action: String,
    match_type: Option<String>,
) -> Result<SecurityRule, String> {
    let match_type = match_type.unwrap_or_else(|| "regex".to_string());
    if match_type == "regex" && regex::Regex::new(&pattern).is_err() {
        return Err("正则表达式无法编译，请检查语法".into());
    }
    if pattern.trim().is_empty() {
        return Err("匹配内容不能为空".into());
    }
    let id = format!("rule_{}", &new_id("r")[5..]);
    let p = pool(&state);
    sqlx::query(
        "INSERT INTO security_rules (id, category, risk, pattern, match_type, enabled, action, created_at) VALUES (?1,?2,?3,?4,?5,1,?6,?7)",
    )
    .bind(&id)
    .bind(&category)
    .bind(&risk)
    .bind(&pattern)
    .bind(&match_type)
    .bind(&action)
    .bind(now_iso())
    .execute(p)
    .await
    .map_err(|e| e.to_string())?;
    record_admin_audit(p, "create", "security_rule", Some(&id), Some(&format!("{category}/{risk}/{match_type}"))).await;
    Ok(SecurityRule {
        id,
        category,
        risk,
        pattern,
        match_type,
        enabled: true,
        action,
        created_at: now_iso(),
    })
}

/// 更新规则（regex 类型必须可编译）
#[tauri::command]
pub async fn update_security_rule(
    state: State<'_, DbState>,
    id: String,
    category: String,
    risk: String,
    pattern: String,
    action: String,
    match_type: Option<String>,
) -> Result<(), String> {
    let match_type = match_type.unwrap_or_else(|| "regex".to_string());
    if match_type == "regex" && regex::Regex::new(&pattern).is_err() {
        return Err("正则表达式无法编译，请检查语法".into());
    }
    if pattern.trim().is_empty() {
        return Err("匹配内容不能为空".into());
    }
    let p = pool(&state);
    sqlx::query("UPDATE security_rules SET category = ?1, risk = ?2, pattern = ?3, action = ?4, match_type = ?5 WHERE id = ?6")
        .bind(&category)
        .bind(&risk)
        .bind(&pattern)
        .bind(&action)
        .bind(&match_type)
        .bind(&id)
        .execute(p)
        .await
        .map_err(|e| e.to_string())?;
    record_admin_audit(p, "update", "security_rule", Some(&id), None).await;
    Ok(())
}

/// 启停规则
#[tauri::command]
pub async fn toggle_security_rule(state: State<'_, DbState>, id: String) -> Result<(), String> {
    let p = pool(&state);
    sqlx::query("UPDATE security_rules SET enabled = CASE WHEN enabled = 1 THEN 0 ELSE 1 END WHERE id = ?1")
        .bind(&id)
        .execute(p)
        .await
        .map_err(|e| e.to_string())?;
    record_admin_audit(p, "toggle", "security_rule", Some(&id), None).await;
    Ok(())
}

/// 批量设置规则启停状态（事务；审计汇总一条）
#[tauri::command]
pub async fn set_security_rules_enabled(
    state: State<'_, DbState>,
    ids: Vec<String>,
    enabled: bool,
) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let p = pool(&state);
    let mut tx = p.begin().await.map_err(|e| e.to_string())?;
    for id in &ids {
        sqlx::query("UPDATE security_rules SET enabled = ?1 WHERE id = ?2")
            .bind(enabled as i64)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    record_admin_audit(
        p,
        if enabled { "batch_enable" } else { "batch_disable" },
        "security_rule",
        None,
        Some(&format!("{} 条", ids.len())),
    )
    .await;
    Ok(())
}

/// 删除规则
#[tauri::command]
pub async fn delete_security_rule(state: State<'_, DbState>, id: String) -> Result<(), String> {
    let p = pool(&state);
    sqlx::query("DELETE FROM security_rules WHERE id = ?1")
        .bind(&id)
        .execute(p)
        .await
        .map_err(|e| e.to_string())?;
    record_admin_audit(p, "delete", "security_rule", Some(&id), None).await;
    Ok(())
}

// ---------------- 审计查询 ----------------

/// 鉴权事件（成功/失败；最近 limit 条）
#[tauri::command]
pub async fn list_auth_events(
    state: State<'_, DbState>,
    limit: Option<i64>,
) -> Result<Vec<AuthEvent>, String> {
    let lim = limit.unwrap_or(100).max(0);
    let rows = sqlx::query_as::<_, AuthEvent>(
        "SELECT id, key_id, success, reason, created_at FROM auth_events ORDER BY created_at DESC LIMIT ?1",
    )
    .bind(lim)
    .fetch_all(pool(&state))
    .await
    .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// 管理操作审计（最近 limit 条）
#[tauri::command]
pub async fn list_admin_audits(
    state: State<'_, DbState>,
    limit: Option<i64>,
) -> Result<Vec<AdminAuditLog>, String> {
    let lim = limit.unwrap_or(100).max(0);
    let rows = sqlx::query_as::<_, AdminAuditLog>(
        "SELECT id, action, target_type, target_id, detail, created_at FROM admin_audit_logs ORDER BY created_at DESC LIMIT ?1",
    )
    .bind(lim)
    .fetch_all(pool(&state))
    .await
    .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// 安全统计：按风险/阶段/处置聚合 + 近 N 天事件趋势
#[tauri::command]
pub async fn security_stats(state: State<'_, DbState>, days: Option<i64>) -> Result<Value, String> {
    let p = pool(&state);
    let d = days.unwrap_or(7).max(1);
    let since = {
        // 简单计算 N 天前的日期（UTC+8 与 now_iso 一致）
        let today = &now_iso()[..10];
        let mut y: i64 = today[..4].parse().unwrap_or(2025);
        let mut m: i64 = today[5..7].parse().unwrap_or(1);
        let mut day: i64 = today[8..10].parse().unwrap_or(1);
        day -= d - 1;
        while day <= 0 {
            m -= 1;
            if m == 0 {
                m = 12;
                y -= 1;
            }
            day += days_in_month(y, m);
        }
        format!("{y:04}-{m:02}-{day:02}")
    };

    let by_risk: Vec<(String, i64)> = sqlx::query_as(
        "SELECT risk, COUNT(*) FROM security_logs WHERE created_at >= ?1 GROUP BY risk",
    )
    .bind(&since)
    .fetch_all(p)
    .await
    .map_err(|e| e.to_string())?;

    let by_stage: Vec<(String, i64)> = sqlx::query_as(
        "SELECT stage, COUNT(*) FROM security_logs WHERE created_at >= ?1 GROUP BY stage",
    )
    .bind(&since)
    .fetch_all(p)
    .await
    .map_err(|e| e.to_string())?;

    let by_action: Vec<(String, i64)> = sqlx::query_as(
        "SELECT action, COUNT(*) FROM security_logs WHERE created_at >= ?1 GROUP BY action",
    )
    .bind(&since)
    .fetch_all(p)
    .await
    .map_err(|e| e.to_string())?;

    let daily: Vec<(String, i64)> = sqlx::query_as(
        "SELECT substr(created_at, 1, 10) AS d, COUNT(*) FROM security_logs WHERE created_at >= ?1 GROUP BY d ORDER BY d",
    )
    .bind(&since)
    .fetch_all(p)
    .await
    .map_err(|e| e.to_string())?;

    Ok(serde_json::json!({
        "byRisk": by_risk,
        "byStage": by_stage,
        "byAction": by_action,
        "daily": daily,
    }))
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}
