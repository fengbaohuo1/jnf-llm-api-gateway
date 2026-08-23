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

/// 从上游响应的 usage 中提取 token 数（缺失按 0 计）
pub(crate) fn extract_tokens(usage: Option<&Value>) -> (i64, i64) {
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
