//! 知识库数据操作层（CRUD / 导入 / 全文同步）。
//!
//! 本层只依赖连接池，不接触 Tauri/服务实例，便于独立单测。
//! 关键设计：
//! - 导入是**两阶段**：文档行先以 `importing` 落库（导入中状态可见），分块与全文
//!   索引入事务写成功后置 `ready`；失败回滚并补写 `failed` + 错误原因
//!   （进程在阶段间崩溃会残留 importing 行，属已知边界，启动清理后续做）；
//! - 全文索引（kb_chunks_fts / kb_chunks_bg）无外键：删除文档/知识库时先在同一
//!   事务内删其 rowid，再删主表行（级联清 chunks），保证不残留孤儿索引行；
//! - 去重：同知识库内相同内容（SHA-256）拒绝重复导入。

use sqlx::SqlitePool;

use crate::commands::util::{new_id, now_iso};

use super::parse;
use super::tokenize;

// ---- 常量与校验范围 ----

/// 分块大小允许范围（字符）
pub(crate) const CHUNK_SIZE_MIN: i64 = 100;
pub(crate) const CHUNK_SIZE_MAX: i64 = 4000;
/// 单文档导入上限（字符）
const MAX_IMPORT_CHARS: usize = 2_000_000;
/// 单文件导入上限（字节）
const MAX_IMPORT_BYTES: usize = 10 * 1024 * 1024;
/// 支持的导入格式
pub(crate) fn is_supported_format(format: &str) -> bool {
    matches!(format, "md" | "markdown" | "txt")
}

// ---- DTO（snake_case 匹配列；序列化给前端 camelCase）----

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbBase {
    pub id: String,
    pub name: String,
    pub description: String,
    pub chunk_size: i64,
    pub chunk_overlap: i64,
    pub excluded_dirs: String,
    pub excluded_files: String,
    pub included_files: String,
    pub embedding_dim: i64,
    pub index_status: String,
    /// 库级状态：active / disabled（跟随项目 channels.status 的 TEXT 惯例）
    pub status: String,
    /// 绑定 embedding 模型名（未启用语义检索时为空）
    pub embedding_model: Option<String>,
    /// 绑定 embedding 渠道 id（逻辑引用 channels 表，不建物理外键）
    pub embedding_channel_id: Option<String>,
    /// 问答渠道 id（RAG 用；与 embedding 绑定解耦，0004 字段）
    pub answer_channel_id: Option<String>,
    /// 问答模型名（绑定问答渠道时落库，默认取渠道默认模型）
    pub answer_model: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// 文档数（列表展示用）
    pub doc_count: i64,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbDocument {
    pub id: String,
    pub kb_id: String,
    pub title: String,
    pub format: String,
    /// 源文件字节大小（粘贴/文本导入为 0）
    pub file_size: i64,
    pub source_type: String,
    pub source_url: Option<String>,
    pub source_path: Option<String>,
    pub doc_meta: String,
    pub file_hash: String,
    pub char_count: i64,
    pub chunk_count: i64,
    pub status: String,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbChunk {
    pub id: i64,
    pub document_id: String,
    pub seq: i64,
    pub heading: String,
    pub content: String,
    pub chars: i64,
    /// 分块元数据 JSON（默认 {}，格式特有信息用）
    pub metadata: String,
}

// ---- 列清单（FromRow 要求显式列出）----

const KBASE_COLS: &str = "id, name, description, chunk_size, chunk_overlap, \
     excluded_dirs, excluded_files, included_files, embedding_dim, \
     index_status, status, embedding_model, embedding_channel_id, \
     answer_channel_id, answer_model, created_at, updated_at";
const KBASE_SELECT: &str = "SELECT id, name, description, chunk_size, chunk_overlap, \
     excluded_dirs, excluded_files, included_files, embedding_dim, \
     index_status, status, embedding_model, embedding_channel_id, \
     answer_channel_id, answer_model, created_at, updated_at, \
     (SELECT COUNT(*) FROM kb_documents d WHERE d.kb_id = kb_knowledge_bases.id) AS doc_count \
     FROM kb_knowledge_bases";
const DOC_COLS: &str = "id, kb_id, title, format, file_size, source_type, source_url, \
     source_path, doc_meta, file_hash, char_count, chunk_count, status, error, \
     created_at, updated_at";

// ---- 知识库 CRUD ----

pub async fn create_base(
    pool: &SqlitePool,
    name: &str,
    description: &str,
    chunk_size: i64,
    chunk_overlap: i64,
) -> Result<KbBase, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("知识库名称不能为空".to_string());
    }
    validate_chunk_params(chunk_size, chunk_overlap)?;
    let exists: Option<String> =
        sqlx::query_scalar("SELECT id FROM kb_knowledge_bases WHERE name = ?1")
            .bind(name)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;
    if exists.is_some() {
        return Err(format!("同名知识库已存在：{name}"));
    }
    let id = new_id("kb");
    let now = now_iso();
    sqlx::query(&format!(
        "INSERT INTO kb_knowledge_bases ({KBASE_COLS}) \
         VALUES (?1,?2,?3,?4,?5,'','','',0,'none','active',NULL,NULL,NULL,NULL,?6,?7)"
    ))
    .bind(&id)
    .bind(name)
    .bind(description.trim())
    .bind(chunk_size)
    .bind(chunk_overlap)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| format!("创建知识库失败: {e}"))?;
    Ok(read_base(pool, &id).await?.expect("新建知识库读取失败"))
}

