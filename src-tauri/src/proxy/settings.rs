//! 代理运行设置：每次请求从 settings 表读取，改设置即时生效。

use std::collections::HashMap;

use sqlx::SqlitePool;

use crate::security::RiskLevel;

/// 内容明细存储级别的合法取值（日志内容按请求命中的最高风险决定是否落库）：
/// - `all`：无论风险都保存完整内容（默认，与旧行为一致）
/// - `low` / `medium` / `high`：仅当命中风险达到该等级才保存内容
/// - `none`：从不保存内容（只记概要列；等价于旧的 log.content_enabled=false）
pub(crate) const LOG_STORE_LEVELS: [&str; 5] = ["all", "low", "medium", "high", "none"];

/// 该请求（最高风险 = risk）是否允许保存内容明细。None 表示没有任何命中。
pub(crate) fn content_allowed(level: &str, risk: Option<RiskLevel>) -> bool {
    match level {
        "none" => false,
        // 低风险及以上：至少要有一条命中（无风险请求不存内容）
        "low" => risk.is_some(),
        "medium" => risk >= Some(RiskLevel::Medium),
        "high" => risk >= Some(RiskLevel::High),
        // all 及非法值兜底：全存
        _ => true,
    }
}

/// 代理相关设置（每次请求从 settings 读取，改设置即时生效）
#[derive(Debug)]
pub(crate) struct ProxySettings {
    pub(crate) server_enabled: bool,
    pub(crate) failover_enabled: bool,
    pub(crate) failover_max_attempts: i64,
    pub(crate) security_enabled: bool,
    /// replace（脱敏后转发，默认）| block（高风险拦截）
    pub(crate) security_mode: String,
    /// 内容明细存储级别：all / low / medium / high / none（见 [`content_allowed`]）
    pub(crate) log_store_level: String,
    /// DeepSeek 思考模式兼容：转发给 protocol=deepseek 的渠道时，
    /// 给"带 tool_calls 的 assistant 消息"补一个空 `reasoning_content`。
    /// DeepSeek 要求思考模式下的 tool-call 轮次把 reasoning_content 回传，
    /// 而 OpenAI 协议客户端（Codex/自研 Agent）通常不回传，导致第二轮 400。
    pub(crate) compat_deepseek_reasoning: bool,
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            server_enabled: true,
            failover_enabled: false,
            failover_max_attempts: 3,
            security_enabled: true,
            security_mode: "replace".to_string(),
            log_store_level: "all".to_string(),
            compat_deepseek_reasoning: true,
        }
    }
}

impl ProxySettings {
    /// 该风险等级的内容明细是否应存储
    pub(crate) fn content_allowed(&self, risk: Option<RiskLevel>) -> bool {
        content_allowed(&self.log_store_level, risk)
    }

    /// 是否可能记录内容（请求阶段用于决定要不要捕获/脱敏请求体快照）
    pub(crate) fn content_possible(&self) -> bool {
        self.log_store_level != "none"
    }
}

pub(crate) async fn read_settings(pool: &SqlitePool) -> ProxySettings {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key IN ('server.enabled','server.port','failover.enabled','failover.max_attempts','security.enabled','security.mode','log.store_level','log.content_enabled','compat.deepseek_reasoning')",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let map: HashMap<String, String> = rows.into_iter().collect();
    let mut s = ProxySettings::default();
    if let Some(v) = map.get("server.enabled") {
        s.server_enabled = v == "true";
    }
    if let Some(v) = map.get("failover.enabled") {
        s.failover_enabled = v == "true";
    }
    if let Some(v) = map.get("failover.max_attempts") {
        s.failover_max_attempts = v.parse::<i64>().unwrap_or(3).max(1);
    }
    if let Some(v) = map.get("security.enabled") {
        s.security_enabled = v == "true";
    }
    if let Some(v) = map.get("security.mode") {
        if v == "block" || v == "replace" {
            s.security_mode = v.clone();
        }
    }
    // 新键优先；旧键 log.content_enabled 作为兼容回退（false → none，true/缺失 → all）
    let level = map
        .get("log.store_level")
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| LOG_STORE_LEVELS.contains(&v.as_str()));
    s.log_store_level = level
        .or_else(|| {
            map.get("log.content_enabled").map(|v| {
                if v == "true" {
                    "all".to_string()
                } else {
                    "none".to_string()
                }
            })
        })
        .unwrap_or_else(|| "all".to_string());
    if let Some(v) = map.get("compat.deepseek_reasoning") {
        s.compat_deepseek_reasoning = v == "true";
    }
    s
}

/// 读取监听地址（server.host 默认 127.0.0.1，server.port 默认 8787）
pub(crate) async fn read_listen_addr(pool: &SqlitePool) -> (String, u16) {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key IN ('server.host', 'server.port')",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let map: HashMap<String, String> = rows.into_iter().collect();
    let host = map
        .get("server.host")
        .filter(|h| !h.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = map
        .get("server.port")
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(8787);
    (host, port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_level_all_stores_everything() {
        assert!(content_allowed("all", None));
        assert!(content_allowed("all", Some(RiskLevel::Low)));
        assert!(content_allowed("all", Some(RiskLevel::High)));
    }

    #[test]
    fn content_level_none_stores_nothing() {
        assert!(!content_allowed("none", None));
        assert!(!content_allowed("none", Some(RiskLevel::High)));
    }

    #[test]
    fn content_level_thresholds() {
        // 低风险及以上：需要至少一条命中
        assert!(!content_allowed("low", None));
        assert!(content_allowed("low", Some(RiskLevel::Low)));
        // 中风险及以上
        assert!(!content_allowed("medium", Some(RiskLevel::Low)));
        assert!(content_allowed("medium", Some(RiskLevel::Medium)));
        // 高风险及以上
        assert!(!content_allowed("high", Some(RiskLevel::Medium)));
        assert!(content_allowed("high", Some(RiskLevel::High)));
        // 非法值兜底为全存
        assert!(content_allowed("bogus", None));
    }

    #[test]
    fn content_possible_only_when_not_none() {
        let mut s = ProxySettings::default();
        assert_eq!(s.log_store_level, "all");
        assert!(s.content_possible());
        s.log_store_level = "none".into();
        assert!(!s.content_possible());
    }
}
