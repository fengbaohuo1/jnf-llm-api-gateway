//! 知识库服务 schema（服务级迁移，由服务框架 `ServiceRegistry::apply_schema` 按版本执行）。
//!
//! 设计说明：
//! - 全部表带 `kb_id` 归属知识库（schema 从第一天支持多库，UI 是否多库另行决定）；
//! - `kb_chunks` 主键用 INTEGER（rowid），与 FTS5 虚拟表按 rowid 对齐；
//! - `kb_chunks_fts` 用 trigram 分词器支持中文子串检索；**非 external content**
//!   （内容双份），删除同步由应用层在同一事务内完成（FTS 无外键）；
//! - `kb_index_meta` 为向量索引状态机（none/building/ready/failed），二期接
//!   embedding 时使用；`kb_sessions/kb_conversations` 为 RAG 问答预留；
//! - `kb_sources` 为目录/git/URL 批量导入源记录（与过滤配置配合，功能后置）；
//! - **0002**：吸收外部方案亮点——库级状态与 embedding 渠道绑定
//!   （embedding_model/embedding_channel_id，逻辑引用 channels 表不建物理外键，
//!   渠道删除策略独立）、文档 file_size、分块 metadata JSON（pdf 页码等格式特有
//!   元数据兜底；md 结构溯源仍走 heading 列）。
//!
//! 迁移不可修改：已应用版本由框架记录并跳过，schema 演进只能追加新版本。

