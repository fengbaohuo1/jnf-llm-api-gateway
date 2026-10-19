//! 路由处理器：聊天补全（非流式/流式）、模型列表、健康检查。
//! 共享的公共前置 `prepare_request` 与响应构造、安全日志落库也在这里。
//!
//! 协议入口：`/v1/chat/completions`（OpenAI）、`/v1/messages`（Anthropic）、
//! `/v1/responses`（OpenAI Responses）——统一走 `handle_entry`，
//! 请求在 `prepare_request` 内经 `protocol::to_openai` 转成内部格式，
//! 响应在出口经 `protocol::from_openai` / `StreamTranslator` 翻译回客户端协议。
//!
//! 内容记录：`prepare_request` 捕获"收到的原始请求"（脱敏后）与"实际转发体"，
//! 成功/失败路径随 `log_request` 一起写入 `request_details` 明细表（见 commands/requests）。

use std::sync::Arc;
use std::time::Instant;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::USER_AGENT;
use axum::http::StatusCode;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::commands::requests::log_request;
use crate::commands::requests::{RequestDetailInput, RequestLogInput};
use crate::commands::util::new_id;
use crate::scheduler::select_channels;
use crate::security::leak::check_leak;
use crate::security::{
    decide, max_risk, Finding, PolicyDecision, RiskLevel, ScanResult, SecurityScanner,
};

use super::auth::{authenticate, extract_api_token, AuthError};
use super::forward::{forward_stream_to_channel, forward_to_channel};
use super::protocol::{self, ClientProtocol};
use super::settings::read_settings;
use super::stream::{count_chars, stream_response};
use super::{PreparedRequest, ProxyState};

/// 内部错误（统一携带状态码/消息/代码，出口按协议包装）
pub(crate) struct ApiError {
    pub(crate) status: u16,
    pub(crate) message: String,
    pub(crate) code: &'static str,
    /// 已知的请求风险（拦截类错误携带命中的最高等级；其余为 None）
    pub(crate) risk: Option<RiskLevel>,
    /// 安全拦截时携带的命中记录（用于失败路径补写 security_logs）
    pub(crate) findings: Vec<Finding>,
}

impl ApiError {
    fn new(status: u16, message: impl Into<String>, code: &'static str) -> Self {
        Self { status, message: message.into(), code, risk: None, findings: Vec::new() }
    }

    /// 记录该错误对应的请求风险（安全拦截时用，供日志存储级别判定）
    fn with_risk(mut self, risk: RiskLevel) -> Self {
        self.risk = Some(risk);
        self
    }

    /// 携带安全命中记录（失败路径写 security_logs，保证拦截可审计）
    fn with_findings(mut self, findings: Vec<Finding>) -> Self {
        self.findings = findings;
        self
    }
}

/// 请求的最终风险：请求侧 + 响应侧命中的最高等级（None = 无命中/未扫描）
pub(crate) fn risk_of(request: &[Finding], response: &[Finding]) -> Option<RiskLevel> {
    let req = max_risk(request);
    let resp = max_risk(response);
    match (req, resp) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// 风险等级 → 日志列字符串（无风险 = none）
pub(crate) fn risk_label(risk: Option<RiskLevel>) -> &'static str {
    risk.map(|r| r.as_str()).unwrap_or("none")
}

/// 按客户端协议包装错误响应
fn error_response(proto: ClientProtocol, err: ApiError) -> Response {
    let body = protocol::error_body(proto, &err.message, err.code);
    let status = StatusCode::from_u16(err.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(body)).into_response()
}

/// 统一入口：按协议 + stream 字段分流
pub(crate) async fn handle_chat_completion(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    handle_entry(ClientProtocol::OpenAi, State(state), headers, body).await
}

pub(crate) async fn handle_anthropic_message(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    handle_entry(ClientProtocol::Anthropic, State(state), headers, body).await
}

pub(crate) async fn handle_responses(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    handle_entry(ClientProtocol::Responses, State(state), headers, body).await
}

async fn handle_entry(
    proto: ClientProtocol,
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let is_stream = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| v.get("stream").and_then(|s| s.as_bool()))
        .unwrap_or(false);
    if is_stream {
        handle_stream_completion(proto, State(state), headers, body).await
    } else {
        handle_non_stream_completion(proto, State(state), headers, body).await
    }
}

/// 客户端 User-Agent（日志上下文用，截断 200 字符）
fn user_agent_of(headers: &HeaderMap) -> String {
    headers
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(200)
        .collect()
}

