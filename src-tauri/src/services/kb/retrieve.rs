//! 问答检索策略层：配置读取 → 检索 → 重排 → 相关度阈值 → 邻居扩展。
//!
//! 边界：`search_any` 是"按分排序的召回原语"（手动检索 / MCP 看原始排序），
//! 本模块是**问答专用策略**——阈值、每文档去重、邻居上下文只在这里生效，
//! 避免手动检索被过滤掉调试所需的结果。
//!
//! 参数全部可通过 `settings` 表覆盖（键前缀 `kb.retrieval.`），而 settings.json
//! 是权威来源且支持热重载，因此改参数无需改代码、也无需重启。

use std::collections::HashMap;

use sqlx::SqlitePool;

use super::search::{self, SearchHit};
use super::tokenize;
use super::KnowledgeBaseService;

/// 最终进上下文的命中数
pub(crate) const DEFAULT_TOP_K: usize = 4;
/// 向量余弦相似度下限（低于此值的纯语义命中丢弃）。
///
/// **按嵌入模型校准**：默认值用 `BAAI/bge-m3` + `kb/evals/sample.json` 实测得出——
/// 可答问题 top1 相似度 0.555–0.749，无答案问题 0.342–0.418，取 0.45 居中留余量。
/// 换嵌入模型（尤其 OpenAI text-embedding 系列）尺度不同，需用评测集重新校准，
/// 可临时改 `kb.retrieval.minSimilarity`。
pub(crate) const DEFAULT_MIN_SIMILARITY: f64 = 0.45;
/// RRF 权重：向量路（语义通常更可靠）
pub(crate) const DEFAULT_RRF_W_VECTOR: f64 = 1.0;
/// RRF 权重：关键词路
pub(crate) const DEFAULT_RRF_W_KEYWORD: f64 = 0.7;
/// 同一文档最多保留的块数
pub(crate) const DEFAULT_PER_DOC_CAP: usize = 2;
/// 是否把命中块的相邻块并入引用上下文
pub(crate) const DEFAULT_NEIGHBOR_EXPAND: bool = true;

/// 候选放大倍数（先多取，重排/过滤后再截断到 top_k，避免"过滤后不够数"）
const FETCH_MULTIPLIER: usize = 3;
/// 候选数上限
const FETCH_MAX: usize = 50;

/// 问答检索参数
#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalConfig {
    pub top_k: usize,
    pub min_similarity: f64,
    pub rrf_weight_vector: f64,
    pub rrf_weight_keyword: f64,
    pub per_doc_cap: usize,
    pub neighbor_expand: bool,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            top_k: DEFAULT_TOP_K,
            min_similarity: DEFAULT_MIN_SIMILARITY,
            rrf_weight_vector: DEFAULT_RRF_W_VECTOR,
            rrf_weight_keyword: DEFAULT_RRF_W_KEYWORD,
            per_doc_cap: DEFAULT_PER_DOC_CAP,
            neighbor_expand: DEFAULT_NEIGHBOR_EXPAND,
        }
    }
}

