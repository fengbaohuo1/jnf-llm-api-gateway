use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::collections::BTreeMap;

/// 单个模型的价格（元/1k tokens，内部记账单位）
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PriceBreakdown {
    /// 缓存未命中输入单价
    pub input: f64,
    /// 缓存命中输入单价（0 = 不区分缓存）
    pub cache_hit: f64,
    /// 输出单价
    pub output: f64,
    /// 思考/推理输出单价（0 = 按输出单价）
    pub reasoning: f64,
}

/// 模型价格：`peak` 为高峰价格；`offPeak` 为 DeepSeek 空闲时段价格（缺省 = 不区分时段）
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPrice {
    pub input: f64,
    pub cache_hit: f64,
    pub output: f64,
    pub reasoning: f64,
    #[serde(default)]
    pub off_peak: Option<PriceBreakdown>,
}

impl ModelPrice {
    pub fn peak_breakdown(&self) -> PriceBreakdown {
        PriceBreakdown {
            input: self.input,
            cache_hit: self.cache_hit,
            output: self.output,
            reasoning: self.reasoning,
        }
    }
}

/// 生效价格（已按高峰/空闲时段折算）
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectivePrice {
    pub input: f64,
    pub cache_hit: f64,
    pub output: f64,
    pub reasoning: f64,
}

/// 解析渠道 `model_prices`（JSON 对象）；非法 JSON 返回空表
pub fn parse_model_prices(raw: &str) -> BTreeMap<String, ModelPrice> {
    let raw = raw.trim();
    if raw.is_empty() {
        return BTreeMap::new();
    }
    serde_json::from_str::<BTreeMap<String, ModelPrice>>(raw).unwrap_or_default()
}

/// DeepSeek 官方已接受的旧模型名 → 当前模型名（价格按当前模型计算）
pub fn price_model_alias(model: &str) -> &str {
    match model.trim() {
        "deepseek-v4-flash" | "deepseek-v4-flash-vision-exp" => "deepseek-flash",
        other => other,
    }
}

/// 解析某模型在该渠道的生效价格：
/// 1. 优先 `model_prices` 中该模型的配置（旧模型名自动映射）；
/// 2. 未配置则回退渠道级默认单价；
/// 3. DeepSeek 协议渠道若命中空闲时段（北京时间工作日 9:00-12:00、14:00-18:00 之外），
///    且该模型配置了 `offPeak`，使用空闲时段价格（官方为空闲 = 高峰 5 折）。
pub fn resolve_model_price(
    model_prices: &BTreeMap<String, ModelPrice>,
    channel_protocol: &str,
    requested_model: &str,
    now_iso: &str,
    defaults: (f64, f64, f64, f64),
) -> EffectivePrice {
    let alias = price_model_alias(requested_model);
    let entry = model_prices
        .get(alias)
        .or_else(|| model_prices.get(requested_model.trim()));

    let Some(price) = entry else {
        let (input, cache_hit, output, reasoning) = defaults;
        return EffectivePrice { input, cache_hit, output, reasoning };
    };

    let peak = price.peak_breakdown();
    let base = EffectivePrice {
        input: if peak.input > 0.0 { peak.input } else { defaults.0 },
        cache_hit: peak.cache_hit,
        output: if peak.output > 0.0 { peak.output } else { defaults.2 },
        reasoning: if peak.reasoning > 0.0 { peak.reasoning } else { defaults.3 },
    };

    if channel_protocol == "deepseek" && is_deepseek_off_peak(now_iso) {
        if let Some(off) = &price.off_peak {
            return EffectivePrice {
                input: if off.input > 0.0 { off.input } else { base.input },
                cache_hit: if off.cache_hit > 0.0 { off.cache_hit } else { base.cache_hit },
                output: if off.output > 0.0 { off.output } else { base.output },
                reasoning: if off.reasoning > 0.0 { off.reasoning } else { base.reasoning },
            };
        }
    }
    base
}

