//! 检索评测：给定「问题 → 期望命中文档」用例集，跑**问答检索路径**
//! （`retrieve.rs`：重排 + 阈值 + 邻居扩展）并输出 recall@k / MRR / 拒答正确率。
//!
//! 用途：检索策略任何改动前后跑同一份用例集，用数字判断是否真的变好；
//! `expectNoAnswer` 用例专门验证阈值与"找不到就明说"这条链路。
//!
//! 与问答保持一致：评测走 `retrieve_hits`（严格模式，不做 hybrid→关键词降级），
//! 避免"静默降级"把两种模式的数字混在一起。

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::retrieve::{self, RetrievalConfig};
use super::search::SearchHit;
use super::KnowledgeBaseService;

/// 单条评测用例（`kb/evals/*.json` 数组元素）
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalCase {
    /// 问题（原样送检索）
    pub question: String,
    /// 期望命中的文档标题（任一命中即算召回）
    #[serde(default)]
    pub expect_docs: Vec<String>,
    /// 期望检索为空（无答案问题：验证阈值过滤与拒答链路）
    #[serde(default)]
    pub expect_no_answer: bool,
}

/// 单条命中（人工核查用）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalHit {
    pub title: String,
    pub score: f64,
    pub vec_score: Option<f64>,
    pub kw_score: Option<f64>,
}

/// 单条用例结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalCaseResult {
    pub question: String,
    pub expect_docs: Vec<String>,
    pub expect_no_answer: bool,
    /// 首个期望文档的排名（1 起）；未命中为 null
    pub matched_rank: Option<usize>,
    pub passed: bool,
    pub hits: Vec<EvalHit>,
}

/// 汇总报告
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalReport {
    pub mode: String,
    pub top_k: usize,
    pub total: usize,
    /// 需要命中文档的用例数（排除 expectNoAnswer）
    pub answerable: usize,
    /// recall@k：期望文档出现在前 k 条的用例占比
    pub recall_at_k: f64,
    /// MRR：首个期望文档排名倒数的均值（未命中计 0）
    pub mrr: f64,
    /// 拒答正确率：expectNoAnswer 用例中检索为空的比例
    pub refuse_accuracy: f64,
    pub cases: Vec<EvalCaseResult>,
}

/// 读取评测用例文件（JSON 数组）
pub fn load_cases(path: &str) -> Result<Vec<EvalCase>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("读取评测文件失败（{path}）: {e}"))?;
    let cases: Vec<EvalCase> = serde_json::from_str(&raw)
        .map_err(|e| format!("解析评测文件失败（{path}）: {e}"))?;
    if cases.is_empty() {
        return Err("评测文件没有用例".to_string());
    }
    Ok(cases)
}

/// 跑一遍评测（严格模式：检索失败直接上抛，不静默降级）
pub async fn run_eval(
    svc: &KnowledgeBaseService,
    pool: &SqlitePool,
    kb_id: &str,
    cases: &[EvalCase],
    mode: &str,
    top_k: usize,
) -> Result<EvalReport, String> {
    let mut cfg = RetrievalConfig::load(pool).await;
    cfg.top_k = top_k.clamp(1, 20);

    let mut results = Vec::with_capacity(cases.len());
    let mut answerable = 0usize;
    let mut recalled = 0usize;
    let mut rr_sum = 0.0f64;
    let mut refuse_total = 0usize;
    let mut refuse_ok = 0usize;

    for case in cases {
        let hits = retrieve::retrieve_hits(svc, pool, kb_id, &case.question, mode, &cfg).await?;
        let matched_rank = case
            .expect_docs
            .iter()
            .filter_map(|want| {
                hits.iter()
                    .position(|h| title_matches(&h.title, want))
                    .map(|i| i + 1)
            })
            .min();
        let passed = if case.expect_no_answer {
            hits.is_empty()
        } else {
            matched_rank.is_some()
        };
        if case.expect_no_answer {
            refuse_total += 1;
            if passed {
                refuse_ok += 1;
            }
        } else if !case.expect_docs.is_empty() {
            answerable += 1;
            if let Some(rank) = matched_rank {
                recalled += 1;
                rr_sum += 1.0 / rank as f64;
            }
        }
        results.push(EvalCaseResult {
            question: case.question.clone(),
            expect_docs: case.expect_docs.clone(),
            expect_no_answer: case.expect_no_answer,
            matched_rank,
            passed,
            hits: hits.iter().map(summarize).collect(),
        });
    }

    Ok(EvalReport {
        mode: mode.to_string(),
        top_k: cfg.top_k,
        total: cases.len(),
        answerable,
        recall_at_k: ratio(recalled, answerable),
        mrr: if answerable == 0 { 0.0 } else { rr_sum / answerable as f64 },
        refuse_accuracy: ratio(refuse_ok, refuse_total),
        cases: results,
    })
}

fn summarize(h: &SearchHit) -> EvalHit {
    EvalHit {
        title: h.title.clone(),
        score: h.score,
        vec_score: h.vec_score,
        kw_score: h.kw_score,
    }
}

