pub mod channels;
pub mod keys;
pub mod requests;
pub mod security;
pub mod settings;
pub mod stats;
pub mod util;

use sqlx::SqlitePool;
use tauri::State;

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