/// 全树脱敏：把 JSON 中所有字符串叶子经扫描器脱敏（供内容落库；不产生 findings，
/// 安全日志以主流程消息扫描的 findings 为准，避免重复计数）。
///
/// 实现：同步递归收集叶子文本 → 逐叶异步扫描建替换表 → 同步递归回填
/// （避免递归 async fn 无法编译的问题）。
async fn mask_json_strings(scanner: &dyn SecurityScanner, v: &mut Value) {
    // ① 收集全部字符串叶子（克隆出所有权，随后才能原地回填）
    fn collect(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(items) => items.iter().for_each(|i| collect(i, out)),
            Value::Object(map) => map.values().for_each(|i| collect(i, out)),
            _ => {}
        }
    }
    let mut leaves: Vec<String> = Vec::new();
    collect(v, &mut leaves);

    // ② 逐叶扫描（每个唯一文本一次；避免重复字符串重复扫描）
    let mut table: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for s in &leaves {
        if !table.contains_key(s) {
            table.insert(s.clone(), scanner.scan(s).await.sanitized);
        }
    }

    // ③ 回填
    fn apply(v: &mut Value, table: &std::collections::HashMap<String, String>) {
        match v {
            Value::String(s) => {
                if let Some(masked) = table.get(s) {
                    *s = masked.clone();
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|i| apply(i, table)),
            Value::Object(map) => map.values_mut().for_each(|i| apply(i, table)),
            _ => {}
        }
    }
    apply(v, &table);
}

/// 内容明细单字段上限（1M 字符；超出截断并标记，避免超大上下文撑爆明细表）
pub(crate) const MAX_DETAIL_CHARS: usize = 1_000_000;

pub(crate) fn clamp_json_body(s: String) -> String {
    if s.chars().count() <= MAX_DETAIL_CHARS {
        s
    } else {
        let head: String = s.chars().take(MAX_DETAIL_CHARS).collect();
        format!("{head}\n…[内容超长已截断]")
    }
}

/// ① 鉴权 → ② 解析请求体 + 协议转换 → ③ 请求侧安全扫描/脱敏 → ④ 调度器选队列。
/// 同时捕获内容记录所需的请求体快照（原始体 + 转发体，脱敏后）。
async fn prepare_request(
    state: &ProxyState,
    headers: &HeaderMap,
    body: &Bytes,
    proto: ClientProtocol,
) -> Result<PreparedRequest, ApiError> {
    let settings = read_settings(&state.pool).await;
    if !settings.server_enabled {
        return Err(ApiError::new(503, "代理已禁用（server.enabled=false）", "server_disabled"));
    }

    // ① 鉴权（Authorization: Bearer 或 x-api-key）
    let token = extract_api_token(headers);
    let auth = match authenticate(&state.pool, token).await {
        Ok(k) => k,
        Err(AuthError::Unauthorized) => {
            return Err(ApiError::new(401, "无效的 API 密钥", "invalid_api_key"));
        }
        Err(AuthError::Forbidden(reason)) => {
            return Err(ApiError::new(403, reason, "key_forbidden"));
        }
        Err(AuthError::QuotaExceeded) => {
            return Err(ApiError::new(429, "密钥配额已用尽，请联系管理员", "quota_exceeded"));
        }
    };

    // ② 解析请求体（透传 JSON，保留扩展字段）→ 协议转换（客户端协议 → 内部 OpenAI 格式）
    let mut body_value: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return Err(ApiError::new(400, "请求体不是合法的 JSON", "invalid_json")),
    };
    // 协议转换器按对象处理字段；数组/标量请求体直接拒绝，避免 unwrap panic
    if !body_value.is_object() {
        return Err(ApiError::new(400, "请求体必须是 JSON 对象", "invalid_json_object"));
    }
    // 内容记录：先捕获客户端协议原文快照（脱敏后落库，保证日志不成为泄密通道）；
    // 是否最终保存由 log.store_level 与请求风险在记账时决定（none 时连快照都不做）
    let request_body = if settings.content_possible() {
        let mut raw = body_value.clone();
        if settings.security_enabled {
            mask_json_strings(state.scanner.as_ref(), &mut raw).await;
        }
        Some(clamp_json_body(serde_json::to_string(&raw).unwrap_or_default()))
    } else {
        None
    };
    if let Err(e) = protocol::to_openai(&mut body_value, proto) {
        return Err(ApiError::new(400, format!("请求协议转换失败: {e}"), "protocol_convert_error"));
    }
    let Some(model) = body_value
        .get("model")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
    else {
        return Err(ApiError::new(400, "缺少 model 字段", "missing_model"));
    };
    let is_stream = body_value.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    if is_stream && body_value.get("stream_options").is_none() {
        // 流式：注入 include_usage，让上游在流尾返回精确 token 统计
        body_value["stream_options"] = serde_json::json!({ "include_usage": true });
    }

    // ③ 请求侧安全扫描 / 脱敏（统一扫描转换后的 OpenAI 格式）
    //    全树扫描字符串叶子：消息正文、多模态 parts、tool_calls.arguments、system 等
    let mut request_findings: Vec<Finding> = Vec::new();
    let mut request_secrets: Vec<String> = Vec::new();
    let mut prompt_chars = (0u64, 0u64);
    if settings.security_enabled {
        let result = scan_json_for_security(state.scanner.as_ref(), &mut body_value, None).await;
        let risk = result.risk;
        let blocked = should_block(&settings.security_mode, &result);
        request_findings = result.findings;
        request_secrets = result.high_fragments;
        if blocked {
            return Err(ApiError::new(
                400,
                "安全策略拦截：检测到高风险内容（密钥/证件/带毒命令等）",
                "security_blocked",
            )
            .with_risk(risk)
            .with_findings(std::mem::take(&mut request_findings)));
        }
    }
    if is_stream {
        if let Some(messages) = body_value.get("messages") {
            prompt_chars = json_char_counts(messages);
        }
    }

    // ④ 调度器：构建有序故障转移队列
    let queue = match select_channels(&state.pool, &model).await {
        Ok(q) => q,
        Err(e) => return Err(ApiError::new(500, format!("调度器错误: {e}"), "scheduler_error")),
    };
    if queue.is_empty() {
        return Err(ApiError::new(
            400,
            format!("模型 {model} 没有可用的启用渠道"),
            "no_available_channel",
        ));
    }

    // 内容记录：实际转发体（协议转换 + 脱敏 + stream_options 注入后）
    let upstream_body = if settings.content_possible() {
        Some(clamp_json_body(serde_json::to_string(&body_value).unwrap_or_default()))
    } else {
        None
    };

    Ok(PreparedRequest {
        auth,
        settings,
        body: body_value,
        model,
        queue,
        request_findings,
        request_secrets,
        prompt_chars,
        client_ua: user_agent_of(headers),
        request_body,
        upstream_body,
    })
}

