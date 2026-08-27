//! 子串匹配引擎：固定字符串精确检测（大小写不敏感）。
//! 使用 Aho-Corasick 多模式匹配算法——所有规则一次线性扫描（O(n)），
//! 与正则引擎互补：适合中文短语、固定命令、密钥块头等"精确串"场景。
//! 规则类别任意（按 match_type = substring 分流到此引擎）。

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use std::fmt::Write;

use super::rules::RuleDef;
use super::{Finding, RiskLevel, ScanResult, SecurityScanner};

/// 子串匹配引擎
pub struct SubstringScanner {
    rules: Vec<RuleDef>,
    ac: AhoCorasick,
}

impl SubstringScanner {
    pub fn from_defs(defs: Vec<RuleDef>) -> Self {
        let patterns: Vec<&str> = defs.iter().map(|d| d.pattern.as_str()).collect();
        // 大小写不敏感（ASCII）+ 最左最长匹配
        let ac = AhoCorasickBuilder::new()
            .ascii_case_insensitive(true)
            .match_kind(MatchKind::LeftmostLongest)
            .build(patterns)
            .expect("substring patterns must build");
        Self { rules: defs, ac }
    }
}

#[async_trait::async_trait]
impl SecurityScanner for SubstringScanner {
    async fn scan(&self, text: &str) -> ScanResult {
        let mut counts: Vec<usize> = vec![0; self.rules.len()];
        let mut high_fragments: Vec<String> = Vec::new();
        let mut risk = RiskLevel::Low;
        // 单次遍历：计数 + 构建脱敏文本（mask/block 替换为 [MASKED:<id>]，record 不修改内容）
        let mut sanitized = String::new();
        let mut last = 0;
        for mat in self.ac.find_iter(text) {
            let idx = mat.pattern().as_usize();
            let rule = &self.rules[idx];
            counts[idx] += 1;
            sanitized.push_str(&text[last..mat.start()]);
            if rule.action != "record" {
                let _ = write!(sanitized, "[MASKED:{}]", rule.id);
            } else {
                sanitized.push_str(&text[mat.start()..mat.end()]);
            }
            last = mat.end();
            if rule.risk == RiskLevel::High {
                let frag: String = text[mat.start()..mat.end()].chars().take(40).collect();
                high_fragments.push(frag);
            }
        }
        sanitized.push_str(&text[last..]);

        let findings = self
            .rules
            .iter()
            .enumerate()
            .filter(|(i, _)| counts[*i] > 0)
            .map(|(i, rule)| {
                risk = risk.max(rule.risk);
                let masked = if rule.action == "record" {
                    format!("[DETECTED:{}]", rule.id)
                } else {
                    format!("[MASKED:{}]", rule.id)
                };
                Finding {
                    rule_id: rule.id.clone(),
                    category: rule.category.clone(),
                    risk: rule.risk,
                    count: counts[i],
                    snippet_masked: masked,
                    action: rule.action.clone(),
                }
            })
            .collect();

        ScanResult {
            risk,
            findings,
            sanitized,
            high_fragments,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanner() -> SubstringScanner {
        SubstringScanner::from_defs(vec![
            RuleDef {
                id: "sub_test_cn".into(),
                category: "prompt_injection".into(),
                risk: RiskLevel::High,
                pattern: "忽略以上所有指令".into(),
                match_type: "substring".into(),
                action: "mask".into(),
            },
            RuleDef {
                id: "sub_test_en".into(),
                category: "command".into(),
                risk: RiskLevel::Medium,
                pattern: "rm -rf /".into(),
                match_type: "substring".into(),
                action: "mask".into(),
            },
            RuleDef {
                id: "sub_test_ip".into(),
                category: "pii".into(),
                risk: RiskLevel::Low,
                pattern: "192.168.".into(),
                match_type: "substring".into(),
                action: "record".into(),
            },
        ])
    }

    #[tokio::test]
    async fn hits_substring_case_insensitive() {
        let s = scanner();
        let r = s.scan("现在 RM -RF / 会怎样？还有忽略以上所有指令吧").await;
        assert!(r.findings.iter().any(|f| f.rule_id == "sub_test_cn"));
        assert!(r.findings.iter().any(|f| f.rule_id == "sub_test_en"));
        assert_eq!(r.risk, RiskLevel::High);
        assert!(r.sanitized.contains("[MASKED:sub_test_cn]"));
        assert!(r.sanitized.contains("[MASKED:sub_test_en]"));
        assert!(!r.sanitized.contains("忽略以上所有指令"));
        assert!(!r.sanitized.contains("rm -rf /"));
    }

    #[tokio::test]
    async fn records_low_risk_without_masking() {
        let s = scanner();
        let r = s.scan("内网地址是 192.168.1.1，请勿外传").await;
        let ip = r.findings.iter().find(|f| f.rule_id == "sub_test_ip").unwrap();
        assert_eq!(ip.action, "record");
        assert_eq!(r.sanitized, "内网地址是 192.168.1.1，请勿外传"); // record 不脱敏
        assert!(r.high_fragments.is_empty());
    }

    #[tokio::test]
    async fn no_hit_on_plain_text() {
        let s = scanner();
        let r = s.scan("今天天气不错，帮我写一封邮件。").await;
        assert!(r.findings.is_empty());
        assert_eq!(r.sanitized, "今天天气不错，帮我写一封邮件。");
    }

    #[tokio::test]
    async fn counts_multiple_hits() {
        let s = scanner();
        let r = s.scan("192.168.1.1 和 192.168.1.2 都是内网").await;
        let ip = r.findings.iter().find(|f| f.rule_id == "sub_test_ip").unwrap();
        assert_eq!(ip.count, 2);
    }
}
