use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::{AdapterError, ChatMessage, ChatRequest, ChatResponse, ChatUsage, ChannelAdapter, TestResult, REQUEST_TIMEOUT};

/// OpenAI 兼容协议的“基类”trait。
///
/// Rust 没有类继承，trait 的**默认方法**就是基类方法的等价物：
/// 实现本 trait 的适配器只需提供厂商自己的配置（名称、默认端点、默认模型、密钥、HTTP 客户端），
/// 即可自动获得完整的 OpenAI 协议 `chat_completion` 与 `test` 实现。
///
/// 各厂商（OpenAI / DeepSeek / 任意 OpenAI 兼容服务）各自实现本 trait，
/// 互不引用、独立演化；协议公共逻辑只在这里维护一份。
#[async_trait::async_trait]
pub trait OpenAiCompatible: ChannelAdapter + Send + Sync {
    /// 厂商名（如 "OpenAI" / "DeepSeek"），用于日志与展示
    fn vendor_name(&self) -> &str;
    /// 上游 API Key
    fn api_key(&self) -> &str;
    /// 厂商默认端点（当渠道未配置 base_url 时使用）
    fn default_base_url(&self) -> &str;
    /// 厂商默认模型（连通性测试使用）
    fn default_model(&self) -> &str;
    /// 共享的 HTTP 客户端
    fn client(&self) -> &reqwest::Client;

    /// 解析后的端点：渠道配置优先，为空则回退到厂商默认端点
    fn resolved_base_url(&self) -> String {
        let b = self.base_url().trim();
        if b.is_empty() {
            self.default_base_url().to_string()
        } else {
            b.to_string()
        }
    }

    /// OpenAI 协议聊天补全（公共实现，各厂商共享）。
    /// 命名为 `compat_*` 以避免与 `ChannelAdapter` 的同名方法冲突。
    async fn compat_chat_completion(&self, req: ChatRequest) -> Result<ChatResponse, AdapterError> {
        let started = Instant::now();

        let body = OpenAiChatBody {
            model: req.model.clone(),
            messages: req
                .messages
                .iter()
                .map(|m| OpenAiMessage {
                    role: m.role.clone(),
                    content: m.content.clone(),
                })
                .collect(),
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            stream: req.stream,
        };

        let resp = self
            .client()
            .post(format!("{}/chat/completions", self.resolved_base_url()))
            .bearer_auth(self.api_key())
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
            .map_err(|e| AdapterError::Parse(format!("OpenAI 兼容响应解析失败: {e}")))?;

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

    /// 连通性测试（公共实现）：用厂商默认模型发一个最小请求
    async fn compat_test(&self) -> Result<TestResult, AdapterError> {
        let req = ChatRequest {
            model: self.default_model().to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "ping".to_string(),
            }],
            temperature: Some(0.0),
            max_tokens: Some(1),
            stream: false,
        };
        let resp = self.compat_chat_completion(req).await?;
        Ok(TestResult {
            ok: true,
            latency_ms: Some(resp.latency_ms),
            message: format!(
                "{} 连接正常（{}），模型 {} 可用（{}ms）",
                self.vendor_name(),
                self.resolved_base_url(),
                resp.model,
                resp.latency_ms
            ),
        })
    }
}

/// 构造一个共享的 reqwest 客户端（带统一超时）
pub(crate) fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("failed to build reqwest client")
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
