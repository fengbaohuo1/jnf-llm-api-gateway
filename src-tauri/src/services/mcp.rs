//! MCP 服务（占位：服务线已注册，工具执行未实现）。
//!
//! 规划（既定方向：受限执行——工具由白名单约束，调用带参数校验与审计）：
//! - 角色：向本机 LLM 侧暴露 MCP（Model Context Protocol）工具服务，或作为 MCP
//!   client 编排外部工具服务器；具体形态待能力落地时定；
//! - 约束：工具执行是高风险操作，必须过白名单 + 参数校验 + admin 审计；
//!   服务 schema 在 init 中幂等自建，开关 `services.mcp.enabled` 归框架；
//! - 端点：MCP 官方协议含 stdio/HTTP transport，进程内端点挂载方式待首个真实
//!   能力落地时在服务契约中扩展（占位阶段不定义空接口）。

use super::{Service, ServiceContext};

/// MCP 服务：Model Context Protocol 工具服务（占位）。
pub struct McpService;

#[async_trait::async_trait]
impl Service for McpService {
    fn id(&self) -> &'static str {
        "mcp"
    }

    fn name(&self) -> &'static str {
        "MCP 服务"
    }

    fn description(&self) -> &'static str {
        "Model Context Protocol 工具执行服务（占位：受限工具执行待实现）"
    }

    async fn init(&self, _ctx: &ServiceContext) -> Result<(), String> {
        Ok(())
    }
}