impl RetrievalConfig {
    /// 从 `settings` 表读取（缺失用默认值；非法值告警并回落默认值）
    pub async fn load(pool: &SqlitePool) -> Self {
        let map: HashMap<String, String> = sqlx::query_as::<_, (String, String)>(
            "SELECT key, value FROM settings WHERE key LIKE 'kb.retrieval.%'",
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
        let d = Self::default();
        Self {
            top_k: read_usize(&map, "kb.retrieval.topK", d.top_k, 1, 20),
            min_similarity: read_f64(&map, "kb.retrieval.minSimilarity", d.min_similarity, 0.0, 1.0),
            rrf_weight_vector: read_f64(
                &map,
                "kb.retrieval.rrfWeightVector",
                d.rrf_weight_vector,
                0.0,
                10.0,
            ),
            rrf_weight_keyword: read_f64(
                &map,
                "kb.retrieval.rrfWeightKeyword",
                d.rrf_weight_keyword,
                0.0,
                10.0,
            ),
            per_doc_cap: read_usize(&map, "kb.retrieval.perDocCap", d.per_doc_cap, 1, 10),
            neighbor_expand: read_bool(&map, "kb.retrieval.neighborExpand", d.neighbor_expand),
        }
    }
}

fn read_usize(map: &HashMap<String, String>, key: &str, default: usize, min: usize, max: usize) -> usize {
    match map.get(key) {
        None => default,
        Some(raw) => match raw.trim().parse::<usize>() {
            Ok(v) => v.clamp(min, max),
            Err(_) => {
                eprintln!("[kb] 设置 {key} 非法（{raw}），使用默认值 {default}");
                default
            }
        },
    }
}

fn read_f64(map: &HashMap<String, String>, key: &str, default: f64, min: f64, max: f64) -> f64 {
    match map.get(key) {
        None => default,
        Some(raw) => match raw.trim().parse::<f64>() {
            Ok(v) if v.is_finite() => v.clamp(min, max),
            _ => {
                eprintln!("[kb] 设置 {key} 非法（{raw}），使用默认值 {default}");
                default
            }
        },
    }
}

fn read_bool(map: &HashMap<String, String>, key: &str, default: bool) -> bool {
    match map.get(key) {
        None => default,
        Some(raw) => match raw.trim() {
            "true" | "1" => true,
            "false" | "0" => false,
            other => {
                eprintln!("[kb] 设置 {key} 非法（{other}），使用默认值 {default}");
                default
            }
        },
    }
}

/// 检索 + 策略：多取候选 → 重排（标题/双路加分、每文档上限）→ 阈值过滤
/// → 截断到 top_k → 邻居扩展。
pub(crate) async fn retrieve_hits(
    svc: &KnowledgeBaseService,
    pool: &SqlitePool,
    kb_id: &str,
    question: &str,
    mode: &str,
    cfg: &RetrievalConfig,
) -> Result<Vec<SearchHit>, String> {
    let fetch_k = (cfg.top_k * FETCH_MULTIPLIER).min(FETCH_MAX);
    let hits = svc
        .search_any_with(
            pool,
            kb_id,
            question,
            mode,
            fetch_k,
            (cfg.rrf_weight_vector, cfg.rrf_weight_keyword),
        )
        .await?;
    let terms = tokenize::query_terms(question);
    let mut hits = search::rerank(hits, &terms, cfg.per_doc_cap);
    hits = search::filter_by_similarity(hits, cfg.min_similarity);
    hits.truncate(cfg.top_k);
    if cfg.neighbor_expand {
        expand_neighbors(pool, hits).await
    } else {
        Ok(hits)
    }
}

/// 问答用检索：hybrid 优先，检索不可用（未绑 embedding / 索引未就绪 / 索引过期）
/// 时**静默降级关键词**——问答要容错，手动检索命令层保持严格报错。
pub(crate) async fn retrieve_for_chat(
    svc: &KnowledgeBaseService,
    pool: &SqlitePool,
    kb_id: &str,
    question: &str,
    cfg: &RetrievalConfig,
) -> Result<Vec<SearchHit>, String> {
    match retrieve_hits(svc, pool, kb_id, question, "hybrid", cfg).await {
        Ok(hits) => Ok(hits),
        Err(e) => {
            eprintln!("[kb] 问答 hybrid 检索不可用，降级关键词检索: {e}");
            retrieve_hits(svc, pool, kb_id, question, "keyword", cfg).await
        }
    }
}

/// 把命中块的相邻块（同文档 seq±1）并入引用上下文。
///
/// 拼接顺序：**命中块 → 下一块 → 上一块**——命中块内容一定在最前（截断也
/// 不会丢），向后衔接通常比向前重复更有用。已在命中列表里的块跳过（避免重复
/// 内容挤占预算）。citation 仍指向原命中块，`[n]` 语义不变。
async fn expand_neighbors(
    pool: &SqlitePool,
    hits: Vec<SearchHit>,
) -> Result<Vec<SearchHit>, String> {
    if hits.is_empty() {
        return Ok(hits);
    }
    let selected: Vec<i64> = hits.iter().map(|h| h.chunk_id).collect();
    let mut out = Vec::with_capacity(hits.len());
    for mut h in hits {
        let rows: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT id, seq, content FROM kb_chunks WHERE document_id = ?1 AND seq IN (?2, ?3)",
        )
        .bind(&h.document_id)
        .bind(h.seq - 1)
        .bind(h.seq + 1)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("读取相邻分块失败: {e}"))?;
        let mut next = None;
        let mut prev = None;
        for (id, seq, content) in rows {
            if selected.contains(&id) || content.trim().is_empty() {
                continue;
            }
            if seq == h.seq + 1 {
                next = Some(content);
            } else if seq == h.seq - 1 {
                prev = Some(content);
            }
        }
        for extra in [next, prev].into_iter().flatten() {
            h.content.push('\n');
            h.content.push_str(&extra);
        }
        out.push(h);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::kb::{data, schema};
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
        sqlx::query("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    async fn set(pool: &SqlitePool, key: &str, value: &str) {
        sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)")
            .bind(key)
            .bind(value)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn config_uses_defaults_then_settings_overrides() {
        let pool = test_pool().await;
        let cfg = RetrievalConfig::load(&pool).await;
        assert_eq!(cfg, RetrievalConfig::default(), "无设置时用默认值");

        set(&pool, "kb.retrieval.topK", "8").await;
        set(&pool, "kb.retrieval.minSimilarity", "0.42").await;
        set(&pool, "kb.retrieval.rrfWeightVector", "1.5").await;
        set(&pool, "kb.retrieval.perDocCap", "1").await;
        set(&pool, "kb.retrieval.neighborExpand", "false").await;
        let cfg = RetrievalConfig::load(&pool).await;
        assert_eq!(cfg.top_k, 8);
        assert!((cfg.min_similarity - 0.42).abs() < 1e-9);
        assert!((cfg.rrf_weight_vector - 1.5).abs() < 1e-9);
        assert_eq!(cfg.rrf_weight_keyword, DEFAULT_RRF_W_KEYWORD, "未设置的键保持默认");
        assert_eq!(cfg.per_doc_cap, 1);
        assert!(!cfg.neighbor_expand);

        // 非法值回落默认 + 越界钳制
        set(&pool, "kb.retrieval.topK", "abc").await;
        set(&pool, "kb.retrieval.minSimilarity", "9").await;
        set(&pool, "kb.retrieval.neighborExpand", "maybe").await;
        let cfg = RetrievalConfig::load(&pool).await;
        assert_eq!(cfg.top_k, DEFAULT_TOP_K, "非法值应回落默认");
        assert!((cfg.min_similarity - 1.0).abs() < 1e-9, "越界应钳制到上界");
        assert!(cfg.neighbor_expand);
    }

