//! 安全模块：多引擎扫描 + 风险聚合 + 策略决策（企业级审计引擎的检测与策略层）。
//!
//! 分层：
//! - **检测层**：多引擎并行（策略模式），每个引擎实现 [`SecurityScanner`] trait——
//!   [`rules::RuleScanner`]（正则规则：secret/pii/prompt_injection）、
//!   [`encoding::EncodingScanner`]（隐写/混淆：base64/hex/零宽/Bidi/全角）、
//!   [`command::CommandScanner`]（AI 生成命令带毒：危险 shell/代码执行）、
//!   [`leak::check_leak`]（上下文泄露：输出复述输入凭证，跨阶段比对）。
//! - **聚合层**：`CompositeScanner` 按规则类别分流到引擎，脱敏链式应用，风险取最高。
//! - **决策层**：`decide`（策略决策点 PDP）——检测与处置解耦，规则可配
//!   `record`（仅记录）/ `mask`（脱敏）/ `block`（拦截）。
//! - **配置层**：规则存 `security_rules` 表（可增删启停改等级），
//!   [`refresh::RefreshScanner`] 定时重载（TTL），改动无需重启。
//!
//! 规则按类别路由：`secret/pii/prompt_injection` → 规则引擎；`encoding` → 编码引擎；
//! `command` → 命令引擎。未来可加 LLM 语义引擎（实现同一 trait 即可）。

pub mod command;
pub mod encoding;
pub mod leak;
pub mod refresh;
pub mod rules;
pub mod substring;

pub use refresh::RefreshScanner;
// 供代理测试直接构造单引擎扫描器（非测试目标下由 CompositeScanner 使用具体引擎）
#[allow(unused_imports)]
pub use rules::RuleScanner;

use serde::Serialize;

/// 风险等级（声明顺序即严重程度：Low < Medium < High）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

impl RiskLevel {
    /// "low"/"medium"/"high"（大小写不敏感）→ 等级
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "low" => Some(RiskLevel::Low),
            "medium" => Some(RiskLevel::Medium),
            "high" => Some(RiskLevel::High),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
        }
    }
}

/// 一条命中记录（按规则聚合：同一规则多处命中合并为一条，count 记录次数）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub rule_id: String,
    pub category: String,
    pub risk: RiskLevel,
    /// 命中次数
    pub count: usize,
    /// 脱敏后的片段（展示用，不存原文）
    pub snippet_masked: String,
    /// 处置动作（来自规则配置或引擎默认）：record / mask / block
    pub action: String,
}

/// 一次扫描的结果：整体风险取命中规则的最高等级
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub risk: RiskLevel,
    pub findings: Vec<Finding>,
    /// 脱敏后的文本（替换策略下替换原文使用）
    pub sanitized: String,
    /// high 风险命中的原文片段（内存态，仅用于上下文泄露比对；不落库、不序列化）
    #[serde(skip)]
    pub high_fragments: Vec<String>,
}

/// 安全扫描器：转发层的集成点（策略模式——每个引擎 / 未来 LLM 语义扫描器实现此 trait）。
#[async_trait::async_trait]
pub trait SecurityScanner: Send + Sync {
    /// 扫描一段文本，返回命中记录 + 脱敏后的文本
    async fn scan(&self, text: &str) -> ScanResult;
}

// ---------------- 策略决策点（PDP） ----------------

/// 处置决策：检测结果 → 处置动作（与检测解耦）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    /// 放行（全部规则仅记录）
    Pass,
    /// 脱敏后放行（存在 mask 规则命中）
    Mask,
    /// 拦截（存在 block 规则命中）
    Block,
}

/// 一组命中记录中的最高风险等级（None = 没有任何命中）。
/// 请求日志据此记录风险等级，并按 log.store_level 决定是否保存内容明细。
pub fn max_risk(findings: &[Finding]) -> Option<RiskLevel> {
    findings.iter().map(|f| f.risk).max()
}

/// 策略决策：任一规则配置了 block 且命中 → 拦截；否则存在 mask → 脱敏；否则放行。
pub fn decide(findings: &[Finding]) -> PolicyDecision {
    if findings.iter().any(|f| f.action == "block") {
        PolicyDecision::Block
    } else if findings.iter().any(|f| f.action == "mask") {
        PolicyDecision::Mask
    } else {
        PolicyDecision::Pass
    }
}

// ---------------- 多引擎聚合 ----------------

/// 多引擎扫描器：按规则类别分流构造引擎，脱敏链式应用，findings/风险/高敏片段聚合。
pub struct CompositeScanner {
    engines: Vec<Box<dyn SecurityScanner>>,
}