/// 递归扫描 JSON 中所有字符串叶子：命中内容按扫描器脱敏回填，并聚合 findings / high 片段。
/// 覆盖消息正文、多模态 parts、tool_calls.arguments、system 等所有文本（之前只看 message.content）。
async fn scan_json_for_security(
    scanner: &dyn SecurityScanner,
    v: &mut Value,
    request_secrets: Option<&[String]>,
) -> ScanResult {
    // ① 收集全部字符串叶子
    fn collect(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(items) => items.iter().for_each(|i| collect(i, out)),
            Value::Object(map) => map.values().for_each(|i| collect(i, out)),
            _ => {}
        }
    }
    let mut leaves: Vec<String> = Vec::new();
    collect(v, &mut leaves);

    // ② 逐叶扫描（同一文本只扫一次）；上下文泄露比对基于原文
    let mut table: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut findings: Vec<Finding> = Vec::new();
    let mut high_fragments: Vec<String> = Vec::new();
    for s in &leaves {
        if table.contains_key(s) {
            continue;
        }
        let r = scanner.scan(s).await;
        if let Some(secrets) = request_secrets {
            if let Some(leak) = check_leak(s, secrets) {
                findings.push(leak);
            }
        }
        findings.extend(r.findings);
        high_fragments.extend(r.high_fragments);
        table.insert(s.clone(), r.sanitized);
    }

    // ③ 回填脱敏结果
    fn apply(v: &mut Value, table: &std::collections::HashMap<String, String>) {
        match v {
            Value::String(s) => {
                if let Some(masked) = table.get(s) {
                    *s = masked.clone();
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|i| apply(i, table)),
            Value::Object(map) => map.values_mut().for_each(|i| apply(i, table)),
            _ => {}
        }
    }
    apply(v, &table);

    let risk = max_risk(&findings).unwrap_or(RiskLevel::Low);
    ScanResult {
        risk,
        findings,
        sanitized: String::new(),
        high_fragments,
    }
}

/// 统计 JSON 中所有字符串叶子的 ASCII / 非 ASCII 字符数（流式估算输入 token 用）
fn json_char_counts(v: &Value) -> (u64, u64) {
    match v {
        Value::String(s) => count_chars(s),
        Value::Array(items) => items.iter().fold((0, 0), |(a, b), i| {
            let (x, y) = json_char_counts(i);
            (a + x, b + y)
        }),
        Value::Object(map) => map.values().fold((0, 0), |(a, b), i| {
            let (x, y) = json_char_counts(i);
            (a + x, b + y)
        }),
        _ => (0, 0),
    }
}