/// 期望文档匹配：相等或**包含**（大小写不敏感）——用例里写「故障转移」
/// 就能命中《02-负载均衡与故障转移》，不必抄全标题。
fn title_matches(hit_title: &str, want: &str) -> bool {
    let h = hit_title.trim().to_lowercase();
    let w = want.trim().to_lowercase();
    !w.is_empty() && (h == w || h.contains(&w))
}

/// 比例（分母 0 视为 1.0，避免"没有该类用例"被判成 0 分）
fn ratio(n: usize, d: usize) -> f64 {
    if d == 0 {
        1.0
    } else {
        n as f64 / d as f64
    }
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

    #[tokio::test]
    async fn eval_computes_recall_mrr_and_refuse_accuracy() {
        let pool = test_pool().await;
        let b = data::create_base(&pool, "评测库", "", 800, 150).await.unwrap();
        data::import_text(
            &pool,
            &b.id,
            "故障转移",
            "txt",
            "上游返回 5xx 或超时后，按 priority 顺序转移到下一个渠道。",
        )
        .await
        .unwrap();
        data::import_text(
            &pool,
            &b.id,
            "向量索引",
            "txt",
            "HNSW 图索引用于近似最近邻检索，向量存在分块表里。",
        )
        .await
        .unwrap();

        let cases = vec![
            EvalCase {
                question: "故障转移是怎么做的".to_string(),
                expect_docs: vec!["故障转移".to_string()],
                expect_no_answer: false,
            },
            EvalCase {
                question: "HNSW 是什么".to_string(),
                expect_docs: vec!["向量索引".to_string()],
                expect_no_answer: false,
            },
            EvalCase {
                question: "库里的正确答案是 42 吗".to_string(),
                expect_docs: vec![],
                expect_no_answer: true,
            },
        ];
        let svc = KnowledgeBaseService::default();
        let report = run_eval(&svc, &pool, &b.id, &cases, "keyword", 4).await.unwrap();

        assert_eq!(report.mode, "keyword");
        assert_eq!(report.total, 3);
        assert_eq!(report.answerable, 2);
        assert_eq!(report.recall_at_k, 1.0, "两条期望文档都应命中");
        assert!(report.mrr > 0.0 && report.mrr <= 1.0);
        assert_eq!(report.cases[0].matched_rank, Some(1));
        assert!(report.cases[0].passed);
        assert_eq!(report.cases[2].matched_rank, None);
        assert!(report.cases[2].passed, "无答案问题检索为空应算通过");
        assert!(report.cases[0].hits[0].kw_score.is_some());
    }

    #[tokio::test]
    async fn eval_counts_miss_and_false_answer() {
        let pool = test_pool().await;
        let b = data::create_base(&pool, "评测库", "", 800, 150).await.unwrap();
        data::import_text(&pool, &b.id, "园艺", "txt", "园艺需要关注土壤与光照。")
            .await
            .unwrap();
        let cases = vec![
            // 期望文档不存在 → 未召回
            EvalCase {
                question: "故障转移怎么做".to_string(),
                expect_docs: vec!["不存在的文档".to_string()],
                expect_no_answer: false,
            },
            // 期望为空，但库里有相关内容 → 拒答失败
            EvalCase {
                question: "园艺要注意什么".to_string(),
                expect_docs: vec![],
                expect_no_answer: true,
            },
        ];
        let svc = KnowledgeBaseService::default();
        let report = run_eval(&svc, &pool, &b.id, &cases, "keyword", 4).await.unwrap();
        assert_eq!(report.recall_at_k, 0.0);
        assert_eq!(report.mrr, 0.0);
        assert_eq!(report.refuse_accuracy, 0.0, "检索非空时拒答失败");
        assert!(report.cases.iter().all(|c| !c.passed));
    }

    #[test]
    fn load_cases_reports_file_and_json_errors() {
        let missing = load_cases("D:/definitely/not/here.json").unwrap_err();
        assert!(missing.contains("读取评测文件失败"), "{missing}");

        let dir = std::env::temp_dir().join(format!("kb-eval-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.json");
        std::fs::write(&bad, "{ not json").unwrap();
        let err = load_cases(bad.to_str().unwrap()).unwrap_err();
        assert!(err.contains("解析评测文件失败"), "{err}");

        let empty = dir.join("empty.json");
        std::fs::write(&empty, "[]").unwrap();
        assert!(load_cases(empty.to_str().unwrap()).unwrap_err().contains("没有用例"));

        let ok = dir.join("ok.json");
        std::fs::write(
            &ok,
            r#"[{"question":"故障转移怎么做","expectDocs":["故障转移"]},{"question":"没有答案的问题","expectNoAnswer":true}]"#,
        )
        .unwrap();
        let cases = load_cases(ok.to_str().unwrap()).unwrap();
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].expect_docs, vec!["故障转移"]);
        assert!(cases[1].expect_no_answer && cases[1].expect_docs.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