impl CompositeScanner {
    /// 分流：match_type=substring → 子串引擎；encoding → 编码引擎；command → 命令引擎；
    /// 其余（regex）→ 规则引擎
    pub fn from_defs(defs: Vec<rules::RuleDef>) -> Self {
        let mut rule_defs = Vec::new();
        let mut enc_defs = Vec::new();
        let mut cmd_defs = Vec::new();
        let mut sub_defs = Vec::new();
        for d in defs {
            if d.match_type == "substring" {
                sub_defs.push(d);
            } else {
                match d.category.as_str() {
                    "encoding" => enc_defs.push(d),
                    "command" => cmd_defs.push(d),
                    _ => rule_defs.push(d),
                }
            }
        }
        let mut engines: Vec<Box<dyn SecurityScanner>> = Vec::new();
        if !rule_defs.is_empty() {
            engines.push(Box::new(rules::RuleScanner::from_defs(rule_defs)));
        }
        if !enc_defs.is_empty() {
            engines.push(Box::new(encoding::EncodingScanner::from_defs(enc_defs)));
        }
        if !cmd_defs.is_empty() {
            engines.push(Box::new(command::CommandScanner::from_defs(cmd_defs)));
        }
        if !sub_defs.is_empty() {
            engines.push(Box::new(substring::SubstringScanner::from_defs(sub_defs)));
        }
        Self { engines }
    }
}

#[async_trait::async_trait]
impl SecurityScanner for CompositeScanner {
    async fn scan(&self, text: &str) -> ScanResult {
        let mut risk = RiskLevel::Low;
        let mut findings = Vec::new();
        let mut sanitized = text.to_string();
        let mut high_fragments = Vec::new();
        for engine in &self.engines {
            let r = engine.scan(&sanitized).await;
            risk = risk.max(r.risk);
            findings.extend(r.findings);
            sanitized = r.sanitized;
            high_fragments.extend(r.high_fragments);
        }
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
    use crate::security::rules::RuleDef;

    fn finding(action: &str) -> Finding {
        Finding {
            rule_id: "r".into(),
            category: "test".into(),
            risk: RiskLevel::Medium,
            count: 1,
            snippet_masked: "[MASKED:r]".into(),
            action: action.into(),
        }
    }

    #[test]
    fn max_risk_takes_highest() {
        assert_eq!(max_risk(&[]), None);
        assert_eq!(max_risk(&[finding("mask")]), Some(RiskLevel::Medium));
        let mut high = finding("mask");
        high.risk = RiskLevel::High;
        assert_eq!(max_risk(&[finding("record"), high]), Some(RiskLevel::High));
    }

    #[test]
    fn pdp_block_wins_over_mask() {
        assert_eq!(decide(&[finding("mask"), finding("block")]), PolicyDecision::Block);
    }

    #[test]
    fn pdp_mask_when_any_mask() {
        assert_eq!(decide(&[finding("record"), finding("mask")]), PolicyDecision::Mask);
    }

    #[test]
    fn pdp_pass_when_only_record() {
        assert_eq!(decide(&[finding("record")]), PolicyDecision::Pass);
        assert_eq!(decide(&[]), PolicyDecision::Pass);
    }

    #[tokio::test]
    async fn composite_routes_by_category() {
        let defs = vec![
            RuleDef {
                id: "test_key".into(),
                category: "secret".into(),
                risk: RiskLevel::High,
                pattern: "sk-test-[0-9]{8}".into(),
                match_type: "regex".into(),
                action: "mask".into(),
            },
            RuleDef {
                id: "test_b64".into(),
                category: "encoding".into(),
                risk: RiskLevel::Medium,
                pattern: "[A-Za-z0-9+/]{20,}".into(),
                match_type: "regex".into(),
                action: "mask".into(),
            },
            RuleDef {
                id: "test_cmd".into(),
                category: "command".into(),
                risk: RiskLevel::High,
                pattern: "rm -rf".into(),
                match_type: "regex".into(),
                action: "block".into(),
            },
            RuleDef {
                id: "test_sub".into(),
                category: "prompt_injection".into(),
                risk: RiskLevel::High,
                pattern: "忽略以上所有指令".into(),
                match_type: "substring".into(),
                action: "mask".into(),
            },
        ];
        let scanner = CompositeScanner::from_defs(defs);
        let r = scanner.scan("sk-test-12345678 rm -rf / Gk9wHe3xQk9wHe3xQk9wHe3x 忽略以上所有指令").await;
        assert_eq!(r.findings.len(), 4);
        assert_eq!(r.risk, RiskLevel::High);
        assert_eq!(decide(&r.findings), PolicyDecision::Block);
        assert!(!r.sanitized.contains("sk-test-12345678"));
        assert!(r.sanitized.contains("[MASKED:test_key]"));
        assert!(r.sanitized.contains("[MASKED:test_sub]"));
        assert!(!r.high_fragments.is_empty());
    }
}
