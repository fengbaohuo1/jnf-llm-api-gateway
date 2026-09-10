//! MCP 服务：向本机 LLM 客户端（Claude Desktop / Cline 等）暴露知识库工具的
//! Model Context Protocol 服务（Streamable HTTP，自研 JSON-RPC 子集）。
//!
//! - 端点：`http://{services.mcp.host}:{services.mcp.port}/mcp`（默认
//!   `127.0.0.1:8790`；host 可设为 `0.0.0.0` 开放局域网，建议同时设置令牌）；
//! - 鉴权：`services.mcp.token` 非空时要求 Bearer Token（**每次请求实时读取**，
//!   轮换立即生效）；
//! - 工具准入：`services.mcp.readOnly` 只读模式 + 单工具开关
//!   `services.mcp.tools.<name>.enabled`，在 `tools.rs` 的执行入口强制拦截；
//! - 调用记录：`mcp_calls` 表（method/tool/结果/耗时/客户端，**不落参数**）；
//! - 生命周期归服务框架：启用开关 `services.mcp.enabled`；init 绑定端口并 spawn
//!   HTTP 任务，shutdown abort 释放端口（框架在停用/重启前调用）。host/port 改动
//!   需重启服务，其余配置即时生效。
//!
//! 依赖说明：与 kb 服务共享同一 SQLite（WAL 多进程安全）；向量检索的 HNSW
//! 文件由本服务内建的 KnowledgeBaseService 实例懒加载，构建任务由本服务触发
//! 时与 UI 共用 kb_tasks 防并发（has_running_task）。

pub mod config;
pub mod protocol;
pub mod schema;
pub mod server;
pub mod tools;

use std::any::Any;
use std::sync::{Arc, Mutex};

use sqlx::SqlitePool;

use config::McpConfig;

use super::{Service, ServiceContext};

/// MCP 服务：进程内 HTTP 服务（句柄由 shutdown 释放）
pub struct McpService {
    handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// 工具注册表（命令层"工具调试台"复用同一实例，共享内存索引缓存）
    tools: Mutex<Option<Arc<tools::Tools>>>,
    /// 实际监听端口（配置为 0 时由内核分配随机端口）
    port: Mutex<Option<u16>>,
}

impl Default for McpService {
    fn default() -> Self {
        Self {
            handle: Mutex::new(None),
            tools: Mutex::new(None),
            port: Mutex::new(None),
        }
    }
}

impl McpService {
    /// 当前工具注册表（服务未启动时为 None）
    pub fn tools(&self) -> Option<Arc<tools::Tools>> {
        self.tools.lock().unwrap().clone()
    }

    /// 实际监听端口（未运行时为 None）
    pub fn bound_port(&self) -> Option<u16> {
        *self.port.lock().unwrap()
    }
}

/// 一条 MCP 调用记录（前端"调用记录"列表）
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCall {
    pub id: i64,
    pub method: String,
    pub tool: Option<String>,
    pub ok: i64,
    pub error: Option<String>,
    pub duration_ms: i64,
    pub client: Option<String>,
    pub created_at: String,
}

/// 记录一次 MCP 请求（**不落参数**：工具参数可能含密钥或文档正文）
pub(crate) async fn record_call(
    pool: &SqlitePool,
    method: &str,
    tool: Option<&str>,
    ok: bool,
    error: Option<&str>,
    duration_ms: i64,
    client: Option<&str>,
) {
    let err = error.map(|e| e.chars().take(300).collect::<String>());
    let cli = client.map(|c| c.chars().take(120).collect::<String>());
    let _ = sqlx::query(
        "INSERT INTO mcp_calls (method, tool, ok, error, duration_ms, client, created_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
    )
    .bind(method)
    .bind(tool)
    .bind(if ok { 1i64 } else { 0i64 })
    .bind(err)
    .bind(duration_ms)
    .bind(cli)
    .bind(crate::commands::util::now_iso())
    .execute(pool)
    .await;
}

/// 最近调用记录（倒序）
pub async fn list_calls(pool: &SqlitePool, limit: i64) -> Result<Vec<McpCall>, String> {
    sqlx::query_as::<_, McpCall>(
        "SELECT id, method, tool, ok, error, duration_ms, client, created_at \
         FROM mcp_calls ORDER BY id DESC LIMIT ?1",
    )
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await
    .map_err(|e| format!("读取 MCP 调用记录失败: {e}"))
}

#[async_trait::async_trait]
impl Service for McpService {
    fn id(&self) -> &'static str {
        "mcp"
    }

    fn name(&self) -> &'static str {
        "MCP 服务"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn description(&self) -> &'static str {
        "Model Context Protocol 工具服务：向 LLM 客户端暴露知识库检索/问答/管理（Streamable HTTP；可配监听地址、端口、令牌、只读模式与单工具开关）"
    }

    fn schema_versions(&self) -> &'static [(&'static str, &'static [&'static str])] {
        schema::MCP_SCHEMA
    }

    async fn init(&self, ctx: &ServiceContext) -> Result<(), String> {
        let cfg = McpConfig::load(&ctx.pool).await;
        if cfg.host == "0.0.0.0" && cfg.token.is_none() {
            eprintln!("[mcp] 警告：监听 0.0.0.0（局域网可访问）且未设置令牌，建议在 MCP 服务页生成令牌");
        }

        // 索引目录与 kb 服务同源（data_dir/kb_index），构建任务写同一处
        let index_dir = ctx.data_dir.join("kb_index");
        std::fs::create_dir_all(&index_dir)
            .map_err(|e| format!("创建索引目录失败: {e}"))?;

        let tools = Arc::new(tools::Tools::new(ctx.pool.clone(), index_dir));
        let state = Arc::new(server::McpServerState::new(tools.clone(), ctx.pool.clone()));
        let app = server::router(state);

        let listener = tokio::net::TcpListener::bind((cfg.host.as_str(), cfg.port))
            .await
            .map_err(|e| {
                format!(
                    "MCP 服务绑定 {}:{} 失败: {e}（端口被占用？可在 MCP 服务页修改后重启服务）",
                    cfg.host, cfg.port
                )
            })?;
        let actual = listener.local_addr().map(|a| a.port()).unwrap_or(cfg.port);
        eprintln!(
            "[mcp] MCP 服务监听 {}（{} 个工具，鉴权 {}，只读模式 {}）",
            cfg.endpoint(actual),
            tools::known_names().len(),
            if cfg.token.is_some() { "开启" } else { "关闭" },
            if cfg.read_only { "是" } else { "否" }
        );
        let handle = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                eprintln!("[mcp] HTTP 服务异常退出: {e}");
            }
        });
        *self.handle.lock().unwrap() = Some(handle);
        *self.tools.lock().unwrap() = Some(tools);
        *self.port.lock().unwrap() = Some(actual);
        Ok(())
    }

    async fn shutdown(&self) {
        if let Some(h) = self.handle.lock().unwrap().take() {
            h.abort();
            eprintln!("[mcp] 已停止 MCP HTTP 服务（端口已释放）");
        }
        *self.tools.lock().unwrap() = None;
        *self.port.lock().unwrap() = None;
    }
}
