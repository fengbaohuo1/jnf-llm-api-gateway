//! 自动刷新扫描器：规则存 DB，改动后 TTL 内自动重载，无需重启应用。
//! 首次扫描即从 DB 加载（规则表为空/不可用时回退内置默认规则）。
//! 本地网关 30s 粒度足够；并发下重复加载一次无害（原子占位）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use tokio::sync::RwLock;

use super::rules::{default_defs, load_rules};
use super::{CompositeScanner, ScanResult, SecurityScanner};

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
            // 初始用内置默认（含子串引擎），首次 scan 立即从 DB 加载
            inner: RwLock::new(Arc::new(CompositeScanner::from_defs(default_defs()))),
            last_load: AtomicU64::new(0),
        }
    }

    async fn refresh(&self) {
        let now = unix_secs();
        if now.saturating_sub(self.last_load.load(Ordering::Relaxed)) < TTL_SECS {
            return;
        }
        self.last_load.store(now, Ordering::Relaxed);
        // 区分「规则表为空（从未初始化/全部删除）→ 内置默认」与
        // 「表里有规则但全部被禁用 → 空引擎」：否则禁用所有规则 30s 后会被默认规则复活
        let total: i64 = sqlx::query_scalar("SELECT count(*) FROM security_rules")
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0);
        let scanner: Arc<dyn SecurityScanner> = if total == 0 {
            Arc::new(CompositeScanner::from_defs(default_defs()))
        } else {
            Arc::new(CompositeScanner::from_defs(load_rules(&self.pool).await))
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

    #[tokio::test]
    async fn all_disabled_rules_mean_no_scanning() {
        let pool = test_pool().await;
        sqlx::query(
            "CREATE TABLE security_rules (
                id TEXT PRIMARY KEY, category TEXT, risk TEXT, pattern TEXT, match_type TEXT,
                enabled INTEGER, action TEXT, created_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // 表里有规则但被禁用：不能回退到内置默认规则
        sqlx::query(
            "INSERT INTO security_rules (id, category, risk, pattern, match_type, enabled, action, created_at)
             VALUES ('t_disabled', 'secret', 'high', 'sk-test-[0-9]{8}', 'regex', 0, 'mask', '2025-01-01 00:00:00')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let scanner = RefreshScanner::new(pool);
        let r = scanner
            .scan("sk-llmapi-abc123def456ghi789jk sk-test-12345678")
            .await;
        assert!(r.findings.is_empty(), "全部禁用时不应有任何命中: {:?}", r.findings);
        assert_eq!(r.sanitized, "sk-llmapi-abc123def456ghi789jk sk-test-12345678");
    }
}
