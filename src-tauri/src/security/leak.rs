//! 上下文泄露检测：模型输出是否"复述"了请求侧扫出的高敏凭证（OWASP LLM06）。
//! 跨阶段比对：请求侧扫描收集 high 命中的原文片段（内存态），
//! 响应侧在脱敏前检查输出是否包含这些片段——防止模型把上下文里的密钥原样吐出来。

use super::{Finding, RiskLevel};

pub const LEAK_RULE_ID: &str = "context_leak";

/// 检查 `text`（模型输出原文）是否包含任一高敏片段。
/// 短片段（< 8 字符）跳过避免误报；命中返回一条聚合 finding（action=mask）。
pub fn check_leak(text: &str, secrets: &[String]) -> Option<Finding> {
    let hits: Vec<&str> = secrets
        .iter()
        .filter(|s| {
            let s = s.trim();
            s.chars().count() >= 8 && text.contains(s)
        })
        .map(|s| s.as_str())
        .collect();
    if hits.is_empty() {
        return None;
    }
    Some(Finding {
        rule_id: LEAK_RULE_ID.to_string(),
        category: "leak".to_string(),
        risk: RiskLevel::High,
        count: hits.len(),
        snippet_masked: "[MASKED:context_leak]".to_string(),
        action: "mask".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_output_replaying_secret() {
        let secrets = vec!["sk-llmapi-abc123def456ghi789jk".to_string()];
        let r = check_leak("好的，你的密钥是 sk-llmapi-abc123def456ghi789jk，请保管好", &secrets);
        assert!(r.is_some());
        let f = r.unwrap();
        assert_eq!(f.rule_id, LEAK_RULE_ID);
        assert_eq!(f.risk, RiskLevel::High);
        assert_eq!(f.count, 1);
    }

    #[test]
    fn ignores_unrelated_output() {
        let secrets = vec!["sk-llmapi-abc123def456ghi789jk".to_string()];
        assert!(check_leak("今天天气不错", &secrets).is_none());
    }

    #[test]
    fn skips_short_fragments() {
        let secrets = vec!["abc".to_string()];
        assert!(check_leak("包含 abc 的文字", &secrets).is_none());
    }

    #[test]
    fn empty_secrets_no_finding() {
        assert!(check_leak("任何文本", &[]).is_none());
    }
}
