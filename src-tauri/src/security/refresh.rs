//! 自动刷新扫描器：规则存 DB，改动后 TTL 内自动重载，无需重启应用。
//! 首次扫描即从 DB 加载（规则表为空/不可用时回退内置默认规则）。
//! 本地网关 30s 粒度足够；并发下重复加载一次无害（原子占位）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use tokio::sync::RwLock;

use super::rules::load_rules;
use super::{CompositeScanner, RuleScanner, ScanResult, SecurityScanner};

/// 规则重载间隔（秒）
const TTL_SECS: u64 = 30;

/// 带 TTL 的扫描器：scan 时检查规则版本，超时则从 DB 重建引擎
pub struct RefreshScanner {
    pool: SqlitePool,
    inner: RwLock<Arc<dyn SecurityScanner>>,
    last_load: AtomicU64,
}

impl RefreshScanner {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            // 初始用内置默认（同步构造），首次 scan 立即从 DB 加载
            inner: RwLock::new(Arc::new(RuleScanner::new())),
            last_load: AtomicU64::new(0),
        }
    }

    async fn refresh(&self) {
        let now = unix_secs();
        if now.saturating_sub(self.last_load.load(Ordering::Relaxed)) < TTL_SECS {
            return;
        }
        self.last_load.store(now, Ordering::Relaxed);
        let scanner: Arc<dyn SecurityScanner> = match load_rules(&self.pool).await {
            defs if !defs.is_empty() => Arc::new(CompositeScanner::from_defs(defs)),
            _ => Arc::new(RuleScanner::new()),
        };
        *self.inner.write().await = scanner;
    }
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[async_trait::async_trait]
impl SecurityScanner for RefreshScanner {
    async fn scan(&self, text: &str) -> ScanResult {
        self.refresh().await;
        self.inner.read().await.scan(text).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::test_util::test_pool;

    #[tokio::test]
    async fn loads_rules_from_db() {
        let pool = test_pool().await;
        // 手工建规则表 + 一条测试规则
        sqlx::query(
            "CREATE TABLE security_rules (
                id TEXT PRIMARY KEY, category TEXT, risk TEXT, pattern TEXT, match_type TEXT,
                enabled INTEGER, action TEXT, created_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO security_rules (id, category, risk, pattern, match_type, enabled, action, created_at)
             VALUES ('t_phone', 'pii', 'medium', '\\b1[3-9]\\d{9}\\b', 'regex', 1, 'block', '2025-01-01 00:00:00')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let scanner = RefreshScanner::new(pool);
        let r = scanner.scan("手机号 13800138000 请勿外传").await;
        assert_eq!(r.findings.len(), 1);
        assert_eq!(r.findings[0].rule_id, "t_phone");
        assert_eq!(r.findings[0].action, "block");
        assert!(!r.sanitized.contains("13800138000"));
        assert!(r.sanitized.contains("[MASKED:t_phone]"));
        // DB 有规则时以 DB 为准：内置默认的 api_key_llmapi 不应生效
        assert!(r.findings.iter().all(|f| f.rule_id != "api_key_llmapi"));
    }

    #[tokio::test]
    async fn falls_back_to_default_rules_when_table_empty() {
        let pool = test_pool().await;
        sqlx::query(
            "CREATE TABLE security_rules (
                id TEXT PRIMARY KEY, category TEXT, risk TEXT, pattern TEXT, match_type TEXT,
                enabled INTEGER, action TEXT, created_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let scanner = RefreshScanner::new(pool);
        let r = scanner.scan("sk-llmapi-abc123def456ghi789jk").await;
        assert!(r.findings.iter().any(|f| f.rule_id == "api_key_llmapi"));
    }
}