/// 日志前的文本脱敏（上游错误信息可能回显请求内容/密钥，落库前先过扫描器）
async fn sanitize_for_log(scanner: &dyn SecurityScanner, enabled: bool, text: &str) -> String {
    if !enabled {
        return text.to_string();
    }
    scanner.scan(text).await.sanitized
}

/// 是否应拦截：全局 block 模式且 high，或任一规则显式配置了 block（与全局模式无关）
fn should_block(mode: &str, result: &ScanResult) -> bool {
    (mode == "block" && result.risk >= RiskLevel::High) || decide(&result.findings) == PolicyDecision::Block
}

/// 组装内容明细输入（由 prepared 快照 + 本次结果字段构成）。
/// 是否真的落库由 `log.store_level` 与本次请求风险决定（见 ProxySettings::content_allowed）：
/// 未达阈值的请求只保留概要行，不写 request_details。
fn detail_of<'a>(
    prepared: &'a PreparedRequest,
    response_body: Option<&'a str>,
    error_body: Option<&'a str>,
    risk: Option<RiskLevel>,
) -> Option<RequestDetailInput<'a>> {
    if !prepared.settings.content_allowed(risk) {
        return None;
    }
    Some(RequestDetailInput {
        request_body: prepared.request_body.as_deref(),
        upstream_body: prepared.upstream_body.as_deref(),
        response_body,
        error_body,
    })
}

/// 鉴权/服务类错误码（这些已有 auth_events 审计或与内容无关，不写请求日志行）
fn is_auth_like(status: u16) -> bool {
    matches!(status, 401 | 403 | 429 | 503)
}

/// prepare 阶段失败（业务错误：无可用渠道/协议转换/缺 model/安全拦截等）也要留痕。
/// channel 未知（NULL），key 鉴权已过但错误对象不携带身份——记空 key；
/// request_body 用原始请求体重新脱敏后落库（与成功行同语义），error_body 记网关错误 JSON。
async fn log_prepare_failure(
    pool: &sqlx::SqlitePool,
    scanner: &Arc<dyn SecurityScanner>,
    proto: ClientProtocol,
    headers: &HeaderMap,
    body: &Bytes,
    err: &ApiError,
) {
    let settings = read_settings(pool).await;
    // 风险：安全拦截类错误携带命中等级；其余错误无已知风险（None）
    let risk = err.risk;
    // 请求体脱敏（失败路径未经过完整扫描，先整树脱敏再落库）
    let request_body = if settings.content_possible() {
        match serde_json::from_slice::<Value>(body) {
            Ok(mut v) => {
                if settings.security_enabled {
                    mask_json_strings(scanner.as_ref(), &mut v).await;
                }
                Some(clamp_json_body(serde_json::to_string(&v).unwrap_or_default()))
            }
            Err(_) => None, // 无效 JSON：不落原文（避免未脱敏内容入库）
        }
    } else {
        None
    };
    // 尽力提取 model（无效 JSON 时为 None）
    let model = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(|s| s.to_string()))
        .unwrap_or_default();
    let error_json = serde_json::to_string(&protocol::error_body(proto, &err.message, err.code))
        .unwrap_or_default();
    let details = if settings.content_possible() && settings.content_allowed(risk) {
        Some(RequestDetailInput {
            request_body: request_body.as_deref(),
            upstream_body: None,
            response_body: None,
            error_body: Some(&error_json),
        })
    } else {
        None
    };
    let input = RequestLogInput {
        channel_id: None,
        channel_name: None,
        key_id: None,
        key_name: None,
        model: &model,
        status: 0,
        latency_ms: 0,
        prompt_tokens: 0,
        completion_tokens: 0,
        cache_hit_tokens: 0,
        reasoning_tokens: 0,
        cost: 0.0,
        error: Some(&format!("{}（{}）", err.message, err.code)),
        protocol: proto.as_str(),
        stream: false,
        client_ua: &user_agent_of(headers),
        attempts: 0,
        risk_level: risk_label(risk),
        details,
    };
    if let Ok(req_id) = log_request(pool, &input).await {
        // 请求侧安全拦截同样写安全事件（此前只记 requests 行，安全页看不到拦截）
        if !err.findings.is_empty() {
            record_security_logs(pool, &req_id, &err.findings, &[]).await;
        }
    }
}

