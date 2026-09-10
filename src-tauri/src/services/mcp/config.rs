//! MCP 服务配置：设置键统一读取、默认值与工具级准入判定。
//!
//! 设置键（settings 表；`settings.json` 是文件权威，UI 写入走 `persist_setting` 双写）：
//! - `services.mcp.enabled`：服务总开关（服务框架消费）
//! - `services.mcp.host`：监听地址，`127.0.0.1`（默认，仅本机）/ `0.0.0.0`（局域网）
//! - `services.mcp.port`：端口，0 = 随机（测试用），默认 8790
//! - `services.mcp.token`：非空则要求 `Authorization: Bearer <token>`（**每次请求实时读取**，
//!   轮换立即生效）
//! - `services.mcp.readOnly`：只读模式，写入/危险工具一律拒绝
//! - `services.mcp.tools.<name>.enabled`：单工具开关（缺省启用；`false` 禁用）
//!
//! host/port 在服务 init 时读取，改动需重启服务；其余键每次调用实时生效。

use std::collections::{HashMap, HashSet};

use sqlx::SqlitePool;

/// 默认监听地址（仅本机回环）
pub(crate) const DEFAULT_HOST: &str = "127.0.0.1";
/// 默认端口
pub(crate) const DEFAULT_PORT: u16 = 8790;
/// 允许的监听地址（避免误填暴露面失控）
pub(crate) const ALLOWED_HOSTS: [&str; 2] = ["127.0.0.1", "0.0.0.0"];

/// 工具风险类别（前端徽章 + 只读模式准入）
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    /// 只读查询
    Read,
    /// 写入（导入/建库/更新/构建索引）
    Write,
    /// 危险（删除，不可撤销）
    Dangerous,
}

impl ToolKind {
    pub fn label(self) -> &'static str {
        match self {
            ToolKind::Read => "只读",
            ToolKind::Write => "写入",
            ToolKind::Dangerous => "危险",
        }
    }

    /// 是否属于"会改动数据"的类别（只读模式拦截对象）
    pub fn mutates(self) -> bool {
        !matches!(self, ToolKind::Read)
    }
}

/// MCP 运行配置快照
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct McpConfig {
    pub host: String,
    pub port: u16,
    pub token: Option<String>,
    pub read_only: bool,
    pub disabled_tools: HashSet<String>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            token: None,
            read_only: false,
            disabled_tools: HashSet::new(),
        }
    }
}

/// 读取 `services.mcp.*` 全部设置（缺失/非法 → 默认值 + 告警）
async fn read_map(pool: &SqlitePool) -> HashMap<String, String> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT key, value FROM settings WHERE key LIKE 'services.mcp.%'",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .collect()
}

impl McpConfig {
    pub async fn load(pool: &SqlitePool) -> Self {
        let map = read_map(pool).await;
        let d = Self::default();

        let host = match map.get("services.mcp.host").map(|v| v.trim()) {
            None | Some("") => d.host,
            Some(v) if ALLOWED_HOSTS.contains(&v) => v.to_string(),
            Some(v) => {
                eprintln!("[mcp] 设置 services.mcp.host 非法（{v}），回落 {DEFAULT_HOST}");
                d.host
            }
        };

        let port = match map.get("services.mcp.port").map(|v| v.trim()) {
            None | Some("") => d.port,
            Some(raw) => match raw.parse::<u16>() {
                Ok(0) => 0, // 0 = 随机端口（测试/调试）
                Ok(v) => v,
                Err(_) => {
                    eprintln!("[mcp] 设置 services.mcp.port 非法（{raw}），回落 {DEFAULT_PORT}");
                    d.port
                }
            },
        };

        let token = map
            .get("services.mcp.token")
            .map(|v| v.trim().to_string())
            .filter(|t| !t.is_empty());

        let read_only = match map.get("services.mcp.readOnly").map(|v| v.trim()) {
            None | Some("") => d.read_only,
            Some("true") | Some("1") => true,
            Some("false") | Some("0") => false,
            Some(other) => {
                eprintln!("[mcp] 设置 services.mcp.readOnly 非法（{other}），按 false 处理");
                false
            }
        };

        let disabled_tools = map
            .iter()
            .filter_map(|(k, v)| {
                let name = k.strip_prefix("services.mcp.tools.")?.strip_suffix(".enabled")?;
                (v.trim() == "false").then(|| name.to_string())
            })
            .collect();

        Self {
            host,
            port,
            token,
            read_only,
            disabled_tools,
        }
    }

    /// 单工具是否启用（缺省启用）
    pub fn tool_enabled(&self, name: &str) -> bool {
        !self.disabled_tools.contains(name)
    }

