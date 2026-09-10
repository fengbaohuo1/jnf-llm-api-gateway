//! 查询分词与 bigram 预处理（纯函数，无 IO，便于单测）。
//!
//! **双索引策略**（路由见 [`bigram_query_terms`] / [`trigram_query_terms`]）：
//!
//! - `kb_chunks_bg`（`unicode61` + [`bigram_text`] 预处理）：CJK 连续段切成
//!   2 字滑窗，查询侧同样切成 bigram 短语后 **OR** 连接。这是无词典条件下
//!   中文检索的标准做法——任何 ≥2 字符的查询都能命中，排序交给 BM25 的 IDF
//!   （罕用 bigram 权重高，噪声 bigram 大多匹配不到东西，基本无害）。
//! - `kb_chunks_fts`（FTS5 `trigram`）：只服务**非 CJK 且 ≥3 字符**的词元，
//!   用短语做子串匹配，补回"英文部分匹配"（`embed` 命中 `embedding`）。
//!
//! 为什么不按词长把整句路由到 trigram：中文没有空格，标点切段后仍是一长串
//! （`网关怎么做故障转移`），整串短语在语料里不可能连续出现，必然 0 命中——
//! 这正是旧实现的根因。故 CJK 一律走 bigram，trigram 只承担英文子串补召回。

/// 停用词（中文虚词/疑问词 + 英文）。整段等于停用词时整段丢弃；
/// bigram 切分时命中停用词的 bigram 也丢弃（如 `怎么` / `什么`）。
const STOPWORDS: &[&str] = &[
    // 中文虚词
    "的", "了", "和", "与", "及", "或", "在", "是", "有", "吗", "呢", "吧", "啊", "着", "被", "把",
    // 中文疑问/指示
    "怎么", "怎样", "如何", "什么", "是什么", "什么是", "哪些", "哪个", "为什么", "多少", "请",
    "介绍", "说明", "告诉", "讲讲", "关于", "一下", "可以", "需要", "有没有", "这个", "那个",
    "我们", "你们",
    // 英文
    "the", "a", "an", "is", "are", "was", "were", "of", "to", "in", "on", "for", "and", "or", "how",
    "what", "why", "which", "please", "about", "with", "that", "this",
];

/// 单次查询最多使用的词元数（长问题的 bigram 噪声收敛，避免超长 MATCH 表达式）
const MAX_QUERY_TERMS: usize = 32;

/// 切词：按非字母数字切段（全角/半角标点、空白都切）→ 丢弃整段停用词 → 去重保序。
/// 不做大小写归一（trigram 与 unicode61 分词器均大小写不敏感）。
pub(crate) fn query_terms(q: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in q.chars() {
        if c.is_alphanumeric() {
            cur.push(c);
        } else if !cur.is_empty() {
            push_term(&mut out, &cur);
            cur.clear();
        }
    }
    if !cur.is_empty() {
        push_term(&mut out, &cur);
    }
    out
}

fn push_term(out: &mut Vec<String>, term: &str) {
    if is_stopword(term) {
        return;
    }
    if !out.iter().any(|t| t.eq_ignore_ascii_case(term)) {
        out.push(term.to_string());
    }
}

fn is_stopword(term: &str) -> bool {
    let lower = term.to_lowercase();
    STOPWORDS.contains(&lower.as_str())
}

/// bigram 索引表用的查询词元：
/// - CJK 段 → 2 字滑窗（跳过停用词 bigram），段内孤立单字保留自身；
/// - 非 CJK 段 → 整词（≥2 字符；单字符英文无检索价值，丢弃）。
///
/// 例：`网关怎么做故障转移` → `网关 关怎 么做 做故 故障 障转 转移`（`怎么` 被剔除）。
pub(crate) fn bigram_query_terms(terms: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in terms {
        let chars: Vec<char> = t.chars().collect();
        if chars.iter().all(|c| is_cjk(*c)) {
            for piece in cjk_bigrams(&chars) {
                if !is_stopword(&piece) {
                    push_unique(&mut out, piece);
                }
            }
        } else if chars.len() >= 2 {
            push_unique(&mut out, t.clone());
        }
    }
    out
}

/// trigram 索引表用的查询词元：只保留**非 CJK 且 ≥3 字符**的段（英文部分匹配）
pub(crate) fn trigram_query_terms(terms: &[String]) -> Vec<String> {
    terms
        .iter()
        .filter(|t| {
            let n = t.chars().count();
            n >= 3 && !t.chars().any(is_cjk)
        })
        .cloned()
        .collect()
}

/// 把词元列表渲染成 FTS5 MATCH 表达式（各段包短语后 OR 连接；无词元返回 None）
pub(crate) fn build_match_query(terms: &[String]) -> Option<String> {
    if terms.is_empty() {
        return None;
    }
    let parts: Vec<String> = terms
        .iter()
        .take(MAX_QUERY_TERMS)
        .map(|t| fts_phrase(t))
        .collect();
    Some(parts.join(" OR "))
}

/// FTS5 短语包裹：内部双引号翻倍（fts5 转义规则）
fn fts_phrase(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

/// 是否 CJK（汉字 + 假名 + 谚文）：这些字符无空格分隔，需要 bigram 滑窗
pub(crate) fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x3400..=0x4DBF      // CJK 扩展 A
        | 0x4E00..=0x9FFF    // CJK 基本区
        | 0xF900..=0xFAFF    // CJK 兼容
        | 0x3040..=0x30FF    // 平假名/片假名
        | 0xAC00..=0xD7AF // 谚文
    )
}

