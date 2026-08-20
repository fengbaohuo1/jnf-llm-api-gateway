use serde::{Deserialize, Serialize};
use std::time::Instant;

use super::{AdapterError, ChatMessage, ChatRequest, ChatResponse, ChatUsage, ChannelAdapter, TestResult, REQUEST_TIMEOUT};

/// OpenAI 兼容协议适配器。
/// 适用于 OpenAI / DeepSeek / 以及任何 OpenAI 兼容的自定义渠道。
pub struct OpenAiAdapter {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl OpenAiAdapter {
    pub fn new(base_url: String, api_key: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("failed to build reqwest client");
        Self { base_url, api_key, client }
    }

    fn chat_endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
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
        let started = Instant::now();

        let body = OpenAiChatBody {
            model: req.model.clone(),
            messages: req.messages.iter().map(|m| OpenAiMessage { role: m.role.clone(), content: m.content.clone() }).collect(),
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            stream: req.stream,
        };

        let resp = self
            .client
            .post(self.chat_endpoint())
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        let bytes = resp.bytes().await?;
        if !status.is_success() {
            let text = String::from_utf8_lossy(&bytes).to_string();
            return Err(AdapterError::Upstream { status: status.as_u16(), body: text });
        }

        let parsed: OpenAiChatResponse = serde_json::from_slice(&bytes)
            .map_err(|e| AdapterError::Parse(format!("OpenAI 响应解析失败: {e}")))?;

        let content = parsed
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();
        let finish_reason = parsed.choices.first().and_then(|c| c.finish_reason.clone());

        let usage = parsed.usage.map(|u| ChatUsage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        });

        Ok(ChatResponse {
            id: parsed.id.unwrap_or_default(),
            model: parsed.model.unwrap_or(req.model),
            content,
            usage,
            finish_reason,
            latency_ms: started.elapsed().as_millis() as u64,
        })
    }

    async fn test(&self) -> Result<TestResult, AdapterError> {
        let req = ChatRequest {
            model: "gpt-3.5-turbo".to_string(),
            messages: vec![ChatMessage { role: "user".to_string(), content: "ping".to_string() }],
            temperature: Some(0.0),
            max_tokens: Some(1),
            stream: false,
        };
        let resp = self.chat_completion(req).await?;
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

// ---------------- OpenAI 协议数据结构 ----------------

#[derive(Serialize)]
struct OpenAiChatBody {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    stream: bool,
}

#[derive(Serialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OpenAiChatResponse {
    id: Option<String>,
    model: Option<String>,
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiResponseMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}
