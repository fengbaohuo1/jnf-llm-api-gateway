//! MCP 服务 schema（服务级迁移，由服务框架 `ServiceRegistry::apply_schema` 执行）。
//!
//! - **0001**：`mcp_calls` 调用记录（method / tool / 结果 / 耗时 / 客户端）。
//!   **不落请求参数**——工具参数可能含密钥或文档正文，留痕只需"谁在什么时候
//!   调了哪个工具、成功与否、多慢"。

pub(crate) static MCP_SCHEMA: &[(&str, &[&str])] = &[(
    "0001_mcp_calls",
    &[
        "CREATE TABLE IF NOT EXISTS mcp_calls (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            method      TEXT NOT NULL,
            tool        TEXT,
            ok          INTEGER NOT NULL DEFAULT 1,
            error       TEXT,
            duration_ms INTEGER NOT NULL DEFAULT 0,
            client      TEXT,
            created_at  TEXT NOT NULL
        )",
        "CREATE INDEX IF NOT EXISTS idx_mcp_calls_created ON mcp_calls(created_at)",
    ],
)];