/// 文本 → bigram 索引串（供 `kb_chunks_bg` 的 unicode61 分词器消费）。
///
/// 规则：
/// - CJK 连续段 → 空格分隔的 2 字滑窗（`网关怎么做` → `网关 关怎 怎么 么做`），
///   **不跨标点/空白**（否则会造出原文不存在的组合）；
/// - 孤立 CJK 单字保留自身（可被单词查询命中）；
/// - 非 CJK 的字母数字段（英文/数字）原样保留，交给 unicode61 按词切分。
pub(crate) fn bigram_text(s: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut cjk_run: Vec<char> = Vec::new();
    let mut word = String::new();
    for c in s.chars() {
        if is_cjk(c) {
            flush_word(&mut word, &mut out);
            cjk_run.push(c);
        } else if c.is_alphanumeric() {
            flush_cjk_run(&mut cjk_run, &mut out);
            word.push(c);
        } else {
            flush_cjk_run(&mut cjk_run, &mut out);
            flush_word(&mut word, &mut out);
        }
    }
    flush_cjk_run(&mut cjk_run, &mut out);
    flush_word(&mut word, &mut out);
    out.join(" ")
}

fn cjk_bigrams(chars: &[char]) -> Vec<String> {
    let mut out = Vec::new();
    match chars.len() {
        0 => {}
        1 => out.push(chars[0].to_string()),
        _ => {
            for w in chars.windows(2) {
                out.push(w.iter().collect::<String>());
            }
        }
    }
    out
}

fn flush_cjk_run(run: &mut Vec<char>, out: &mut Vec<String>) {
    out.extend(cjk_bigrams(run));
    run.clear();
}

fn flush_word(word: &mut String, out: &mut Vec<String>) {
    if !word.is_empty() {
        out.push(std::mem::take(word));
    }
}

fn push_unique(out: &mut Vec<String>, t: String) {
    if !out.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
        out.push(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_terms_splits_on_punctuation_and_drops_stopword_segments() {
        let t = query_terms("网关怎么做故障转移？请介绍 HNSW 的原理");
        // 中文没有空格，标点切段后仍是一长串（这就是必须用 bigram 的原因）
        assert_eq!(t, vec!["网关怎么做故障转移", "请介绍", "HNSW", "的原理"]);
        // 整段等于停用词 → 丢弃；重复词去重
        assert_eq!(query_terms("什么 是 向量检索，向量检索；vector-search"),
            vec!["向量检索", "vector", "search"]);
        assert_eq!(query_terms("什么是 向量检索"), vec!["向量检索"]);
        assert!(query_terms("   ").is_empty());
    }

    #[test]
    fn bigram_terms_slide_cjk_and_drop_stopword_bigrams() {
        let terms = query_terms("网关怎么做故障转移");
        assert_eq!(
            bigram_query_terms(&terms),
            vec!["网关", "关怎", "么做", "做故", "故障", "障转", "转移"],
            "`怎么` 应作为停用词 bigram 被剔除"
        );
        // 2 字查询就是一个 bigram
        assert_eq!(bigram_query_terms(&query_terms("限流")), vec!["限流"]);
        // 英文整词保留；单字符英文丢弃（无检索价值）；孤立 CJK 单字保留
        assert_eq!(bigram_query_terms(&query_terms("hnsw")), vec!["hnsw"]);
        assert!(bigram_query_terms(&["a".to_string()]).is_empty());
        assert_eq!(bigram_query_terms(&["b7".to_string()]), vec!["b7"]);
        assert_eq!(bigram_query_terms(&query_terms("网")), vec!["网"]);
    }

    #[test]
    fn trigram_terms_only_take_non_cjk_ge3() {
        let terms = query_terms("故障转移 hnsw embed 向量");
        assert_eq!(
            trigram_query_terms(&terms),
            vec!["hnsw", "embed"],
            "CJK 一律走 bigram，trigram 只补英文子串"
        );
        assert!(trigram_query_terms(&query_terms("故障转移")).is_empty());
    }

    #[test]
    fn match_query_ors_phrases_and_escapes_quotes() {
        let terms = vec!["故障转移".to_string(), "hnsw".to_string()];
        assert_eq!(
            build_match_query(&terms).unwrap(),
            "\"故障转移\" OR \"hnsw\""
        );
        assert_eq!(
            build_match_query(&vec!["a\"b".to_string()]).unwrap(),
            "\"a\"\"b\"",
            "内部双引号应翻倍转义"
        );
        assert!(build_match_query(&[]).is_none());
        // 超长查询截断到上限
        let many: Vec<String> = (0..50).map(|i| format!("词{i}号")).collect();
        let q = build_match_query(&many).unwrap();
        assert_eq!(q.matches(" OR ").count(), MAX_QUERY_TERMS - 1, "应截断到上限");
    }

    #[test]
    fn bigram_text_slides_within_cjk_runs_only() {
        assert_eq!(bigram_text("网关怎么做"), "网关 关怎 怎么 么做");
        // 标点/空白不跨段（不造出跨句的伪组合）
        assert_eq!(bigram_text("网关，怎么做"), "网关 怎么 么做");
        // 孤立单字保留自身
        assert_eq!(bigram_text("与"), "与");
        // ASCII 段原样透传
        assert_eq!(bigram_text("HNSW 与 RAG"), "HNSW 与 RAG");
        assert_eq!(bigram_text("bge-m3 向量"), "bge m3 向量");
        assert_eq!(bigram_text(""), "");
    }

    #[test]
    fn cjk_detection_covers_common_ranges() {
        assert!(is_cjk('机') && is_cjk('あ') && is_cjk('한'));
        assert!(!is_cjk('a') && !is_cjk('3') && !is_cjk('，'));
    }
}
