//! SSE 流式透传：行缓冲解析、token 统计（精确 usage 优先，估算兜底）、delta 脱敏、流结束记账。

use std::sync::Arc;
use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use sqlx::SqlitePool;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::commands::requests::log_request;
use crate::commands::requests::{RequestDetailInput, RequestLogInput};
use crate::scheduler::ChannelChoice;
use crate::security::leak::check_leak;
use crate::security::{decide, Finding, PolicyDecision, RiskLevel, SecurityScanner};

use super::forward::{compute_cost, extract_usage, UsageTokens};
use super::handlers::{record_security_logs, risk_label, risk_of};
use super::protocol::{self, ClientProtocol, StreamTranslator};
use super::settings::content_allowed;
use super::{PreparedRequest, ProxyState};

/// SSE 透传：后台任务读取上游字节流 → 行缓冲 → 统计/脱敏 → 按客户端协议翻译 → 转发；流结束记账
pub(crate) async fn stream_response(
    state: &ProxyState,
    upstream: reqwest::Response,
    channel: &ChannelChoice,
    prepared: &PreparedRequest,
    started: std::time::Instant,
    proto: ClientProtocol,
    attempts: i64,
) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);

    // 按模型定价 + DeepSeek 高峰/空闲时段自动折算
    let price = crate::models::resolve_model_price(
        &channel.model_prices,
        &channel.protocol,
        &prepared.model,
        &crate::commands::util::now_iso(),
        (
            channel.price_per_1k_tokens,
            channel.cache_price_per_1k_tokens,
            channel.price_per_1k_tokens,
            channel.reasoning_price_per_1k_tokens,
        ),
    );

    let mut stats = StreamStats::new(
        state.pool.clone(),
        state.scanner.clone(),
        &channel.id,
        &channel.name,
        prepared,
        price.input,
        price.cache_hit,
        price.output,
        price.reasoning,
        started,
        proto,
        attempts,
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
                    if splitter.overflow {
                        stats.finish_failed("SSE 单行超过 1 MiB，已中断连接（上游响应异常）").await;
                        break;
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

/// SSE 行缓冲：网络 chunk 可能跨行，累积到完整行再处理。
/// 单行上限 1 MiB：防御永不发换行的异常上游把缓冲撑爆（超限直接中断流并记账）。
struct SseLineSplitter {
    buf: Vec<u8>,
    overflow: bool,
}

/// 单条 SSE 行最大字节数
const MAX_SSE_LINE: usize = 1024 * 1024;

impl SseLineSplitter {
    fn new() -> Self {
        Self { buf: Vec::new(), overflow: false }
    }

    /// 喂入字节，返回所有完整行（含行尾 \n）；缓冲超过单行上限时置 overflow 并清空
    fn feed(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(bytes);
        if self.buf.len() > MAX_SSE_LINE {
            self.overflow = true;
            self.buf.clear();
            return Vec::new();
        }
        let mut lines = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            lines.push(self.buf.drain(..=pos).collect());
        }
        lines
    }
}

/// 流式统计器：token 统计（精确 usage 优先，估算兜底）+ 响应侧脱敏 + 协议翻译 + 流结束记账
struct StreamStats {
    pool: SqlitePool,
    scanner: Arc<dyn SecurityScanner>,
    channel_id: String,
    channel_name: String,
    key_id: String,
    key_name: String,
    model: String,
    input_price: f64,
    cache_price: f64,
    output_price: f64,
    reasoning_price: f64,
    security_enabled: bool,
    security_mode: String,
    request_findings: Vec<Finding>,
    /// 请求侧 high 命中的原文片段（响应侧上下文泄露比对用，不落库）
    request_secrets: Vec<String>,
    prompt_chars: (u64, u64),
    completion_chars: (u64, u64),
    exact_usage: Option<UsageTokens>,
    response_findings: Vec<Finding>,
    started: Instant,
    finished: bool,
    /// 客户端协议；OpenAI 时无翻译器（透传），Anthropic/Responses 时逐事件翻译
    proto: ClientProtocol,
    translator: Option<StreamTranslator>,
    // ---- 内容记录（0010）----
    /// 客户端 UA / 本次尝试序号
    client_ua: String,
    attempts: i64,
    /// 收到的原始请求（脱敏后）与转发体（脱敏后）
    request_body: Option<String>,
    upstream_body: Option<String>,
    /// 流式响应重建器（把 SSE delta 拼回完整响应 JSON 供落库）
    rebuild: Option<SseRebuilder>,
    /// 内容明细存储级别（全部/低/中/高风险及以上/不存），流结束按最终风险判定
    log_store_level: String,
    /// 是否因安全策略中断（用于落库状态与错误信息）
    blocked: bool,
}

impl StreamStats {
    #[allow(clippy::too_many_arguments)]
    fn new(
        pool: SqlitePool,
        scanner: Arc<dyn SecurityScanner>,
        channel_id: &str,
        channel_name: &str,
        prepared: &PreparedRequest,
        input_price: f64,
        cache_price: f64,
        output_price: f64,
        reasoning_price: f64,
        started: Instant,
        proto: ClientProtocol,
        attempts: i64,
    ) -> Self {
        // 非 OpenAI 协议：为 message_start / response.created 准备 input_tokens（估算）
        let translator = match proto {
            ClientProtocol::OpenAi => None,
            p => {
                let input_tokens = estimate_tokens(prepared.prompt_chars.0, prepared.prompt_chars.1);
                Some(StreamTranslator::new(p, &prepared.model, input_tokens))
            }
        };
        Self {
            pool,
            scanner,
            channel_id: channel_id.to_string(),
            channel_name: channel_name.to_string(),
            key_id: prepared.auth.id.clone(),
            key_name: prepared.auth.name.clone(),
            model: prepared.model.clone(),
            input_price,
            cache_price,
            output_price,
            reasoning_price,
            security_enabled: prepared.settings.security_enabled,
            security_mode: prepared.settings.security_mode.clone(),
            request_findings: prepared.request_findings.clone(),
            request_secrets: prepared.request_secrets.clone(),
            prompt_chars: prepared.prompt_chars,
            completion_chars: (0, 0),
            exact_usage: None,
            response_findings: Vec::new(),
            started,
            finished: false,
            proto,
            translator,
            client_ua: prepared.client_ua.clone(),
            attempts,
            request_body: prepared.request_body.clone(),
            upstream_body: prepared.upstream_body.clone(),
            rebuild: if prepared.settings.content_possible() {
                Some(SseRebuilder::default())
            } else {
                None
            },
            log_store_level: prepared.settings.log_store_level.clone(),
            blocked: false,
        }
    }

    /// 处理一个上游 chunk：解析 SSE 行、统计 token、脱敏 delta 内容、按协议翻译。
    /// 返回 (透传字节, 是否继续流)；安全拦截时向客户端注入协议 error 事件并返回 keep=false。
    async fn process(&mut self, bytes: &[u8], splitter: &mut SseLineSplitter) -> (Bytes, bool) {
        let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
        for line in splitter.feed(bytes) {
            // SSE 规范允许 "data:" 后不带空格（如 data:{"id":...}），两种都解析
            let Some(rest) = line.strip_prefix(b"data:") else {
                // 非 data 行：OpenAI 透传；其他协议丢弃（客户端不认上游的 event: 行）
                if self.proto == ClientProtocol::OpenAi {
                    out.extend_from_slice(&line);
                }
                continue;
            };
            let payload = rest.strip_prefix(b" ").unwrap_or(rest);
            if payload.trim_ascii() == b"[DONE]" {
                if self.proto == ClientProtocol::OpenAi {
                    out.extend_from_slice(&line);
                } else if let Some(t) = &mut self.translator {
                    out.extend_from_slice(&t.on_done());
                }
                continue;
            }
            let mut event: Value = match serde_json::from_slice(payload) {
                Ok(v) => v,
                Err(_) => {
                    if self.proto == ClientProtocol::OpenAi {
                        out.extend_from_slice(&line);
                    }
                    continue;
                }
            };

            // 精确 token 统计（stream_options.include_usage 时上游在流尾返回）
            if let Some(usage) = event.get("usage") {
                self.exact_usage = Some(extract_usage(Some(usage)));
            }

            // 所有 choices 的增量：正文 / 工具调用参数都做字符统计 + 扫描脱敏 + 泄露比对
            let mut blocked_now = false;
            if let Some(choices) = event.get_mut("choices").and_then(|c| c.as_array_mut()) {
                for choice in choices.iter_mut() {
                    if let Some(content) = choice.pointer_mut("/delta/content") {
                        if let Value::String(s) = content {
                            let (a, b) = count_chars(s);
                            self.completion_chars.0 += a;
                            self.completion_chars.1 += b;
                            if self.security_enabled {
                                let result = self.scanner.scan(s).await;
                                self.response_findings.extend(result.findings.clone());
                                if let Some(leak) = check_leak(s, &self.request_secrets) {
                                    self.response_findings.push(leak);
                                }
                                if (self.security_mode == "block" && result.risk >= RiskLevel::High)
                                    || decide(&result.findings) == PolicyDecision::Block
                                {
                                    blocked_now = true;
                                }
                                *s = result.sanitized;
                            }
                        }
                    }
                    // 工具调用参数（流式分片）：同样扫描脱敏
                    if let Some(tcs) = choice
                        .pointer_mut("/delta/tool_calls")
                        .and_then(|t| t.as_array_mut())
                    {
                        for tc in tcs.iter_mut() {
                            let Some(args) = tc.pointer_mut("/function/arguments") else {
                                continue;
                            };
                            let Value::String(s) = args else {
                                continue;
                            };
                            if !self.security_enabled {
                                continue;
                            }
                            let result = self.scanner.scan(s).await;
                            self.response_findings.extend(result.findings.clone());
                            if let Some(leak) = check_leak(s, &self.request_secrets) {
                                self.response_findings.push(leak);
                            }
                            if (self.security_mode == "block" && result.risk >= RiskLevel::High)
                                || decide(&result.findings) == PolicyDecision::Block
                            {
                                blocked_now = true;
                            }
                            *s = result.sanitized;
                        }
                    }
                }
            }
            if blocked_now {
                self.blocked = true;
                match &self.translator {
                    Some(t) => out.extend_from_slice(&t.error_event(
                        "安全策略拦截：响应包含高风险内容，已中断",
                    )),
                    None => out.extend_from_slice(&protocol::openai_error_event(
                        "安全策略拦截：响应包含高风险内容，已中断",
                    )),
                }
                return (Bytes::from(out), false);
            }

            // 内容记录：把脱敏后的上游事件喂给重建器（客户端协议无关——上游恒为 OpenAI 格式）
            if let Some(rebuild) = &mut self.rebuild {
                rebuild.feed(&event);
            }

            // 输出：OpenAI 原样透传；其他协议经翻译器逐事件转换
            match &mut self.translator {
                None => {
                    out.extend_from_slice(b"data: ");
                    out.extend_from_slice(serde_json::to_string(&event).unwrap_or_default().as_bytes());
                    out.push(b'\n');
                }
                Some(t) => {
                    out.extend_from_slice(&t.on_event(&event));
                }
            }
        }
        (Bytes::from(out), true)
    }

    /// 流正常结束：记账（status=1）；安全拦截中断时记 status=0 + 原因
    async fn finish_ok(&mut self) {
        if self.blocked {
            self.finish(
                0,
                Some("安全策略拦截：响应包含高风险内容，已中断（response_security_blocked）".to_string()),
            )
            .await;
        } else {
            self.finish(1, None).await;
        }
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
        let tokens = match self.exact_usage {
            Some(t) => t,
            None => UsageTokens {
                prompt: estimate_tokens(self.prompt_chars.0, self.prompt_chars.1),
                completion: estimate_tokens(self.completion_chars.0, self.completion_chars.1),
                cache_hit: 0, // 估算路径拿不到缓存明细
                reasoning: 0, // 估算路径拿不到思考明细
            },
        };
        let cost = compute_cost(tokens, self.input_price, self.cache_price, self.output_price, self.reasoning_price);
        let latency = self.started.elapsed().as_millis() as i64;
        // 风险：请求侧 + 响应侧最高等级；内容是否落库由 log.store_level 决定
        let risk = risk_of(&self.request_findings, &self.response_findings);
        let response_body = self
            .rebuild
            .as_mut()
            .and_then(|r| r.build())
            .map(crate::proxy::handlers::clamp_json_body);
        let details = if content_allowed(&self.log_store_level, risk)
            && (self.request_body.is_some() || self.upstream_body.is_some() || response_body.is_some())
        {
            Some(RequestDetailInput {
                request_body: self.request_body.as_deref(),
                upstream_body: self.upstream_body.as_deref(),
                response_body: response_body.as_deref(),
                error_body: None,
            })
        } else {
            None
        };
        let input = RequestLogInput {
            channel_id: Some(&self.channel_id),
            channel_name: Some(&self.channel_name),
            key_id: Some(&self.key_id),
            key_name: Some(&self.key_name),
            model: &self.model,
            status,
            latency_ms: latency,
            prompt_tokens: tokens.prompt,
            completion_tokens: tokens.completion,
            cache_hit_tokens: tokens.cache_hit,
            reasoning_tokens: tokens.reasoning,
            cost,
            error: error.as_deref(),
            protocol: self.proto.as_str(),
            stream: true,
            client_ua: &self.client_ua,
            attempts: self.attempts,
            risk_level: risk_label(risk),
            details,
        };
        match log_request(&self.pool, &input).await {
            Ok(req_id) => {
                record_security_logs(&self.pool, &req_id, &self.request_findings, &self.response_findings).await;
            }
            Err(e) => eprintln!("[proxy] 流式记账失败: {e}"),
        }
    }
}

// ---------------- 流式响应重建（内容记录用） ----------------

/// 把上游 OpenAI 格式的 SSE delta 事件拼回一条完整的 chat.completion 响应 JSON。
/// 支持多 choice 与工具调用（tool_calls 按 index 分片累积）。
#[derive(Default)]
struct SseRebuilder {
    id: Option<String>,
    created: Option<i64>,
    model: Option<String>,
    choices: Vec<RebuiltChoice>,
    usage: Option<Value>,
}

#[derive(Default)]
struct RebuiltChoice {
    index: i64,
    content: String,
    tool_calls: Vec<RebuiltToolCall>,
    finish_reason: Option<String>,
}

#[derive(Default)]
struct RebuiltToolCall {
    index: i64,
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl SseRebuilder {
    /// 喂入一个上游 data 事件（已脱敏），提取并拼接内容
    fn feed(&mut self, event: &Value) {
        if self.id.is_none() {
            if let Some(id) = event.get("id").and_then(|v| v.as_str()) {
                self.id = Some(id.to_string());
            }
            if let Some(m) = event.get("model").and_then(|v| v.as_str()) {
                self.model = Some(m.to_string());
            }
            if let Some(c) = event.get("created").and_then(|v| v.as_i64()) {
                self.created = Some(c);
            }
        }
        // usage（include_usage 时流尾独立事件）
        if let Some(u) = event.get("usage") {
            self.usage = Some(u.clone());
        }
        let Some(choices) = event.get("choices").and_then(|v| v.as_array()) else {
            return;
        };
        for c in choices {
            let idx = c.get("index").and_then(|v| v.as_i64()).unwrap_or(0);
            if !self.choices.iter().any(|x| x.index == idx) {
                self.choices.push(RebuiltChoice { index: idx, ..Default::default() });
            }
            let acc = self.choices.iter_mut().find(|x| x.index == idx).expect("存在");
            // 文本增量
            if let Some(text) = c
                .pointer("/delta/content")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                acc.content.push_str(text);
            }
            // 工具调用增量（按调用 index 分片）
            if let Some(tcs) = c.pointer("/delta/tool_calls").and_then(|v| v.as_array()) {
                for tc in tcs {
                    let t_idx = tc.get("index").and_then(|v| v.as_i64()).unwrap_or(0);
                    if !acc.tool_calls.iter().any(|t| t.index == t_idx) {
                        acc.tool_calls.push(RebuiltToolCall { index: t_idx, ..Default::default() });
                    }
                    let t = acc.tool_calls.iter_mut().find(|t| t.index == t_idx).expect("存在");
                    if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                        t.id = Some(id.to_string());
                    }
                    if let Some(name) = tc.pointer("/function/name").and_then(|v| v.as_str()) {
                        t.name = Some(name.to_string());
                    }
                    if let Some(args) = tc.pointer("/function/arguments").and_then(|v| v.as_str()) {
                        t.arguments.push_str(args);
                    }
                }
            }
            // 结束原因（最后一个非 null 值）
            if let Some(fr) = c.get("finish_reason").and_then(|v| v.as_str()) {
                acc.finish_reason = Some(fr.to_string());
            }
        }
    }

    /// 拼成完整响应 JSON（无任何内容时返回 None）
    fn build(&self) -> Option<String> {
        if self.choices.is_empty() && self.usage.is_none() {
            return None;
        }
        let choices: Vec<Value> = self
            .choices
            .iter()
            .map(|c| {
                let tool_calls: Vec<Value> = c
                    .tool_calls
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "id": t.id.clone().unwrap_or_default(),
                            "type": "function",
                            "function": {
                                "name": t.name.clone().unwrap_or_default(),
                                "arguments": t.arguments,
                            }
                        })
                    })
                    .collect();
                let mut message = serde_json::json!({ "role": "assistant", "content": c.content });
                if !tool_calls.is_empty() {
                    message["tool_calls"] = Value::Array(tool_calls);
                }
                let mut choice = serde_json::json!({ "index": c.index, "message": message });
                if let Some(fr) = &c.finish_reason {
                    choice["finish_reason"] = Value::String(fr.clone());
                }
                choice
            })
            .collect();
        let mut body = serde_json::json!({
            "id": self.id.clone().unwrap_or_default(),
            "object": "chat.completion",
            "created": self.created.unwrap_or(0),
            "model": self.model.clone().unwrap_or_default(),
            "choices": choices,
        });
        if let Some(u) = &self.usage {
            body["usage"] = u.clone();
        }
        Some(serde_json::to_string(&body).unwrap_or_default())
    }
}

