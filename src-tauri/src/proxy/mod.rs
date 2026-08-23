//! 转发代理：本地 HTTP 服务（OpenAI 兼容入口）。
//!
//! `POST http://127.0.0.1:{port}/v1/chat/completions` 完整处理链路：
//! ① 密钥鉴权 → ② 请求侧安全扫描/脱敏 → ③ 调度器选有序故障转移队列 →
//! ④ 透传转发（原样 JSON，保留 tools 等扩展字段）+ 故障转移循环 →
//! ⑤ 响应侧安全扫描/脱敏 → ⑥ 安全结果合并落库 → ⑦ 记账（requests/daily_stats/used_tokens）
//!
//! 设计取向：
//! - **透传而非字段映射**：请求体原样转发给上游（只改 URL 与 Authorization），
//!   保证 tools / response_format 等 OpenAI 扩展字段 100% 兼容；
//!   适配器层保留给渠道测试与未来的协议转换。
//! - **没有故障就不要负载均衡**：固定走主渠道，失败才按序转移（failover 配置）。
//! - 转移可追溯：每次尝试（含失败）都写 requests 日志。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::Value;
use sqlx::SqlitePool;

use crate::commands::requests::log_request;
use crate::commands::{hash_key, new_id, now_iso};
use crate::scheduler::{select_channels, ChannelChoice};
use crate::security::{Finding, RiskLevel, SecurityScanner};

/// 代理运行状态（axum 注入）
#[derive(Clone)]
pub struct ProxyState {
    pool: SqlitePool,
    scanner: Arc<dyn SecurityScanner>,
    client: reqwest::Client,
}

/// 启动本地 HTTP 服务（地址取自 settings `server.host`/`server.port`，默认 127.0.0.1:8787）。
/// 随应用常驻；`server.enabled=false` 时仍监听但返回 503（设置即时生效）。
pub async fn serve(pool: SqlitePool, scanner: Arc<dyn SecurityScanner>) {
    let (host, port) = read_listen_addr(&pool).await;
    let state = ProxyState {
        pool,
        scanner,
        client: reqwest::Client::builder()
            .timeout(crate::adapters::REQUEST_TIMEOUT)
            .build()
            .expect("failed to build proxy http client"),
    };

    let app = Router::new()
        .route("/v1/chat/completions", post(handle_chat_completion))
        .fallback(handle_not_found)
        .with_state(state);

    let addr = format!("{host}:{port}");
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[proxy] 监听 {addr} 失败: {e}");
            return;
        }
    };
    eprintln!("[proxy] 转发代理已启动: http://{addr}/v1/chat/completions");
    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("[proxy] 服务异常退出: {e}");
    }
}

// ---------------- 设置 ----------------

/// 代理相关设置（每次请求从 settings 读取，改设置即时生效）
#[derive(Debug)]
struct ProxySettings {
    server_enabled: bool,
    failover_enabled: bool,
    failover_max_attempts: i64,
    security_enabled: bool,
    /// replace（脱敏后转发，默认）| block（高风险拦截）
    security_mode: String,
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            server_enabled: true,
            failover_enabled: true,
            failover_max_attempts: 3,
            security_enabled: true,
            security_mode: "replace".to_string(),
        }
    }
}

async fn read_settings(pool: &SqlitePool) -> ProxySettings {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key IN ('server.enabled','server.port','failover.enabled','failover.max_attempts','security.enabled','security.mode')",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let map: HashMap<String, String> = rows.into_iter().collect();
    let mut s = ProxySettings::default();
    if let Some(v) = map.get("server.enabled") {
        s.server_enabled = v == "true";
    }
    if let Some(v) = map.get("failover.enabled") {
        s.failover_enabled = v == "true";
    }
    if let Some(v) = map.get("failover.max_attempts") {
        s.failover_max_attempts = v.parse::<i64>().unwrap_or(3).max(1);
    }
    if let Some(v) = map.get("security.enabled") {
        s.security_enabled = v == "true";
    }
    if let Some(v) = map.get("security.mode") {
        if v == "block" || v == "replace" {
            s.security_mode = v.clone();
        }
    }
    s
}