pub async fn list_bases(pool: &SqlitePool) -> Result<Vec<KbBase>, String> {
    sqlx::query_as::<_, KbBase>(&format!("{KBASE_SELECT} ORDER BY created_at DESC"))
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())
}

pub async fn update_base(
    pool: &SqlitePool,
    id: &str,
    name: Option<&str>,
    description: Option<&str>,
    chunk_size: Option<i64>,
    chunk_overlap: Option<i64>,
    status: Option<&str>,
) -> Result<KbBase, String> {
    let Some(base) = read_base(pool, id).await? else {
        return Err("知识库不存在".to_string());
    };
    let name = match name {
        Some(n) => {
            let n = n.trim();
            if n.is_empty() {
                return Err("知识库名称不能为空".to_string());
            }
            n.to_string()
        }
        None => base.name,
    };
    // 重名检查（排除自身）
    let dup: Option<String> =
        sqlx::query_scalar("SELECT id FROM kb_knowledge_bases WHERE name = ?1 AND id != ?2")
            .bind(&name)
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;
    if dup.is_some() {
        return Err(format!("同名知识库已存在：{name}"));
    }
    let description = description.unwrap_or(&base.description).trim().to_string();
    let chunk_size = chunk_size.unwrap_or(base.chunk_size);
    let chunk_overlap = chunk_overlap.unwrap_or(base.chunk_overlap);
    validate_chunk_params(chunk_size, chunk_overlap)?;
    let status = match status {
        Some(s) if s == "active" || s == "disabled" => s.to_string(),
        Some(_) => return Err("知识库状态只允许 active / disabled".to_string()),
        None => base.status,
    };
    let now = now_iso();
    sqlx::query(
        "UPDATE kb_knowledge_bases SET name=?1, description=?2, chunk_size=?3, \
         chunk_overlap=?4, status=?5, updated_at=?6 WHERE id=?7",
    )
    .bind(name)
    .bind(description)
    .bind(chunk_size)
    .bind(chunk_overlap)
    .bind(status)
    .bind(&now)
    .bind(id)
    .execute(pool)
    .await
    .map_err(|e| format!("更新知识库失败: {e}"))?;
    Ok(read_base(pool, id).await?.expect("更新后读取失败"))
}

pub async fn delete_base(pool: &SqlitePool, id: &str) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    // FTS 无外键：先清本库全部 chunk 的全文行（两张索引表），再删库（级联文档/分块）
    for table in ["kb_chunks_fts", "kb_chunks_bg"] {
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE rowid IN (SELECT c.id FROM kb_chunks c \
             JOIN kb_documents d ON d.id = c.document_id WHERE d.kb_id = ?1)"
        ))
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("清理全文索引失败: {e}"))?;
    }
    let r = sqlx::query("DELETE FROM kb_knowledge_bases WHERE id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    if r.rows_affected() == 0 {
        return Err("知识库不存在".to_string()); // 事务回滚
    }
    tx.commit().await.map_err(|e| e.to_string())
}

