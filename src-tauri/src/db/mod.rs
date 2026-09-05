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

#[cfg(test)]
mod tests {
    use super::*;

    /// SQLite 能力基线：本构建必须带 FTS5 + trigram 分词器（知识库全文检索依赖）。
    /// 若失败说明链接的 SQLite 构建缺 FTS5（换 bundled 特性），知识库检索需改 Plan B。
    #[tokio::test]
    async fn sqlite_build_has_fts5_with_trigram() {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();

        // 编译选项检查：FTS5 是否启用
        let fts5: i64 =
            sqlx::query_scalar("SELECT sqlite_compileoption_used('ENABLE_FTS5')")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(fts5, 1, "当前 SQLite 构建未编译 FTS5");
        let version: String = sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(&pool)
            .await
            .unwrap();
        // trigram 分词器要求 SQLite >= 3.34
        assert!(
            version
                .split('.')
                .zip(["3", "34"])
                .map(|(a, b)| a.parse::<u32>().unwrap() >= b.parse::<u32>().unwrap())
                .next()
                .unwrap(),
            "SQLite 版本过低，不支持 trigram: {version}"
        );

        // trigram 中文子串检索实测（SQLite 默认分词器对连续中文当整词，无法部分匹配）
        sqlx::query("CREATE VIRTUAL TABLE fts_t USING fts5(content, tokenize='trigram')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO fts_t (content) VALUES (?1)")
            .bind("机器学习与深度学习基础")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO fts_t (content) VALUES (?1)")
            .bind("强化学习在游戏中的应用")
            .execute(&pool)
            .await
            .unwrap();

        // 词首命中（≥3 字符）
        let hits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fts_t WHERE fts_t MATCH ?1",
        )
        .bind("机器学习")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(hits, 1);

        // 词中子串命中（trigram 特性）
        let hits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fts_t WHERE fts_t MATCH ?1",
        )
        .bind("学习与深")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(hits, 1);

        // BM25 排序可用
        let row: (i64, f64) = sqlx::query_as(
            "SELECT rowid, bm25(fts_t) FROM fts_t WHERE fts_t MATCH ?1 ORDER BY bm25(fts_t) LIMIT 1",
        )
        .bind("机器学习")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(row.0 > 0);

        // trigram 边界：<3 字符查询不命中（产品层需回退 LIKE 或提示）
        let hits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fts_t WHERE fts_t MATCH ?1",
        )
        .bind("学习")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(hits, 0, "trigram 少于 3 字符应无命中（已知边界）");
    }
}