/// DeepSeek 高峰时段（北京时间）：工作日 09:00-12:00、14:00-18:00（对应 UTC 01-04、06-10）。
/// 注意：中国法定节假日未建模（官方节假日全天按空闲计费）。
pub fn is_deepseek_peak(now_iso: &str) -> bool {
    let Some((date, time)) = now_iso.trim().split_once(' ') else {
        return false;
    };
    let mut parts = date.split('-');
    let (Some(y), Some(m), Some(d)) = (
        parts.next().and_then(|v| v.parse::<i64>().ok()),
        parts.next().and_then(|v| v.parse::<i64>().ok()),
        parts.next().and_then(|v| v.parse::<i64>().ok()),
    ) else {
        return false;
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return false;
    }
    let weekday = weekday_from_civil(y, m, d); // 0 = 周一
    if weekday >= 5 {
        return false; // 周末全天空闲
    }
    let mut hm = time.split(':');
    let hour = hm.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    let minute = hm.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    let minutes = hour * 60 + minute;
    (9 * 60..12 * 60).contains(&minutes) || (14 * 60..18 * 60).contains(&minutes)
}

pub fn is_deepseek_off_peak(now_iso: &str) -> bool {
    !is_deepseek_peak(now_iso)
}

/// 1970-01-01 起的天数（Howard Hinnant civil_from_days 的逆运算），返回 0 = 周一
fn weekday_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    (days + 3).rem_euclid(7) // 1970-01-01 是周四 → 周一索引 = 3
}

/// 解析 `channels.models`（JSON 字符串数组）：非法 JSON / 空串返回空数组。
/// 数组内空白项会被丢弃（保留顺序与重复项由调用方决定）。
pub fn parse_models_json(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    serde_json::from_str::<Vec<String>>(raw)
        .unwrap_or_default()
        .into_iter()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect()
}

/// 渠道生效模型列表：`models` 优先，为空时回退旧的单模型字段 `model`。
/// 返回空数组表示「匹配任意模型」。
pub fn channel_models(models_json: &str, legacy_model: &str) -> Vec<String> {
    let list = parse_models_json(models_json);
    if !list.is_empty() {
        return list;
    }
    let legacy = legacy_model.trim();
    if legacy.is_empty() {
        Vec::new()
    } else {
        vec![legacy.to_string()]
    }
}

/// 请求模型能否由该渠道服务：模型列表为空 = 通配任意模型；否则必须命中列表。
pub fn channel_serves_model(models_json: &str, legacy_model: &str, requested: &str) -> bool {
    let list = channel_models(models_json, legacy_model);
    list.is_empty() || list.iter().any(|m| m == requested)
}

/// 规范化前端提交的模型列表：去空白、去重（保留首次出现顺序）。
pub fn normalize_models(models: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(models.len());
    for m in models {
        let m = m.trim();
        if m.is_empty() || out.iter().any(|x| x == m) {
            continue;
        }
        out.push(m.to_string());
    }
    out
}

#[cfg(test)]
mod model_list_tests {
    use super::*;

