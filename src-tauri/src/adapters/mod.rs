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

/// 渠道协议归一化：把「渠道类型 / 显式 protocol」统一成适配器工厂认识的协议名。
/// - 显式 protocol 优先（openai/deepseek/anthropic/gemini）
/// - protocol 为空时按渠道类型推导：OpenAI/Custom→openai、DeepSeek→deepseek、
///   Claude→anthropic、Gemini→gemini
pub fn effective_protocol(channel_type: &str, protocol: &str) -> String {
    let p = protocol.trim().to_ascii_lowercase();
    if matches!(p.as_str(), "openai" | "deepseek" | "anthropic" | "gemini") {
        return p;
    }
    match channel_type.trim().to_ascii_lowercase().as_str() {
        "deepseek" => "deepseek".to_string(),
        "claude" | "anthropic" => "anthropic".to_string(),
        "gemini" => "gemini".to_string(),
        _ => "openai".to_string(),
    }
}

/// 渠道适配器：把统一模型映射为各 LLM 厂商的协议。
/// 每个渠道类型（OpenAI/DeepSeek/Claude/Gemini/...）一个实现。
#[async_trait::async_trait]
pub trait ChannelAdapter: Send + Sync {
    /// 适配器对应的渠道类型名（与 channels.type 一致）
    #[allow(dead_code)]
    fn channel_type(&self) -> &str;
    /// 上游 Base URL
    fn base_url(&self) -> &str;

    /// 聊天补全（非流式）。
    /// 注：当前仅知识库问答调用 `chat_completion`；`test` 供渠道连通性测试。
    #[allow(dead_code)]
    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatResponse, AdapterError>;

    /// 连通性测试：发送一个最小请求验证密钥/端点/模型是否可用。
    /// `model` 为空时使用厂商默认模型；渠道配置了模型时应传入，否则会用默认模型测出 400。
    async fn test(&self, model: Option<&str>) -> Result<TestResult, AdapterError>;
}

// ---------------- 工厂 ----------------

/// 根据协议（或旧版渠道类型）创建对应适配器。
/// - openai / Custom：OpenAI 兼容协议
/// - deepseek：DeepSeek（OpenAI 兼容 + DeepSeek 默认端点/模型 + 思考模式兼容）
/// - anthropic / gemini：原生协议（后续迭代实现）
pub fn create_adapter(
    protocol: &str,
    base_url: &str,
    api_key: &str,
) -> Result<Box<dyn ChannelAdapter>, AdapterError> {
    let p = protocol.trim().to_ascii_lowercase();
    match p.as_str() {
        "openai" | "custom" | "" => Ok(Box::new(openai::OpenAiAdapter::new(
            base_url.to_string(),
            api_key.to_string(),
        ))),
        "deepseek" => Ok(Box::new(deepseek::DeepSeekAdapter::new(
            base_url.to_string(),
            api_key.to_string(),
        ))),
        "anthropic" | "claude" => Err(AdapterError::Config(
            "Anthropic 原生协议适配器尚未实现（可将协议改为 OpenAI 兼容并接入兼容端点）".into(),
        )),
        "gemini" => Err(AdapterError::Config(
            "Gemini 原生协议适配器尚未实现（可将协议改为 OpenAI 兼容并接入兼容端点）".into(),
        )),
        other => Err(AdapterError::Config(format!("未知渠道协议: {other}"))),
    }
}

// ---------------- 共享工具 ----------------

/// 统一的请求超时（90s：适配器 chat_completion 供渠道连通测试与知识库 RAG 问答
/// 使用，长文档上下文首 token 较慢；代理转发透传路径是独立 client，不受影响）
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
