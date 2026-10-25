//! 索引构建任务管道（kb_tasks）。
//!
//! embedding 外呼是分钟级异步操作，必须任务化：
//! - `kb_build_index` 命令建任务行并在后台 spawn 本模块的 [`run_embed_index_task`]；
//! - 管道流程：收集缺向量分块 → 分批 embedding（写回 chunks.embedding）→
//!   从全量向量构建 HNSW → save 文件 → 写 index_meta（ready）→ 任务 ready；
//! - 失败：任务 failed + error；index_meta/bases.index_status 同步 failed；
//! - 中断恢复：应用启动时 [`recover_interrupted_tasks`] 把残留 running/pending
//!   标记为 failed（中断），可重新构建（已入库向量不会重复 embed——只扫缺向量块）。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;

use crate::commands::util::{new_id, now_iso};

use super::data;
use super::embed::{self, Embedder};
use super::index::VectorIndex;

/// 单批 embedding 文本数
const BATCH_SIZE: usize = 64;
/// 单批失败重试次数（含首次共 3 次尝试）
const MAX_ATTEMPTS: u32 = 3;

/// 嵌入文本配方版本：v1 = `标题 + 章节路径 + 正文`（上下文前缀）。
/// 写入 `kb_index_meta.text_version`；与检索侧常量不一致时要求重建索引
/// （旧配方向量没有标题/章节语义，混用会让同一库出现两套语义）。
pub const EMBED_TEXT_VERSION: i64 = 1;

/// 单块送入 embedding 的文本（存储与展示仍是原始分块内容）
pub(crate) fn embed_text(title: &str, heading: &str, content: &str) -> String {
    let mut s = String::new();
    for part in [title.trim(), heading.trim(), content.trim()] {
        if part.is_empty() {
            continue;
        }
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str(part);
    }
    s
}

/// 任务 DTO（前端进度轮询）
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbTask {
    pub id: String,
    pub kb_id: String,
    pub task_type: String,
    /// pending | running | ready | failed
    pub status: String,
    pub total_items: i64,
    pub done_items: i64,
    pub progress: i64,
    pub error_message: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

/// index_meta 行（状态机读取用；index_type/built_at/error 为 DTO 完整列，
/// 构建与检索路径按需读取，未来索引管理命令会全量使用）
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct IndexMetaRow {
    pub kb_id: String,
    #[allow(dead_code)]
    pub index_type: String,
    pub embedding_dim: i64,
    pub chunk_count: i64,
    pub index_path: Option<String>,
    #[allow(dead_code)]
    pub built_at: Option<String>,
    pub status: String,
    #[allow(dead_code)]
    pub error: Option<String>,
    /// 嵌入文本配方版本（见 [`EMBED_TEXT_VERSION`]）
    pub text_version: i64,
}

const TASK_COLS: &str = "id, kb_id, task_type, status, total_items, done_items, progress, \
     error_message, created_at, completed_at";

// ---- 任务 CRUD ----

pub async fn create_task(pool: &SqlitePool, kb_id: &str, task_type: &str) -> Result<String, String> {
    let id = new_id("tsk");
    let now = now_iso();
    sqlx::query(&format!(
        "INSERT INTO kb_tasks ({TASK_COLS}) VALUES (?1,?2,?3,'pending',0,0,0,NULL,?4,NULL)"
    ))
    .bind(&id)
    .bind(kb_id)
    .bind(task_type)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| format!("创建任务失败: {e}"))?;
    Ok(id)
}

pub async fn list_tasks(
    pool: &SqlitePool,
    kb_id: &str,
    limit: i64,
) -> Result<Vec<KbTask>, String> {
    sqlx::query_as::<_, KbTask>(&format!(
        "SELECT {TASK_COLS} FROM kb_tasks WHERE kb_id = ?1 ORDER BY created_at DESC LIMIT ?2"
    ))
    .bind(kb_id)
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())
}

/// 是否存在进行中的同类任务（防并发构建）
pub async fn has_running_task(pool: &SqlitePool, kb_id: &str) -> Result<bool, String> {
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM kb_tasks WHERE kb_id = ?1 AND status IN ('pending','running')",
    )
    .bind(kb_id)
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