/// 非流式聊天补全：固定走主渠道，失败按序转移（5xx/429/超时/网络可转移，4xx 直接返回）
async fn handle_non_stream_completion(
    proto: ClientProtocol,
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let prepared = match prepare_request(&state, &headers, &body, proto).await {
        Ok(p) => p,
        Err(err) => {
            if !is_auth_like(err.status) {
                log_prepare_failure(&state.pool, &state.scanner, proto, &headers, &body, &err).await;
            }
            return error_response(proto, err);
        }
    };

    let attempts = if prepared.settings.failover_enabled {
        (prepared.settings.failover_max_attempts as usize).min(prepared.queue.len())
    } else {
        1
    };
    let mut last_error = String::new();
    // 最后一次失败的上游状态码（非可转移 4xx 原样返回给客户端，而不是统一 502）
    let mut last_status: Option<u16> = None;
    for (idx, channel) in prepared.queue.iter().take(attempts).enumerate() {
        let started = Instant::now();
        match forward_to_channel(&state.client, channel, &prepared.body).await {
            Ok(mut resp_value) => {
                let latency = started.elapsed().as_millis() as u64;

                // ⑤ 响应侧安全扫描 / 脱敏（含上下文泄露比对）：整条 message 子树
                //    （content / tool_calls.arguments / reasoning_content 等全部字符串）
                let mut response_findings: Vec<Finding> = Vec::new();
                if prepared.settings.security_enabled {
                    if let Some(choices) = resp_value.get_mut("choices").and_then(|c| c.as_array_mut()) {
                        for choice in choices.iter_mut() {
                            let Some(message) = choice.get_mut("message") else {
                                continue;
                            };
                            let result = scan_json_for_security(
                                state.scanner.as_ref(),
                                message,
                                Some(&prepared.request_secrets),
                            )
                            .await;
                            response_findings.extend(result.findings);
                        }
                    }
                }
                let blocked = (prepared.settings.security_mode == "block"
                    && max_risk(&response_findings) >= Some(RiskLevel::High))
                    || decide(&response_findings) == PolicyDecision::Block;
                if blocked {
                    // 拦截同样要留痕：请求日志（status=0、风险=high）+ 安全事件；
                    // 高风险请求按存储级别通常会保存内容，便于事后复盘
                    let risk = risk_of(&prepared.request_findings, &response_findings);
                    let blocked_risk = max_risk(&response_findings).unwrap_or(RiskLevel::High);
                    let err_text =
                        "安全策略拦截：响应包含高风险内容，已丢弃（response_security_blocked）";
                    let err_json = serde_json::to_string(&protocol::error_body(
                        proto,
                        "安全策略拦截：响应包含高风险内容，已丢弃",
                        "response_security_blocked",
                    ))
                    .unwrap_or_default();
                    let log_input = RequestLogInput {
                        channel_id: Some(&channel.id),
                        channel_name: Some(&channel.name),
                        key_id: Some(&prepared.auth.id),
                        key_name: Some(&prepared.auth.name),
                        model: &prepared.model,
                        status: 0,
                        latency_ms: latency as i64,
                        prompt_tokens: 0,
                        completion_tokens: 0,
                        cache_hit_tokens: 0,
                        reasoning_tokens: 0,
                        cost: 0.0,
                        error: Some(err_text),
                        protocol: proto.as_str(),
                        stream: false,
                        client_ua: &prepared.client_ua,
                        attempts: (idx + 1) as i64,
                        risk_level: risk_label(risk),
                        details: detail_of(&prepared, None, Some(&err_json), risk),
                    };
                    match log_request(&state.pool, &log_input).await {
                        Ok(req_id) => {
                            record_security_logs(
                                &state.pool,
                                &req_id,
                                &prepared.request_findings,
                                &response_findings,
                            )
                            .await;
                        }
                        Err(e) => eprintln!("[proxy] 拦截日志写入失败: {e}"),
                    }
                    return error_response(
                        proto,
                        ApiError::new(502, "安全策略拦截：响应包含高风险内容，已丢弃", "response_security_blocked")
                            .with_risk(blocked_risk),
                    );
                }

                // ⑥ 记账（成功）：缓存/思考感知拆分计费；风险取请求侧 + 响应侧最高等级
                let risk = risk_of(&prepared.request_findings, &response_findings);
                let tokens = super::forward::extract_usage(resp_value.get("usage"));
                let cost = super::forward::compute_cost(
                    tokens,
                    channel.price_per_1k_tokens,
                    channel.cache_price_per_1k_tokens,
                    channel.reasoning_price_per_1k_tokens,
                );
                let response_json = serde_json::to_string(&resp_value).unwrap_or_default();
                let response_body_owned = clamp_json_body(response_json);
                let log_input = RequestLogInput {
                    channel_id: Some(&channel.id),
                    channel_name: Some(&channel.name),
                    key_id: Some(&prepared.auth.id),
                    key_name: Some(&prepared.auth.name),
                    model: &prepared.model,
                    status: 1,
                    latency_ms: latency as i64,
                    prompt_tokens: tokens.prompt,
                    completion_tokens: tokens.completion,
                    cache_hit_tokens: tokens.cache_hit,
                    reasoning_tokens: tokens.reasoning,
                    cost,
                    error: None,
                    protocol: proto.as_str(),
                    stream: false,
                    client_ua: &prepared.client_ua,
                    attempts: (idx + 1) as i64,
                    risk_level: risk_label(risk),
                    details: detail_of(&prepared, Some(response_body_owned.as_str()), None, risk),
                };
                match log_request(&state.pool, &log_input).await {
                    Ok(req_id) => {
                        // 安全结果合并落库（请求侧 + 响应侧）
                        record_security_logs(&state.pool, &req_id, &prepared.request_findings, &response_findings).await;
                    }
                    Err(e) => eprintln!("[proxy] 记账失败: {e}"),
                }

                // 出口：内部 OpenAI 响应 → 客户端协议
                return json_ok(protocol::from_openai(resp_value, proto));
            }
            Err(e) => {
                last_error = e.message.clone();
                if let Some(s) = e.status {
                    last_status = Some(s);
                }
                let is_last_try = idx + 1 == attempts;
                let risk = max_risk(&prepared.request_findings);
                // 上游错误信息可能回显请求内容/密钥：落库前先过安全扫描器脱敏
                let err_text = sanitize_for_log(
                    state.scanner.as_ref(),
                    prepared.settings.security_enabled,
                    &e.message,
                )
                .await;
                let err_body = match e.error_body.as_deref() {
                    Some(b) => Some(
                        sanitize_for_log(
                            state.scanner.as_ref(),
                            prepared.settings.security_enabled,
                            b,
                        )
                        .await,
                    ),
                    None => None,
                };
                // 失败尝试也要记日志（转移透明可追溯）；最后一次失败补上请求内容，便于排查
                let log_input = RequestLogInput {
                    channel_id: Some(&channel.id),
                    channel_name: Some(&channel.name),
                    key_id: Some(&prepared.auth.id),
                    key_name: Some(&prepared.auth.name),
                    model: &prepared.model,
                    status: 0,
                    latency_ms: started.elapsed().as_millis() as i64,
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    cache_hit_tokens: 0,
                    reasoning_tokens: 0,
                    cost: 0.0,
                    error: Some(&format!("第{}次尝试失败（共{attempts}次机会）: {}", idx + 1, err_text)),
                    protocol: proto.as_str(),
                    stream: false,
                    client_ua: &prepared.client_ua,
                    attempts: (idx + 1) as i64,
                    risk_level: risk_label(risk),
                    details: if is_last_try {
                        detail_of(&prepared, None, err_body.as_deref(), risk)
                    } else {
                        None
                    },
                };
                let _ = log_request(&state.pool, &log_input).await;
                if !e.retryable {
                    break;
                }
            }
        }
    }

    error_response(
        proto,
        ApiError::new(
            last_status.unwrap_or(502),
            format!("所有渠道转发失败: {last_error}"),
            "all_channels_failed",
        ),
    )
}