    /// 工具准入判定：只读模式与单工具开关在这里统一拦截（不是只改 UI 显示）
    pub fn check_tool(&self, name: &str, kind: ToolKind) -> Result<(), String> {
        if !self.tool_enabled(name) {
            return Err(format!(
                "工具 {name} 已在 MCP 服务设置中禁用（services.mcp.tools.{name}.enabled = false）"
            ));
        }
        if self.read_only && kind.mutates() {
            return Err(format!(
                "MCP 服务处于只读模式，已拒绝 {} 工具 {name}（可在 MCP 服务页关闭只读模式）",
                kind.label()
            ));
        }
        Ok(())
    }

    /// 展示用端点（0.0.0.0 对外应填本机地址，这里统一显示回环便于客户端直接粘贴）
    pub fn endpoint(&self, actual_port: u16) -> String {
        let host = if self.host == "0.0.0.0" { "127.0.0.1" } else { &self.host };
        format!("http://{host}:{actual_port}/mcp")
    }

    /// 令牌掩码（仅展示用）
    pub fn token_masked(&self) -> Option<String> {
        self.token.as_deref().map(crate::commands::util::mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn set(pool: &SqlitePool, key: &str, value: &str) {
        sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)")
            .bind(key)
            .bind(value)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn defaults_and_overrides() {
        let pool = test_pool().await;
        let cfg = McpConfig::load(&pool).await;
        assert_eq!(cfg.host, DEFAULT_HOST);
        assert_eq!(cfg.port, DEFAULT_PORT);
        assert!(cfg.token.is_none() && !cfg.read_only);
        assert!(cfg.tool_enabled("kb_search"), "缺省全部工具启用");

        set(&pool, "services.mcp.host", "0.0.0.0").await;
        set(&pool, "services.mcp.port", "9100").await;
        set(&pool, "services.mcp.token", "  mcp_abcdefgh1234  ").await;
        set(&pool, "services.mcp.readOnly", "true").await;
        set(&pool, "services.mcp.tools.kb_delete_base.enabled", "false").await;
        let cfg = McpConfig::load(&pool).await;
        assert_eq!(cfg.host, "0.0.0.0");
        assert_eq!(cfg.port, 9100);
        assert_eq!(cfg.token.as_deref(), Some("mcp_abcdefgh1234"), "应去掉空白");
        assert!(cfg.read_only);
        assert!(!cfg.tool_enabled("kb_delete_base"));
        assert!(cfg.tool_enabled("kb_search"));
        assert_eq!(cfg.endpoint(9100), "http://127.0.0.1:9100/mcp", "0.0.0.0 展示回环");
        assert_eq!(cfg.token_masked().as_deref(), Some("mcp_••••1234"));
    }

    #[tokio::test]
    async fn invalid_values_fall_back() {
        let pool = test_pool().await;
        set(&pool, "services.mcp.host", "1.2.3.4").await;
        set(&pool, "services.mcp.port", "not-a-port").await;
        set(&pool, "services.mcp.readOnly", "maybe").await;
        set(&pool, "services.mcp.tools.kb_search.enabled", "true").await;
        let cfg = McpConfig::load(&pool).await;
        assert_eq!(cfg.host, DEFAULT_HOST);
        assert_eq!(cfg.port, DEFAULT_PORT);
        assert!(!cfg.read_only);
        assert!(cfg.tool_enabled("kb_search"));
    }

    #[tokio::test]
    async fn read_only_blocks_mutating_tools_only() {
        let pool = test_pool().await;
        set(&pool, "services.mcp.readOnly", "true").await;
        set(&pool, "services.mcp.tools.kb_search.enabled", "false").await;
        let cfg = McpConfig::load(&pool).await;
        assert!(cfg.check_tool("kb_search", ToolKind::Read).is_err(), "被禁用的只读工具也拦");
        let cfg = McpConfig {
            disabled_tools: HashSet::new(),
            ..cfg
        };
        assert!(cfg.check_tool("kb_search", ToolKind::Read).is_ok());
        assert!(cfg.check_tool("kb_import_text", ToolKind::Write).is_err());
        assert!(cfg.check_tool("kb_delete_base", ToolKind::Dangerous).is_err());
        let err = cfg.check_tool("kb_import_text", ToolKind::Write).unwrap_err();
        assert!(err.contains("只读模式") && err.contains("kb_import_text"), "{err}");
    }

    #[test]
    fn port_zero_means_random() {
        // 0 是合法值（测试用随机端口），不应被回落成默认端口
        let cfg = McpConfig { port: 0, ..McpConfig::default() };
        assert_eq!(cfg.port, 0);
    }
}
