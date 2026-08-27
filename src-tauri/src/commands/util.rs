//! 公共工具函数：时间、密钥、ID、脱敏。
//! 命令层与转发代理（proxy）共用。

use sha2::{Digest, Sha256};
use uuid::Uuid;

/// 当前时间（北京时间，UTC+8，无夏令时），格式 "YYYY-MM-DD HH:MM:SS"
pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    // UNIX 时间戳是 UTC；+8 小时得到北京时间后再做公历换算
    let secs = secs + 8 * 3600;
    let days = secs / 86400;
    let rem = secs % 86400;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// 当前日期 "YYYY-MM-DD"
pub fn today_iso() -> String {
    now_iso()[..10].to_string()
}

/// UNIX 天数 -> 公历 (y, m, d)，使用 proleptic Gregorian 算法。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// SHA-256 十六进制哈希（密钥鉴权用，keys 表只存哈希）
pub fn hash_key(key: &str) -> String {
    let mut h = Sha256::new();
    h.update(key.as_bytes());
    format!("{:x}", h.finalize())
}

/// 生成形如 "req_ab12cd34ef56" 的记录 ID
pub fn new_id(prefix: &str) -> String {
    let s = Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &s[..12])
}

/// 生成网关接入密钥：sk-llmapi- + 32 位 hex（UUID v4 全量，128 bit 熵）
pub fn gen_key_secret() -> String {
    format!("sk-llmapi-{}", Uuid::new_v4().simple())
}

/// 展示用脱敏（保留首尾 4 字符）
pub fn mask(secret: &str) -> String {
    if secret.is_empty() {
        return String::new();
    }
    if secret.len() <= 8 {
        "••••••••".to_string()
    } else {
        format!("{}••••{}", &secret[..4], &secret[secret.len() - 4..])
    }
}

/// 管理操作审计：渠道/密钥/设置/规则的变更留痕（写失败静默，不影响主流程）
pub async fn record_admin_audit(
    pool: &sqlx::SqlitePool,
    action: &str,
    target_type: &str,
    target_id: Option<&str>,
    detail: Option<&str>,
) {
    let _ = sqlx::query(
        "INSERT INTO admin_audit_logs (id, action, target_type, target_id, detail, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
    )
    .bind(new_id("aud"))
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .bind(detail)
    .bind(now_iso())
    .execute(pool)
    .await;
}
