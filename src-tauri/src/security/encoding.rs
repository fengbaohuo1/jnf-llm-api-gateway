//! 编码检测引擎：隐写 / 混淆攻击（OWASP LLM 提示注入混淆变体）。
//! 检测长 base64/hex 载荷、零宽字符、Bidi 控制符、全角混淆等特征。
//! 复用 [`super::rules::RegexScanner`] 实现，规则类别为 `encoding`。

use super::rules::{RegexScanner, RuleDef};
use super::{ScanResult, SecurityScanner};

/// 编码/隐写检测引擎（规则类别 = encoding）
pub struct EncodingScanner {
    inner: RegexScanner,
}

impl EncodingScanner {
    pub fn from_defs(defs: Vec<RuleDef>) -> Self {
        Self {
            inner: RegexScanner::from_defs(defs),
        }
    }
}

#[async_trait::async_trait]
impl SecurityScanner for EncodingScanner {
    async fn scan(&self, text: &str) -> ScanResult {
        self.inner.scan(text).await
    }
}
