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

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;
use sqlx::SqlitePool;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::commands::requests::log_request;
use crate::commands::{hash_key, new_id, now_iso};
use crate::scheduler::{select_channels, ChannelChoice};
use crate::security::{Finding, RiskLevel, SecurityScanner};

/// 代理运行状态（axum 注入）
#[derive(Clone)]
pub struct ProxyState {
    pool: SqlitePool,
    scanner: Arc<dyn SecurityScanner>,
    /// 非流式请求客户端（30s 总超时）
    client: reqwest::Client,
    /// 流式请求客户端（长超时，SSE 长连接用）
    stream_client: reqwest::Client,
}

/// 流式请求总超时（SSE 长连接，模型生成几分钟内应完成）
const STREAM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

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
        stream_client: reqwest::Client::builder()
            .timeout(STREAM_TIMEOUT)
            .build()
            .expect("failed to build proxy stream http client"),
    };

    let app = Router::new()
        .route("/v1/chat/completions", post(handle_chat_completion))
        .route("/v1/models", get(handle_models))
        .route("/health", get(handle_health))
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

/// 公共前置处理的结果（非流式/流式共用）
struct PreparedRequest {
    auth: AuthKey,
    settings: ProxySettings,
    /// 脱敏后的请求体（透传 JSON）
    body: Value,
    model: String,
    queue: Vec<ChannelChoice>,
    request_findings: Vec<Finding>,
    /// 请求侧消息总字符数（流式无精确 usage 时的估算输入）
    prompt_chars: (u64, u64),
}

/// ① 鉴权 → ② 解析请求体（透传 JSON）→ ③ 请求侧安全扫描/脱敏 → ④ 调度器选队列。
/// 任何一步失败返回 OpenAI 兼容错误响应。
async fn prepare_request(
    state: &ProxyState,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<PreparedRequest, Response> {
    let settings = read_settings(&state.pool).await;
    if !settings.server_enabled {
        return Err(json_error(503, "代理已禁用（server.enabled=false）", "server_disabled"));
    }

    // ① 鉴权
    let auth = match authenticate(&state.pool, headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok())).await {
        Ok(k) => k,
        Err(AuthError::Unauthorized) => {
            return Err(json_error(401, "无效的 API 密钥", "invalid_api_key"));
        }
        Err(AuthError::Forbidden(reason)) => {
            return Err(json_error(403, reason, "key_forbidden"));
        }
        Err(AuthError::QuotaExceeded) => {
            return Err(json_error(429, "密钥配额已用尽，请联系管理员", "quota_exceeded"));
        }
    };

    // ② 解析请求体（透传 JSON，保留扩展字段）
    let mut body_value: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return Err(json_error(400, "请求体不是合法的 JSON", "invalid_json")),
    };
    let Some(model) = body_value
        .get("model")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
    else {
        return Err(json_error(400, "缺少 model 字段", "missing_model"));
    };
    let is_stream = body_value.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    if is_stream && body_value.get("stream_options").is_none() {
        // 流式：注入 include_usage，让上游在流尾返回精确 token 统计
        body_value["stream_options"] = serde_json::json!({ "include_usage": true });
    }

    // ③ 请求侧安全扫描 / 脱敏
    let mut request_findings: Vec<Finding> = Vec::new();
    let mut prompt_chars = (0u64, 0u64);
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
                    return Err(json_error(400, "安全策略拦截：检测到高风险内容（密钥/证件等敏感信息）", "security_blocked"));
                }
                *s = result.sanitized;
            }
        }
    }
    if is_stream {
        if let Some(messages) = body_value.get("messages").and_then(|m| m.as_array()) {
            for msg in messages {
                if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                    let (a, b) = count_chars(content);
                    prompt_chars.0 += a;
                    prompt_chars.1 += b;
                }
            }
        }
    }

    // ④ 调度器：构建有序故障转移队列
    let queue = match select_channels(&state.pool, &model).await {
        Ok(q) => q,
        Err(e) => return Err(json_error(500, format!("调度器错误: {e}"), "scheduler_error")),
    };
    if queue.is_empty() {
        return Err(json_error(400, format!("模型 {model} 没有可用的启用渠道"), "no_available_channel"));
    }

    Ok(PreparedRequest {
        auth,
        settings,
        body: body_value,
        model,
        queue,
        request_findings,
        prompt_chars,
    })
}

/// 统一入口：按请求体的 stream 字段分流（false → 非流式 JSON；true → 流式 SSE）
async fn handle_chat_completion(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let is_stream = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| v.get("stream").and_then(|s| s.as_bool()))
        .unwrap_or(false);
    if is_stream {
        handle_stream_completion(State(state), headers, body).await
    } else {
        handle_non_stream_completion(State(state), headers, body).await
    }
}