/// 读取监听地址（server.host 默认 127.0.0.1，server.port 默认 8787）
async fn read_listen_addr(pool: &SqlitePool) -> (String, u16) {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key IN ('server.host', 'server.port')",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let map: HashMap<String, String> = rows.into_iter().collect();
    let host = map
        .get("server.host")
        .filter(|h| !h.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = map
        .get("server.port")
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(8787);
    (host, port)
}

// ---------------- 鉴权 ----------------

struct AuthKey {
    id: String,
    name: String,
}

#[derive(Debug)]
enum AuthError {
    /// 密钥不存在 / 格式错（无身份，不记日志）
    Unauthorized,
    /// 已禁用 / 已过期（身份已知）
    Forbidden(String),
    /// 配额用尽
    QuotaExceeded,
}

/// 校验 Authorization: Bearer <sk-llmapi-xxx>，返回密钥身份
async fn authenticate(
    pool: &SqlitePool,
    auth_header: Option<&str>,
) -> Result<AuthKey, AuthError> {
    let header = auth_header
        .and_then(|h| h.strip_prefix("Bearer "))
        .or_else(|| auth_header.and_then(|h| h.strip_prefix("bearer ")))
        .ok_or(AuthError::Unauthorized)?;
    let token = header.trim();
    if token.is_empty() {
        return Err(AuthError::Unauthorized);
    }

    let hash = hash_key(token);
    let row: Option<(String, String, String, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT id, name, status, expires_at, quota_limit, used_tokens FROM keys WHERE key_hash = ?1",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await
    .map_err(|_| AuthError::Unauthorized)?;

    let Some((id, name, status, expires_at, quota_limit, used_tokens)) = row else {
        return Err(AuthError::Unauthorized);
    };

    if status != "active" {
        return Err(AuthError::Forbidden("密钥已禁用".into()));
    }
    if let Some(exp) = expires_at {
        if exp.as_str() < now_iso().as_str() {
            return Err(AuthError::Forbidden("密钥已过期".into()));
        }
    }
    if quota_limit >= 0 && used_tokens >= quota_limit {
        return Err(AuthError::QuotaExceeded);
    }
    Ok(AuthKey { id, name })
}

// ---------------- 处理器 ----------------

async fn handle_chat_completion(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let settings = read_settings(&state.pool).await;
    if !settings.server_enabled {
        return json_error(503, "代理已禁用（server.enabled=false）", "server_disabled");
    }

    // ① 鉴权
    let auth = match authenticate(&state.pool, headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok())).await {
        Ok(k) => k,
        Err(AuthError::Unauthorized) => {
            return json_error(401, "无效的 API 密钥", "invalid_api_key");
        }
        Err(AuthError::Forbidden(reason)) => {
            return json_error(403, reason, "key_forbidden");
        }
        Err(AuthError::QuotaExceeded) => {
            return json_error(429, "密钥配额已用尽，请联系管理员", "quota_exceeded");
        }
    };

    // ② 解析请求体（透传 JSON，保留扩展字段）
    let mut body_value: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return json_error(400, "请求体不是合法的 JSON", "invalid_json"),
    };
    let Some(model) = body_value
        .get("model")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
    else {
        return json_error(400, "缺少 model 字段", "missing_model");
    };
    if body_value.get("stream").and_then(|s| s.as_bool()).unwrap_or(false) {
        return json_error(400, "流式（stream=true）暂不支持，请关闭流式后重试", "stream_not_supported");
    }

    // ③ 请求侧安全扫描 / 脱敏
    let mut request_findings: Vec<Finding> = Vec::new();
    if settings.security_enabled {
        if let Some(messages) = body_value.get_mut("messages").and_then(|m| m.as_array_mut()) {
            for msg in messages.iter_mut() {
                let Some(content) = msg.get_mut("content") else {
                    continue;
                };
                let serde_json::Value::String(s) = content else {
                    continue;
                };
                let result = state.scanner.scan(s).await;
                request_findings.extend(result.findings);
                if settings.security_mode == "block" && result.risk >= RiskLevel::High {
                    return json_error(400, "安全策略拦截：检测到高风险内容（密钥/证件等敏感信息）", "security_blocked");
                }
                *s = result.sanitized;
            }
        }
    }

    // ④ 调度器：构建有序故障转移队列
    let queue = match select_channels(&state.pool, &model).await {
        Ok(q) => q,
        Err(e) => return json_error(500, format!("调度器错误: {e}"), "scheduler_error"),
    };
    if queue.is_empty() {
        return json_error(400, format!("模型 {model} 没有可用的启用渠道"), "no_available_channel");
    }

    // ⑤ 转发 + 故障转移循环
    let attempts = if settings.failover_enabled {
        (settings.failover_max_attempts as usize).min(queue.len())
    } else {
        1
    };
    let mut last_error = String::new();
    for (idx, channel) in queue.iter().take(attempts).enumerate() {
        let started = Instant::now();
        match forward_to_channel(&state.client, channel, &body_value).await {
            Ok(mut resp_value) => {
                let latency = started.elapsed().as_millis() as u64;

                // ⑥ 响应侧安全扫描 / 脱敏
                let mut response_findings: Vec<Finding> = Vec::new();
                if settings.security_enabled {
                    if let Some(choices) = resp_value.get_mut("choices").and_then(|c| c.as_array_mut()) {
                        for choice in choices.iter_mut() {
                            let Some(content) = choice.get_mut("message").and_then(|m| m.get_mut("content")) else {
                                continue;
                            };
                            let serde_json::Value::String(s) = content else {
                                continue;
                            };
                            let result = state.scanner.scan(s).await;
                            response_findings.extend(result.findings);
                            if settings.security_mode == "block" && result.risk >= RiskLevel::High {
                                return json_error(502, "安全策略拦截：响应包含高风险内容，已丢弃", "response_security_blocked");
                            }
                            *s = result.sanitized;
                        }
                    }
                }

                // ⑦ 记账（成功）
                let (prompt_tokens, completion_tokens) = extract_tokens(resp_value.get("usage"));
                let total = prompt_tokens + completion_tokens;
                let cost = total as f64 / 1000.0 * channel.price_per_1k_tokens;
                match log_request(
                    &state.pool,
                    &channel.id,
                    &channel.name,
                    Some(&auth.id),
                    Some(&auth.name),
                    &model,
                    1,
                    latency as i64,
                    prompt_tokens,
                    completion_tokens,
                    cost,
                    None,
                )
                .await
                {
                    Ok(req_id) => {
                        // 安全结果合并落库（请求侧 + 响应侧）
                        record_security_logs(&state.pool, &req_id, &request_findings, &response_findings).await;
                    }
                    Err(e) => eprintln!("[proxy] 记账失败: {e}"),
                }

                return json_ok(resp_value);
            }
            Err(e) => {
                last_error = e.message.clone();
                // 失败尝试也要记日志（转移透明可追溯）
                let _ = log_request(
                    &state.pool,
                    &channel.id,
                    &channel.name,
                    Some(&auth.id),
                    Some(&auth.name),
                    &model,
                    0,
                    started.elapsed().as_millis() as i64,
                    0,
                    0,
                    0.0,
                    Some(&format!("第{}次尝试失败（共{attempts}次机会）: {}", idx + 1, e.message)),
                )
                .await;
                if !e.retryable {
                    break;
                }
            }
        }
    }

    json_error(
        502,
        format!("所有渠道转发失败: {last_error}"),
        "all_channels_failed",
    )
}