/// 流式聊天补全（SSE）：透传上游事件流（按协议逐事件翻译），统计 token，流结束记账。
/// 失败转移只在**响应头发出前**生效（流开始后客户端已收到 200，无法再切换渠道）。
async fn handle_stream_completion(
    proto: ClientProtocol,
    State(state): State<ProxyState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let prepared = match prepare_request(&state, &headers, &body, proto).await {
        Ok(p) => p,
        Err(err) => {
            if !is_auth_like(err.status) {
                log_prepare_failure(&state.pool, &state.scanner, proto, &headers, &body, &err).await;
            }
            return error_response(proto, err);
        }
    };

    let attempts = if prepared.settings.failover_enabled {
        (prepared.settings.failover_max_attempts as usize).min(prepared.queue.len())
    } else {
        1
    };
    let mut last_error = String::new();
    let mut last_status: Option<u16> = None;
    for (idx, channel) in prepared.queue.iter().take(attempts).enumerate() {
        let started = Instant::now();
        match forward_stream_to_channel(&state, channel, &prepared.body).await {
            Ok(upstream) => {
                // 上游已 200：开始 SSE 透传（记账/脱敏/统计/协议翻译都在流任务里完成）
                return stream_response(&state, upstream, channel, &prepared, started, proto, (idx + 1) as i64).await;
            }
            Err(e) => {
                last_error = e.message.clone();
                if let Some(s) = e.status {
                    last_status = Some(s);
                }
                let is_last_try = idx + 1 == attempts;
                let risk = max_risk(&prepared.request_findings);
                // 上游错误信息可能回显请求内容/密钥：落库前先过安全扫描器脱敏
                let err_text = sanitize_for_log(
                    state.scanner.as_ref(),
                    prepared.settings.security_enabled,
                    &e.message,
                )
                .await;
                let err_body = match e.error_body.as_deref() {
                    Some(b) => Some(
                        sanitize_for_log(
                            state.scanner.as_ref(),
                            prepared.settings.security_enabled,
                            b,
                        )
                        .await,
                    ),
                    None => None,
                };
                let log_input = RequestLogInput {
                    channel_id: Some(&channel.id),
                    channel_name: Some(&channel.name),
                    key_id: Some(&prepared.auth.id),
                    key_name: Some(&prepared.auth.name),
                    model: &prepared.model,
                    status: 0,
                    latency_ms: started.elapsed().as_millis() as i64,
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    cache_hit_tokens: 0,
                    reasoning_tokens: 0,
                    cost: 0.0,
                    error: Some(&format!("第{}次尝试失败（共{attempts}次机会）: {}", idx + 1, err_text)),
                    protocol: proto.as_str(),
                    stream: true,
                    client_ua: &prepared.client_ua,
                    attempts: (idx + 1) as i64,
                    risk_level: risk_label(risk),
                    details: if is_last_try {
                        detail_of(&prepared, None, err_body.as_deref(), risk)
                    } else {
                        None
                    },
                };
                let _ = log_request(&state.pool, &log_input).await;
                if !e.retryable {
                    break;
                }
            }
        }
    }

    error_response(
        proto,
        ApiError::new(
            last_status.unwrap_or(502),
            format!("所有渠道转发失败: {last_error}"),
            "all_channels_failed",
        ),
    )
}

