//! 知识库服务：文档入库（解析 → 分块 → 全文索引）与向量检索（HNSW）。
//!
//! 分层（本目录）：
//! - `schema.rs`：服务级 schema（版本化迁移，由服务框架执行）
//! - `parse.rs`：文档解析与分块（纯函数，md 结构化 + txt）
//! - `data.rs`：CRUD / 导入 / 全文同步（仅依赖连接池，可独立单测）
//! - `embed.rs`：Embedder 抽象（渠道外呼 + Dummy）与库级绑定管理
//! - `index.rs`：HNSW 索引封装（usearch）+ 向量 BLOB 转换
//! - `indexing.rs`：异步任务管道（embed + 索引构建、进度、中断恢复）
//! - `search.rs`：关键词检索（FTS5/BM25）与溯源组装
//!
//! 服务实例持有每个库的**内存 HNSW 缓存**（`indexes` map）：启动时按
//! index_meta（ready）加载，检索命令经 `as_any().downcast_ref` 取回实例调用
//! [`KnowledgeBaseService::search_vector`]。SQLite 内向量与磁盘索引文件是权威
//! 与缓存关系，见 `index.rs` 的注释。

pub mod data;
pub mod embed;
mod hnsw;
pub mod index;
pub mod indexing;
mod parse;
mod schema;
pub mod search;

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use super::{Service, ServiceContext};
use index::VectorIndex;

/// 知识库服务：文档导入、分块、全文与向量检索
pub struct KnowledgeBaseService {
    /// 内存 HNSW 缓存（kb_id → 索引句柄）；文件与库为权威，本 map 只是热缓存
    indexes: RwLock<HashMap<String, Arc<VectorIndex>>>,
}

impl Default for KnowledgeBaseService {
    fn default() -> Self {
        Self {
            indexes: RwLock::new(HashMap::new()),
        }
    }
}

impl KnowledgeBaseService {
    /// 取库的内存索引；未加载时按给定 meta（须 ready）从文件加载。
    /// 同步 IO（本地小文件 ms 级）；调用方保证 meta 与库一致。
    fn ensure_loaded(
        &self,
        kb_id: &str,
        meta: &indexing::IndexMetaRow,
    ) -> Result<Arc<VectorIndex>, String> {
        if let Some(ix) = self.indexes.read().unwrap().get(kb_id) {
            return Ok(ix.clone());
        }
        let Some(path) = meta.index_path.as_deref() else {
            return Err("索引记录缺少文件路径，请重新构建".to_string());
        };
        let ix = VectorIndex::load(path, meta.embedding_dim as usize)?;
        let arc = Arc::new(ix);
        self.indexes.write().unwrap().insert(kb_id.to_string(), arc.clone());
        Ok(arc)
    }

    /// 丢弃库的内存索引（删除库 / 重建前调用）
    pub fn drop_index(&self, kb_id: &str) {
        self.indexes.write().unwrap().remove(kb_id);
    }

    /// 向量检索（服务入口）：绑定/索引状态/数据一致性三重校验后走 HNSW
    pub async fn search_vector(
        &self,
        pool: &sqlx::SqlitePool,
        kb_id: &str,
        query_vec: Vec<f32>,
        top_k: usize,
    ) -> Result<Vec<search::SearchHit>, String> {
        let Some(base) = data::read_base(pool, kb_id).await? else {
            return Err("知识库不存在".to_string());
        };
        if base.status != "active" {
            return Err("知识库已停用，无法检索".to_string());
        }
        if base.embedding_dim <= 0 || base.embedding_channel_id.is_none() {
            return Err("知识库尚未绑定 embedding 渠道，无法进行向量检索".to_string());
        }
        if query_vec.len() as i64 != base.embedding_dim {
            return Err(format!(
                "查询向量维度 {} 与库绑定维度 {} 不一致",
                query_vec.len(),
                base.embedding_dim
            ));
        }
        // 一致性守卫：索引构建时的分块数与当前一致，文档增删后未重建必须报错
        let actual = indexing::actual_chunk_count(pool, kb_id).await?;
        let meta = indexing::read_index_meta(pool, kb_id).await?;
        let Some(meta) = meta else {
            return Err("该知识库尚未构建向量索引，请先构建".to_string());
        };
        if meta.status != "ready" {
            return Err(format!("向量索引状态异常（{}），请重新构建", meta.status));
        }
        if actual != meta.chunk_count {
            return Err(format!(
                "索引与文档不同步（索引 {} 块，当前 {} 块），导入/删除文档后请重新构建",
                meta.chunk_count, actual
            ));
        }
        let ix = self.ensure_loaded(kb_id, &meta)?;
        let raw = ix.search(&query_vec, top_k.clamp(1, 100))?;
        if raw.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i64> = raw.iter().map(|(k, _)| *k).collect();
        let rows = search::chunks_by_ids(pool, &ids).await?;
        // 按距离顺序组装，score = 余弦相似度
        let mut hits = Vec::with_capacity(raw.len());
        for (id, dist) in raw {
            if let Some(r) = rows.iter().find(|r| r.id == id) {
                hits.push(search::SearchHit {
                    chunk_id: r.id,
                    document_id: r.document_id.clone(),
                    title: r.title.clone(),
                    heading: r.heading.clone(),
                    seq: r.seq,
                    content: r.content.clone(),
                    score: (1.0 - dist as f64).clamp(0.0, 1.0),
                });
            }
        }
        Ok(hits)
    }
}

#[async_trait::async_trait]
impl Service for KnowledgeBaseService {
    fn id(&self) -> &'static str {
        "kb"
    }

    fn name(&self) -> &'static str {
        "知识库"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn description(&self) -> &'static str {
        "本地知识库：文档导入、分块、全文检索与向量索引（embedding + HNSW）"
    }

    fn schema_versions(&self) -> &'static [(&'static str, &'static [&'static str])] {
        schema::KB_SCHEMA
    }

    async fn init(&self, ctx: &ServiceContext) -> Result<(), String> {
        // 中断的任务标记 failed（进程退出残留）
        indexing::recover_interrupted_tasks(&ctx.pool).await;
        // 加载全部 ready 索引到内存（单个失败仅告警，不阻塞启动）
        let metas = indexing::list_ready_index_metas(&ctx.pool)
            .await
            .unwrap_or_default();
        let mut loaded = 0usize;
        for m in metas {
            if let Some(path) = m.index_path {
                match VectorIndex::load(&path, m.embedding_dim as usize) {
                    Ok(ix) => {
                        self.indexes.write().unwrap().insert(m.kb_id.clone(), Arc::new(ix));
                        loaded += 1;
                    }
                    Err(e) => {
                        eprintln!(
                            "[kb] 加载向量索引 {} 失败（可重新构建）: {e}",
                            m.kb_id
                        );
                    }
                }
            }
        }
        if loaded > 0 {
            eprintln!("[kb] 已加载 {loaded} 个向量索引到内存");
        }
        Ok(())
    }
}
