use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use tauri::State;

use crate::commands::DbState;
use crate::models::{RequestDetail, RequestLog};

use super::util::{new_id, now_iso, today_iso};
use super::pool;

/// 上次执行过期清理的 Unix 秒（进程级，约每 6 小时一次；0 = 尚未执行）
static LAST_PRUNE_AT: AtomicU64 = AtomicU64::new(0);

#[tauri::command]
pub async fn list_requests(
    state: State<'_, DbState>,
    keyword: Option<String>,
    channel: Option<String>,
    model: Option<String>,
    risk: Option<String>,
) -> Result<Vec<RequestLog>, String> {
    let mut sql = String::from(
        "SELECT id, timestamp, channel_id, channel_name, key_id, key_name, model, protocol, stream, client_ua, attempts, risk_level, status, latency_ms, prompt_tokens, completion_tokens, total_tokens, cache_hit_tokens, reasoning_tokens, cost, error FROM requests WHERE 1=1",
    );
    let mut kw: Option<String> = None;
    if let Some(k) = keyword {
        let k = k.trim();
        if !k.is_empty() {
            kw = Some(format!("%{k}%"));
            // 概要列 + 内容明细表（请求体/转发体/响应体/错误体）关键词检索
            sql.push_str(
                " AND (id LIKE ?1 OR channel_name LIKE ?1 OR key_name LIKE ?1 OR model LIKE ?1 OR error LIKE ?1 \
                 OR EXISTS (SELECT 1 FROM request_details d WHERE d.request_id = requests.id \
                 AND (d.request_body LIKE ?1 OR d.upstream_body LIKE ?1 OR d.response_body LIKE ?1 OR d.error_body LIKE ?1)))",
            );
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
    // 风险等级筛选：none（无命中）/ low / medium / high
    let mut rk: Option<String> = None;
    if let Some(r) = risk {
        let r = r.trim().to_ascii_lowercase();
        if !r.is_empty() && r != "全部" && r != "all" {
            rk = Some(r);
            sql.push_str(" AND risk_level = ?");
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
    if let Some(ref r) = rk {
        q = q.bind(r.clone());
    }
    let rows = q.fetch_all(pool(&state)).await.map_err(|e| e.to_string())?;
    Ok(rows)
}

/// 按 ID 读取请求内容明细（大字段按需加载，日志详情查看用）
#[tauri::command]
pub async fn get_request_detail(
    state: State<'_, DbState>,
    id: String,
) -> Result<Option<RequestDetail>, String> {
    let row = sqlx::query_as::<_, RequestDetail>(
        "SELECT request_id, request_body, upstream_body, response_body, error_body FROM request_details WHERE request_id = ?1",
    )
    .bind(&id)
    .fetch_optional(pool(&state))
    .await
    .map_err(|e| e.to_string())?;
    Ok(row)
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
        &RequestLogInput {
            channel_id: Some(&channel_id),
            channel_name: Some(&channel_name),
            key_id: key_id.as_deref(),
            key_name: key_name.as_deref(),
            model: &model,
            status,
            latency_ms,
            prompt_tokens,
            completion_tokens,
            cache_hit_tokens,
            reasoning_tokens,
            cost,
            error: error.as_deref(),
            protocol: "",
            stream: false,
            client_ua: "",
            attempts: 1,
            risk_level: "none",
            details: None,
        },
    )
    .await
    .map(|_| ())
}

/// 一次请求日志的完整输入：概要列 + 上下文列（协议/流式/UA/尝试次数）+ 内容明细
pub(crate) struct RequestLogInput<'a> {
    /// 渠道未定时（调度前失败）为 None → channel 列存 NULL
    pub(crate) channel_id: Option<&'a str>,
    pub(crate) channel_name: Option<&'a str>,
    pub(crate) key_id: Option<&'a str>,
    pub(crate) key_name: Option<&'a str>,
    pub(crate) model: &'a str,
    pub(crate) status: i64,
    pub(crate) latency_ms: i64,
    pub(crate) prompt_tokens: i64,
    pub(crate) completion_tokens: i64,
    pub(crate) cache_hit_tokens: i64,
    pub(crate) reasoning_tokens: i64,
    pub(crate) cost: f64,
    pub(crate) error: Option<&'a str>,
    /// 客户端入口协议：openai / anthropic / responses
    pub(crate) protocol: &'a str,
    /// 是否流式请求
    pub(crate) stream: bool,
    /// 客户端 User-Agent
    pub(crate) client_ua: &'a str,
    /// 第几次尝试（1 = 主渠道一次成功）
    pub(crate) attempts: i64,
    /// 请求命中的最高安全风险：none / low / medium / high（安全扫描关闭时为 none）
    pub(crate) risk_level: &'a str,
    /// 请求/响应内容明细（启用内容记录时由 proxy 传入；None = 只记概要）
    pub(crate) details: Option<RequestDetailInput<'a>>,
}

/// 内容明细输入（大字段，原样落库；启用内容记录时由 proxy 侧组装）
pub(crate) struct RequestDetailInput<'a> {
    pub(crate) request_body: Option<&'a str>,
    pub(crate) upstream_body: Option<&'a str>,
    pub(crate) response_body: Option<&'a str>,
    pub(crate) error_body: Option<&'a str>,
}

/// 记账：写 requests 日志（含上下文列）+ 可选内容明细 + 累加 daily_stats + 扣减 keys.used_tokens。
/// 转发代理（proxy 模块）与 `insert_request` 命令共用，返回新请求 id 供安全日志关联。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn log_request(
    pool: &SqlitePool,
    input: &RequestLogInput<'_>,
) -> Result<String, String> {
    prune_expired_if_due(pool).await;
    let id = new_id("req");
    let ts = now_iso();
    let total = input.prompt_tokens + input.completion_tokens;
    // 请求行 / 内容明细 / daily_stats / 配额用量在同一事务内写入：
    // 任一步失败整体回滚，避免出现「有请求行但没计费」的脏数据
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query(
        "INSERT INTO requests (id, timestamp, channel_id, channel_name, key_id, key_name, model, protocol, stream, client_ua, attempts, risk_level, status, latency_ms, prompt_tokens, completion_tokens, total_tokens, cache_hit_tokens, reasoning_tokens, cost, error) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
    )
    .bind(&id)
    .bind(&ts)
    .bind(input.channel_id)
    .bind(input.channel_name)
    .bind(input.key_id)
    .bind(input.key_name)
    .bind(input.model)
    .bind(input.protocol)
    .bind(input.stream)
    .bind(input.client_ua)
    .bind(input.attempts)
    .bind(input.risk_level)
    .bind(input.status)
    .bind(input.latency_ms)
    .bind(input.prompt_tokens)
    .bind(input.completion_tokens)
    .bind(total)
    .bind(input.cache_hit_tokens)
    .bind(input.reasoning_tokens)
    .bind(input.cost)
    .bind(input.error)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    // 内容明细（独立大字段表；仅在提供了内容时写；列 NOT NULL，None 绑空串）
    if let Some(d) = &input.details {
        let has_any = d.request_body.is_some()
            || d.upstream_body.is_some()
            || d.response_body.is_some()
            || d.error_body.is_some();
        if has_any {
            sqlx::query(
                "INSERT INTO request_details (request_id, request_body, upstream_body, response_body, error_body) VALUES (?1,?2,?3,?4,?5)",
            )
            .bind(&id)
            .bind(d.request_body.unwrap_or(""))
            .bind(d.upstream_body.unwrap_or(""))
            .bind(d.response_body.unwrap_or(""))
            .bind(d.error_body.unwrap_or(""))
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        }
    }

    upsert_daily(&mut tx, &today_iso(), 1, total, input.cache_hit_tokens, input.completion_tokens, input.cost, input.latency_ms).await?;
    if let Some(kid) = input.key_id {
        sqlx::query("UPDATE keys SET used_tokens = used_tokens + ?1 WHERE id = ?2")
            .bind(total)
            .bind(kid)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(id)
}

/// 过期日志清理（settings: general.log_retention，天数；0/未设置 = 永久保留）。
/// 由 log_request 触发，进程内约每 6 小时最多执行一次；删除请求日志 + 内容明细 +
/// 关联安全日志（security_logs 无级联 FK，需先删避免孤儿）。daily_stats/admin/auth 审计保留。
async fn prune_expired_if_due(pool: &SqlitePool) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let last = LAST_PRUNE_AT.load(Ordering::Relaxed);
    if now.saturating_sub(last) < 6 * 3600 {
        return;
    }
    LAST_PRUNE_AT.store(now, Ordering::Relaxed);

    // 读保留天数（静默失败，读不到视为永久保留）
    let retention: Option<String> = sqlx::query_scalar(
        "SELECT value FROM settings WHERE key = 'general.log_retention'",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let Some(days) = retention.and_then(|v| v.trim().parse::<i64>().ok()) else {
        return;
    };
    if days <= 0 {
        return;
    }

    // cutoff = 现在(UTC+8，与 now_iso 同基准) 前 N 天；datetime 修饰符用绑定参数
    let modifier = format!("-{days} days");
    let _ = sqlx::query(
        "DELETE FROM security_logs WHERE request_id IN (SELECT id FROM requests WHERE timestamp < datetime('now','+8 hours',?1))",
    )
    .bind(&modifier)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM requests WHERE timestamp < datetime('now','+8 hours',?1)",
    )
    .bind(&modifier)
    .execute(pool)
    .await;
}

