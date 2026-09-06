//! 检索：关键词（FTS5 BM25 / 短查询 LIKE 回退）与向量命中的溯源组装。
//!
//! 关键词检索的已知边界：trigram 少于 3 字符无命中 → 自动回退 LIKE 全表扫描
//! （个人库量级可接受，且带 ESCAPE 防通配注入）。

use sqlx::SqlitePool;

/// 统一检索命中（keyword / vector / 未来 hybrid 共用，前端一个渲染路径）
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub chunk_id: i64,
    pub document_id: String,
    pub title: String,
    pub heading: String,
    pub seq: i64,
    pub content: String,
    /// 相关度：keyword = bm25 分；vector = 余弦相似度（0..1）
    pub score: f64,
}

/// 溯源原始行（chunks_by_ids 用）
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct ChunkRow {
    pub id: i64,
    pub document_id: String,
    pub title: String,
    pub heading: String,
    pub seq: i64,
    pub content: String,
}

/// 关键词检索（库内）：≥3 字符走 FTS5 trigram + BM25；不足回退 LIKE
pub async fn search_keyword(
    pool: &SqlitePool,
    kb_id: &str,
    query: &str,
    top_k: usize,
) -> Result<Vec<SearchHit>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let top_k = top_k.clamp(1, 100) as i64;
    if q.chars().count() < 3 {
        // trigram 边界回退：子串 LIKE（转义 % _ \）
        let esc = like_escape(q);
        let pattern = format!("%{esc}%");
        sqlx::query_as::<_, SearchHit>(
            "SELECT c.id AS chunk_id, d.id AS document_id, d.title, c.heading, c.seq, \
             c.content, 0.0 AS score \
             FROM kb_chunks c JOIN kb_documents d ON d.id = c.document_id \
             WHERE d.kb_id = ?1 AND c.content LIKE ?2 ESCAPE '\\' \
             ORDER BY length(c.content) LIMIT ?3",
        )
        .bind(kb_id)
        .bind(pattern)
        .bind(top_k)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("关键词检索失败: {e}"))
    } else {
        sqlx::query_as::<_, SearchHit>(
            "SELECT c.id AS chunk_id, d.id AS document_id, d.title, c.heading, c.seq, \
             c.content, bm25(kb_chunks_fts) AS score \
             FROM kb_chunks_fts \
             JOIN kb_chunks c ON c.rowid = kb_chunks_fts.rowid \
             JOIN kb_documents d ON d.id = c.document_id \
             WHERE kb_chunks_fts MATCH ?1 AND d.kb_id = ?2 \
             ORDER BY score LIMIT ?3",
        )
        .bind(fts_phrase(q))
        .bind(kb_id)
        .bind(top_k)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("关键词检索失败: {e}"))
    }
}

/// 按 chunk id 批量取溯源行（向量检索命中后组装；顺序不保证，由调用方重排）
pub(crate) async fn chunks_by_ids(
    pool: &SqlitePool,
    ids: &[i64],
) -> Result<Vec<ChunkRow>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders: Vec<String> = (0..ids.len()).map(|i| format!("?{}", i + 1)).collect();
    let sql = format!(
        "SELECT c.id, c.document_id, d.title, c.heading, c.seq, c.content \
         FROM kb_chunks c JOIN kb_documents d ON d.id = c.document_id \
         WHERE c.id IN ({})",
        placeholders.join(",")
    );
    let mut q = sqlx::query_as::<_, ChunkRow>(&sql);
    for id in ids {
        q = q.bind(id);
    }
    q.fetch_all(pool).await.map_err(|e| e.to_string())
}

/// LIKE 通配符转义（% _ \）
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// FTS5 短语包裹：内部双引号翻倍（fts5 转义规则）
fn fts_phrase(query: &str) -> String {
    format!("\"{}\"", query.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::kb::{data::import_text, schema};
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

    #[tokio::test]
    async fn keyword_fts_hits_with_bm25_order() {
        let pool = test_pool().await;
        let b = crate::services::kb::data::create_base(&pool, "库", "", 800, 150)
            .await
            .unwrap();
        import_text(&pool, &b.id, "doc1", "txt", "机器学习基础概念介绍。")
            .await
            .unwrap();
        import_text(&pool, &b.id, "doc2", "txt", "深度学习与机器学习的关系讨论。")
            .await
            .unwrap();
        import_text(&pool, &b.id, "doc3", "txt", "完全无关的内容，讲园艺。")
            .await
            .unwrap();

        let hits = search_keyword(&pool, &b.id, "机器学习", 10).await.unwrap();
        assert_eq!(hits.len(), 2, "应命中两个含关键词的文档");
        for h in &hits {
            assert!(h.content.contains("机器学习"));
            assert!(h.score < 0.0, "bm25 分应为负值: {}", h.score);
        }
        // 溯源：文档标题正确
        let titles: Vec<&str> = hits.iter().map(|h| h.title.as_str()).collect();
        assert!(titles.contains(&"doc1") && titles.contains(&"doc2"));
        assert!(!titles.contains(&"doc3"));
    }

    #[tokio::test]
    async fn short_query_falls_back_to_like() {
        let pool = test_pool().await;
        let b = crate::services::kb::data::create_base(&pool, "库", "", 800, 150)
            .await
            .unwrap();
        import_text(&pool, &b.id, "d", "txt", "含有 学习 二字的短词检索验证。")
            .await
            .unwrap();
        let hits = search_keyword(&pool, &b.id, "学习", 10).await.unwrap();
        assert_eq!(hits.len(), 1, "<3 字符查询应回退 LIKE 命中");
        // 通配符不注入
        let hits = search_keyword(&pool, &b.id, "%_", 10).await.unwrap();
        assert!(hits.is_empty(), "通配符应被转义");
    }

    #[tokio::test]
    async fn chunks_by_ids_returns_trace_rows() {
        let pool = test_pool().await;
        let b = crate::services::kb::data::create_base(&pool, "库", "", 800, 150)
            .await
            .unwrap();
        let doc = import_text(
            &pool,
            &b.id,
            "md文档",
            "md",
            "# 第一章\n\n向量检索溯源测试内容。",
        )
        .await
        .unwrap();
        let all = crate::services::kb::data::list_chunks(&pool, &doc.id).await.unwrap();
        let ids: Vec<i64> = all.iter().map(|c| c.id).collect();
        let rows = chunks_by_ids(&pool, &ids).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "md文档");
        assert_eq!(rows[0].heading, "第一章");
        assert!(rows[0].content.contains("向量检索溯源"));
        assert!(chunks_by_ids(&pool, &[]).await.unwrap().is_empty());
    }
}
