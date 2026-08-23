use serde::Serialize;
use sqlx::FromRow;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub base_url: String,
    pub model: String,
    pub status: String,
    pub api_key_masked: String,
    pub latency_ms: Option<i64>,
    pub priority: i64,
    pub weight: i64,
    /// 每 1k token 的成本单价（元），用于转发层记账
    pub price_per_1k_tokens: f64,
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
    pub channel_id: String,
    pub channel_name: String,
    pub key_id: Option<String>,
    pub key_name: Option<String>,
    pub model: String,
    pub status: i64,
    pub latency_ms: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    pub cost: f64,
    pub error: Option<String>,
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
    pub created_at: String,
}
