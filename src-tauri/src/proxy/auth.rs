//! 密钥鉴权与配额检查。

use sqlx::SqlitePool;

use crate::commands::util::{hash_key, now_iso};

/// 鉴权通过的密钥身份
pub(crate) struct AuthKey {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Debug)]
pub(crate) enum AuthError {
    /// 密钥不存在 / 格式错（无身份，不记日志）
    Unauthorized,
    /// 已禁用 / 已过期（身份已知）
    Forbidden(String),
    /// 配额用尽
    QuotaExceeded,
}

/// 校验 Authorization: Bearer <sk-llmapi-xxx>，返回密钥身份
pub(crate) async fn authenticate(
    pool: &SqlitePool,
    auth_header: Option<&str>,
) -> Result<AuthKey, AuthError> {
    let header = auth_header
        .and_then(|h| h.strip_prefix("Bearer "))
        .or_else(|| auth_header.and_then(|h| h.strip_prefix("bearer ")))
        .ok_or(AuthError::Unauthorized)?;
    let token = header.trim();
    if token.is_empty() {
        return Err(AuthError::Unauthorized);
    }

    let hash = hash_key(token);
    let row: Option<(String, String, String, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT id, name, status, expires_at, quota_limit, used_tokens FROM keys WHERE key_hash = ?1",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await
    .map_err(|_| AuthError::Unauthorized)?;

    let Some((id, name, status, expires_at, quota_limit, used_tokens)) = row else {
        return Err(AuthError::Unauthorized);
    };

    if status != "active" {
        return Err(AuthError::Forbidden("密钥已禁用".into()));
    }
    if let Some(exp) = expires_at {
        if exp.as_str() < now_iso().as_str() {
            return Err(AuthError::Forbidden("密钥已过期".into()));
        }
    }
    if quota_limit >= 0 && used_tokens >= quota_limit {
        return Err(AuthError::QuotaExceeded);
    }
    Ok(AuthKey { id, name })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::test_util::{insert_key, test_pool};

    #[tokio::test]
    async fn auth_accepts_valid_key() {
        let pool = test_pool().await;
        insert_key(&pool, "k1", "sk-llmapi-abcdefghij1234567890", "active", -1, 0, None).await;
        let key = authenticate(&pool, Some("Bearer sk-llmapi-abcdefghij1234567890"))
            .await
            .unwrap();
        assert_eq!(key.id, "k1");
    }

    #[tokio::test]
    async fn auth_rejects_wrong_or_missing_key() {
        let pool = test_pool().await;
        insert_key(&pool, "k1", "sk-llmapi-abcdefghij1234567890", "active", -1, 0, None).await;
        assert!(matches!(
            authenticate(&pool, Some("Bearer sk-llmapi-wrongwrong1234567890")).await,
            Err(AuthError::Unauthorized)
        ));
        assert!(matches!(authenticate(&pool, None).await, Err(AuthError::Unauthorized)));
        assert!(matches!(
            authenticate(&pool, Some("Basic dXNlcjpwYXNz")).await,
            Err(AuthError::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn auth_rejects_disabled_expired_quota() {
        let pool = test_pool().await;
        insert_key(&pool, "k-disabled", "sk-llmapi-disabledkey1234567890", "disabled", -1, 0, None).await;
        insert_key(&pool, "k-expired", "sk-llmapi-expiredkey1234567890", "active", -1, 0, Some("2020-01-01 00:00:00")).await;
        insert_key(&pool, "k-quota", "sk-llmapi-quotakey1234567890", "active", 100, 100, None).await;

        assert!(matches!(
            authenticate(&pool, Some("Bearer sk-llmapi-disabledkey1234567890")).await,
            Err(AuthError::Forbidden(_))
        ));
        assert!(matches!(
            authenticate(&pool, Some("Bearer sk-llmapi-expiredkey1234567890")).await,
            Err(AuthError::Forbidden(_))
        ));
        assert!(matches!(
            authenticate(&pool, Some("Bearer sk-llmapi-quotakey1234567890")).await,
            Err(AuthError::QuotaExceeded)
        ));
    }
}