pub(crate) async fn upsert_daily(
    conn: &mut sqlx::SqliteConnection,
    date: &str,
    reqs: i64,
    tokens: i64,
    cache_hit: i64,
    completion: i64,
    cost: f64,
    latency: i64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO daily_stats (date, requests, tokens, cache_hit_tokens, completion_tokens, cost, avg_latency_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(date) DO UPDATE SET
           requests = requests + ?2,
           tokens = tokens + ?3,
           cache_hit_tokens = cache_hit_tokens + ?4,
           completion_tokens = completion_tokens + ?5,
           cost = cost + ?6,
           avg_latency_ms = (avg_latency_ms * requests + ?7) / (requests + ?2)",
    )
    .bind(date)
    .bind(reqs)
    .bind(tokens)
    .bind(cache_hit)
    .bind(completion)
    .bind(cost)
    .bind(latency)
    .execute(&mut *conn)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use sqlx::SqlitePool;

    use super::*;

    /// 带完整迁移的内存库（校验 0011 requests.risk_level 与 0010 的明细表）
    async fn migrated_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    fn base_input<'a>(risk: &'a str, details: Option<RequestDetailInput<'a>>) -> RequestLogInput<'a> {
        RequestLogInput {
            channel_id: None,
            channel_name: None,
            key_id: None,
            key_name: None,
            model: "test-model",
            status: 1,
            latency_ms: 12,
            prompt_tokens: 10,
            completion_tokens: 5,
            cache_hit_tokens: 2,
            reasoning_tokens: 1,
            cost: 0.5,
            error: None,
            protocol: "openai",
            stream: false,
            client_ua: "test",
            attempts: 1,
            risk_level: risk,
            details,
        }
    }

    #[tokio::test]
    async fn log_request_persists_risk_level_and_details() {
        let pool = migrated_pool().await;
        let details = RequestDetailInput {
            request_body: Some("{\"a\":1}"),
            upstream_body: Some("{\"b\":2}"),
            response_body: Some("{\"c\":3}"),
            error_body: None,
        };
        let id = log_request(&pool, &base_input("high", Some(details)))
            .await
            .unwrap();

        let (risk, total): (String, i64) =
            sqlx::query_as("SELECT risk_level, total_tokens FROM requests WHERE id = ?1")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(risk, "high");
        assert_eq!(total, 15);

        let body: (String,) =
            sqlx::query_as("SELECT request_body FROM request_details WHERE request_id = ?1")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(body.0, "{\"a\":1}");

        // 记账同时累加 daily_stats
        let daily: i64 = sqlx::query_scalar("SELECT requests FROM daily_stats")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(daily, 1);
    }

    #[tokio::test]
    async fn log_request_without_details_keeps_summary_only() {
        let pool = migrated_pool().await;
        let id = log_request(&pool, &base_input("none", None)).await.unwrap();

        let risk: (String,) = sqlx::query_as("SELECT risk_level FROM requests WHERE id = ?1")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(risk.0, "none");
        let details: i64 =
            sqlx::query_scalar("SELECT count(*) FROM request_details WHERE request_id = ?1")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(details, 0, "未提供明细时不应写入 request_details");
    }
}
