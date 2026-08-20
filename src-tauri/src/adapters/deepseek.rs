use super::openai_compat::{build_client, OpenAiCompatible};
use super::{AdapterError, ChatRequest, ChatResponse, ChannelAdapter, TestResult};

/// DeepSeek 渠道适配器。
///
/// 与 OpenAI 各自独立：只共享 `OpenAiCompatible` 协议基类（因为 DeepSeek 走 OpenAI 兼容协议），
/// 不依赖 `OpenAiAdapter` 类型。两家厂商可独立演化——
/// 例如 DeepSeek 的 `deepseek-reasoner` 响应含推理内容，将来可在此处单独处理。
pub struct DeepSeekAdapter {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl DeepSeekAdapter {
    pub fn new(base_url: String, api_key: String) -> Self {
        Self {
            base_url,
            api_key,
            client: build_client(),
        }
    }
}

#[async_trait::async_trait]
impl ChannelAdapter for DeepSeekAdapter {
    fn channel_type(&self) -> &str {
        "DeepSeek"
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatResponse, AdapterError> {
        <Self as OpenAiCompatible>::compat_chat_completion(self, req).await
    }

    async fn test(&self) -> Result<TestResult, AdapterError> {
        <Self as OpenAiCompatible>::compat_test(self).await
    }
}

#[async_trait::async_trait]
impl OpenAiCompatible for DeepSeekAdapter {
    fn vendor_name(&self) -> &str {
        "DeepSeek"
    }

    fn api_key(&self) -> &str {
        &self.api_key
    }

    fn default_base_url(&self) -> &str {
        "https://api.deepseek.com"
    }

    fn default_model(&self) -> &str {
        "deepseek-chat"
    }

    fn client(&self) -> &reqwest::Client {
        &self.client
    }
}