async fn handle_not_found() -> Response {
    json_error(404, "未找到该路径，入口: POST /v1/chat/completions", "not_found")
}

// ---------------- 转发 ----------------

/// 转发错误：retryable=false 表示配置错误（4xx），转移无意义，直接返回
#[derive(Debug)]
struct ForwardError {
    retryable: bool,
    message: String,
}

/// 透传转发：请求体原样发给上游（只改 URL 与 Authorization），响应原样返回
async fn forward_to_channel(
    client: &reqwest::Client,
    channel: &ChannelChoice,
    body: &Value,
) -> Result<Value, ForwardError> {
    let url = format!("{}/chat/completions", channel.base_url.trim_end_matches('/'));
    let resp = match client
        .post(&url)
        .bearer_auth(&channel.api_key)
        .json(body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return Err(ForwardError {
                retryable: true, // 网络错误 / 超时：可转移
                message: format!("网络错误或超时: {e}"),
            });
        }
    };

    let status = resp.status();
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return Err(ForwardError {
                retryable: false,
                message: format!("读取上游响应失败: {e}"),
            });
        }
    };

    if !status.is_success() {
        let text = String::from_utf8_lossy(&bytes).to_string();
        let retryable = status.is_server_error() || status.as_u16() == 429;
        return Err(ForwardError {
            retryable,
            message: format!("上游 {status}: {}", truncate(&text, 300)),
        });
    }

    serde_json::from_slice(&bytes).map_err(|e| ForwardError {
        retryable: false,
        message: format!("上游响应解析失败: {e}"),
    })
}