/// 知识库 v1：核心表（库/文档/分块/全文索引）+ 预留表（来源/索引元数据/会话/对话）
pub(crate) static KB_SCHEMA: &[(&'static str, &'static [&'static str])] = &[
    (
        "0001_kb_core",
        &[
            // ---- 知识库 ----
            "CREATE TABLE IF NOT EXISTS kb_knowledge_bases (
                id             TEXT PRIMARY KEY,
                name           TEXT NOT NULL UNIQUE,
                description    TEXT NOT NULL DEFAULT '',
                chunk_size     INTEGER NOT NULL DEFAULT 800,
                chunk_overlap  INTEGER NOT NULL DEFAULT 150,
                excluded_dirs  TEXT NOT NULL DEFAULT '',
                excluded_files TEXT NOT NULL DEFAULT '',
                included_files TEXT NOT NULL DEFAULT '',
                embedding_dim  INTEGER NOT NULL DEFAULT 0,
                index_status   TEXT NOT NULL DEFAULT 'none',
                created_at     TEXT NOT NULL,
                updated_at     TEXT NOT NULL
            )",
            // ---- 文档 ----
            "CREATE TABLE IF NOT EXISTS kb_documents (
                id          TEXT PRIMARY KEY,
                kb_id       TEXT NOT NULL REFERENCES kb_knowledge_bases(id) ON DELETE CASCADE,
                title       TEXT NOT NULL,
                format      TEXT NOT NULL DEFAULT 'txt',
                source_type TEXT NOT NULL DEFAULT 'upload',
                source_url  TEXT,
                source_path TEXT,
                doc_meta    TEXT NOT NULL DEFAULT '{}',
                file_hash   TEXT NOT NULL,
                char_count  INTEGER NOT NULL DEFAULT 0,
                chunk_count INTEGER NOT NULL DEFAULT 0,
                status      TEXT NOT NULL DEFAULT 'importing',
                error       TEXT,
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL
            )",
            "CREATE INDEX IF NOT EXISTS idx_kb_documents_kb ON kb_documents(kb_id, created_at)",
            "CREATE UNIQUE INDEX IF NOT EXISTS uq_kb_documents_hash ON kb_documents(kb_id, file_hash)",
            // ---- 分块（检索与溯源的最小单位）----
            "CREATE TABLE IF NOT EXISTS kb_chunks (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                document_id TEXT NOT NULL REFERENCES kb_documents(id) ON DELETE CASCADE,
                seq         INTEGER NOT NULL,
                heading     TEXT NOT NULL DEFAULT '',
                content     TEXT NOT NULL,
                chars       INTEGER NOT NULL DEFAULT 0,
                embedding   BLOB
            )",
            "CREATE INDEX IF NOT EXISTS idx_kb_chunks_doc ON kb_chunks(document_id, seq)",
            // ---- 全文索引（trigram：中文 3 字符子串匹配）----
            "CREATE VIRTUAL TABLE IF NOT EXISTS kb_chunks_fts USING fts5(
                content,
                tokenize = 'trigram'
            )",
            // ---- 批量导入源（git/url/local_dir，功能后置）----
            "CREATE TABLE IF NOT EXISTS kb_sources (
                id          TEXT PRIMARY KEY,
                kb_id       TEXT NOT NULL REFERENCES kb_knowledge_bases(id) ON DELETE CASCADE,
                source_type TEXT NOT NULL,
                source_url  TEXT,
                source_path TEXT,
                branch      TEXT,
                status      TEXT NOT NULL DEFAULT 'pending',
                file_count  INTEGER NOT NULL DEFAULT 0,
                error       TEXT,
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL
            )",
            "CREATE INDEX IF NOT EXISTS idx_kb_sources_kb ON kb_sources(kb_id)",
            // ---- 向量索引状态机（二期 embedding 用）----
            "CREATE TABLE IF NOT EXISTS kb_index_meta (
                kb_id         TEXT PRIMARY KEY REFERENCES kb_knowledge_bases(id) ON DELETE CASCADE,
                index_type    TEXT NOT NULL DEFAULT 'hnsw',
                embedding_dim INTEGER NOT NULL DEFAULT 0,
                chunk_count   INTEGER NOT NULL DEFAULT 0,
                index_path    TEXT,
                built_at      TEXT,
                status        TEXT NOT NULL DEFAULT 'none',
                error         TEXT
            )",
            // ---- RAG 问答会话（功能后置）----
            "CREATE TABLE IF NOT EXISTS kb_sessions (
                id         TEXT PRIMARY KEY,
                kb_id      TEXT NOT NULL REFERENCES kb_knowledge_bases(id) ON DELETE CASCADE,
                title      TEXT NOT NULL DEFAULT '新对话',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
            "CREATE INDEX IF NOT EXISTS idx_kb_sessions_kb ON kb_sessions(kb_id, created_at)",
            "CREATE TABLE IF NOT EXISTS kb_conversations (
                id         TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES kb_sessions(id) ON DELETE CASCADE,
                role       TEXT NOT NULL,
                content    TEXT NOT NULL,
                sources    TEXT,
                model      TEXT,
                tokens_used INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            )",
            "CREATE INDEX IF NOT EXISTS idx_kb_conversations_session ON kb_conversations(session_id, created_at)",
        ],
    ),
    (
        "0002_kb_embedding_and_metadata",
        &[
            // 库级状态（active/disabled 跟随项目 channels.status 的 TEXT 惯例）
            "ALTER TABLE kb_knowledge_bases ADD COLUMN status TEXT NOT NULL DEFAULT 'active'",
            // embedding 渠道绑定：逻辑引用 channels（不建物理外键，渠道删除策略独立）
            "ALTER TABLE kb_knowledge_bases ADD COLUMN embedding_model TEXT",
            "ALTER TABLE kb_knowledge_bases ADD COLUMN embedding_channel_id TEXT",
            // 文件字节大小（文本粘贴导入为 0）
            "ALTER TABLE kb_documents ADD COLUMN file_size INTEGER NOT NULL DEFAULT 0",
            // 分块元数据 JSON（格式特有信息；md 结构溯源仍用 heading 列）
            "ALTER TABLE kb_chunks ADD COLUMN metadata TEXT NOT NULL DEFAULT '{}'",
        ],
    ),
];

/// 直接执行全部 schema（幂等）。测试与临时工具用；正式路径由
/// `ServiceRegistry::apply_schema` 版本化管理。
#[allow(dead_code)] // 目前仅测试使用；保留供维护/诊断命令复用
pub(crate) async fn apply_schema_direct(pool: &sqlx::SqlitePool) -> Result<(), String> {
    for (_ver, stmts) in KB_SCHEMA {
        for sql in *stmts {
            sqlx::query(sql)
                .execute(pool)
                .await
                .map_err(|e| format!("执行知识库 schema 失败: {e}"))?;
        }
    }
    Ok(())
}
