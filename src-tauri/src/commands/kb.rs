//! 知识库 Tauri 命令（薄包装层：参数校验与错误透传委托给 kb::data）。
//! 命名 kb_*；参数由 Tauri 自动 camelCase → snake_case。

use tauri::State;

use crate::commands::DbState;
use crate::services::kb::data::{self, KbBase, KbChunk, KbDocument};

use super::pool;

/// 知识库列表（含文档数）
#[tauri::command]
pub async fn kb_list_bases(state: State<'_, DbState>) -> Result<Vec<KbBase>, String> {
    data::list_bases(pool(&state)).await
}

/// 创建知识库
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn kb_create_base(
    state: State<'_, DbState>,
    name: String,
    description: Option<String>,
    chunk_size: Option<i64>,
    chunk_overlap: Option<i64>,
) -> Result<KbBase, String> {
    data::create_base(
        pool(&state),
        &name,
        description.as_deref().unwrap_or(""),
        chunk_size.unwrap_or(800),
        chunk_overlap.unwrap_or(150),
    )
    .await
}

/// 更新知识库（仅更新传入的字段；status 支持 active/disabled）
#[tauri::command]
pub async fn kb_update_base(
    state: State<'_, DbState>,
    id: String,
    name: Option<String>,
    description: Option<String>,
    chunk_size: Option<i64>,
    chunk_overlap: Option<i64>,
    status: Option<String>,
) -> Result<KbBase, String> {
    data::update_base(
        pool(&state),
        &id,
        name.as_deref(),
        description.as_deref(),
        chunk_size,
        chunk_overlap,
        status.as_deref(),
    )
    .await
}

/// 删除知识库（级联删除全部文档/分块/全文索引）
#[tauri::command]
pub async fn kb_delete_base(state: State<'_, DbState>, id: String) -> Result<(), String> {
    data::delete_base(pool(&state), &id).await
}

/// 知识库文档列表
#[tauri::command]
pub async fn kb_list_documents(
    state: State<'_, DbState>,
    kb_id: String,
) -> Result<Vec<KbDocument>, String> {
    data::list_documents(pool(&state), &kb_id).await
}

/// 导入文本（md/txt；粘贴导入用）
#[tauri::command]
pub async fn kb_import_text(
    state: State<'_, DbState>,
    kb_id: String,
    title: String,
    format: String,
    text: String,
) -> Result<KbDocument, String> {
    data::import_text(pool(&state), &kb_id, &title, &format, &text).await
}

/// 导入本地文件（按扩展名识别 md/txt）
#[tauri::command]
pub async fn kb_import_file(
    state: State<'_, DbState>,
    kb_id: String,
    path: String,
) -> Result<KbDocument, String> {
    data::import_file(pool(&state), &kb_id, &path).await
}

/// 删除文档（级联分块并同步清理全文索引）
#[tauri::command]
pub async fn kb_delete_document(state: State<'_, DbState>, document_id: String) -> Result<(), String> {
    data::delete_document(pool(&state), &document_id).await
}

/// 文档分块列表（详情/预览用，按 seq 排序）
#[tauri::command]
pub async fn kb_list_chunks(
    state: State<'_, DbState>,
    document_id: String,
) -> Result<Vec<KbChunk>, String> {
    data::list_chunks(pool(&state), &document_id).await
}
