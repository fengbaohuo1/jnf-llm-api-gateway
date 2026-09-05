//! 知识库服务（占位：服务线已注册，检索能力未实现）。
//!
//! 规划（既定方向：网关做本地资源层 + 受限执行，agent 循环留客户端）：
//! - 能力：文档/片段入库、本地向量化与语义检索；检索结果供上下文注入，或经
//!   服务端点提供给客户端；
//! - 约束：内容进出 LLM 上下文前必须经过 security 引擎（ServiceContext 后续注入
//!   受管审计句柄）；服务 schema 在 init 中幂等自建，开关 `services.kb.enabled` 归框架；
//! - 端点：HTTP/命令挂载方式待首个真实能力落地时在服务契约中扩展（占位阶段不定义空接口）。

use super::{Service, ServiceContext};

/// 知识库服务：本地文档检索与上下文注入（占位）。
pub struct KnowledgeBaseService;

#[async_trait::async_trait]
impl Service for KnowledgeBaseService {
    fn id(&self) -> &'static str {
        "kb"
    }

    fn name(&self) -> &'static str {
        "知识库"
    }

    fn description(&self) -> &'static str {
        "本地文档检索与上下文注入（占位：服务线已注册，能力待实现）"
    }

    async fn init(&self, _ctx: &ServiceContext) -> Result<(), String> {
        Ok(())
    }
}
