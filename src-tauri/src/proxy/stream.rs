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
use crate::scheduler::ChannelChoice;
use crate::security::{Finding, RiskLevel, SecurityScanner};

use super::forward::{compute_cost, extract_usage, UsageTokens};
use super::handlers::record_security_logs;
use super::{PreparedRequest, ProxyState};

/// SSE 透传：后台任务读取上游字节流 → 行缓冲 → 统计/脱敏 → 转发给客户端；流结束记账
pub(crate) async fn stream_response(
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
        channel.cache_price_per_1k_tokens,
        channel.reasoning_price_per_1k_tokens,
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
    cache_price: f64,
    reasoning_price: f64,
    security_enabled: bool,
    security_mode: String,
    request_findings: Vec<Finding>,
    prompt_chars: (u64, u64),
    completion_chars: (u64, u64),
    exact_usage: Option<UsageTokens>,
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
        cache_price: f64,
        reasoning_price: f64,
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
            cache_price,
            reasoning_price,
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
                self.exact_usage = Some(extract_usage(Some(usage)));
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
        let tokens = match self.exact_usage {
            Some(t) => t,
            None => UsageTokens {
                prompt: estimate_tokens(self.prompt_chars.0, self.prompt_chars.1),
                completion: estimate_tokens(self.completion_chars.0, self.completion_chars.1),
                cache_hit: 0, // 估算路径拿不到缓存明细
                reasoning: 0, // 估算路径拿不到思考明细
            },
        };
        let cost = compute_cost(tokens, self.price, self.cache_price, self.reasoning_price);
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
            tokens.prompt,
            tokens.completion,
            tokens.cache_hit,
            tokens.reasoning,
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
        StreamStats::new(
            pool,
            Arc::new(RuleScanner::new()),
            "c1",
            "测试渠道",
            "k1",
            "测试密钥",
            "gpt-4o",
            0.0,
            0.0,
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
}