/// 非流式聊天补全：固定走主渠道，失败按序转移（5xx/429/超时/网络可转移，4xx 直接返回）
async fn handle_non_stream_completion(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let prepared = match prepare_request(&state, &headers, &body).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let attempts = if prepared.settings.failover_enabled {
        (prepared.settings.failover_max_attempts as usize).min(prepared.queue.len())
    } else {
        1
    };
    let mut last_error = String::new();
    for (idx, channel) in prepared.queue.iter().take(attempts).enumerate() {
        let started = Instant::now();
        match forward_to_channel(&state.client, channel, &prepared.body).await {
            Ok(mut resp_value) => {
                let latency = started.elapsed().as_millis() as u64;

                // ⑤ 响应侧安全扫描 / 脱敏
                let mut response_findings: Vec<Finding> = Vec::new();
                if prepared.settings.security_enabled {
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
                            if prepared.settings.security_mode == "block" && result.risk >= RiskLevel::High {
                                return json_error(502, "安全策略拦截：响应包含高风险内容，已丢弃", "response_security_blocked");
                            }
                            *s = result.sanitized;
                        }
                    }
                }

                // ⑥ 记账（成功）
                let (prompt_tokens, completion_tokens) = extract_tokens(resp_value.get("usage"));
                let total = prompt_tokens + completion_tokens;
                let cost = total as f64 / 1000.0 * channel.price_per_1k_tokens;
                match log_request(
                    &state.pool,
                    &channel.id,
                    &channel.name,
                    Some(&prepared.auth.id),
                    Some(&prepared.auth.name),
                    &prepared.model,
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
                        record_security_logs(&state.pool, &req_id, &prepared.request_findings, &response_findings).await;
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
                    Some(&prepared.auth.id),
                    Some(&prepared.auth.name),
                    &prepared.model,
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

/// 流式聊天补全（SSE）：透传上游事件流，统计 token（精确 usage 优先，本地估算兜底），流结束记账。
/// 失败转移只在**响应头发出前**生效（流开始后客户端已收到 200，无法再切换渠道）。
async fn handle_stream_completion(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let prepared = match prepare_request(&state, &headers, &body).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let attempts = if prepared.settings.failover_enabled {
        (prepared.settings.failover_max_attempts as usize).min(prepared.queue.len())
    } else {
        1
    };
    let mut last_error = String::new();
    for (idx, channel) in prepared.queue.iter().take(attempts).enumerate() {
        let started = Instant::now();
        match forward_stream_to_channel(&state, channel, &prepared.body).await {
            Ok(upstream) => {
                // 上游已 200：开始 SSE 透传（记账/脱敏/统计都在流任务里完成）
                return stream_response(&state, upstream, channel, &prepared, started).await;
            }
            Err(e) => {
                last_error = e.message.clone();
                let _ = log_request(
                    &state.pool,
                    &channel.id,
                    &channel.name,
                    Some(&prepared.auth.id),
                    Some(&prepared.auth.name),
                    &prepared.model,
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

/// 模型列表（OpenAI 兼容格式）：聚合所有启用渠道上配置的模型，去重
async fn handle_models(State(state): State<ProxyState>, headers: HeaderMap) -> Response {
    if let Err(resp) = authenticate(&state.pool, headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok())).await {
        return match resp {
            AuthError::Unauthorized => json_error(401, "无效的 API 密钥", "invalid_api_key"),
            AuthError::Forbidden(reason) => json_error(403, reason, "key_forbidden"),
            AuthError::QuotaExceeded => json_error(429, "密钥配额已用尽，请联系管理员", "quota_exceeded"),
        };
    }

    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT model FROM channels WHERE status = 'active' AND model != '' ORDER BY model",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let data: Vec<Value> = rows
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "id": m,
                "object": "model",
                "created": 0,
                "owned_by": "llm-api-gateway",
            })
        })
        .collect();
    json_ok(serde_json::json!({ "object": "list", "data": data }))
}

/// 健康检查（无需鉴权，本地探活用）
async fn handle_health() -> Response {
    json_ok(serde_json::json!({
        "status": "ok",
        "service": "llm-api-gateway",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn handle_not_found() -> Response {
    json_error(
        404,
        "未找到该路径，可用入口: POST /v1/chat/completions、GET /v1/models、GET /health",
        "not_found",
    )
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

// ---------------- 流式转发（SSE） ----------------

/// 向上游发起流式请求；仅返回已确认 200 的响应（非 2xx 按可转移/不可转移分类）
async fn forward_stream_to_channel(
    state: &ProxyState,
    channel: &ChannelChoice,
    body: &Value,
) -> Result<reqwest::Response, ForwardError> {
    let url = format!("{}/chat/completions", channel.base_url.trim_end_matches('/'));
    let resp = match state
        .stream_client
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
    if !status.is_success() {
        let retryable = status.is_server_error() || status.as_u16() == 429;
        let message = match resp.text().await {
            Ok(t) => format!("上游 {status}: {}", truncate(&t, 300)),
            Err(_) => format!("上游 {status}"),
        };
        return Err(ForwardError { retryable, message });
    }
    Ok(resp)
}

/// SSE 透传：后台任务读取上游字节流 → 行缓冲 → 统计/脱敏 → 转发给客户端；流结束记账
async fn stream_response(
    state: &ProxyState,
    upstream: reqwest::Response,
    channel: &ChannelChoice,
    prepared: &PreparedRequest,
    started: Instant,
) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);

    let mut stats = StreamStats::new(
        state.pool.clone(),
        state.scanner.clone(),
        &channel.id,
        &channel.name,
        &prepared.auth.id,
        &prepared.auth.name,
        &prepared.model,
        channel.price_per_1k_tokens,
        prepared.settings.security_enabled,
        &prepared.settings.security_mode,
        prepared.request_findings.clone(),
        prepared.prompt_chars,
        started,
    );

    tokio::spawn(async move {
        let mut splitter = SseLineSplitter::new();
        let mut stream = upstream.bytes_stream();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    let (out, keep) = stats.process(&bytes, &mut splitter).await;
                    if tx.send(Ok(out)).await.is_err() {
                        break; // 客户端已断开
                    }
                    if !keep {
                        break; // 安全拦截中断
                    }
                }
                Err(e) => {
                    stats.finish_failed(&format!("上游流中断: {e}")).await;
                    break;
                }
            }
        }
        // 正常结束（finish 幂等，异常路径已记账则跳过）
        stats.finish_ok().await;
    });

    (
        StatusCode::OK,
        [
            (CONTENT_TYPE, "text/event-stream; charset=utf-8"),
            (CACHE_CONTROL, "no-cache"),
        ],
        Body::from_stream(ReceiverStream::new(rx)),
    )
        .into_response()
}

/// SSE 行缓冲：网络 chunk 可能跨行，累积到完整行再处理
struct SseLineSplitter {
    buf: Vec<u8>,
}

impl SseLineSplitter {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// 喂入字节，返回所有完整行（含行尾 \n）
    fn feed(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(bytes);
        let mut lines = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            lines.push(self.buf.drain(..=pos).collect());
        }
        lines
    }
}

/// 流式统计器：token 统计（精确 usage 优先，估算兜底）+ 响应侧脱敏 + 流结束记账
struct StreamStats {
    pool: SqlitePool,
    scanner: Arc<dyn SecurityScanner>,
    channel_id: String,
    channel_name: String,
    key_id: String,
    key_name: String,
    model: String,
    price: f64,
    security_enabled: bool,
    security_mode: String,
    request_findings: Vec<Finding>,
    prompt_chars: (u64, u64),
    completion_chars: (u64, u64),
    exact_usage: Option<(i64, i64)>,
    response_findings: Vec<Finding>,
    started: Instant,
    finished: bool,
}

impl StreamStats {
    #[allow(clippy::too_many_arguments)]
    fn new(
        pool: SqlitePool,
        scanner: Arc<dyn SecurityScanner>,
        channel_id: &str,
        channel_name: &str,
        key_id: &str,
        key_name: &str,
        model: &str,
        price: f64,
        security_enabled: bool,
        security_mode: &str,
        request_findings: Vec<Finding>,
        prompt_chars: (u64, u64),
        started: Instant,
    ) -> Self {
        Self {
            pool,
            scanner,
            channel_id: channel_id.to_string(),
            channel_name: channel_name.to_string(),
            key_id: key_id.to_string(),
            key_name: key_name.to_string(),
            model: model.to_string(),
            price,
            security_enabled,
            security_mode: security_mode.to_string(),
            request_findings,
            prompt_chars,
            completion_chars: (0, 0),
            exact_usage: None,
            response_findings: Vec::new(),
            started,
            finished: false,
        }
    }

    /// 处理一个上游 chunk：解析 SSE 行、统计 token、脱敏 delta 内容。
    /// 返回 (透传字节, 是否继续流)；安全拦截时向客户端注入 error 事件并返回 keep=false。
    async fn process(&mut self, bytes: &[u8], splitter: &mut SseLineSplitter) -> (Bytes, bool) {
        let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
        for line in splitter.feed(bytes) {
            let Some(payload) = line.strip_prefix(b"data: ") else {
                // event: / id: / 空行等非 data 行原样透传
                out.extend_from_slice(&line);
                continue;
            };
            if payload.trim_ascii() == b"[DONE]" {
                out.extend_from_slice(&line);
                continue;
            }
            let mut event: Value = match serde_json::from_slice(payload) {
                Ok(v) => v,
                Err(_) => {
                    out.extend_from_slice(&line);
                    continue;
                }
            };

            // 精确 token 统计（stream_options.include_usage 时上游在流尾返回）
            if let Some(usage) = event.get("usage") {
                self.exact_usage = Some(extract_tokens(Some(usage)));
            }

            // delta 内容：字符统计 + 安全扫描/脱敏
            if let Some(content) = event.pointer_mut("/choices/0/delta/content") {
                if let serde_json::Value::String(s) = content {
                    let (a, b) = count_chars(s);
                    self.completion_chars.0 += a;
                    self.completion_chars.1 += b;
                    if self.security_enabled {
                        let result = self.scanner.scan(s).await;
                        self.response_findings.extend(result.findings);
                        if self.security_mode == "block" && result.risk >= RiskLevel::High {
                            out.extend_from_slice(
                                "event: error\ndata: {\"error\":{\"message\":\"安全策略拦截：响应包含高风险内容，已中断\",\"type\":\"invalid_response\",\"code\":\"response_security_blocked\"}}\n\n".as_bytes(),
                            );
                            return (Bytes::from(out), false);
                        }
                        *s = result.sanitized;
                    }
                }
            }

            out.extend_from_slice(b"data: ");
            out.extend_from_slice(serde_json::to_string(&event).unwrap_or_default().as_bytes());
            out.push(b'\n');
        }
        (Bytes::from(out), true)
    }

    /// 流正常结束：记账（status=1）
    async fn finish_ok(&mut self) {
        self.finish(1, None).await;
    }

    /// 流异常结束：记账（status=0；按已收到的内容统计，上游确实产生了这些 token）
    async fn finish_failed(&mut self, error: &str) {
        self.finish(0, Some(error.to_string())).await;
    }

    async fn finish(&mut self, status: i64, error: Option<String>) {
        if self.finished {
            return;
        }
        self.finished = true;
        let (prompt, completion) = match self.exact_usage {
            Some((p, c)) => (p, c),
            None => (
                estimate_tokens(self.prompt_chars.0, self.prompt_chars.1),
                estimate_tokens(self.completion_chars.0, self.completion_chars.1),
            ),
        };
        let total = prompt + completion;
        let cost = total as f64 / 1000.0 * self.price;
        let latency = self.started.elapsed().as_millis() as i64;
        match log_request(
            &self.pool,
            &self.channel_id,
            &self.channel_name,
            Some(&self.key_id),
            Some(&self.key_name),
            &self.model,
            status,
            latency,
            prompt,
            completion,
            cost,
            error.as_deref(),
        )
        .await
        {
            Ok(req_id) => {
                record_security_logs(&self.pool, &req_id, &self.request_findings, &self.response_findings).await;
            }
            Err(e) => eprintln!("[proxy] 流式记账失败: {e}"),
        }
    }
}

/// 统计字符串中 ASCII / 非 ASCII 字符数
fn count_chars(s: &str) -> (u64, u64) {
    s.chars().fold((0, 0), |(a, b), c| {
        if c.is_ascii() {
            (a + 1, b)
        } else {
            (a, b + 1)
        }
    })
}

/// 粗略估算 token 数：1 token ≈ 4 个 ASCII 字符 或 ≈ 1.5 个非 ASCII 字符。
/// 仅在上游未返回精确 usage（stream_options.include_usage 不可用）时兜底。
fn estimate_tokens(ascii_chars: u64, other_chars: u64) -> i64 {
    ((ascii_chars + 3) / 4 + (other_chars * 2 + 2) / 3) as i64
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
    use crate::security::RuleScanner;
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

    // ---------------- SSE 流式 ----------------

    #[test]
    fn sse_splitter_handles_partial_lines() {
        let mut splitter = SseLineSplitter::new();
        // 第一块没有换行：留在缓冲
        assert!(splitter.feed(b"data: {\"a\":1}").is_empty());
        // 第二块补齐换行 + 新行
        let lines = splitter.feed(b"\ndata: {\"b\":2}\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], b"data: {\"a\":1}\n");
        assert_eq!(lines[1], b"data: {\"b\":2}\n");
    }

    #[test]
    fn estimate_tokens_roughly() {
        // 80 个 ASCII 字符 ≈ 20 token
        assert_eq!(estimate_tokens(80, 0), 20);
        // 3 个中文字 ≈ 2 token
        assert_eq!(estimate_tokens(0, 3), 2);
        // 混合
        assert!(estimate_tokens(40, 3) > 0);
    }

    #[test]
    fn count_chars_splits_ascii_and_other() {
        // "hello 世界"：h/e/l/l/o + 空格 = 6 个 ASCII，世/界 = 2 个非 ASCII
        assert_eq!(count_chars("hello 世界"), (6, 2));
        assert_eq!(count_chars(""), (0, 0));
    }

    fn stream_stats(
        pool: SqlitePool,
        security_mode: &str,
    ) -> StreamStats {
        StreamStats::new(
            pool,
            Arc::new(RuleScanner::new()),
            "c1",
            "测试渠道",
            "k1",
            "测试密钥",
            "gpt-4o",
            0.0,
            true,
            security_mode,
            vec![],
            (0, 0),
            Instant::now(),
        )
    }

    #[tokio::test]
    async fn stream_process_masks_delta_and_parses_usage() {
        let pool = test_pool().await;
        let mut stats = stream_stats(pool, "replace");
        let mut splitter = SseLineSplitter::new();

        // delta 带密钥：应被脱敏
        let delta = format!(
            "data: {}\n\n",
            serde_json::json!({
                "id": "x",
                "choices": [{"delta": {"content": "密钥 sk-llmapi-abc123def456ghi789jk 已保存"}, "finish_reason": null}]
            })
        );
        let (out, keep) = stats.process(delta.as_bytes(), &mut splitter).await;
        assert!(keep);
        let out_str = String::from_utf8_lossy(&out).to_string();
        assert!(!out_str.contains("sk-llmapi-"));
        assert!(out_str.contains("[MASKED:api_key_llmapi]"));
        // 中文 3 字 + 其他字符计入 completion_chars
        assert!(stats.completion_chars.0 > 0 || stats.completion_chars.1 > 0);

        // usage 行：记录精确 token
        let usage = format!(
            "data: {}\n\n",
            serde_json::json!({"choices": [], "usage": {"prompt_tokens": 11, "completion_tokens": 22, "total_tokens": 33}})
        );
        let (_, keep2) = stats.process(usage.as_bytes(), &mut splitter).await;
        assert!(keep2);
        assert_eq!(stats.exact_usage, Some((11, 22)));

        // [DONE] 与空行透传
        let done = b"data: [DONE]\n\n";
        let (out3, keep3) = stats.process(done, &mut splitter).await;
        assert!(keep3);
        assert!(out3.windows(6).any(|w| w == b"[DONE]"));
    }

    #[tokio::test]
    async fn stream_process_block_interrupts_on_high_risk() {
        let pool = test_pool().await;
        let mut stats = stream_stats(pool, "block");
        let mut splitter = SseLineSplitter::new();

        let delta = format!(
            "data: {}\n\n",
            serde_json::json!({
                "id": "x",
                "choices": [{"delta": {"content": "我的密钥是 sk-llmapi-abc123def456ghi789jk"}, "finish_reason": null}]
            })
        );
        let (out, keep) = stats.process(delta.as_bytes(), &mut splitter).await;
        assert!(!keep, "block 模式 high 命中应中断流");
        let out_str = String::from_utf8_lossy(&out).to_string();
        assert!(out_str.contains("response_security_blocked"));
        assert!(!out_str.contains("sk-llmapi-"), "拦截时不应泄露原始内容");
    }

    #[tokio::test]
    async fn stream_process_passthrough_plain_delta() {
        let pool = test_pool().await;
        let mut stats = stream_stats(pool, "replace");
        let mut splitter = SseLineSplitter::new();

        let delta = format!(
            "data: {}\n\n",
            serde_json::json!({
                "id": "x",
                "choices": [{"delta": {"content": "今天天气不错"}, "finish_reason": null}]
            })
        );
        let (out, keep) = stats.process(delta.as_bytes(), &mut splitter).await;
        assert!(keep);
        let out_str = String::from_utf8_lossy(&out).to_string();
        assert!(out_str.contains("今天天气不错"));
        assert!(stats.response_findings.is_empty());
    }
}