    #[tokio::test]
    async fn neighbor_expansion_orders_hit_then_next_then_prev() {
        let pool = test_pool().await;
        let b = data::create_base(&pool, "邻块库", "", 100, 0).await.unwrap();
        let body = "甲".repeat(150) + "\n\n" + &"乙".repeat(150) + "\n\n" + &"丙".repeat(150);
        let doc = data::import_text(&pool, &b.id, "长文", "txt", &body).await.unwrap();
        let chunks = data::list_chunks(&pool, &doc.id).await.unwrap();
        assert!(chunks.len() >= 3, "应至少 3 块，实际 {}", chunks.len());

        let mid = chunks[1].clone();
        let hit = SearchHit {
            chunk_id: mid.id,
            document_id: doc.id.clone(),
            title: doc.title.clone(),
            heading: mid.heading.clone(),
            seq: mid.seq,
            content: mid.content.clone(),
            score: 1.0,
            vec_score: None,
            kw_score: None,
        };
        let out = expand_neighbors(&pool, vec![hit]).await.unwrap();
        assert_eq!(out.len(), 1);
        let merged = &out[0].content;
        assert!(merged.starts_with(&mid.content), "命中块内容必须保留在最前");
        assert!(
            merged.contains(&chunks[2].content),
            "应并入下一块内容"
        );
        assert!(
            merged.contains(&chunks[0].content),
            "应并入上一块内容"
        );
        let next_pos = merged.find(&chunks[2].content).unwrap();
        let prev_pos = merged.find(&chunks[0].content).unwrap();
        assert!(next_pos < prev_pos, "下一块应排在上一块之前");

        // 空输入直接返回
        assert!(expand_neighbors(&pool, Vec::new()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn neighbor_expansion_skips_chunks_already_selected() {
        let pool = test_pool().await;
        let b = data::create_base(&pool, "邻块库", "", 100, 0).await.unwrap();
        let body = "甲".repeat(150) + "\n\n" + &"乙".repeat(150) + "\n\n" + &"丙".repeat(150);
        let doc = data::import_text(&pool, &b.id, "长文", "txt", &body).await.unwrap();
        let chunks = data::list_chunks(&pool, &doc.id).await.unwrap();
        let mk = |c: &crate::services::kb::data::KbChunk| SearchHit {
            chunk_id: c.id,
            document_id: doc.id.clone(),
            title: doc.title.clone(),
            heading: c.heading.clone(),
            seq: c.seq,
            content: c.content.clone(),
            score: 1.0,
            vec_score: None,
            kw_score: None,
        };
        // 相邻两块都在引用列表里 → 已入选的邻块不得被重复拼接
        let out = expand_neighbors(&pool, vec![mk(&chunks[0]), mk(&chunks[1])])
            .await
            .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].content, chunks[0].content,
            "唯一邻块已入选，块 0 不应被追加内容"
        );
    }
}