async fn read_task(pool: &SqlitePool, task_id: &str) -> Result<Option<KbTask>, String> {
    sqlx::query_as::<_, KbTask>(&format!("SELECT {TASK_COLS} FROM kb_tasks WHERE id = ?1"))
        .bind(task_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())
}

async fn update_task(
    pool: &SqlitePool,
    task_id: &str,
    status: &str,
    total_items: i64,
    done_items: i64,
    error_message: Option<&str>,
) -> Result<(), String> {
    let progress = if total_items <= 0 {
        0
    } else {
        (done_items * 100 / total_items).min(100)
    };
    sqlx::query(
        "UPDATE kb_tasks SET status=?1, total_items=?2, done_items=?3, progress=?4, \
         error_message=?5, completed_at=CASE WHEN ?1 IN ('ready','failed') THEN ?6 ELSE NULL END \
         WHERE id=?7",
    )
    .bind(status)
    .bind(total_items)
    .bind(done_items)
    .bind(progress)
    .bind(error_message)
    .bind(&now_iso())
    .bind(task_id)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 应用启动时调用：中断残留的任务标记为 failed（进程退出时的 running/pending）
pub async fn recover_interrupted_tasks(pool: &SqlitePool) {
    let _ = sqlx::query(
        "UPDATE kb_tasks SET status='failed', \
         error_message='任务被应用中断，未完成（可重新构建）', \
         completed_at=?1 WHERE status IN ('pending','running')",
    )
    .bind(now_iso())
    .execute(pool)
    .await;
}

// ---- index_meta 读写 ----

pub(crate) async fn read_index_meta(
    pool: &SqlitePool,
    kb_id: &str,
) -> Result<Option<IndexMetaRow>, String> {
    sqlx::query_as::<_, IndexMetaRow>(
        "SELECT kb_id, index_type, embedding_dim, chunk_count, index_path, built_at, status, \
         error, text_version FROM kb_index_meta WHERE kb_id = ?1",
    )
    .bind(kb_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())
}

/// 全部 ready 索引（服务启动加载用）
pub(crate) async fn list_ready_index_metas(pool: &SqlitePool) -> Result<Vec<IndexMetaRow>, String> {
    sqlx::query_as::<_, IndexMetaRow>(
        "SELECT kb_id, index_type, embedding_dim, chunk_count, index_path, built_at, status, \
         error, text_version FROM kb_index_meta WHERE status = 'ready' ORDER BY built_at",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())
}

/// 库内实际分块数（一致性校验：索引 chunk_count 必须等于它）
pub(crate) async fn actual_chunk_count(pool: &SqlitePool, kb_id: &str) -> Result<i64, String> {
    sqlx::query_scalar(
        "SELECT count(*) FROM kb_chunks c JOIN kb_documents d ON d.id = c.document_id \
         WHERE d.kb_id = ?1",
    )
    .bind(kb_id)
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())
}

/// 库对应的 HNSW 文件路径
pub fn index_file_path(index_dir: &Path, kb_id: &str) -> String {
    index_dir
        .join(format!("{kb_id}.hnsw"))
        .to_string_lossy()
        .to_string()
}

// ---- 构建管道 ----

/// 按库绑定构造渠道 embedder（无绑定/渠道不可用时 Err）
pub async fn build_embedder(pool: &SqlitePool, kb_id: &str) -> Result<Arc<dyn Embedder>, String> {
    let (channel_id, model) = embed::bound_binding(pool, kb_id).await?;
    Ok(Arc::new(
        embed::ChannelEmbedder::from_channel(pool, &channel_id, &model).await?,
    ))
}