/// 模型列表（OpenAI 兼容格式）：聚合所有启用渠道上配置的模型，去重
pub(crate) async fn handle_models(State(state): State<ProxyState>, headers: HeaderMap) -> Response {
    if let Err(err) = authenticate(&state.pool, extract_api_token(&headers)).await {
        return match err {
            AuthError::Unauthorized => {
                error_response(ClientProtocol::OpenAi, ApiError::new(401, "无效的 API 密钥", "invalid_api_key"))
            }
            AuthError::Forbidden(reason) => {
                error_response(ClientProtocol::OpenAi, ApiError::new(403, reason, "key_forbidden"))
            }
            AuthError::QuotaExceeded => {
                error_response(ClientProtocol::OpenAi, ApiError::new(429, "密钥配额已用尽，请联系管理员", "quota_exceeded"))
            }
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
    json_error_openai(
        404,
        "未找到该路径，可用入口: POST /v1/chat/completions、POST /v1/messages、POST /v1/responses、GET /v1/models、GET /health",
        "not_found",
    )
}

// ---------------- 安全日志落库 ----------------

/// 请求侧 + 响应侧发现合并写入 security_logs（每个 finding 一行，含处置动作）
pub(crate) async fn record_security_logs(
    pool: &SqlitePool,
    req_id: &str,
    request_findings: &[Finding],
    response_findings: &[Finding],
) {
    for (stage, findings) in [("request", request_findings), ("response", response_findings)] {
        for f in findings {
            let result = sqlx::query(
                "INSERT INTO security_logs (id, request_id, stage, rule_id, category, risk, snippet_masked, action, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            )
            .bind(new_id("sec"))
            .bind(req_id)
            .bind(stage)
            .bind(&f.rule_id)
            .bind(&f.category)
            .bind(f.risk.as_str())
            .bind(&f.snippet_masked)
            .bind(&f.action)
            .bind(crate::commands::util::now_iso())
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
fn json_error_openai(status: u16, message: impl Into<String>, code: &str) -> Response {
    let body = protocol::error_body(ClientProtocol::OpenAi, &message.into(), code);
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(body)).into_response()
}

fn json_ok(value: Value) -> Response {
    (StatusCode::OK, Json(value)).into_response()
}

#[cfg(test)]
mod tests {
    //! 代理端到端测试：真实 axum 处理器 + mock 上游 + 迁移后的内存库，
    //! 覆盖「按安全等级存储内容」的落库行为（risk_level / request_details）。

    use std::str::FromStr;
    use std::sync::Arc;

    use axum::extract::State;
    use axum::http::header::{AUTHORIZATION, USER_AGENT};
    use axum::http::HeaderMap;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use sqlx::SqlitePool;

    use super::*;
    use crate::commands::util::hash_key;
    use crate::proxy::test_util::mock_upstream;
    use crate::security::RuleScanner;

    const KEY_SECRET: &str = "sk-llmapi-abcdefghijklmnopqrstuvwxyz012345";

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

    /// 建一个可用状态：key + channel(mock 上游) + 日志存储级别
    async fn setup(store_level: &str, upstream_body: &'static str) -> ProxyState {
        let pool = migrated_pool().await;
        let base = mock_upstream(200, upstream_body).await;

        sqlx::query(
            "INSERT INTO keys (id, name, key_hash, key_prefix, status, quota_limit, used_tokens, created_at)
             VALUES ('k1', '测试密钥', ?1, 'sk-llmapi-', 'active', -1, 0, '2025-01-01 00:00:00')",
        )
        .bind(hash_key(KEY_SECRET))
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO channels (id, name, type, base_url, model, status, api_key_encrypted, latency_ms, priority, weight, price_per_1k_tokens, cache_price_per_1k_tokens, reasoning_price_per_1k_tokens, created_at, updated_at)
             VALUES ('c1', '测试渠道', 'OpenAI', ?1, 'gpt-4o', 'active', 'upstream-key', NULL, 10, 1, 0, 0, 0, '2025-01-01 00:00:00', '2025-01-01 00:00:00')",
        )
        .bind(&base)
        .execute(&pool)
        .await
        .unwrap();

        for (k, v) in [
            ("security.enabled", "true"),
            ("security.mode", "replace"),
            ("log.store_level", store_level),
        ] {
            sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, '2025-01-01 00:00:00')")
                .bind(k)
                .bind(v)
                .execute(&pool)
                .await
                .unwrap();
        }

        ProxyState {
            pool,
            scanner: Arc::new(RuleScanner::new()),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap(),
            stream_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap(),
        }
    }

    fn auth_headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, format!("Bearer {KEY_SECRET}").parse().unwrap());
        h.insert(USER_AGENT, "gateway-test/1.0".parse().unwrap());
        h
    }

    const UPSTREAM_OK: &str = r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"index":0,"message":{"role":"assistant","content":"你好"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}}"#;

    #[tokio::test]
    async fn benign_request_under_medium_level_stores_summary_only() {
        let state = setup("medium", UPSTREAM_OK).await;
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "今天天气不错" }]
        });
        let resp = handle_chat_completion(
            State(state.clone()),
            auth_headers(),
            Bytes::from(serde_json::to_vec(&body).unwrap()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let (risk, count): (String, i64) = sqlx::query_as(
            "SELECT risk_level, (SELECT count(*) FROM request_details d WHERE d.request_id = r.id) FROM requests r LIMIT 1",
        )
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(risk, "none");
        assert_eq!(count, 0, "无风险请求在 medium 级别下不应保存内容明细");
    }

    #[tokio::test]
    async fn risky_request_under_medium_level_stores_masked_details() {
        let state = setup("medium", UPSTREAM_OK).await;
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": format!("我的密钥是 {KEY_SECRET}，请帮我看看") }]
        });
        let resp = handle_chat_completion(
            State(state.clone()),
            auth_headers(),
            Bytes::from(serde_json::to_vec(&body).unwrap()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let row: (String, String) = sqlx::query_as(
            "SELECT r.risk_level, d.request_body FROM requests r JOIN request_details d ON d.request_id = r.id LIMIT 1",
        )
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(row.0, "high");
        assert!(row.1.contains("[MASKED:api_key_llmapi]"), "落库内容必须已脱敏");
        assert!(!row.1.contains(KEY_SECRET), "落库内容不应包含原始密钥");

        // 安全事件同样落库（命中记录）
        let findings: i64 =
            sqlx::query_scalar("SELECT count(*) FROM security_logs")
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert!(findings >= 1);
    }

    #[tokio::test]
    async fn none_level_never_stores_details() {
        let state = setup("none", UPSTREAM_OK).await;
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": format!("密钥 {KEY_SECRET}") }]
        });
        let resp = handle_chat_completion(
            State(state.clone()),
            auth_headers(),
            Bytes::from(serde_json::to_vec(&body).unwrap()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM request_details")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        // 概要仍记录
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM requests")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }
}
