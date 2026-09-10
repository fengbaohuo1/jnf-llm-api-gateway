//! 检索：关键词（bigram 索引 + trigram 英文子串补召回 + LIKE 兜底）与
//! 向量命中的溯源组装、RRF 融合、重排与相关度过滤。
//!
//! 关键词路的两张索引表（见 `tokenize.rs` 模块注释）：
//! - `kb_chunks_bg`（unicode61 + bigram 预处理）：中文主力，任意 ≥2 字符查询；
//! - `kb_chunks_fts`（trigram）：仅非 CJK ≥3 字符词元，补英文部分匹配。
//!
//! 两条路按**词元类型互斥**（CJK 只进 bigram，英文只进 trigram），
//! 命中后 RRF 融合；两者都无词元（如纯 1 字符查询）才回退 LIKE 全表扫描
//! （个人库量级可接受，且带 ESCAPE 防通配注入）。

use std::collections::HashMap;

use sqlx::SqlitePool;

use super::data;
use super::tokenize;

/// bm25 列权重：content / heading / title（顺序与 FTS 表列定义一致）
const BM25_WEIGHTS: &str = "1.0, 2.0, 3.0";

/// 统一检索命中（keyword / vector / hybrid 共用，前端一个渲染路径）
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub chunk_id: i64,
    pub document_id: String,
    pub title: String,
    pub heading: String,
    pub seq: i64,
    pub content: String,
    /// 当前模式的排序分：keyword = bm25 分（负值，越小越相关；LIKE 回退为
    /// `-(命中词元数)`）；vector = 余弦相似度（0..1）；hybrid = RRF 融合分
    pub score: f64,
    /// 向量路原始余弦相似度（仅 vector / hybrid 且该块被向量路命中时有值）
    #[sqlx(default)]
    pub vec_score: Option<f64>,
    /// 关键词路原始分（bm25 或 `-(命中词元数)`；两路皆中时取更负者）
    #[sqlx(default)]
    pub kw_score: Option<f64>,
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

/// 关键词检索（库内）：bigram 路 + trigram 路（英文子串）→ RRF 融合；
/// 两路都无词元时回退 LIKE。`score` 为关键词路排序分（负值越相关）。
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
    let terms = tokenize::query_terms(q);
    if terms.is_empty() {
        return Ok(Vec::new());
    }

    let mut lists: Vec<Vec<SearchHit>> = Vec::new();

    // bigram 路（中文主力）：索引与分块不一致时跳过（安全降级，不返回过期结果）
    let bg_terms = tokenize::bigram_query_terms(&terms);
    if let Some(m) = tokenize::build_match_query(&bg_terms) {
        if data::bigram_index_ready(pool).await {
            lists.push(search_fts(pool, kb_id, "kb_chunks_bg", &m, top_k).await?);
        } else {
            eprintln!("[kb] bigram 关键词索引与分块不一致，跳过该路（可重建关键词索引）");
        }
    }

    // trigram 路（英文子串补召回）
    let tri_terms = tokenize::trigram_query_terms(&terms);
    if let Some(m) = tokenize::build_match_query(&tri_terms) {
        lists.push(search_fts(pool, kb_id, "kb_chunks_fts", &m, top_k).await?);
    }

    match lists.len() {
        0 => search_like(pool, kb_id, &terms, top_k).await,
        1 => Ok(lists.pop().expect("单路结果")),
        _ => {
            let trigram = lists.pop().expect("trigram 路");
            let bigram = lists.pop().expect("bigram 路");
            Ok(rrf_fuse(bigram, trigram, top_k as usize, 1.0, 1.0))
        }
    }
}