// ---- 文档 CRUD ----

pub async fn list_documents(pool: &SqlitePool, kb_id: &str) -> Result<Vec<KbDocument>, String> {
    sqlx::query_as::<_, KbDocument>(&format!(
        "SELECT {DOC_COLS} FROM kb_documents WHERE kb_id = ?1 ORDER BY created_at DESC"
    ))
    .bind(kb_id)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())
}

pub async fn list_chunks(pool: &SqlitePool, document_id: &str) -> Result<Vec<KbChunk>, String> {
    sqlx::query_as::<_, KbChunk>(
        "SELECT id, document_id, seq, heading, content, chars, metadata FROM kb_chunks \
         WHERE document_id = ?1 ORDER BY seq",
    )
    .bind(document_id)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())
}

pub async fn delete_document(pool: &SqlitePool, document_id: &str) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    // 先删全文行（chunks 级联删除后子查询会查不到，顺序不可反）
    for table in ["kb_chunks_fts", "kb_chunks_bg"] {
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE rowid IN \
             (SELECT id FROM kb_chunks WHERE document_id = ?1)"
        ))
        .bind(document_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("清理全文索引失败: {e}"))?;
    }
    let r = sqlx::query("DELETE FROM kb_documents WHERE id = ?1")
        .bind(document_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    if r.rows_affected() == 0 {
        return Err("文档不存在".to_string());
    }
    tx.commit().await.map_err(|e| e.to_string())
}

// ---- 导入 ----

/// 文本导入（粘贴/测试用）：格式 md/txt，file_size 记 0
pub async fn import_text(
    pool: &SqlitePool,
    kb_id: &str,
    title: &str,
    format: &str,
    text: &str,
) -> Result<KbDocument, String> {
    import_core(pool, kb_id, title, format, "paste", None, 0, text).await
}

/// 文件导入：按扩展名识别格式，内容须为 UTF-8 文本
pub async fn import_file(
    pool: &SqlitePool,
    kb_id: &str,
    path: &str,
) -> Result<KbDocument, String> {
    let p = std::path::Path::new(path);
    let Some(ext) = p.extension().and_then(|e| e.to_str()) else {
        return Err("无法识别文件类型（缺少扩展名）".to_string());
    };
    let ext = ext.to_lowercase();
    let format = match ext.as_str() {
        "md" | "markdown" => "md",
        "txt" => "txt",
        other => return Err(format!("暂不支持文件格式: .{other}（当前支持 md/txt）")),
    };
    let bytes = std::fs::read(p).map_err(|e| format!("读取文件失败: {e}"))?;
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err(format!("文件过大（{} 字节），上限 {}MB", bytes.len(), MAX_IMPORT_BYTES / 1024 / 1024));
    }
    let file_size = bytes.len() as i64;
    let text = String::from_utf8(bytes)
        .map_err(|_| "文件不是有效的 UTF-8 文本，暂不支持".to_string())?;
    let title = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("未命名")
        .to_string();
    let source_path = Some(path.to_string());
    import_core(pool, kb_id, &title, format, "upload", source_path, file_size, &text).await
}