/// 统计字符串中 ASCII / 非 ASCII 字符数
pub(crate) fn count_chars(s: &str) -> (u64, u64) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::test_util::test_pool;
    use crate::security::RuleScanner;

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

    fn stream_stats(pool: SqlitePool, security_mode: &str) -> StreamStats {
        let mut settings = crate::proxy::settings::ProxySettings::default();
        settings.security_enabled = true;
        settings.security_mode = security_mode.to_string();
        let prepared = PreparedRequest {
            auth: crate::proxy::auth::AuthKey { id: "k1".into(), name: "测试密钥".into() },
            settings,
            body: serde_json::json!({}),
            model: "gpt-4o".into(),
            queue: vec![],
            request_findings: vec![],
            request_secrets: vec![],
            prompt_chars: (0, 0),
            client_ua: String::new(),
            request_body: None,
            upstream_body: None,
        };
        StreamStats::new(
            pool,
            Arc::new(RuleScanner::new()),
            "c1",
            "测试渠道",
            &prepared,
            0.0,
            0.0,
            0.0,
            0.0,
            Instant::now(),
            ClientProtocol::OpenAi,
            1,
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

        // usage 行：记录精确 token（含缓存明细解析）
        let usage = format!(
            "data: {}\n\n",
            serde_json::json!({
                "choices": [],
                "usage": {
                    "prompt_tokens": 11,
                    "prompt_cache_hit_tokens": 5,
                    "completion_tokens": 22,
                    "total_tokens": 33
                }
            })
        );
        let (_, keep2) = stats.process(usage.as_bytes(), &mut splitter).await;
        assert!(keep2);
        assert_eq!(
            stats.exact_usage,
            Some(UsageTokens { prompt: 11, completion: 22, cache_hit: 5, reasoning: 0 })
        );

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

    // ---- SseRebuilder ----

    #[test]
    fn rebuilder_reconstructs_full_response_with_tools() {
        let mut r = SseRebuilder::default();
        // 首 chunk：id/model/created + role + 文本开头
        r.feed(&serde_json::json!({
            "id": "chatcmpl-abc",
            "object": "chat.completion.chunk",
            "created": 1710000000,
            "model": "gpt-4o",
            "choices": [{"index": 0, "delta": {"role": "assistant", "content": "你好"}, "finish_reason": null}]
        }));
        // 文本续段
        r.feed(&serde_json::json!({
            "choices": [{"index": 0, "delta": {"content": "，世界"}, "finish_reason": null}]
        }));
        // 工具调用分片（index=0 调用：id/name 首段）
        r.feed(&serde_json::json!({
            "choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "call_x", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":"}}]}, "finish_reason": null}]
        }));
        // 工具调用 arguments 续片 + 结束原因
        r.feed(&serde_json::json!({
            "choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "function": {"arguments": "\"北京\"}"}}]}, "finish_reason": "tool_calls"}]
        }));
        // usage 事件
        r.feed(&serde_json::json!({
            "choices": [],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        }));

        let json = r.build().expect("应有重建内容");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["id"], "chatcmpl-abc");
        assert_eq!(v["model"], "gpt-4o");
        assert_eq!(v["choices"][0]["message"]["role"], "assistant");
        assert_eq!(v["choices"][0]["message"]["content"], "你好，世界");
        assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(v["choices"][0]["message"]["tool_calls"][0]["id"], "call_x");
        assert_eq!(
            v["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"city\":\"北京\"}"
        );
        assert_eq!(v["usage"]["total_tokens"], 15);
    }

    #[test]
    fn rebuilder_empty_stream_yields_none() {
        let r = SseRebuilder::default();
        assert!(r.build().is_none());
    }

    #[test]
    fn rebuilder_skips_empty_content_deltas() {
        let mut r = SseRebuilder::default();
        // 上游首 chunk 常带 content:""，不应影响重建（空增量跳过）
        r.feed(&serde_json::json!({
            "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": null}]
        }));
        r.feed(&serde_json::json!({
            "choices": [{"index": 0, "delta": {"content": "结果"}, "finish_reason": "stop"}]
        }));
        let json = r.build().expect("应有重建内容");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["choices"][0]["message"]["content"], "结果");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
    }
}
