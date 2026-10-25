use serde::Serialize;
use sqlx::FromRow;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
    /// 显式上游协议（openai/deepseek/anthropic/gemini）；空串 = 按渠道类型自动推导
    pub protocol: String,
    /// DeepSeek 思考模式兼容：转发时给带 tool_calls 的 assistant 消息补空 reasoning_content
    pub compat_reasoning_content: bool,
    pub base_url: String,
    pub model: String,
    pub status: String,
    pub api_key_masked: String,
    pub latency_ms: Option<i64>,
    pub priority: i64,
    pub weight: i64,
    /// 每 1k token 的成本单价（元），用于转发层记账
    pub price_per_1k_tokens: f64,
    /// 每 1k token 的缓存命中单价（元），0 = 未配置（按正常单价计费）
    pub cache_price_per_1k_tokens: f64,
    /// 每 1k token 的思考（reasoning）单价（元），0 = 未配置（按正常输出单价计费）
    pub reasoning_price_per_1k_tokens: f64,
    pub created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    pub key_prefix: String,
    pub status: String,
    pub quota_limit: i64,
    pub used_tokens: i64,
    pub expires_at: Option<String>,
    pub created_at: String,
}

/// 创建密钥时，一次性返回完整明文密钥（仅这一次），之后不再可查。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedKey {
    pub id: String,
    pub name: String,
    pub key: String,
    pub key_prefix: String,
    pub quota_limit: i64,
    pub expires_at: Option<String>,
    pub created_at: String,
}

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RequestLog {
    pub id: String,
    pub timestamp: String,
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
    pub key_id: Option<String>,
    pub key_name: Option<String>,
    pub model: String,
    /// 客户端入口协议：openai / anthropic / responses
    pub protocol: String,
    /// 是否流式请求
    pub stream: bool,
    /// 客户端 User-Agent（个人工具识别是哪个客户端在调用）
    pub client_ua: String,
    /// 本次请求尝试的渠道数（成功行 = 第几次尝试成功；失败行 = 已尝试次数）
    pub attempts: i64,
    /// 请求命中的最高安全风险：none / low / medium / high（安全扫描关闭时为 none）
    pub risk_level: String,
    pub status: i64,
    pub latency_ms: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    /// 输入 token 中命中前缀缓存的个数（缓存感知计费用）
    pub cache_hit_tokens: i64,
    /// 输出中用于推理（thinking）的 token 数（reasoning 感知计费用）
    pub reasoning_tokens: i64,
    pub cost: f64,
    pub error: Option<String>,
}

/// 请求内容明细（0010）：请求/转发/响应/错误的完整 JSON，一对一行按需加载
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RequestDetail {
    pub request_id: String,
    /// 收到的原始请求体（客户端协议原文，脱敏后落库）
    pub request_body: Option<String>,
    /// 实际发给上游的请求体（内部 OpenAI 格式，脱敏 + 转换 + stream_options 注入后）
    pub upstream_body: Option<String>,
    /// 成功响应体（非流式原文 / 流式重建完整响应）
    pub response_body: Option<String>,
    /// 失败时的错误响应体（上游错误 JSON / 网关错误 JSON）
    pub error_body: Option<String>,
}

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub key: String,
    pub value: String,
}

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct DailyStat {
    pub date: String,
    pub requests: i64,
    pub tokens: i64,
    /// 输入 token 中命中前缀缓存的累计数（缓存感知计费明细）
    pub cache_hit_tokens: i64,
    /// 输出（completion）token 累计数
    pub completion_tokens: i64,
    pub cost: f64,
    pub avg_latency_ms: f64,
}

/// 安全事件日志（请求侧/响应侧扫描命中），一对多挂在 requests 下
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SecurityLog {
    pub id: String,
    pub request_id: String,
    pub stage: String,
    pub rule_id: String,
    pub category: String,
    pub risk: String,
    pub snippet_masked: String,
    /// 处置动作：detected（仅记录）/ masked（已脱敏）/ blocked（已拦截）
    pub action: String,
    pub created_at: String,
}

/// 安全规则（DB 可配置，扫描引擎启动/定时重载）
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SecurityRule {
    pub id: String,
    pub category: String,
    pub risk: String,
    pub pattern: String,
    /// 匹配方式：regex（正则）/ substring（大小写不敏感子串，Aho-Corasick 多模式）
    pub match_type: String,
    pub enabled: bool,
    pub action: String,
    pub created_at: String,
}

/// 鉴权事件（成功/失败，含匿名失败）
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AuthEvent {
    pub id: String,
    pub key_id: Option<String>,
    pub success: bool,
    pub reason: Option<String>,
    pub created_at: String,
}

/// 管理操作审计（渠道/密钥/设置/规则变更留痕）
#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AdminAuditLog {
    pub id: String,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub detail: Option<String>,
    pub created_at: String,
}
