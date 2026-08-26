//! 向上游透传转发与错误分类。

use serde_json::Value;

use crate::scheduler::ChannelChoice;

use super::ProxyState;

/// 转发错误：retryable=false 表示配置错误（4xx），转移无意义，直接返回
#[derive(Debug)]
pub(crate) struct ForwardError {
    pub(crate) retryable: bool,
    pub(crate) message: String,
}

/// 透传转发（非流式）：请求体原样发给上游（只改 URL 与 Authorization），响应原样返回
pub(crate) async fn forward_to_channel(
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

/// 向上游发起流式请求；仅返回已确认 200 的响应（非 2xx 按可转移/不可转移分类）
pub(crate) async fn forward_stream_to_channel(
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

/// 上游 usage 中的 token 明细（缓存/思考感知计费的输入）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct UsageTokens {
    pub prompt: i64,
    pub completion: i64,
    /// 输入中命中前缀缓存的 token 数（≤ prompt）
    pub cache_hit: i64,
    /// 输出中用于推理（thinking）的 token 数（≤ completion）
    pub reasoning: i64,
}

/// 从上游响应的 usage 中提取 token 明细（缺失按 0 计）。
/// 明细字段兼容主流厂商格式：
/// - 缓存命中：DeepSeek `prompt_cache_hit_tokens`；OpenAI `prompt_tokens_details.cached_tokens`
/// - 思考 token：DeepSeek-R1 / OpenAI o 系列 `completion_tokens_details.reasoning_tokens`
pub(crate) fn extract_usage(usage: Option<&Value>) -> UsageTokens {
    let get = |k: &str| {
        usage
            .and_then(|u| u.get(k))
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
    };
    let prompt = get("prompt_tokens");
    let completion = get("completion_tokens");
    let cache_hit = get("prompt_cache_hit_tokens").max(
        usage
            .and_then(|u| u.get("prompt_tokens_details"))
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
    );
    let reasoning = usage
        .and_then(|u| u.get("completion_tokens_details"))
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    UsageTokens {
        prompt,
        completion,
        cache_hit: cache_hit.min(prompt),       // 命中不可能超过输入总数
        reasoning: reasoning.min(completion),   // 思考不可能超过输出总数
    }
}

/// 缓存/思考感知计费：
/// - 输入：未命中部分 × 正常单价，命中部分 × 缓存单价
/// - 输出：思考部分 × 思考单价，其余 × 正常单价
/// 缓存单价 / 思考单价 ≤ 0 视为未配置，回落到正常单价（与旧行为一致）。
pub(crate) fn compute_cost(tokens: UsageTokens, price: f64, cache_price: f64, reasoning_price: f64) -> f64 {
    let cache_price = if cache_price > 0.0 { cache_price } else { price };
    let reasoning_price = if reasoning_price > 0.0 { reasoning_price } else { price };
    let miss = (tokens.prompt - tokens.cache_hit).max(0);
    let normal_out = (tokens.completion - tokens.reasoning).max(0);
    (miss as f64 * price
        + tokens.cache_hit as f64 * cache_price
        + normal_out as f64 * price
        + tokens.reasoning as f64 * reasoning_price)
        / 1000.0
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::test_util::{choice, mock_upstream};

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
    async fn extract_usage_handles_missing() {
        let t = extract_usage(None);
        assert_eq!(t, UsageTokens::default());
    }

    #[tokio::test]
    async fn extract_usage_deepseek_format() {
        // DeepSeek：缓存命中平铺在 usage；思考 token 在 completion_tokens_details
        let usage = serde_json::json!({
            "prompt_tokens": 100,
            "prompt_cache_hit_tokens": 80,
            "prompt_cache_miss_tokens": 20,
            "completion_tokens": 50,
            "completion_tokens_details": { "reasoning_tokens": 30 }
        });
        let t = extract_usage(Some(&usage));
        assert_eq!(t.prompt, 100);
        assert_eq!(t.completion, 50);
        assert_eq!(t.cache_hit, 80);
        assert_eq!(t.reasoning, 30);
    }

    #[tokio::test]
    async fn extract_usage_openai_format() {
        // OpenAI：prompt_tokens_details.cached_tokens 嵌套
        let usage = serde_json::json!({
            "prompt_tokens": 100,
            "prompt_tokens_details": { "cached_tokens": 75 },
            "completion_tokens": 50,
            "completion_tokens_details": { "reasoning_tokens": 20 }
        });
        let t = extract_usage(Some(&usage));
        assert_eq!(t.cache_hit, 75);
        assert_eq!(t.reasoning, 20);
    }

    #[tokio::test]
    async fn extract_usage_clamps_details() {
        // 上游数据异常：命中/思考数超过总数，截断
        let usage = serde_json::json!({
            "prompt_tokens": 10,
            "prompt_cache_hit_tokens": 999,
            "completion_tokens": 5,
            "completion_tokens_details": { "reasoning_tokens": 999 }
        });
        let t = extract_usage(Some(&usage));
        assert_eq!(t.cache_hit, 10);
        assert_eq!(t.reasoning, 5);
    }

    #[test]
    fn compute_cost_splits_cache_pricing() {
        // 无缓存/思考单价配置：全部按正常单价（与旧行为一致）
        let tokens = UsageTokens { prompt: 1000, completion: 500, cache_hit: 600, reasoning: 0 };
        assert_eq!(compute_cost(tokens, 2.0, 0.0, 0.0), 3.0); // (1000+500)/1000×2
        // 有缓存单价：命中部分用缓存价
        // miss=400×2 + hit=600×0.2 + completion=500×2 = 800+120+1000 = 1920 → 1.92
        assert!((compute_cost(tokens, 2.0, 0.2, 0.0) - 1.92).abs() < 1e-9);
    }

    #[test]
    fn compute_cost_splits_reasoning_pricing() {
        let tokens = UsageTokens { prompt: 0, completion: 1000, cache_hit: 0, reasoning: 300 };
        // 思考部分用思考单价：300×0.5 + 普通输出 700×2 = 150+1400 = 1550 → 1.55
        assert!((compute_cost(tokens, 2.0, 0.0, 0.5) - 1.55).abs() < 1e-9);
        // 思考单价未配置：全部按正常输出价 → 1000×2/1000 = 2.0
        assert_eq!(compute_cost(tokens, 2.0, 0.0, 0.0), 2.0);
    }

    #[test]
    fn compute_cost_zero_tokens() {
        let tokens = UsageTokens::default();
        assert_eq!(compute_cost(tokens, 2.0, 0.2, 0.5), 0.0);
    }

    #[test]
    fn truncate_limits_length() {
        assert_eq!(truncate("abc", 10), "abc");
        assert!(truncate(&"x".repeat(500), 100).chars().count() <= 101);
    }
}