/// 执行一次 embed+index 任务直到终态（ready/failed）。
/// `embedder`：测试注入 Dummy 用；None 时按库绑定构造渠道 embedder。
pub async fn run_embed_index_task(
    pool: &SqlitePool,
    task_id: &str,
    index_dir: &Path,
    embedder: Option<Arc<dyn Embedder>>,
) -> Result<(), String> {
    let Some(task) = read_task(pool, task_id).await? else {
        return Err(format!("任务不存在：{task_id}"));
    };
    let kb_id = task.kb_id.clone();
    let result: Result<(), String> = async {
        let Some(base) = data::read_base(pool, &kb_id).await? else {
            return Err("知识库不存在".to_string());
        };
        if base.status != "active" {
            return Err("知识库已停用，无法构建索引".to_string());
        }
        let embedder = match embedder {
            Some(e) => e,
            None => build_embedder(pool, &kb_id).await?,
        };
        eprintln!("[kb] 任务 {task_id}（{kb_id}）embedder: {}", embedder.describe());
        if base.embedding_dim as usize != embedder.dimensions() {
            return Err(format!(
                "embedding 维度不匹配：库绑定 {}，实际 {}（换绑模型后请重新绑定）",
                base.embedding_dim,
                embedder.dimensions()
            ));
        }
        // 收集缺向量分块（已 embed 的跳过——任务可安全重跑）
        let pending: Vec<(i64, String, String, String)> = sqlx::query_as(
            "SELECT c.id, c.content, c.heading, d.title FROM kb_chunks c \
             JOIN kb_documents d ON d.id = c.document_id \
             WHERE d.kb_id = ?1 AND c.embedding IS NULL ORDER BY c.id",
        )
        .bind(&kb_id)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
        let total = pending.len() as i64;
        update_task(pool, task_id, "running", total, 0, None).await?;

        // 分批 embedding 并写回
        let mut done = 0i64;
        let mut batch_no = 0usize;
        for batch in pending.chunks(BATCH_SIZE) {
            batch_no += 1;
            let texts: Vec<String> = batch
                .iter()
                .map(|(_, content, heading, title)| embed_text(title, heading, content))
                .collect();
            let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
            let vecs = retry_embed(embedder.as_ref(), &refs).await.map_err(|e| {
                format!("第 {batch_no} 批（{} 块）embedding 失败: {e}", batch.len())
            })?;
            let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
            for ((id, _, _, _), v) in batch.iter().zip(vecs.iter()) {
                sqlx::query("UPDATE kb_chunks SET embedding = ?1 WHERE id = ?2")
                    .bind(super::index::blob_from_vec(v))
                    .bind(id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            tx.commit().await.map_err(|e| e.to_string())?;
            done += batch.len() as i64;
            update_task(pool, task_id, "running", total, done, None).await?;
        }

        // 从全量向量构建 HNSW（向量权威存储 = chunks.embedding）
        let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as(
            "SELECT c.id, c.embedding FROM kb_chunks c \
             JOIN kb_documents d ON d.id = c.document_id \
             WHERE d.kb_id = ?1 AND c.embedding IS NOT NULL ORDER BY c.id",
        )
        .bind(&kb_id)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
        if rows.is_empty() {
            return Err("没有可索引的向量（库内无分块）".to_string());
        }
        let dim = embedder.dimensions();
        let mut ix = VectorIndex::new(dim)?;
        for (id, blob) in &rows {
            let v = super::index::vec_from_blob(blob)?;
            ix.add(*id, &v)?;
        }
        let path = index_file_path(index_dir, &kb_id);
        ix.save(&path)?;

        // 索引元数据 + 库状态
        let now = now_iso();
        sqlx::query(
            "INSERT INTO kb_index_meta (kb_id, index_type, embedding_dim, chunk_count, \
             index_path, built_at, status, error, text_version) \
             VALUES (?1,'hnsw',?2,?3,?4,?5,'ready',NULL,?6) \
             ON CONFLICT(kb_id) DO UPDATE SET index_type='hnsw', embedding_dim=?2, \
             chunk_count=?3, index_path=?4, built_at=?5, status='ready', error=NULL, \
             text_version=?6",
        )
        .bind(&kb_id)
        .bind(dim as i64)
        .bind(rows.len() as i64)
        .bind(&path)
        .bind(&now)
        .bind(EMBED_TEXT_VERSION)
        .execute(pool)
        .await
        .map_err(|e| format!("写入索引元数据失败: {e}"))?;
        sqlx::query("UPDATE kb_knowledge_bases SET index_status='ready', updated_at=?1 WHERE id=?2")
            .bind(&now)
            .bind(&kb_id)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    .await;

    match &result {
        Ok(()) => {
            // 完成时以任务行**当前**进度为准：管道运行中已累加 total/done，
            // 用启动前读到的快照会把进度覆盖成 0/0（历史 bug）。
            let cur = read_task(pool, task_id).await?;
            let total = cur.as_ref().map(|t| t.total_items).unwrap_or(0);
            update_task(pool, task_id, "ready", total, total, None).await?;
        }
        Err(e) => {
            let cur = read_task(pool, task_id).await?;
            let (total, done) = cur
                .as_ref()
                .map(|t| (t.total_items, t.done_items))
                .unwrap_or((0, 0));
            update_task(pool, task_id, "failed", total, done, Some(e)).await?;
            let _ = sqlx::query(
                "INSERT INTO kb_index_meta (kb_id, index_type, embedding_dim, chunk_count, \
                 index_path, built_at, status, error) VALUES (?1,'hnsw',0,0,NULL,NULL,'failed',?2) \
                 ON CONFLICT(kb_id) DO UPDATE SET status='failed', error=?2, built_at=NULL",
            )
            .bind(&kb_id)
            .bind(e)
            .execute(pool)
            .await;
            let _ = sqlx::query(
                "UPDATE kb_knowledge_bases SET index_status='failed', updated_at=?1 WHERE id=?2",
            )
            .bind(now_iso())
            .bind(&kb_id)
            .execute(pool)
            .await;
        }
    }
    result
}

/// 单批 embedding 带重试（渠道瞬时故障 429/5xx/超时）
async fn retry_embed(embedder: &dyn Embedder, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
    let mut last = String::new();
    for attempt in 0..MAX_ATTEMPTS {
        match embedder.embed_texts(texts).await {
            Ok(v) => return Ok(v),
            Err(err) => {
                last = err;
                if attempt + 1 < MAX_ATTEMPTS {
                    tokio::time::sleep(Duration::from_millis(300 * (attempt as u64 + 1))).await;
                }
            }
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::kb::schema;
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
        schema::apply_schema_direct(&pool).await.unwrap();
        // 最小渠道表（bind 校验需要）
        sqlx::query(
            "CREATE TABLE channels (id TEXT PRIMARY KEY, name TEXT, type TEXT, protocol TEXT NOT NULL DEFAULT '', base_url TEXT, \
             model TEXT, status TEXT, api_key_encrypted TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO channels (id, name, type, base_url, model, status, api_key_encrypted) \
             VALUES ('ch_mock', 'mock', 'OpenAI', 'http://mock.local', 'mock-model', 'active', 'k')",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn tmp_index_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kb-idx-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 记录送进 embedding 的文本（验证上下文前缀配方）
    struct RecordingEmbedder {
        dims: usize,
        seen: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl Embedder for RecordingEmbedder {
        fn dimensions(&self) -> usize {
            self.dims
        }
        fn describe(&self) -> String {
            "recording".to_string()
        }
        async fn embed_texts(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
            self.seen
                .lock()
                .unwrap()
                .extend(texts.iter().map(|t| t.to_string()));
            Ok(texts
                .iter()
                .map(|t| embed::dummy_vector(t, self.dims))
                .collect())
        }
    }

    #[test]
    fn embed_text_prefixes_title_and_heading() {
        assert_eq!(embed_text("标题", "第一章", "正文"), "标题\n第一章\n正文");
        // 空段跳过，不留空行
        assert_eq!(embed_text("标题", "", "正文"), "标题\n正文");
        assert_eq!(embed_text("", "", "正文"), "正文");
        assert_eq!(embed_text(" 标题 ", "  ", " 正文 "), "标题\n正文");
    }

    /// 端到端：嵌入文本带标题/章节前缀，索引元数据记录配方版本
    #[tokio::test]
    async fn pipeline_embeds_context_prefix_and_records_version() {
        let pool = test_pool().await;
        let b = data::create_base(&pool, "前缀库", "", 800, 150).await.unwrap();
        data::import_text(&pool, &b.id, "网关笔记", "md", "# 第四章 故障转移\n\n上游 5xx 时按序转移。")
            .await
            .unwrap();
        embed::bind_embedding(&pool, &b.id, "ch_mock", "text-embedding-3-small")
            .await
            .unwrap();
        let dir = tmp_index_dir("prefix").await;
        let task_id = create_task(&pool, &b.id, "embed_index").await.unwrap();
        let rec = std::sync::Arc::new(RecordingEmbedder {
            dims: 1536,
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let e: Arc<dyn Embedder> = rec.clone();
        run_embed_index_task(&pool, &task_id, &dir, Some(e)).await.unwrap();

        let seen = rec.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].starts_with("网关笔记\n第四章 故障转移\n"), "嵌入文本应带标题与章节前缀: {}", seen[0]);
        assert!(seen[0].contains("上游 5xx 时按序转移"));

        let meta = read_index_meta(&pool, &b.id).await.unwrap().unwrap();
        assert_eq!(meta.text_version, EMBED_TEXT_VERSION, "应记录嵌入文本配方版本");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 端到端：绑定 → 导入 → 构建（Dummy）→ 向量检索命中 → 一致性守卫
    #[tokio::test]
    async fn full_pipeline_dummy_embed_and_vector_search() {
        let pool = test_pool().await;
        let b = data::create_base(&pool, "向量库", "", 800, 150).await.unwrap();
        data::import_text(&pool, &b.id, "机器学习", "txt", "机器学习是人工智能的核心分支，研究如何让计算机从数据中自动学习。")
            .await
            .unwrap();
        data::import_text(&pool, &b.id, "园艺", "txt", "园艺种植需要注意土壤、光照与浇水的平衡。")
            .await
            .unwrap();

        // 绑定（text-embedding-3-small → 1536 维）
        let b = embed::bind_embedding(&pool, &b.id, "ch_mock", "text-embedding-3-small")
            .await
            .unwrap();
        assert_eq!(b.embedding_dim, 1536);
        assert_eq!(b.index_status, "none");

        // 构建（Dummy 注入，维度与绑定一致）
        let dir = tmp_index_dir("pipe").await;
        let task_id = create_task(&pool, &b.id, "embed_index").await.unwrap();
        let embedder: Arc<dyn Embedder> = Arc::new(embed::DummyEmbedder { dims: 1536 });
        run_embed_index_task(&pool, &task_id, &dir, Some(embedder))
            .await
            .unwrap();

        // 任务 ready、向量全量写回、索引元数据 ready、文件落盘
        let tasks = list_tasks(&pool, &b.id, 10).await.unwrap();
        assert_eq!(tasks[0].status, "ready");
        assert_eq!(
            (tasks[0].total_items, tasks[0].done_items, tasks[0].progress),
            (2, 2, 100),
            "完成的任务必须保留真实进度（不能覆盖成 0/0）"
        );
        let remain: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM kb_chunks c JOIN kb_documents d ON d.id = c.document_id \
             WHERE d.kb_id = ?1 AND c.embedding IS NULL",
        )
        .bind(&b.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remain, 0, "全部块应已向量化");
        let meta = read_index_meta(&pool, &b.id).await.unwrap().unwrap();
        assert_eq!(meta.status, "ready");
        assert_eq!(meta.embedding_dim, 1536);
        assert_eq!(meta.chunk_count, 2);
        assert!(std::path::Path::new(meta.index_path.as_deref().unwrap()).exists());

        // 向量检索：查询向量按同一配方（标题 + 章节 + 正文）生成 → 应为最近命中
        let svc = crate::services::kb::KnowledgeBaseService::default();
        let query = embed::dummy_vector(
            &embed_text(
                "机器学习",
                "",
                "机器学习是人工智能的核心分支，研究如何让计算机从数据中自动学习。",
            ),
            1536,
        );
        let hits = svc
            .search_vector(&pool, &b.id, query.clone(), 3)
            .await
            .unwrap();
        assert_eq!(hits.len(), 2, "两块的库 top3 应全部返回");
        assert_eq!(hits[0].title, "机器学习", "同文本块应排第一");
        assert!(hits[0].score > 0.99, "同文本相似度应≈1: {}", hits[0].score);
        assert!(
            hits[0].score > hits[1].score,
            "命中块相关度应高于无关块: {} vs {}",
            hits[0].score,
            hits[1].score
        );

        // 一致性守卫：导入新文档后未重建 → 明确报错而非返回过期结果
        data::import_text(&pool, &b.id, "新增", "txt", "导入新文档会让索引过期。").await.unwrap();
        let err = svc
            .search_vector(&pool, &b.id, query.clone(), 3)
            .await
            .unwrap_err();
        assert!(err.contains("不同步"), "应提示重建: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn rebind_invalidates_old_vectors() {
        let pool = test_pool().await;
        let b = data::create_base(&pool, "库", "", 800, 150).await.unwrap();
        data::import_text(&pool, &b.id, "d", "txt", "换绑测试文档内容。").await.unwrap();
        embed::bind_embedding(&pool, &b.id, "ch_mock", "text-embedding-3-small")
            .await
            .unwrap();
        let dir = tmp_index_dir("rebind").await;
        let task_id = create_task(&pool, &b.id, "embed_index").await.unwrap();
        let e: Arc<dyn Embedder> = Arc::new(embed::DummyEmbedder { dims: 1536 });
        run_embed_index_task(&pool, &task_id, &dir, Some(e)).await.unwrap();
        assert_eq!(read_index_meta(&pool, &b.id).await.unwrap().unwrap().status, "ready");

        // 换绑另一模型 → 旧向量与索引元数据全部作废
        let b = embed::bind_embedding(&pool, &b.id, "ch_mock", "bge-m3").await.unwrap();
        assert_eq!(b.embedding_dim, 1024);
        assert_eq!(b.index_status, "none");
        let remain: i64 = sqlx::query_scalar("SELECT count(*) FROM kb_chunks WHERE embedding IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remain, 0, "换绑应清空全部旧向量");
        assert!(read_index_meta(&pool, &b.id).await.unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn stale_embed_text_version_is_rejected() {
        let pool = test_pool().await;
        let b = data::create_base(&pool, "版本库", "", 800, 150).await.unwrap();
        data::import_text(&pool, &b.id, "d", "txt", "旧配方索引校验内容。").await.unwrap();
        embed::bind_embedding(&pool, &b.id, "ch_mock", "text-embedding-3-small")
            .await
            .unwrap();
        let dir = tmp_index_dir("stale").await;
        let task_id = create_task(&pool, &b.id, "embed_index").await.unwrap();
        let e: Arc<dyn Embedder> = Arc::new(embed::DummyEmbedder { dims: 1536 });
        run_embed_index_task(&pool, &task_id, &dir, Some(e)).await.unwrap();

        // 模拟旧版构建：配方版本回退 → 检索必须明确要求重建，而非静默混用两套语义
        sqlx::query("UPDATE kb_index_meta SET text_version = 0 WHERE kb_id = ?1")
            .bind(&b.id)
            .execute(&pool)
            .await
            .unwrap();
        let svc = crate::services::kb::KnowledgeBaseService::default();
        let err = svc
            .search_vector(&pool, &b.id, vec![0.1; 1536], 3)
            .await
            .unwrap_err();
        assert!(err.contains("重建索引"), "应提示重建: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn interrupted_tasks_recovered_on_start() {
        let pool = test_pool().await;
        let b = data::create_base(&pool, "库", "", 800, 150).await.unwrap();
        let tid = create_task(&pool, &b.id, "embed_index").await.unwrap();
        // 模拟进程中断：任务残留 running
        sqlx::query("UPDATE kb_tasks SET status='running' WHERE id=?1")
            .bind(&tid)
            .execute(&pool)
            .await
            .unwrap();
        recover_interrupted_tasks(&pool).await;
        let tasks = list_tasks(&pool, &b.id, 10).await.unwrap();
        assert_eq!(tasks[0].status, "failed");
        assert!(tasks[0].error_message.as_deref().unwrap_or("").contains("中断"));
    }
}
