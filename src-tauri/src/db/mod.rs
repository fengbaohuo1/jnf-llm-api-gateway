use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::path::Path;
use std::str::FromStr;

/// 打开（必要时创建）SQLite 连接池并执行迁移。
pub async fn init(db_path: &Path) -> Result<SqlitePool, String> {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))
        .map_err(|e| e.to_string())?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await
        .map_err(|e| format!("连接数据库失败: {e}"))?;

    // 编译期嵌入并执行迁移
    sqlx::migrate!()
        .run(&pool)
        .await
        .map_err(|e| format!("执行数据库迁移失败: {e}"))?;

    Ok(pool)
}