/// 导入核心：去重校验 → 解析分块 → 两阶段落库（importing → ready/failed）
#[allow(clippy::too_many_arguments)]
async fn import_core(
    pool: &SqlitePool,
    kb_id: &str,
    title: &str,
    format: &str,
    source_type: &str,
    source_path: Option<String>,
    file_size: i64,
    text: &str,
) -> Result<KbDocument, String> {
    if read_base(pool, kb_id).await?.is_none() {
        return Err("知识库不存在".to_string());
    }
    if !is_supported_format(format) {
        return Err(format!("暂不支持格式: {format}（当前支持 md/txt）"));
    }
    let title = title.trim();
    if title.is_empty() {
        return Err("标题不能为空".to_string());
    }
    if text.trim().is_empty() {
        return Err("内容为空".to_string());
    }
    let char_count = text.chars().count();
    if char_count > MAX_IMPORT_CHARS {
        return Err(format!("内容过长（{char_count} 字符），单文档上限 {MAX_IMPORT_CHARS}"));
    }
    let hash = sha256_hex(text.as_bytes());

    // 去重：同库同内容
    let dup: Option<String> =
        sqlx::query_scalar("SELECT title FROM kb_documents WHERE kb_id = ?1 AND file_hash = ?2")
            .bind(kb_id)
            .bind(&hash)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;
    if let Some(t) = dup {
        return Err(format!("内容重复：库中已存在文档《{t}》，如需更新请先删除"));
    }

    // 解析分块（纯函数；base 的分块参数在此生效）
    let base = read_base(pool, kb_id).await?.expect("已校验存在");
    let chunks = parse::split_sections(
        parse::parse_by_format(format, text),
        base.chunk_size as usize,
        base.chunk_overlap as usize,
    );
    if chunks.is_empty() {
        return Err("解析后无可分块内容".to_string());
    }
    let chunk_count = chunks.len() as i64;

    // 阶段一：文档行落库（importing），导入中状态可见
    let doc_id = new_id("doc");
    let now = now_iso();
    sqlx::query(
        "INSERT INTO kb_documents (id, kb_id, title, format, file_size, source_type, \
         source_url, source_path, doc_meta, file_hash, char_count, chunk_count, status, \
         error, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,NULL,?7,'{}',?8,?9,0,'importing',NULL,?10,?10)",
    )
    .bind(&doc_id)
    .bind(kb_id)
    .bind(title)
    .bind(format)
    .bind(file_size)
    .bind(source_type)
    .bind(source_path)
    .bind(&hash)
    .bind(char_count as i64)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| format!("写入文档失败: {e}"))?;

    // 阶段二：事务写分块 + 全文行 + 置 ready；失败回滚并补 failed
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let result: Result<(), String> = async {
        let mut seq = 0i64;
        for c in &chunks {
            seq += 1;
            sqlx::query(
                "INSERT INTO kb_chunks (document_id, seq, heading, content, chars) \
                 VALUES (?1,?2,?3,?4,?5)",
            )
            .bind(&doc_id)
            .bind(seq)
            .bind(&c.heading)
            .bind(&c.content)
            .bind(c.content.chars().count() as i64)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
            let chunk_id: i64 =
                sqlx::query_scalar("SELECT last_insert_rowid()")
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|e| e.to_string())?;
            sqlx::query(
                "INSERT INTO kb_chunks_fts (rowid, content, heading, title) VALUES (?1,?2,?3,?4)",
            )
            .bind(chunk_id)
            .bind(&c.content)
            .bind(&c.heading)
            .bind(title)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("写入全文索引失败: {e}"))?;
            // bigram 索引（unicode61）：CJK 转 2 字滑窗，英文整词透传
            sqlx::query(
                "INSERT INTO kb_chunks_bg (rowid, content_bg, heading_bg, title_bg) \
                 VALUES (?1,?2,?3,?4)",
            )
            .bind(chunk_id)
            .bind(tokenize::bigram_text(&c.content))
            .bind(tokenize::bigram_text(&c.heading))
            .bind(tokenize::bigram_text(title))
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("写入 bigram 索引失败: {e}"))?;
        }
        sqlx::query(
            "UPDATE kb_documents SET status='ready', chunk_count=?1, updated_at=?2 WHERE id=?3",
        )
        .bind(chunk_count)
        .bind(&now_iso())
        .bind(&doc_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }
    .await;
    match result {
        Ok(()) => {
            tx.commit().await.map_err(|e| e.to_string())?;
        }
        Err(e) => {
            // 事务回滚；文档行补 failed 留痕
            let _ = sqlx::query(
                "UPDATE kb_documents SET status='failed', error=?1, updated_at=?2 WHERE id=?3",
            )
            .bind(&e)
            .bind(&now_iso())
            .bind(&doc_id)
            .execute(pool)
            .await;
            return Err(format!("导入失败（已回滚分块）: {e}"));
        }
    }
    Ok(read_document(pool, &doc_id).await?.expect("导入后读取失败"))
}

// ---- 内部工具 ----

