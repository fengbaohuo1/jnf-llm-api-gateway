use super::openai::OpenAiAdapter;
use super::{AdapterError, ChatMessage, ChatRequest, ChatResponse, ChannelAdapter, TestResult};

/// DeepSeek 适配器。
/// DeepSeek 使用 OpenAI 兼容协议，但有自己的默认端点与默认模型：
/// - 默认 Base URL：`https://api.deepseek.com`
/// - 默认模型：`deepseek-chat`（另有 `deepseek-reasoner`，其响应含推理内容）
///
/// 实现方式：组合复用 `OpenAiAdapter` 的协议逻辑，仅覆盖渠道标识与测试默认值。
pub struct DeepSeekAdapter {
    inner: OpenAiAdapter,
}

impl DeepSeekAdapter {
    pub fn new(base_url: String, api_key: String) -> Self {
        // DeepSeek 官方端点不带 /v1 也可（两者都支持）；留空则用官方默认
        let base = if base_url.trim().is_empty() {
            "https://api.deepseek.com".to_string()
        } else {
            base_url
        };
        Self {
            inner: OpenAiAdapter::new(base, api_key),
        }
    }
}

#[async_trait::async_trait]
impl ChannelAdapter for DeepSeekAdapter {
    fn channel_type(&self) -> &str {
        "DeepSeek"
    }

    fn base_url(&self) -> &str {
        self.inner.base_url()
    }

    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatResponse, AdapterError> {
        self.inner.chat_completion(req).await
    }

    async fn test(&self) -> Result<TestResult, AdapterError> {
        let req = ChatRequest {
            model: "deepseek-chat".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "ping".to_string(),
            }],
            temperature: Some(0.0),
            max_tokens: Some(1),
            stream: false,
        };
        let resp = self.inner.chat_completion(req).await?;
        Ok(TestResult {
            ok: true,
            latency_ms: Some(resp.latency_ms),
            message: format!(
                "连接正常（{}），模型 {} 可用（{}ms）",
                self.base_url(),
                resp.model,
                resp.latency_ms
            ),
        })
    }
}
