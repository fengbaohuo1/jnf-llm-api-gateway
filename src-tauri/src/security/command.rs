//! 命令检测引擎：AI 生成命令带毒（OWASP LLM05 输出处理不当）。
//! 检测模型输出中的危险 shell / 代码执行模式（破坏性命令、管道执行、编码执行、
//! 脚本执行、反弹 shell）。复用 [`super::rules::RegexScanner`] 实现，规则类别为 `command`。

use super::rules::{RegexScanner, RuleDef};
use super::{ScanResult, SecurityScanner};

/// 命令带毒检测引擎（规则类别 = command，响应侧重）
pub struct CommandScanner {
    inner: RegexScanner,
}

impl CommandScanner {
    pub fn from_defs(defs: Vec<RuleDef>) -> Self {
        Self {
            inner: RegexScanner::from_defs(defs),
        }
    }
}

#[async_trait::async_trait]
impl SecurityScanner for CommandScanner {
    async fn scan(&self, text: &str) -> ScanResult {
        self.inner.scan(text).await
    }
}
