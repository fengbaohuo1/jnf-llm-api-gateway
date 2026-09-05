pub mod channels;
pub mod kb;
pub mod keys;
pub mod requests;
pub mod security;
pub mod services;
pub mod settings;
pub mod stats;
pub mod util;

use std::sync::atomic::AtomicBool;

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

/// 进程级运行状态（窗口事件同步回调中无法异步查库，用内存标志位）。
pub struct AppState {
    /// 关闭按钮 → 隐藏到托盘（settings: general.close_to_tray）
    pub close_to_tray: AtomicBool,
}