    #[test]
    fn parse_models_json_handles_invalid_and_empty() {
        assert!(parse_models_json("").is_empty());
        assert!(parse_models_json("not json").is_empty());
        assert!(parse_models_json("[]").is_empty());
        assert_eq!(
            parse_models_json("[\"a\", \" b \", \"\"]"),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn channel_models_prefers_models_then_legacy() {
        assert_eq!(channel_models("[\"a\",\"b\"]", "legacy"), vec!["a", "b"]);
        assert_eq!(channel_models("", "legacy"), vec!["legacy"]);
        assert!(channel_models("", "").is_empty());
    }

    #[test]
    fn serves_model_wildcard_and_membership() {
        assert!(channel_serves_model("", "", "any-model"));
        assert!(channel_serves_model("[\"a\",\"b\"]", "", "b"));
        assert!(!channel_serves_model("[\"a\"]", "", "b"));
        assert!(channel_serves_model("", "legacy", "legacy"));
    }

    #[test]
    fn normalize_models_trims_and_dedupes() {
        assert_eq!(
            normalize_models(&[" a ".into(), "a".into(), "".into(), "b".into()]),
            vec!["a", "b"]
        );
    }

    #[test]
    fn parse_model_prices_handles_json_and_invalid() {
        let raw = r#"{"deepseek-flash":{"input":0.002,"cacheHit":0.00004,"output":0.008,"reasoning":0,"offPeak":{"input":0.001,"cacheHit":0.00002,"output":0.004,"reasoning":0}}}"#;
        let prices = parse_model_prices(raw);
        let p = prices.get("deepseek-flash").expect("model price parsed");
        assert_eq!(p.input, 0.002);
        assert_eq!(p.off_peak.as_ref().map(|o| o.output), Some(0.004));
        assert!(parse_model_prices("not json").is_empty());
        assert!(parse_model_prices("").is_empty());
    }

    #[test]
    fn resolve_model_price_prefers_model_then_channel_default() {
        let prices = parse_model_prices(
            r#"{"deepseek-v4-pro":{"input":0.009,"cacheHit":0.0003,"output":0.027,"reasoning":0}}"#,
        );
        let defaults = (0.002, 0.0, 0.008, 0.0);
        let p = resolve_model_price(&prices, "deepseek", "deepseek-v4-pro", "2026-10-25 20:00:00", defaults);
        assert_eq!(p.input, 0.009);
        assert_eq!(p.output, 0.027);
        // 未配置价格的模型回退渠道默认单价
        let fallback = resolve_model_price(&prices, "deepseek", "deepseek-flash", "2026-10-25 20:00:00", defaults);
        assert_eq!(fallback.input, 0.002);
        // 旧模型名映射到当前模型价格
        let alias = resolve_model_price(&prices, "deepseek", "deepseek-v4-flash", "2026-10-25 20:00:00", defaults);
        assert_eq!(alias.input, 0.002);
    }

    #[test]
    fn deepseek_off_peak_uses_discounted_price() {
        let prices = parse_model_prices(
            r#"{"deepseek-flash":{"input":0.002,"cacheHit":0.00004,"output":0.008,"reasoning":0,"offPeak":{"input":0.001,"cacheHit":0.00002,"output":0.004,"reasoning":0}}}"#,
        );
        // 2026-10-26 是周一：10:00 高峰，20:00 空闲
        let peak = resolve_model_price(&prices, "deepseek", "deepseek-flash", "2026-10-26 10:00:00", (0.0, 0.0, 0.0, 0.0));
        assert_eq!(peak.input, 0.002);
        let off = resolve_model_price(&prices, "deepseek", "deepseek-flash", "2026-10-26 20:00:00", (0.0, 0.0, 0.0, 0.0));
        assert_eq!(off.input, 0.001);
        assert_eq!(off.output, 0.004);
        // 周末全天空闲
        let weekend = resolve_model_price(&prices, "deepseek", "deepseek-flash", "2026-10-24 10:00:00", (0.0, 0.0, 0.0, 0.0));
        assert_eq!(weekend.input, 0.001);
        // 非 DeepSeek 协议不套用空闲价
        let other = resolve_model_price(&prices, "openai", "deepseek-flash", "2026-10-26 20:00:00", (0.0, 0.0, 0.0, 0.0));
        assert_eq!(other.input, 0.002);
    }

    #[test]
    fn deepseek_peak_windows_are_beijing_workday_hours() {
        assert!(is_deepseek_peak("2026-10-26 09:00:00")); // 周一上午
        assert!(is_deepseek_peak("2026-10-26 17:59:00")); // 周一下午
        assert!(!is_deepseek_peak("2026-10-26 12:00:00"));
        assert!(!is_deepseek_peak("2026-10-26 18:00:00"));
        assert!(!is_deepseek_peak("2026-10-26 08:59:00"));
        assert!(!is_deepseek_peak("2026-10-25 10:00:00")); // 周日
    }
}

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
    /// 主模型（models 数组第一个；空串 = 匹配任意模型），用于展示与知识库默认模型
    pub model: String,
    /// 渠道支持的模型列表（JSON 数组解析后）；空数组 = 匹配任意模型
    pub models: Vec<String>,
    /// 按模型定价：模型名 -> 价格（元/1k，DeepSeek 含空闲时段价）；空表 = 用渠道默认单价
    pub model_prices: BTreeMap<String, ModelPrice>,
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
