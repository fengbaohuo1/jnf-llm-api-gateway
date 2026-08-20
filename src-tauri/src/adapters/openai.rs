use super::openai_compat::{build_client, OpenAiCompatible};
use super::{AdapterError, ChatRequest, ChatResponse, ChannelAdapter, TestResult};

/// OpenAI 官方渠道适配器。
/// 协议逻辑继承自 `OpenAiCompatible` 基类，这里只提供 OpenAI 的厂商配置。
pub struct OpenAiAdapter {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl OpenAiAdapter {
    pub fn new(base_url: String, api_key: String) -> Self {
        Self {
            base_url,
            api_key,
            client: build_client(),
        }
    }
}

#[async_trait::async_trait]
impl ChannelAdapter for OpenAiAdapter {
    fn channel_type(&self) -> &str {
        "OpenAI"
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
impl OpenAiCompatible for OpenAiAdapter {
    fn vendor_name(&self) -> &str {
        "OpenAI"
    }

    fn api_key(&self) -> &str {
        &self.api_key
    }

    fn default_base_url(&self) -> &str {
        "https://api.openai.com/v1"
    }

    fn default_model(&self) -> &str {
        "gpt-3.5-turbo"
    }

    fn client(&self) -> &reqwest::Client {
        &self.client
    }
}
