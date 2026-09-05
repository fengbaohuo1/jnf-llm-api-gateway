//! 知识库服务：本地文档入库（解析 → 分块 → 全文索引）与数据操作层。
//!
//! 分层（本目录）：
//! - `schema.rs`：服务级 schema（版本化迁移，由服务框架执行）
//! - `parse.rs`：文档解析与分块（纯函数，md 结构化 + txt）
//! - `data.rs`：CRUD / 导入 / 全文同步（仅依赖连接池，可独立单测）
//!
//! 命令层（commands/kb.rs）只做薄包装；检索命令与 UI 在下一步接入。

mod parse;
mod schema;
pub mod data;

use super::{Service, ServiceContext};

/// 知识库服务：文档导入、分块与检索数据层
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
        "本地知识库：文档导入、分块与全文检索（数据模型与解析管道）"
    }

    fn schema_versions(&self) -> &'static [(&'static str, &'static [&'static str])] {
        schema::KB_SCHEMA
    }

    async fn init(&self, _ctx: &ServiceContext) -> Result<(), String> {
        Ok(())
    }
}