fn validate_chunk_params(chunk_size: i64, chunk_overlap: i64) -> Result<(), String> {
    if !(CHUNK_SIZE_MIN..=CHUNK_SIZE_MAX).contains(&chunk_size) {
        return Err(format!("分块大小应在 {CHUNK_SIZE_MIN}–{CHUNK_SIZE_MAX} 之间"));
    }
    if chunk_overlap < 0 || chunk_overlap >= chunk_size {
        return Err("重叠长度应不小于 0 且小于分块大小".to_string());
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// 内部读取（同目录模块共享；embedding 绑定/索引管道/检索前置校验用）
pub(crate) async fn read_base(pool: &SqlitePool, id: &str) -> Result<Option<KbBase>, String> {
    sqlx::query_as::<_, KbBase>(&format!("{KBASE_SELECT} WHERE kb_knowledge_bases.id = ?1"))
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())
}

/// 读取文档（同目录模块共享）
pub(crate) async fn read_document(pool: &SqlitePool, id: &str) -> Result<Option<KbDocument>, String> {
    sqlx::query_as::<_, KbDocument>(&format!("SELECT {DOC_COLS} FROM kb_documents WHERE id = ?1"))
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())
}

// ---- 关键词索引维护 ----

/// bigram 索引是否与分块表一致（不一致时检索层跳过 bigram 路，避免漏召回）
pub(crate) async fn bigram_index_ready(pool: &SqlitePool) -> bool {
    let chunks: i64 = sqlx::query_scalar("SELECT count(*) FROM kb_chunks")
        .fetch_one(pool)
        .await
        .unwrap_or(-1);
    let bg: i64 = sqlx::query_scalar("SELECT count(*) FROM kb_chunks_bg")
        .fetch_one(pool)
        .await
        .unwrap_or(-2);
    chunks == bg
}