/// 单张 FTS 表的关键词检索（表名由调用方给出，仅接受内部常量）
async fn search_fts(
    pool: &SqlitePool,
    kb_id: &str,
    table: &str,
    match_q: &str,
    top_k: i64,
) -> Result<Vec<SearchHit>, String> {
    let sql = format!(
        "SELECT c.id AS chunk_id, d.id AS document_id, d.title, c.heading, c.seq, c.content, \
         bm25({table}, {BM25_WEIGHTS}) AS score \
         FROM {table} \
         JOIN kb_chunks c ON c.rowid = {table}.rowid \
         JOIN kb_documents d ON d.id = c.document_id \
         WHERE {table} MATCH ?1 AND d.kb_id = ?2 \
         ORDER BY score LIMIT ?3"
    );
    let mut hits = sqlx::query_as::<_, SearchHit>(&sql)
        .bind(match_q)
        .bind(kb_id)
        .bind(top_k)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("关键词检索失败（{table}）: {e}"))?;
    for h in hits.iter_mut() {
        h.kw_score = Some(h.score);
    }
    Ok(hits)
}

/// LIKE 兜底：各词元命中数作为排序分（负值，与 bm25 语义一致）。
/// 仅在查询无任何 FTS 词元时使用（例如单字符查询）。
async fn search_like(
    pool: &SqlitePool,
    kb_id: &str,
    terms: &[String],
    top_k: i64,
) -> Result<Vec<SearchHit>, String> {
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let n = terms.len();
    let cases: Vec<String> = (1..=n)
        .map(|i| {
            format!(
                "(CASE WHEN c.content LIKE ?{i} ESCAPE '\\' OR c.heading LIKE ?{i} ESCAPE '\\' \
                 OR d.title LIKE ?{i} ESCAPE '\\' THEN 1 ELSE 0 END)"
            )
        })
        .collect();
    let sum = cases.join(" + ");
    let sql = format!(
        "SELECT c.id AS chunk_id, d.id AS document_id, d.title, c.heading, c.seq, c.content, \
         CAST(-({sum}) AS REAL) AS score \
         FROM kb_chunks c JOIN kb_documents d ON d.id = c.document_id \
         WHERE d.kb_id = ?{} AND ({sum}) \
         ORDER BY score, length(c.content) LIMIT ?{}",
        n + 1,
        n + 2
    );
    let mut q = sqlx::query_as::<_, SearchHit>(&sql);
    for t in terms {
        q = q.bind(format!("%{}%", like_escape(t)));
    }
    let mut hits = q
        .bind(kb_id)
        .bind(top_k)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("关键词检索失败: {e}"))?;
    for h in hits.iter_mut() {
        h.kw_score = Some(h.score);
    }
    Ok(hits)
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

/// RRF（Reciprocal Rank Fusion）融合两路命中，按 chunk 去重对齐。
///
/// 融合分 = `Σ w_leg / (60 + rank)`，rank 从 1 起——出现在两路的块天然高于只
/// 出现在一路的块，且无需跨路归一化 bm25（负值）与余弦相似度（0..1）的量纲。
/// 命中详情优先取 `primary`；两路原始分写入 `vec_score` / `kw_score`
/// （向量取较大余弦，关键词取更负的 bm25）。返回按融合分降序截断到 `top_k`。
pub fn rrf_fuse(
    mut primary: Vec<SearchHit>,
    mut secondary: Vec<SearchHit>,
    top_k: usize,
    w_primary: f64,
    w_secondary: f64,
) -> Vec<SearchHit> {
    let k = 60.0;
    // chunk_id → (融合分, 命中详情)
    let mut fused: HashMap<i64, (f64, SearchHit)> = HashMap::new();
    for (rank, hit) in primary.drain(..).enumerate() {
        let add = w_primary / (k + rank as f64 + 1.0);
        fused.insert(hit.chunk_id, (add, hit));
    }
    for (rank, hit) in secondary.drain(..).enumerate() {
        let add = w_secondary / (k + rank as f64 + 1.0);
        match fused.get_mut(&hit.chunk_id) {
            Some(entry) => {
                entry.0 += add;
                merge_raw_scores(&mut entry.1, &hit);
            }
            None => {
                fused.insert(hit.chunk_id, (add, hit));
            }
        }
    }
    let mut out: Vec<(f64, SearchHit)> = fused.into_values().collect();
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
    out.truncate(top_k);
    out.into_iter()
        .map(|(score, mut hit)| {
            hit.score = score;
            hit
        })
        .collect()
}

