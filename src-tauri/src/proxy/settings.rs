//! 代理运行设置：每次请求从 settings 表读取，改设置即时生效。

use std::collections::HashMap;

use sqlx::SqlitePool;

/// 代理相关设置（每次请求从 settings 读取，改设置即时生效）
#[derive(Debug)]
pub(crate) struct ProxySettings {
    pub(crate) server_enabled: bool,
    pub(crate) failover_enabled: bool,
    pub(crate) failover_max_attempts: i64,
    pub(crate) security_enabled: bool,
    /// replace（脱敏后转发，默认）| block（高风险拦截）
    pub(crate) security_mode: String,
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            server_enabled: true,
            failover_enabled: false,
            failover_max_attempts: 3,
            security_enabled: true,
            security_mode: "replace".to_string(),
        }
    }
}

pub(crate) async fn read_settings(pool: &SqlitePool) -> ProxySettings {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM settings WHERE key IN ('server.enabled','server.port','failover.enabled','failover.max_attempts','security.enabled','security.mode')",
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
