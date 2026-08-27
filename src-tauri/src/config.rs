//! 配置文件（settings.json，Tauri Store 同格式的扁平 KV）：
//! - 首次启动：把 settings 表导出生成配置文件（带全部现有键值，可手改）
//! - 之后启动：配置文件存在则导入并覆盖 DB（改文件重启生效，权威语义）
//! - UI 修改设置（set_setting）：写 DB 后同步全量导出回配置文件（双写）
//! 文件位置：用户主目录/.llm-api-gateway/settings.json（Windows: C:\Users\<user>\.llm-api-gateway\）

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sqlx::SqlitePool;
use tauri::Manager;

/// 配置文件名
pub const SETTINGS_FILE: &str = "settings.json";

/// 数据目录：用户主目录/.llm-api-gateway（配置文件与数据库都放这里）
pub fn data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let home = app.path().home_dir().map_err(|e| e.to_string())?;
    Ok(home.join(".llm-api-gateway"))
}

/// 配置文件完整路径
pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join(SETTINGS_FILE)
}

/// 读取配置文件。文件不存在返回 None；解析失败时把坏文件备份为 settings.json.bak
/// 并返回 None（用 DB 现状重新生成），保证损坏的配置文件不会拖垮启动。
pub fn load_settings_file(config_dir: &Path) -> Option<HashMap<String, String>> {
    let path = settings_path(config_dir);
    if !path.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<HashMap<String, String>>(&text) {
        Ok(map) => Some(map),
        Err(e) => {
            let backup = path.with_extension("json.bak");
            let _ = std::fs::rename(&path, &backup);
            eprintln!("[config] settings.json 解析失败（{e}），已备份为 settings.json.bak");
            None
        }
    }
}

/// 把 settings 表全量导出为配置文件（格式化 JSON，人类可读可编辑）。
/// 写失败仅打日志，不影响主流程。
pub async fn dump_settings_file(pool: &SqlitePool, config_dir: &Path) {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM settings ORDER BY key")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    let map: HashMap<String, String> = rows.into_iter().collect();
    let json = serde_json::to_string_pretty(&map).unwrap_or_default();
    let path = settings_path(config_dir);
    if let Err(e) = std::fs::write(&path, json) {
        eprintln!("[config] 写入 settings.json 失败: {e}");
    }
}

/// 把配置文件导入并覆盖 settings 表（启动时与"重新加载"按钮调用）。
/// 返回导入的键数量。
pub async fn import_settings_file(
    pool: &SqlitePool,
    config_dir: &Path,
) -> Result<usize, String> {
    let Some(map) = load_settings_file(config_dir) else {
        return Ok(0);
    };
    for (k, v) in &map {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1,?2,?3)
             ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at = ?3",
        )
        .bind(k)
        .bind(v)
        .bind(crate::commands::util::now_iso())
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(map.len())
}
