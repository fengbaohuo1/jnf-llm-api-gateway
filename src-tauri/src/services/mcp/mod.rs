//! MCP 服务：向本机 LLM 客户端（Claude Desktop / Cline 等）暴露知识库工具的
//! Model Context Protocol 服务（Streamable HTTP，自研 JSON-RPC 子集）。
//!
//! - 端点：`http://127.0.0.1:{services.mcp.port}/mcp`（默认端口 8790）；
//! - 鉴权：`services.mcp.token` 非空时要求 Bearer Token（默认空 = 仅本机回环）；
//! - 生命周期归服务框架：启用开关 `services.mcp.enabled`；init 绑定端口并 spawn
//!   HTTP 任务，shutdown abort 释放端口（框架在停用/重启前调用）；
//! - 工具集：知识库只读/写入/问答（见 `tools.rs`；渠道绑定类不暴露）。
//!
//! 依赖说明：与 kb 服务共享同一 SQLite（WAL 多进程安全）；向量检索的 HNSW
//! 文件由本服务内建的 KnowledgeBaseService 实例懒加载，构建任务由本服务触发
//! 时与 UI 共用 kb_tasks 防并发（has_running_task）。

pub mod protocol;
pub mod server;
pub mod tools;

use std::any::Any;
use std::sync::{Arc, Mutex};

use sqlx::SqlitePool;

use super::{Service, ServiceContext};

/// 默认监听端口（可用设置键 `services.mcp.port` 覆盖）
const DEFAULT_PORT: u16 = 8790;

/// MCP 服务：进程内 HTTP 服务（句柄由 shutdown 释放）
pub struct McpService {
    handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Default for McpService {
    fn default() -> Self {
        Self {
            handle: Mutex::new(None),
        }
    }
}

/// 读取服务设置键（settings 表；文件权威由命令层维护，这里只读）
async fn read_setting(pool: &SqlitePool, key: &str) -> Option<String> {
    sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?1")
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .map(|v| v.trim().to_string())
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
        "Model Context Protocol 工具服务：向本机 LLM 客户端暴露知识库检索/问答/管理（Streamable HTTP，127.0.0.1:8790/mcp，端口可配）"
    }

    async fn init(&self, ctx: &ServiceContext) -> Result<(), String> {
        // 端口设置：services.mcp.port；0 = 随机端口（调试/测试用），缺省 8790
        let port: u16 = read_setting(&ctx.pool, "services.mcp.port")
            .await
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_PORT);
        let token = read_setting(&ctx.pool, "services.mcp.token").await.filter(|t| !t.is_empty());

        // 索引目录与 kb 服务同源（data_dir/kb_index），构建任务写同一处
        let index_dir = ctx.data_dir.join("kb_index");
        std::fs::create_dir_all(&index_dir)
            .map_err(|e| format!("创建索引目录失败: {e}"))?;

        let tools = Arc::new(tools::Tools::new(ctx.pool.clone(), index_dir));
        let state = Arc::new(server::McpServerState::new(tools, token));
        let app = server::router(state);

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|e| {
                format!(
                    "MCP 服务绑定端口 {port} 失败: {e}（端口被占用？可在设置 services.mcp.port 修改后重启服务）"
                )
            })?;
        let actual = listener
            .local_addr()
            .map(|a| a.port())
            .unwrap_or(port);
        eprintln!("[mcp] MCP 服务监听 http://127.0.0.1:{actual}/mcp");
        let handle = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                eprintln!("[mcp] HTTP 服务异常退出: {e}");
            }
        });
        *self.handle.lock().unwrap() = Some(handle);
        Ok(())
    }

    async fn shutdown(&self) {
        if let Some(h) = self.handle.lock().unwrap().take() {
            h.abort();
            eprintln!("[mcp] 已停止 MCP HTTP 服务（端口已释放）");
        }
    }
}
