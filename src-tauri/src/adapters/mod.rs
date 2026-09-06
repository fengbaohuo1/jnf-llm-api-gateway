pub mod deepseek;
pub mod openai;
pub mod openai_compat;

use std::time::Duration;

use serde::{Deserialize, Serialize};

// ---------------- 统一请求/响应模型（适配器模式的核心契约） ----------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub role: String, // system / user / assistant
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// 是否流式（本次先实现非流式，流式作为后续扩展点）
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatResponse {
    pub id: String,
    pub model: String,
    pub content: String,
    pub usage: Option<ChatUsage>,
    pub finish_reason: Option<String>,
    /// 本次请求耗时（毫秒）
    pub latency_ms: u64,
}

/// 连通性测试结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub ok: bool,
    pub latency_ms: Option<u64>,
    pub message: String,
}

// ---------------- 错误类型 ----------------

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("HTTP 请求失败: {0}")]
    Http(#[from] reqwest::Error),
    #[error("上游返回错误状态 {status}: {body}")]
    Upstream { status: u16, body: String },
    #[error("响应解析失败: {0}")]
    Parse(String),
    #[error("适配器配置错误: {0}")]
    Config(String),
}

// ---------------- 渠道适配器 trait（统一契约） ----------------

/// 渠道适配器：把统一模型映射为各 LLM 厂商的协议。
/// 每个渠道类型（OpenAI/DeepSeek/Claude/Gemini/...）一个实现。
#[async_trait::async_trait]
pub trait ChannelAdapter: Send + Sync {
    /// 适配器对应的渠道类型名（与 channels.type 一致）
    fn channel_type(&self) -> &str;
    /// 上游 Base URL
    fn base_url(&self) -> &str;

    /// 聊天补全（非流式）。
    /// 注：当前仅 test_channel 调用 `test()`；`chat_completion` 是网关转发层的契约方法，
    /// 转发命令实现后会使用，届时移除本标注。
    #[allow(dead_code)]
    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatResponse, AdapterError>;

    /// 连通性测试：发送一个最小请求验证密钥/端点/模型是否可用
    async fn test(&self) -> Result<TestResult, AdapterError>;
}

// ---------------- 工厂 ----------------

/// 根据渠道类型创建对应适配器。
/// - OpenAI / Custom：OpenAI 兼容协议
/// - DeepSeek：独立适配器（OpenAI 兼容 + DeepSeek 默认端点/模型）
/// - Claude / Gemini：各自原生协议（后续迭代实现）
pub fn create_adapter(
    channel_type: &str,
    base_url: &str,
    api_key: &str,
) -> Result<Box<dyn ChannelAdapter>, AdapterError> {
    match channel_type {
        "OpenAI" | "Custom" => {
            Ok(Box::new(openai::OpenAiAdapter::new(base_url.to_string(), api_key.to_string())))
        }
        "DeepSeek" => Ok(Box::new(deepseek::DeepSeekAdapter::new(base_url.to_string(), api_key.to_string()))),
        "Claude" => Err(AdapterError::Config("Claude 适配器尚未实现".into())),
        "Gemini" => Err(AdapterError::Config("Gemini 适配器尚未实现".into())),
        other => Err(AdapterError::Config(format!("未知渠道类型: {other}"))),
    }
}

// ---------------- 共享工具 ----------------

/// 统一的请求超时（90s：适配器 chat_completion 供渠道连通测试与知识库 RAG 问答
/// 使用，长文档上下文首 token 较慢；代理转发透传路径是独立 client，不受影响）
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
