pub mod channels;
pub mod keys;
pub mod requests;
pub mod settings;
pub mod stats;

use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tauri::State;
use uuid::Uuid;

/// 全局 SQLite 连接池状态。
pub struct DbState(pub SqlitePool);

impl DbState {
    /// 访问底层连接池。
    pub fn pool(&self) -> &SqlitePool {
        &self.0
    }
}

/// 从 Tauri State 中取出连接池引用。
pub(crate) fn pool<'a>(state: &'a State<'_, DbState>) -> &'a SqlitePool {
    state.pool()
}

pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let days = secs / 86400;
    let rem = secs % 86400;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, (rem % 3600) / 60, rem % 60)
}

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

pub fn hash_key(key: &str) -> String {
    let mut h = Sha256::new();
    h.update(key.as_bytes());
    format!("{:x}", h.finalize())
}

pub fn new_id(prefix: &str) -> String {
    let s = Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &s[..12])
}

pub fn gen_key_secret() -> String {
    let s = Uuid::new_v4().simple().to_string();
    format!("sk-waliapi-{}", &s[..20])
}

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
