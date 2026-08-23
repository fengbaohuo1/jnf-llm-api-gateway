//! 路由处理器：聊天补全（非流式/流式）、模型列表、健康检查。
//! 共享的公共前置 `prepare_request` 与响应构造、安全日志落库也在这里。

use std::time::Instant;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::commands::requests::log_request;
use crate::commands::util::{new_id, now_iso};
use crate::scheduler::select_channels;
use crate::security::{Finding, RiskLevel};

use super::auth::{authenticate, AuthError};
use super::forward::{forward_stream_to_channel, forward_to_channel};
use super::settings::read_settings;
use super::stream::{count_chars, stream_response};
use super::{PreparedRequest, ProxyState};

/// 统一入口：按请求体的 stream 字段分流（false → 非流式 JSON；true → 流式 SSE）
pub(crate) async fn handle_chat_completion(
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
                let (prompt_tokens, completion_tokens) = super::forward::extract_tokens(resp_value.get("usage"));
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
pub(crate) async fn handle_models(State(state): State<ProxyState>, headers: HeaderMap) -> Response {
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
pub(crate) async fn handle_health() -> Response {
    json_ok(serde_json::json!({
        "status": "ok",
        "service": "llm-api-gateway",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

pub(crate) async fn handle_not_found() -> Response {
    json_error(
        404,
        "未找到该路径，可用入口: POST /v1/chat/completions、GET /v1/models、GET /health",
        "not_found",
    )
}

// ---------------- 安全日志落库 ----------------

/// 请求侧 + 响应侧发现合并写入 security_logs（每个 finding 一行）
pub(crate) async fn record_security_logs(
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