/// 回填 / 重建 bigram 索引（服务启动时调用，幂等）。
///
/// bigram 串无法用 SQL 生成，只能由应用层从 `kb_chunks JOIN kb_documents`
/// 全量重建（迁移只建空表）。已一致时直接返回 0，不重复写。
pub(crate) async fn ensure_bigram_index(pool: &SqlitePool) -> Result<usize, String> {
    let chunks: i64 = sqlx::query_scalar("SELECT count(*) FROM kb_chunks")
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
    let bg: i64 = sqlx::query_scalar("SELECT count(*) FROM kb_chunks_bg")
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
    if chunks == bg {
        return Ok(0);
    }
    let rows: Vec<(i64, String, String, String)> = sqlx::query_as(
        "SELECT c.id, c.content, c.heading, d.title FROM kb_chunks c \
         JOIN kb_documents d ON d.id = c.document_id ORDER BY c.id",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query("DELETE FROM kb_chunks_bg")
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    for (id, content, heading, title) in &rows {
        sqlx::query(
            "INSERT INTO kb_chunks_bg (rowid, content_bg, heading_bg, title_bg) \
             VALUES (?1,?2,?3,?4)",
        )
        .bind(id)
        .bind(tokenize::bigram_text(content))
        .bind(tokenize::bigram_text(heading))
        .bind(tokenize::bigram_text(title))
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("回填 bigram 索引失败: {e}"))?;
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(rows.len())
}

// ---- 全文检索（第 2 步 UI/命令消费；先供内部测试断言同步正确性）----

#[cfg(test)]
pub(crate) async fn fts_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM kb_chunks_fts")
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

#[cfg(test)]
pub(crate) async fn bg_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM kb_chunks_bg")
        .fetch_one(pool)
        .await
        .unwrap_or(0)
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
        pool
    }

    async fn mk_base(pool: &SqlitePool) -> KbBase {
        create_base(pool, "测试库", "描述", 800, 150).await.unwrap()
    }

    const SAMPLE_MD: &str = "# 第一章 概述\n\n机器学习是一门让计算机从数据中学习的学科。\n\n## 1.1 历史\n\n深度学习自 2012 年起快速发展。\n\n# 第二章\n\n全文检索与分块是本项目的核心。";

    #[tokio::test]
    async fn base_crud_and_validation() {
        let pool = test_pool().await;
        // 创建 + 列表
        let b = create_base(&pool, "我的知识", "", 800, 150).await.unwrap();
        assert_eq!(b.doc_count, 0);
        assert_eq!(b.status, "active", "新库默认 active");
        assert!(b.embedding_model.is_none() && b.embedding_channel_id.is_none());
        let list = list_bases(&pool).await.unwrap();
        assert_eq!(list.len(), 1);
        // 校验：空名 / 重名 / 越界
        assert!(create_base(&pool, "  ", "", 800, 150).await.is_err());
        assert!(create_base(&pool, "我的知识", "", 800, 150).await.is_err());
        assert!(create_base(&pool, "x", "", 50, 10).await.is_err());
        assert!(create_base(&pool, "x", "", 800, 800).await.is_err());
        // 更新（部分字段）+ 库级停用 + 与其它库重名冲突
        let b = update_base(&pool, &b.id, Some("新名字"), None, Some(600), Some(100), None)
            .await
            .unwrap();
        assert_eq!(b.name, "新名字");
        assert_eq!(b.chunk_size, 600);
        let b = update_base(&pool, &b.id, None, None, None, None, Some("disabled"))
            .await
            .unwrap();
        assert_eq!(b.status, "disabled");
        assert!(update_base(&pool, &b.id, None, None, None, None, Some("paused")).await.is_err());
        let b2 = create_base(&pool, "另一个库", "", 800, 150).await.unwrap();
        assert!(update_base(&pool, &b.id, Some("另一个库"), None, None, None, None).await.is_err());
        delete_base(&pool, &b2.id).await.unwrap();
        // 更新不存在
        assert!(update_base(&pool, "kb_nope", Some("a"), None, None, None, None).await.is_err());
        // 删除
        delete_base(&pool, &b.id).await.unwrap();
        assert!(list_bases(&pool).await.unwrap().is_empty());
        assert!(delete_base(&pool, &b.id).await.is_err());
    }

    #[tokio::test]
    async fn import_md_produces_chunks_heading_and_fts_rows() {
        let pool = test_pool().await;
        let b = mk_base(&pool).await;
        let doc = import_text(&pool, &b.id, "示例文档", "md", SAMPLE_MD).await.unwrap();
        assert_eq!(doc.status, "ready");
        assert_eq!(doc.format, "md");
        assert_eq!(doc.chunk_count, 3, "两章三节应产生 3 块");
        assert_eq!(doc.char_count, SAMPLE_MD.chars().count() as i64);

        // 分块溯源
        let chunks = list_chunks(&pool, &doc.id).await.unwrap();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].seq, 1);
        assert_eq!(chunks[0].heading, "第一章 概述");
        assert_eq!(chunks[1].heading, "第一章 概述 / 1.1 历史");
        assert_eq!(chunks[2].heading, "第二章");
        assert_eq!(chunks[0].metadata, "{}", "分块元数据默认空对象");

        // 全文索引同步（每块一行）
        assert_eq!(fts_count(&pool).await, 3);

        // 去重：同内容拒绝
        let err = import_text(&pool, &b.id, "重复", "md", SAMPLE_MD).await.unwrap_err();
        assert!(err.contains("内容重复"), "应提示重复: {err}");
        assert_eq!(list_documents(&pool, &b.id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn import_long_txt_splits_with_overlap() {
        let pool = test_pool().await;
        let b = create_base(&pool, "长文库", "", 400, 100).await.unwrap();
        let body = "甲".repeat(1500);
        let doc = import_text(&pool, &b.id, "长文", "txt", &body).await.unwrap();
        assert_eq!(doc.status, "ready");
        assert!(doc.chunk_count >= 5, "1500 字符 / 400 窗口应多块: {}", doc.chunk_count);

        let chunks = list_chunks(&pool, &doc.id).await.unwrap();
        // 相邻块重叠衔接：块 2 以块 1 尾部 100 字符开头
        let c1 = chunks[0].content.clone();
        let c2 = &chunks[1].content;
        let chars: Vec<char> = c1.chars().collect();
        let tail: String = chars[chars.len().saturating_sub(100)..].iter().collect();
        assert!(c2.starts_with(&tail), "重叠窗口衔接失败");
    }

    #[tokio::test]
    async fn delete_document_cascades_chunks_and_fts() {
        let pool = test_pool().await;
        let b = mk_base(&pool).await;
        let doc = import_text(&pool, &b.id, "待删", "md", SAMPLE_MD).await.unwrap();
        assert_eq!(fts_count(&pool).await, 3);
        delete_document(&pool, &doc.id).await.unwrap();
        assert_eq!(list_documents(&pool, &b.id).await.unwrap().len(), 0);
        let remain: i64 = sqlx::query_scalar("SELECT count(*) FROM kb_chunks")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remain, 0, "分块应级联删除");
        assert_eq!(fts_count(&pool).await, 0, "全文行应同步删除");
        assert!(delete_document(&pool, &doc.id).await.is_err());
    }

    #[tokio::test]
    async fn bigram_index_syncs_with_chunks_and_backfills() {
        let pool = test_pool().await;
        let b = mk_base(&pool).await;
        let doc = import_text(&pool, &b.id, "网关笔记", "md", SAMPLE_MD).await.unwrap();
        // 导入即写两张索引表
        assert_eq!(fts_count(&pool).await, 3);
        assert_eq!(bg_count(&pool).await, 3);
        assert!(bigram_index_ready(&pool).await);
        // 标题进 bigram 索引（2 字词可命中标题）
        let hit: i64 =
            sqlx::query_scalar("SELECT count(*) FROM kb_chunks_bg WHERE kb_chunks_bg MATCH ?1")
                .bind("\"网关\"")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(hit > 0, "标题 bigram 应可检索");

        // 模拟迁移后的旧库：bg 表为空 → 启动回填（幂等）
        sqlx::query("DELETE FROM kb_chunks_bg").execute(&pool).await.unwrap();
        assert!(!bigram_index_ready(&pool).await);
        assert_eq!(ensure_bigram_index(&pool).await.unwrap(), 3, "应回填 3 行");
        assert!(bigram_index_ready(&pool).await);
        assert_eq!(ensure_bigram_index(&pool).await.unwrap(), 0, "已一致时不再重建");

        // 删除文档后两张表同步清空
        delete_document(&pool, &doc.id).await.unwrap();
        assert_eq!(fts_count(&pool).await, 0);
        assert_eq!(bg_count(&pool).await, 0);
    }

    #[tokio::test]
    async fn delete_base_cascades_everything() {
        let pool = test_pool().await;
        let b = mk_base(&pool).await;
        let d1 = import_text(&pool, &b.id, "一", "txt", "第一份文档内容。").await.unwrap();
        let d2 = import_text(&pool, &b.id, "二", "md", SAMPLE_MD).await.unwrap();
        let _ = (&d1, &d2);
        assert_eq!(fts_count(&pool).await, 4);
        delete_base(&pool, &b.id).await.unwrap();
        for (tbl, want) in [
            ("kb_knowledge_bases", 0i64),
            ("kb_documents", 0),
            ("kb_chunks", 0),
        ] {
            let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {tbl}"))
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(n, want, "{tbl} 应清空");
        }
        assert_eq!(fts_count(&pool).await, 0);
    }

    #[tokio::test]
    async fn import_validation_and_file_import() {
        let pool = test_pool().await;
        // 知识库不存在
        assert!(import_text(&pool, "kb_nope", "t", "txt", "x").await.is_err());
        let b = mk_base(&pool).await;
        // 空标题/空内容/坏格式
        assert!(import_text(&pool, &b.id, " ", "txt", "x").await.is_err());
        assert!(import_text(&pool, &b.id, "t", "txt", "  ").await.is_err());
        assert!(import_text(&pool, &b.id, "t", "pdf", "x").await.is_err());

        // 文件导入：临时目录写 md
        let dir = std::env::temp_dir();
        let path = dir.join("kb_import_test_notes.md");
        let content = "# 文件标题\n\n文件正文内容。";
        std::fs::write(&path, content).unwrap();
        let doc = import_file(&pool, &b.id, path.to_str().unwrap()).await.unwrap();
        assert_eq!(doc.title, "kb_import_test_notes");
        assert_eq!(doc.format, "md");
        assert_eq!(doc.source_type, "upload");
        assert_eq!(doc.file_size, content.len() as i64, "file_size 记录源文件字节数");
        assert_eq!(doc.chunk_count, 1);
        let _ = std::fs::remove_file(&path);

        // 坏扩展名
        assert!(import_file(&pool, &b.id, "x.exe").await.is_err());
    }
}