/// 合并同一块的原始分：向量取更大余弦（越接近 1 越好），关键词取更负 bm25
fn merge_raw_scores(dst: &mut SearchHit, src: &SearchHit) {
    dst.vec_score = match (dst.vec_score, src.vec_score) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    dst.kw_score = match (dst.kw_score, src.kw_score) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
}

/// 融合后的确定性重排（零模型成本）：按输入名次构造基准分再做乘法加分，
/// **不改写 `score`**（关键词分为负值，直接对分数做乘法会翻转排序方向）。
///
/// 加分项：
/// - 两路皆中 ×1.15（字面与语义证据同时成立）
/// - 词元命中标题 ×1.10、命中 heading ×1.05（结构化溯源是强信号）
/// - 超长块轻微惩罚 ×(1 − 0.1 × 超长比)，避免单块吞掉上下文预算
///
/// 去重：同一文档最多保留 `per_doc_cap` 块（相邻块重叠会挤占引用位）。
pub(crate) fn rerank(
    hits: Vec<SearchHit>,
    terms: &[String],
    per_doc_cap: usize,
) -> Vec<SearchHit> {
    let terms_lc: Vec<String> = terms.iter().map(|t| t.to_lowercase()).collect();
    let mut keyed: Vec<(f64, usize, SearchHit)> = hits
        .into_iter()
        .enumerate()
        .map(|(rank, h)| {
            // 输入已按当前模式的相关度排序 → 名次越靠前基准分越高
            let base = 1.0 / (60.0 + rank as f64 + 1.0);
            let mut factor = 1.0f64;
            if h.vec_score.is_some() && h.kw_score.is_some() {
                factor *= 1.15;
            }
            let title_lc = h.title.to_lowercase();
            let heading_lc = h.heading.to_lowercase();
            if terms_lc.iter().any(|t| title_lc.contains(t)) {
                factor *= 1.10;
            }
            if terms_lc.iter().any(|t| heading_lc.contains(t)) {
                factor *= 1.05;
            }
            let len = h.content.chars().count();
            if len > 1200 {
                let over = ((len - 1200) as f64 / 1200.0).min(1.0);
                factor *= 1.0 - 0.1 * over;
            }
            (base * factor, rank, h)
        })
        .collect();
    keyed.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));

    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::with_capacity(keyed.len());
    for (_, _, h) in keyed {
        let count = seen.entry(h.document_id.clone()).or_insert(0);
        if *count >= per_doc_cap.max(1) {
            continue;
        }
        *count += 1;
        out.push(h);
    }
    out
}

/// 相关度阈值过滤：**纯语义命中**（无关键词分）低于 `min_cos` 丢弃。
///
/// 有关键词分的命中一律保留——字面命中是真实信号（精确术语、错误码、API 名），
/// 其余弦偏低往往只是嵌入模型对短查询/专名的语义表征弱，不该因此丢证据。
pub(crate) fn filter_by_similarity(hits: Vec<SearchHit>, min_cos: f64) -> Vec<SearchHit> {
    hits.into_iter()
        .filter(|h| h.kw_score.is_some() || h.vec_score.map_or(true, |v| v >= min_cos))
        .collect()
}

