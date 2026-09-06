//! 知识库 Tauri 命令（薄包装层：参数校验与错误透传委托给 kb 数据/服务层）。
//! 命名 kb_*；参数由 Tauri 自动 camelCase → snake_case。

use std::sync::Arc;

use tauri::State;

use crate::commands::DbState;
use crate::services::kb::data::{self, KbBase, KbChunk, KbDocument};
use crate::services::kb::embed::{self, Embedder};
use crate::services::kb::rag::{self, ChatReply, KbMessage, KbSession};
use crate::services::kb::search::{self, SearchHit};
use crate::services::kb::{indexing, KnowledgeBaseService};
use crate::services::ServiceRegistry;

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

/// 删除知识库（级联删除全部文档/分块/全文索引；同步丢弃内存向量索引）
#[tauri::command]
pub async fn kb_delete_base(
    state: State<'_, DbState>,
    registry: State<'_, ServiceRegistry>,
    id: String,
) -> Result<(), String> {
    data::delete_base(pool(&state), &id).await?;
    if let Some(svc) = registry.get("kb") {
        if let Some(kb) = svc.as_any().downcast_ref::<KnowledgeBaseService>() {
            kb.drop_index(&id);
        }
    }
    Ok(())
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
pub async fn kb_delete_document(
    state: State<'_, DbState>,
    document_id: String,
) -> Result<(), String> {
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

/// 绑定 embedding 渠道与模型（换绑即作废旧向量，需重新构建）
#[tauri::command]
pub async fn kb_bind_embedding(
    state: State<'_, DbState>,
    kb_id: String,
    channel_id: String,
    model: String,
) -> Result<KbBase, String> {
    embed::bind_embedding(pool(&state), &kb_id, &channel_id, &model).await
}

/// 解除 embedding 绑定（清空模型/渠道/维度与全部向量）
#[tauri::command]
pub async fn kb_unbind_embedding(
    state: State<'_, DbState>,
    kb_id: String,
) -> Result<KbBase, String> {
    embed::unbind_embedding(pool(&state), &kb_id).await
}

/// 发起索引构建（embedding + HNSW）：后台任务执行，返回任务 id（进度查 kb_list_tasks）
#[tauri::command]
pub async fn kb_build_index(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    kb_id: String,
) -> Result<String, String> {
    let p = pool(&state);
    if indexing::has_running_task(p, &kb_id).await? {
        return Err("该知识库已有进行中的构建任务".to_string());
    }
    // 校验绑定可用（提前暴露渠道问题，不留到后台）
    let (_ch, _model) = embed::bound_binding(p, &kb_id).await?;
    let task_id = indexing::create_task(p, &kb_id, "embed_index").await?;

    // 索引目录：数据目录/kb_index（HNSW 文件与 SQLite 同处，随库备份/迁移）
    let dir = crate::config::data_dir(&app)?.join("kb_index");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建索引目录失败: {e}"))?;
    let pool2 = p.clone();
    let dir2 = dir;
    let task_id2 = task_id.clone();
    tauri::async_runtime::spawn(async move {
        match indexing::run_embed_index_task(&pool2, &task_id2, &dir2, None).await {
            Ok(()) => eprintln!("[kb] 索引任务 {task_id2} 完成"),
            Err(e) => eprintln!("[kb] 索引任务 {task_id2} 失败: {e}"),
        }
    });
    Ok(task_id)
}

/// 知识库任务列表（构建进度轮询用）
#[tauri::command]
pub async fn kb_list_tasks(
    state: State<'_, DbState>,
    kb_id: String,
    limit: Option<i64>,
) -> Result<Vec<indexing::KbTask>, String> {
    indexing::list_tasks(pool(&state), &kb_id, limit.unwrap_or(20)).await
}

/// 检索知识库：mode = keyword（FTS5 BM25）| vector（embedding + HNSW）
/// | hybrid（两路检索 RRF 融合；前置校验同 vector）
#[tauri::command]
pub async fn kb_search(
    state: State<'_, DbState>,
    registry: State<'_, ServiceRegistry>,
    kb_id: String,
    query: String,
    mode: Option<String>,
    top_k: Option<i64>,
) -> Result<Vec<SearchHit>, String> {
    let p = pool(&state);
    let top_k = top_k.unwrap_or(8).clamp(1, 100) as usize;
    match mode.as_deref().unwrap_or("vector") {
        "keyword" => search::search_keyword(p, &kb_id, &query, top_k).await,
        "vector" => {
            let v = embed_query(p, &kb_id, &query).await?;
            let svc = registry.get("kb").ok_or("知识库服务不可用")?.clone();
            let kb: &KnowledgeBaseService = svc
                .as_any()
                .downcast_ref::<KnowledgeBaseService>()
                .ok_or("知识库服务类型异常")?;
            kb.search_vector(p, &kb_id, v, top_k).await
        }
        "hybrid" => {
            let v = embed_query(p, &kb_id, &query).await?;
            let svc = registry.get("kb").ok_or("知识库服务不可用")?.clone();
            let kb: &KnowledgeBaseService = svc
                .as_any()
                .downcast_ref::<KnowledgeBaseService>()
                .ok_or("知识库服务类型异常")?;
            // 两路各取 top_k×5（上限 100）再融合，融合排名更稳
            let fetch_k = (top_k * 5).min(100);
            let kw = search::search_keyword(p, &kb_id, &query, fetch_k).await?;
            let vec = kb.search_vector(p, &kb_id, v, fetch_k).await?;
            Ok(search::rrf_fuse(kw, vec, top_k))
        }
        other => Err(format!("未知检索模式: {other}（支持 keyword / vector / hybrid）")),
    }
}

/// 按库绑定把查询文本向量化（与索引向量同一渠道/模型，保证向量空间一致）
async fn embed_query(p: &sqlx::SqlitePool, kb_id: &str, query: &str) -> Result<Vec<f32>, String> {
    let (channel_id, model) = embed::bound_binding(p, kb_id).await?;
    let emb: Arc<dyn Embedder> = Arc::new(
        embed::ChannelEmbedder::from_channel(p, &channel_id, &model).await?,
    );
    let vecs = emb
        .embed_texts(&[query.trim()])
        .await
        .map_err(|e| format!("查询向量化失败: {e}"))?;
    vecs.into_iter().next().ok_or("embedding 返回为空".to_string())
}

// ---- RAG 问答：绑定 / 会话 / 问答 ----

/// 绑定问答渠道与模型（与 embedding 绑定解耦；不指定模型时取渠道默认）
#[tauri::command]
pub async fn kb_bind_answer(
    state: State<'_, DbState>,
    kb_id: String,
    channel_id: String,
    model: Option<String>,
) -> Result<KbBase, String> {
    rag::bind_answer(pool(&state), &kb_id, &channel_id, model.as_deref()).await
}

/// 解除问答绑定
#[tauri::command]
pub async fn kb_unbind_answer(
    state: State<'_, DbState>,
    kb_id: String,
) -> Result<KbBase, String> {
    rag::unbind_answer(pool(&state), &kb_id).await
}

/// 创建问答会话（标题缺省"新对话"，首问后自动摘要替换）
#[tauri::command]
pub async fn kb_create_session(
    state: State<'_, DbState>,
    kb_id: String,
    title: Option<String>,
) -> Result<KbSession, String> {
    rag::create_session(pool(&state), &kb_id, title.as_deref()).await
}

/// 知识库会话列表（按最近活跃排序）
#[tauri::command]
pub async fn kb_list_sessions(
    state: State<'_, DbState>,
    kb_id: String,
) -> Result<Vec<KbSession>, String> {
    rag::list_sessions(pool(&state), &kb_id).await
}

/// 删除会话（级联删除其全部消息）
#[tauri::command]
pub async fn kb_delete_session(
    state: State<'_, DbState>,
    session_id: String,
) -> Result<(), String> {
    rag::delete_session(pool(&state), &session_id).await
}

/// 会话消息列表（升序；sources 已解析为引用数组）
#[tauri::command]
pub async fn kb_list_messages(
    state: State<'_, DbState>,
    session_id: String,
) -> Result<Vec<KbMessage>, String> {
    rag::list_messages(pool(&state), &session_id).await
}

/// 发送问题并回答：hybrid 检索（索引未就绪自动降级关键词）+ 引用溯源落库
#[tauri::command]
pub async fn kb_chat(
    state: State<'_, DbState>,
    registry: State<'_, ServiceRegistry>,
    session_id: String,
    question: String,
) -> Result<ChatReply, String> {
    let svc = registry.get("kb").ok_or("知识库服务不可用")?.clone();
    let kb: &KnowledgeBaseService = svc
        .as_any()
        .downcast_ref::<KnowledgeBaseService>()
        .ok_or("知识库服务类型异常")?;
    kb.chat(pool(&state), &session_id, &question).await
}
