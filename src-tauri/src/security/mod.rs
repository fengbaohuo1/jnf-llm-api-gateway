//! 安全模块：请求侧/响应侧的敏感信息扫描与脱敏。
//!
//! 转发层通过 [`SecurityScanner`] trait 集成（不写死规则实现），
//! 内置实现见 [`rules::RuleScanner`]：正则规则集，命中后把敏感片段替换为 `[MASKED:<rule_id>]`。
//!
//! 设计取向（个人网关）：
//! - 默认 **replace**（脱敏后照常转发），不打断正常使用；
//! - `security.mode = block` 时，转发层对 high 风险命中直接拦截（400 / 中断流）。

pub mod rules;

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
}

/// 一次扫描的结果：整体风险取命中规则的最高等级
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub risk: RiskLevel,
    pub findings: Vec<Finding>,
    /// 脱敏后的文本（替换策略下替换原文使用）
    pub sanitized: String,
}

/// 安全扫描器：转发层的集成点。
/// 当前实现是本地规则扫描器；未来可替换为 LLM 扫描器 / 外部安全服务。
#[async_trait::async_trait]
pub trait SecurityScanner: Send + Sync {
    /// 扫描一段文本，返回命中记录 + 脱敏后的文本
    async fn scan(&self, text: &str) -> ScanResult;
}