/// LIKE 通配符转义（% _ \）
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
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

    async fn seed(pool: &SqlitePool) -> String {
        let b = crate::services::kb::data::create_base(pool, "库", "", 800, 150)
            .await
            .unwrap();
        import_text(pool, &b.id, "doc1", "txt", "机器学习基础概念介绍。")
            .await
            .unwrap();
        import_text(pool, &b.id, "doc2", "txt", "深度学习与机器学习的关系讨论。")
            .await
            .unwrap();
        import_text(pool, &b.id, "doc3", "txt", "完全无关的内容，讲园艺。")
            .await
            .unwrap();
        b.id
    }

    #[tokio::test]
    async fn keyword_fts_hits_with_bm25_order() {
        let pool = test_pool().await;
        let kb = seed(&pool).await;
        let hits = search_keyword(&pool, &kb, "机器学习", 10).await.unwrap();
        assert_eq!(hits.len(), 2, "应命中两个含关键词的文档");
        for h in &hits {
            assert!(h.content.contains("机器学习"));
            assert!(h.score < 0.0, "bm25 分应为负值: {}", h.score);
            assert!(h.kw_score.is_some(), "关键词路原始分应保留");
        }
        let titles: Vec<&str> = hits.iter().map(|h| h.title.as_str()).collect();
        assert!(titles.contains(&"doc1") && titles.contains(&"doc2"));
        assert!(!titles.contains(&"doc3"));
    }

    /// 回归：旧实现把整句当 FTS 短语，中文自然语言问题必然 0 命中
    #[tokio::test]
    async fn keyword_hits_full_sentence_question() {
        let pool = test_pool().await;
        let kb = seed(&pool).await;
        let hits = search_keyword(&pool, &kb, "机器学习是怎么定义的？", 10)
            .await
            .unwrap();
        assert!(!hits.is_empty(), "整句问题应能命中（bigram 滑窗 + OR）");
        assert!(hits.iter().any(|h| h.title == "doc1"));
        // 全部是停用词 → 无词元 → 空结果
        assert!(search_keyword(&pool, &kb, "什么 是 的", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn keyword_matches_two_char_term_and_title() {
        let pool = test_pool().await;
        let b = crate::services::kb::data::create_base(&pool, "库", "", 800, 150)
            .await
            .unwrap();
        // 2 字词只出现在标题里 → 标题列（bigram 索引）可命中
        import_text(&pool, &b.id, "限流策略", "txt", "网关转发层的实现细节。")
            .await
            .unwrap();
        import_text(&pool, &b.id, "其他", "txt", "完全无关的园艺内容。")
            .await
            .unwrap();
        let hits = search_keyword(&pool, &b.id, "限流", 10).await.unwrap();
        assert_eq!(hits.len(), 1, "2 字词应走 bigram 索引命中: {hits:?}");
        assert_eq!(hits[0].title, "限流策略");
    }

    #[tokio::test]
    async fn like_fallback_ranks_by_matched_term_count() {
        let pool = test_pool().await;
        let b = crate::services::kb::data::create_base(&pool, "库", "", 800, 150)
            .await
            .unwrap();
        import_text(&pool, &b.id, "两词", "txt", "x 与 y 都出现，还有 x 和 y。")
            .await
            .unwrap();
        import_text(&pool, &b.id, "一词", "txt", "只有 x 出现。")
            .await
            .unwrap();
        // 单字符英文词元进不了任何 FTS 表（bigram 需 2 字符、trigram 需 3 字符）
        // → LIKE 兜底，按命中词元数排序
        let hits = search_keyword(&pool, &b.id, "x y", 10).await.unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "两词", "命中两个词元的块应排前");
        assert!(hits[0].score < hits[1].score, "score 为负命中数: {hits:?}");
        // 无词元（纯标点）→ 空结果
        assert!(search_keyword(&pool, &b.id, "%", 10).await.unwrap().is_empty());
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

    // ---- RRF 融合 / 重排 / 阈值（纯函数） ----

    fn hit(id: i64, title: &str, content: &str, score: f64) -> SearchHit {
        SearchHit {
            chunk_id: id,
            document_id: format!("doc_{id}"),
            title: title.to_string(),
            heading: String::new(),
            seq: id,
            content: content.to_string(),
            score,
            vec_score: None,
            kw_score: None,
        }
    }

    fn vhit(id: i64, title: &str, content: &str, cos: f64) -> SearchHit {
        let mut h = hit(id, title, content, cos);
        h.vec_score = Some(cos);
        h
    }

    fn khit(id: i64, title: &str, content: &str, bm25: f64) -> SearchHit {
        let mut h = hit(id, title, content, bm25);
        h.kw_score = Some(bm25);
        h
    }

    fn in_doc(mut h: SearchHit, doc: &str) -> SearchHit {
        h.document_id = doc.to_string();
        h
    }

    #[test]
    fn rrf_boosts_hits_present_in_both_lists() {
        // chunk 1 两路都第 1（2/61）；chunk 2 只在一路第 2（1/62）
        let primary = vec![vhit(1, "双路", "内容1", 0.9), vhit(2, "单路", "内容2", 0.8)];
        let secondary = vec![khit(1, "双路", "内容1", -5.0)];
        let fused = rrf_fuse(primary, secondary, 10, 1.0, 1.0);
        assert_eq!(fused.len(), 2);
        assert_eq!(fused[0].chunk_id, 1, "两路皆中应排第一");
        assert!(
            (fused[0].score - 2.0 / 61.0).abs() < 1e-9,
            "双路第 1 = 2/61: {}",
            fused[0].score
        );
        assert!(
            (fused[1].score - 1.0 / 62.0).abs() < 1e-9,
            "单路第 2 = 1/62: {}",
            fused[1].score
        );
        assert!(
            fused[0].score > fused[1].score,
            "RRF 不应被量纲差异带偏（bm25 为负值也如此）"
        );
        // 原始分保留：双路块同时有向量与关键词分
        assert_eq!(fused[0].vec_score, Some(0.9));
        assert_eq!(fused[0].kw_score, Some(-5.0));
        assert_eq!(fused[1].vec_score, Some(0.8));
        assert_eq!(fused[1].kw_score, None, "单路块另一路分应为空");
    }

    #[test]
    fn rrf_weights_and_primary_details_and_truncation() {
        let primary = vec![vhit(1, "P标题", "P内容", 0.9)];
        let secondary = vec![khit(1, "S标题", "S内容", -2.0), khit(2, "S2", "S2内容", -1.0)];
        let fused = rrf_fuse(primary, secondary, 1, 1.0, 0.5);
        assert_eq!(fused.len(), 1, "应截断到 top_k");
        assert_eq!(fused[0].chunk_id, 1);
        assert_eq!(fused[0].title, "P标题", "详情应优先取 primary");
        assert!(
            (fused[0].score - (1.0 / 61.0 + 0.5 / 61.0)).abs() < 1e-9,
            "加权融合分: {}",
            fused[0].score
        );
        assert!(rrf_fuse(Vec::new(), Vec::new(), 5, 1.0, 1.0).is_empty());
    }

    #[test]
    fn rerank_boosts_title_match_and_caps_per_document() {
        let terms = vec!["故障转移".to_string()];
        let hits = vec![
            in_doc(khit(1, "无关标题", "内容甲", -1.0), "d1"),
            in_doc(khit(2, "故障转移", "内容乙", -1.0), "d2"),
            in_doc(khit(3, "无关标题", "内容丙", -1.0), "d1"),
            in_doc(khit(4, "无关标题", "内容丁", -1.0), "d2"),
        ];
        // 每篇文档只留 1 块 → d1/d2 各剩一条；标题命中的排第一
        let out = rerank(hits, &terms, 1);
        assert_eq!(out.len(), 2, "同一文档超上限的块应被丢弃");
        assert_eq!(out[0].chunk_id, 2, "标题命中应加权到第一");
        assert_eq!(out[1].chunk_id, 1, "名次靠前者优先保留");
        // 关键词分为负值也不影响排序方向（名次基准分恒正）
        assert!(out[0].score < 0.0 && out[1].score < 0.0);
    }

    #[test]
    fn rerank_boosts_both_legs_hits() {
        let mut both = vhit(1, "A", "内容", 1.0);
        both.kw_score = Some(-1.0);
        let only_vec = vhit(2, "B", "内容", 1.0);
        let out = rerank(vec![only_vec, both], &[], 5);
        assert_eq!(out[0].chunk_id, 1, "两路皆中应加分靠前");
    }

    #[test]
    fn filter_by_similarity_drops_weak_vectors_but_keeps_keyword_evidence() {
        let mut both_legs_low_vec = vhit(4, "弱向量强字面", "内容", 0.12);
        both_legs_low_vec.kw_score = Some(-1.5);
        let hits = vec![
            vhit(1, "强", "内容", 0.82),
            vhit(2, "弱", "内容", 0.20),
            khit(3, "仅关键词", "内容", -3.0),
            both_legs_low_vec,
        ];
        let out = filter_by_similarity(hits, 0.35);
        let ids: Vec<i64> = out.iter().map(|h| h.chunk_id).collect();
        assert_eq!(
            ids,
            vec![1, 3, 4],
            "低相似度纯语义命中应丢弃；字面命中（含两路皆中）保留"
        );
    }
}