/// 从上游响应的 usage 中提取 token 数（缺失按 0 计）
fn extract_tokens(usage: Option<&Value>) -> (i64, i64) {
    let get = |k: &str| {
        usage
            .and_then(|u| u.get(k))
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
    };
    (get("prompt_tokens"), get("completion_tokens"))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

// ---------------- 安全日志落库 ----------------

/// 请求侧 + 响应侧发现合并写入 security_logs（每个 finding 一行）
async fn record_security_logs(
    pool: &SqlitePool,
    req_id: &str,
    request_findings: &[Finding],
    response_findings: &[Finding],
) {
    for (stage, findings) in [("request", request_findings), ("response", response_findings)] {
        for f in findings {
            let risk = match f.risk {
                RiskLevel::Low => "low",
                RiskLevel::Medium => "medium",
                RiskLevel::High => "high",
            };
            let result = sqlx::query(
                "INSERT INTO security_logs (id, request_id, stage, rule_id, category, risk, snippet_masked, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            )
            .bind(new_id("sec"))
            .bind(req_id)
            .bind(stage)
            .bind(&f.rule_id)
            .bind(&f.category)
            .bind(risk)
            .bind(&f.snippet_masked)
            .bind(now_iso())
            .execute(pool)
            .await;
            if let Err(e) = result {
                eprintln!("[proxy] 安全日志落库失败: {e}");
            }
        }
    }
}

// ---------------- 响应构造 ----------------

/// OpenAI 兼容错误格式：{"error": {"message", "type", "code"}}
fn json_error(status: u16, message: impl Into<String>, code: &str) -> Response {
    let message = message.into();
    let body = serde_json::json!({
        "error": {
            "message": message,
            "type": "invalid_request_error",
            "code": code,
        }
    });
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (code, Json(body)).into_response()
}

fn json_ok(value: Value) -> Response {
    (StatusCode::OK, Json(value)).into_response()
}

// ---------------- 测试 ----------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE keys (
                id TEXT PRIMARY KEY, name TEXT, key_hash TEXT UNIQUE, key_prefix TEXT,
                status TEXT, quota_limit INTEGER, used_tokens INTEGER, expires_at TEXT, created_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn insert_key(
        pool: &SqlitePool,
        id: &str,
        secret: &str,
        status: &str,
        quota_limit: i64,
        used_tokens: i64,
        expires_at: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO keys (id, name, key_hash, key_prefix, status, quota_limit, used_tokens, expires_at, created_at)
             VALUES (?1, '测试密钥', ?2, 'sk-llmapi-', ?3, ?4, ?5, ?6, '2025-01-01 00:00:00')",
        )
        .bind(id)
        .bind(hash_key(secret))
        .bind(status)
        .bind(quota_limit)
        .bind(used_tokens)
        .bind(expires_at)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn auth_accepts_valid_key() {
        let pool = test_pool().await;
        insert_key(&pool, "k1", "sk-llmapi-abcdefghij1234567890", "active", -1, 0, None).await;
        let key = authenticate(&pool, Some("Bearer sk-llmapi-abcdefghij1234567890"))
            .await
            .unwrap();
        assert_eq!(key.id, "k1");
    }

    #[tokio::test]
    async fn auth_rejects_wrong_or_missing_key() {
        let pool = test_pool().await;
        insert_key(&pool, "k1", "sk-llmapi-abcdefghij1234567890", "active", -1, 0, None).await;
        assert!(matches!(
            authenticate(&pool, Some("Bearer sk-llmapi-wrongwrong1234567890")).await,
            Err(AuthError::Unauthorized)
        ));
        assert!(matches!(authenticate(&pool, None).await, Err(AuthError::Unauthorized)));
        assert!(matches!(
            authenticate(&pool, Some("Basic dXNlcjpwYXNz")).await,
            Err(AuthError::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn auth_rejects_disabled_expired_quota() {
        let pool = test_pool().await;
        insert_key(&pool, "k-disabled", "sk-llmapi-disabledkey1234567890", "disabled", -1, 0, None).await;
        insert_key(&pool, "k-expired", "sk-llmapi-expiredkey1234567890", "active", -1, 0, Some("2020-01-01 00:00:00")).await;
        insert_key(&pool, "k-quota", "sk-llmapi-quotakey1234567890", "active", 100, 100, None).await;

        assert!(matches!(
            authenticate(&pool, Some("Bearer sk-llmapi-disabledkey1234567890")).await,
            Err(AuthError::Forbidden(_))
        ));
        assert!(matches!(
            authenticate(&pool, Some("Bearer sk-llmapi-expiredkey1234567890")).await,
            Err(AuthError::Forbidden(_))
        ));
        assert!(matches!(
            authenticate(&pool, Some("Bearer sk-llmapi-quotakey1234567890")).await,
            Err(AuthError::QuotaExceeded)
        ));
    }

    /// 起一个 mock 上游（返回固定状态码 + 固定 body）
    async fn mock_upstream(status: u16, body: &'static str) -> String {
        let app = Router::new()
            .route("/chat/completions", get(mock_respond).post(mock_respond))
            .with_state((status, body));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    async fn mock_respond(State((status, body)): State<(u16, &'static str)>) -> Response {
        (StatusCode::from_u16(status).unwrap(), Bytes::from_static(body.as_bytes())).into_response()
    }

    fn choice(base_url: &str) -> ChannelChoice {
        ChannelChoice {
            id: "c1".into(),
            name: "测试渠道".into(),
            channel_type: "OpenAI".into(),
            base_url: base_url.into(),
            api_key: "k".into(),
            model: "gpt-4o".into(),
            priority: 0,
            weight: 1,
            price_per_1k_tokens: 0.0,
        }
    }

    #[tokio::test]
    async fn forward_marks_5xx_retryable() {
        let base = mock_upstream(500, r#"{"error":"boom"}"#).await;
        let client = reqwest::Client::new();
        let err = forward_to_channel(&client, &choice(&base), &serde_json::json!({"model": "gpt-4o"}))
            .await
            .unwrap_err();
        assert!(err.retryable);
        assert!(err.message.contains("500"));
    }

    #[tokio::test]
    async fn forward_marks_4xx_fatal() {
        let base = mock_upstream(401, r#"{"error":{"message":"bad key"}}"#).await;
        let client = reqwest::Client::new();
        let err = forward_to_channel(&client, &choice(&base), &serde_json::json!({"model": "gpt-4o"}))
            .await
            .unwrap_err();
        assert!(!err.retryable);
        assert!(err.message.contains("401"));
    }

    #[tokio::test]
    async fn forward_parses_success_response() {
        let base = mock_upstream(
            200,
            r#"{"id":"x","model":"gpt-4o","choices":[{"message":{"content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}}"#,
        )
        .await;
        let client = reqwest::Client::new();
        let v = forward_to_channel(&client, &choice(&base), &serde_json::json!({"model": "gpt-4o"}))
            .await
            .unwrap();
        assert_eq!(v["choices"][0]["message"]["content"], "hi");
    }

    #[tokio::test]
    async fn extract_tokens_handles_missing_usage() {
        assert_eq!(extract_tokens(None), (0, 0));
        let usage = serde_json::json!({"prompt_tokens": 10, "completion_tokens": 3});
        assert_eq!(extract_tokens(Some(&usage)), (10, 3));
    }

    #[test]
    fn truncate_limits_length() {
        assert_eq!(truncate("abc", 10), "abc");
        assert!(truncate(&"x".repeat(500), 100).chars().count() <= 101);
    }
}
