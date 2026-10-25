//! MCP 知识库工具集：工具定义表 + 参数校验 + 执行（薄包装 kb 数据/服务层）。
//!
//! 约束与边界：
//! - **只通过本注册表暴露**：渠道绑定类工具不暴露（涉及密钥与外部费用，归应用
//!   UI 管理）；删除类操作要求参数显式 `confirm: true`（LLM 工具执行不可撤销）；
//! - **留痕**：写操作与 kb_ask 执行后记 `admin_audit_logs`（mcp.tool_call）；
//! - 全部错误中文；参数类型错误回 JSON-RPC 校验失败（由协议层 -32602 之上包装，
//!   这里返回 Err 文本由协议层放入 isError——与 MCP 工具错误惯例一致）。

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use sqlx::SqlitePool;

use crate::adapters::ChatMessage;
use crate::commands::util::record_admin_audit;
use crate::services::kb::data;
use crate::services::kb::embed;
use crate::services::kb::rag::{normalize_question, AskOutcome};
use crate::services::kb::search::SearchHit;
use crate::services::kb::{indexing, KnowledgeBaseService};

use super::config::{McpConfig, ToolKind};
use super::protocol::ToolSet;

/// 工具执行上下文：连接池 + 内存索引缓存实例（懒加载，与 kb 服务同构）+ 索引目录
pub struct ToolCtx {
    pub pool: SqlitePool,
    pub kb: KnowledgeBaseService,
    pub index_dir: PathBuf,
}

impl ToolCtx {
    pub fn new(pool: SqlitePool, index_dir: PathBuf) -> Self {
        Self {
            pool,
            kb: KnowledgeBaseService::default(),
            index_dir,
        }
    }
}

/// 工具元信息（tools/list 数据来源；执行分发见 [`Tools::run_tool`]）
struct ToolDef {
    name: &'static str,
    description: &'static str,
    /// inputSchema（JSON Schema draft-07 子集；properties 每个带 type）
    schema: Value,
    /// 必填参数名
    required: &'static [&'static str],
    /// 是否记 admin 审计（写操作与问答记，只读查询不记）
    audit: bool,
}

/// 风险类别（单一来源：按工具名归类，避免 13 处重复声明）
pub fn kind_of(name: &str) -> ToolKind {
    match name {
        "kb_delete_base" | "kb_delete_document" => ToolKind::Dangerous,
        "kb_create_base" | "kb_update_base" | "kb_import_text" | "kb_import_file"
        | "kb_build_index" => ToolKind::Write,
        _ => ToolKind::Read,
    }
}

/// 是否会产生外部费用（调用上游模型 / embedding 渠道）
pub fn is_costly(name: &str) -> bool {
    matches!(name, "kb_ask" | "kb_build_index")
}

/// 参数说明（前端工具详情表用；从 inputSchema 提取）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpParamInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub required: bool,
    pub description: String,
    pub enum_values: Vec<String>,
}

/// 工具完整信息（MCP 服务页数据源）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub kind: ToolKind,
    pub kind_label: String,
    /// 会产生外部费用（问答/建索引）
    pub costly: bool,
    /// 是否记入管理审计
    pub audit: bool,
    pub required: Vec<String>,
    pub params: Vec<McpParamInfo>,
    /// 原始 inputSchema（前端展示/复制）
    pub schema: Value,
    /// 当前是否启用（设置 services.mcp.tools.<name>.enabled）
    pub enabled: bool,
}

/// 全部工具名（命令层校验用）
pub fn known_names() -> Vec<&'static str> {
    defs().into_iter().map(|d| d.name).collect()
}

/// 工具清单（含风险类别、参数说明与启用状态）
pub fn tool_infos(cfg: &McpConfig) -> Vec<McpToolInfo> {
    defs()
        .into_iter()
        .map(|d| {
            let kind = kind_of(d.name);
            let props = d.schema.get("properties").and_then(Value::as_object);
            let params = props
                .map(|m| {
                    m.iter()
                        .map(|(name, v)| McpParamInfo {
                            name: name.clone(),
                            type_name: v
                                .get("type")
                                .and_then(Value::as_str)
                                .unwrap_or("any")
                                .to_string(),
                            required: d.required.contains(&name.as_str()),
                            description: v
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            enum_values: v
                                .get("enum")
                                .and_then(Value::as_array)
                                .map(|a| {
                                    a.iter()
                                        .filter_map(Value::as_str)
                                        .map(str::to_string)
                                        .collect()
                                })
                                .unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            McpToolInfo {
                name: d.name.to_string(),
                description: d.description.to_string(),
                kind,
                kind_label: kind.label().to_string(),
                costly: is_costly(d.name),
                audit: d.audit,
                required: d.required.iter().map(|s| s.to_string()).collect(),
                params,
                schema: d.schema,
                enabled: cfg.tool_enabled(d.name),
            }
        })
        .collect()
}

fn schema(props: Value, required: &'static [&'static str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

/// 工具清单（静态声明；每项对应 kb 命令层同一语义的函数）
fn defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "list_knowledge_bases",
            description: "列出全部知识库（含文档数、索引状态与绑定摘要），用于发现可用的库",
            schema: schema(json!({}), &[]),
            required: &[],
            audit: false,
        },
        ToolDef {
            name: "kb_create_base",
            description: "新建知识库（本地文档检索与问答的容器）",
            schema: schema(
                json!({
                    "name": { "type": "string", "description": "库名称（唯一）" },
                    "description": { "type": "string", "description": "一句话描述" },
                    "chunkSize": { "type": "integer", "description": "分块大小字符，默认 800（100~4000）" },
                    "chunkOverlap": { "type": "integer", "description": "分块重叠字符，默认 150" }
                }),
                &["name"],
            ),
            required: &["name"],
            audit: true,
        },
        ToolDef {
            name: "kb_update_base",
            description: "更新知识库（名称/描述/分块参数/状态；只传要改的字段）",
            schema: schema(
                json!({
                    "id": { "type": "string", "description": "库 id" },
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "chunkSize": { "type": "integer" },
                    "chunkOverlap": { "type": "integer" },
                    "status": { "type": "string", "enum": ["active", "disabled"] }
                }),
                &["id"],
            ),
            required: &["id"],
            audit: true,
        },
        ToolDef {
            name: "kb_delete_base",
            description: "删除知识库（级联删除全部文档/分块/索引/会话）。不可撤销，必须显式 confirm=true",
            schema: schema(
                json!({
                    "id": { "type": "string", "description": "库 id" },
                    "confirm": { "type": "boolean", "description": "确认删除，必须为 true" }
                }),
                &["id", "confirm"],
            ),
            required: &["id", "confirm"],
            audit: true,
        },
        ToolDef {
            name: "kb_list_documents",
            description: "列出知识库内文档（含分块数/状态）",
            schema: schema(json!({ "kbId": { "type": "string" } }), &["kbId"]),
            required: &["kbId"],
            audit: false,
        },
        ToolDef {
            name: "kb_import_text",
            description: "把文本导入知识库（md/txt；md 支持章节级溯源分块）",
            schema: schema(
                json!({
                    "kbId": { "type": "string" },
                    "title": { "type": "string", "description": "文档标题" },
                    "format": { "type": "string", "enum": ["md", "markdown", "txt"], "description": "默认 txt" },
                    "text": { "type": "string", "description": "文档内容" }
                }),
                &["kbId", "title", "text"],
            ),
            required: &["kbId", "title", "text"],
            audit: true,
        },
        ToolDef {
            name: "kb_import_file",
            description: "导入宿主机器上的 .md/.txt 文件（path 为绝对路径，仅本机宿主可用）",
            schema: schema(
                json!({
                    "kbId": { "type": "string" },
                    "path": { "type": "string", "description": "文件绝对路径" }
                }),
                &["kbId", "path"],
            ),
            required: &["kbId", "path"],
            audit: true,
        },
        ToolDef {
            name: "kb_delete_document",
            description: "删除文档及其全部分块（不可撤销，必须显式 confirm=true）",
            schema: schema(
                json!({
                    "documentId": { "type": "string" },
                    "confirm": { "type": "boolean" }
                }),
                &["documentId", "confirm"],
            ),
            required: &["documentId", "confirm"],
            audit: true,
        },
        ToolDef {
            name: "kb_list_chunks",
            description: "列出文档的分块（含章节溯源 heading；limit 默认 200）",
            schema: schema(
                json!({
                    "documentId": { "type": "string" },
                    "limit": { "type": "integer", "description": "最多返回条数" }
                }),
                &["documentId"],
            ),
            required: &["documentId"],
            audit: false,
        },
        ToolDef {
            name: "kb_search",
            description: "检索知识库分块。mode：keyword（字面 BM25）| vector（语义，需向量索引）| hybrid（两路融合，默认）。返回按相关度排序的分块与溯源",
            schema: schema(
                json!({
                    "kbId": { "type": "string" },
                    "query": { "type": "string", "description": "查询内容" },
                    "mode": { "type": "string", "enum": ["keyword", "vector", "hybrid"], "description": "默认 hybrid" },
                    "topK": { "type": "integer", "description": "返回条数 1~100，默认 8" }
                }),
                &["kbId", "query"],
            ),
            required: &["kbId", "query"],
            audit: false,
        },
        ToolDef {
            name: "kb_ask",
            description: "基于知识库内容回答问题（RAG：检索引用材料 → 调用库绑定的问答渠道 → 返回回答与 [n] 引用）。需该库已绑定问答渠道；无向量索引时自动降级关键词检索。回答未经会话落库，多轮请自行携带上下文循环调用",
            schema: schema(
                json!({
                    "kbId": { "type": "string" },
                    "question": { "type": "string", "description": "要问的问题" }
                }),
                &["kbId", "question"],
            ),
            required: &["kbId", "question"],
            audit: true,
        },
        ToolDef {
            name: "kb_build_index",
            description: "为知识库构建向量索引（embedding 全部缺向量分块 → 建 HNSW 图）。异步任务，返回 taskId，进度查 kb_list_tasks。文档增删后需重建",
            schema: schema(json!({ "kbId": { "type": "string" } }), &["kbId"]),
            required: &["kbId"],
            audit: true,
        },
        ToolDef {
            name: "kb_list_tasks",
            description: "知识库最近任务（索引构建进度/结果，limit 默认 10）",
            schema: schema(
                json!({
                    "kbId": { "type": "string" },
                    "limit": { "type": "integer" }
                }),
                &["kbId"],
            ),
            required: &["kbId"],
            audit: false,
        },
    ]
}

/// 工具注册表实现（协议层 [`ToolSet`] 的唯一消费者）
pub struct Tools {
    ctx: Arc<ToolCtx>,
}

impl Tools {
    pub fn new(pool: SqlitePool, index_dir: PathBuf) -> Self {
        Self {
            ctx: Arc::new(ToolCtx::new(pool, index_dir)),
        }
    }

    fn find(name: &str) -> Option<ToolDef> {
        defs().into_iter().find(|d| d.name == name)
    }
}

#[async_trait::async_trait]
impl ToolSet for Tools {
    fn list_tools(&self) -> Vec<Value> {
        defs()
            .into_iter()
            .map(|d| {
                let kind = kind_of(d.name);
                json!({
                    "name": d.name,
                    "description": d.description,
                    "inputSchema": d.schema,
                    // MCP 规范注解：让外部客户端也能感知风险
                    "annotations": {
                        "title": d.name,
                        "readOnlyHint": kind == ToolKind::Read,
                        "destructiveHint": kind == ToolKind::Dangerous,
                        "idempotentHint": false,
                        "openWorldHint": is_costly(d.name),
                    }
                })
            })
            .collect()
    }

    async fn run_tool(&self, name: &str, args: &Value) -> Result<String, String> {
        let Some(def) = Self::find(name) else {
            return Err(format!("未知工具: {name}"));
        };
        // 准入：单工具开关 + 只读模式（每次调用实时读设置，改完立即生效）
        let cfg = McpConfig::load(&self.ctx.pool).await;
        cfg.check_tool(name, kind_of(name))?;
        validate_args(&def, args)?;
        // 工具级语义校验（confirm 等）
        match name {
            "kb_delete_base" | "kb_delete_document" => require_confirm(args)?,
            _ => {}
        }

        // 统一的审计封装：只记写操作与问答（detail 截断，防日志膨胀）
        if def.audit {
            let detail: String = args.to_string().chars().take(200).collect();
            let kb_id = args.get("kbId").or_else(|| args.get("id")).and_then(Value::as_str);
            let _ = record_admin_audit(
                &self.ctx.pool,
                "mcp.tool_call",
                "kb",
                kb_id,
                Some(&format!("{name} {detail}")),
            )
            .await;
        }

        let text = self.execute(name, args).await?;
        Ok(text)
    }
}

impl Tools {
    /// 具体执行分发：每项调用 kb 数据/服务层既有函数（与命令层同一语义）。
    async fn execute(&self, name: &str, args: &Value) -> Result<String, String> {
        let pool = &self.ctx.pool;
        let kb = &self.ctx.kb;
        let to_text = |v: &Value| -> Result<String, String> {
            serde_json::to_string(v).map_err(|e| format!("序列化结果失败: {e}"))
        };

        match name {
            "list_knowledge_bases" => {
                let bases = data::list_bases(pool).await?;
                let list: Vec<Value> = bases
                    .iter()
                    .map(|b| {
                        json!({
                            "id": b.id,
                            "name": b.name,
                            "description": b.description,
                            "docCount": b.doc_count,
                            "status": b.status,
                            "indexStatus": b.index_status,
                            "chunkSize": b.chunk_size,
                            "embeddingModel": b.embedding_model,
                            "answerModel": b.answer_model,
                        })
                    })
                    .collect();
                to_text(&json!({ "bases": list }))
            }
            "kb_create_base" => {
                let name = req_str(args, "name")?;
                let b = data::create_base(
                    pool,
                    &name,
                    &opt_str(args, "description").unwrap_or_default(),
                    opt_i64(args, "chunkSize").unwrap_or(800),
                    opt_i64(args, "chunkOverlap").unwrap_or(150),
                )
                .await?;
                to_text(&json!({ "base": { "id": b.id, "name": b.name, "indexStatus": b.index_status } }))
            }
            "kb_update_base" => {
                let id = req_str(args, "id")?;
                let b = data::update_base(
                    pool,
                    &id,
                    opt_str(args, "name").as_deref(),
                    opt_str(args, "description").as_deref(),
                    opt_i64(args, "chunkSize"),
                    opt_i64(args, "chunkOverlap"),
                    opt_str(args, "status").as_deref(),
                )
                .await?;
                to_text(&json!({ "base": { "id": b.id, "name": b.name, "status": b.status } }))
            }
            "kb_delete_base" => {
                let id = req_str(args, "id")?;
                data::delete_base(pool, &id).await?;
                kb.drop_index(&id); // 同步丢弃内存索引缓存
                to_text(&json!({ "deleted": true, "id": id }))
            }
            "kb_list_documents" => {
                let kb_id = req_str(args, "kbId")?;
                let docs = data::list_documents(pool, &kb_id).await?;
                let list: Vec<Value> = docs
                    .iter()
                    .map(|d| {
                        json!({
                            "id": d.id,
                            "title": d.title,
                            "format": d.format,
                            "charCount": d.char_count,
                            "chunkCount": d.chunk_count,
                            "status": d.status,
                            "error": d.error,
                        })
                    })
                    .collect();
                to_text(&json!({ "documents": list }))
            }
            "kb_import_text" => {
                let kb_id = req_str(args, "kbId")?;
                let title = req_str(args, "title")?;
                let format = opt_str(args, "format").unwrap_or_else(|| "txt".to_string());
                let text = req_str(args, "text")?;
                let d = data::import_text(pool, &kb_id, &title, &format, &text).await?;
                to_text(&json!({ "document": { "id": d.id, "title": d.title, "chunkCount": d.chunk_count, "status": d.status } }))
            }
            "kb_import_file" => {
                let kb_id = req_str(args, "kbId")?;
                let path = req_str(args, "path")?;
                let d = data::import_file(pool, &kb_id, &path).await?;
                to_text(&json!({ "document": { "id": d.id, "title": d.title, "chunkCount": d.chunk_count, "status": d.status } }))
            }
            "kb_delete_document" => {
                let document_id = req_str(args, "documentId")?;
                data::delete_document(pool, &document_id).await?;
                to_text(&json!({ "deleted": true, "documentId": document_id }))
            }
            "kb_list_chunks" => {
                let document_id = req_str(args, "documentId")?;
                let mut chunks = data::list_chunks(pool, &document_id).await?;
                let limit = (opt_i64(args, "limit").unwrap_or(200)).max(1) as usize;
                chunks.truncate(limit);
                let list: Vec<Value> = chunks
                    .iter()
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "seq": c.seq,
                            "heading": c.heading,
                            "chars": c.chars,
                            "content": c.content,
                        })
                    })
                    .collect();
                to_text(&json!({ "chunks": list }))
            }
            "kb_search" => {
                let kb_id = req_str(args, "kbId")?;
                let query = req_str(args, "query")?;
                let mode = opt_str(args, "mode").unwrap_or_else(|| "hybrid".to_string());
                let top_k = opt_i64(args, "topK").unwrap_or(8) as usize;
                let hits: Vec<SearchHit> = kb.search_any(pool, &kb_id, &query, &mode, top_k).await?;
                let list: Vec<Value> = hits
                    .iter()
                    .map(|h| {
                        json!({
                            "chunkId": h.chunk_id,
                            "documentId": h.document_id,
                            "title": h.title,
                            "heading": h.heading,
                            "seq": h.seq,
                            "content": h.content,
                            "score": h.score,
                            "vecScore": h.vec_score,
                            "kwScore": h.kw_score,
                        })
                    })
                    .collect();
                to_text(&json!({ "hits": list }))
            }
            "kb_ask" => {
                let kb_id = req_str(args, "kbId")?;
                let question = normalize_question(&req_str(args, "question")?)?;
                let empty_history: Vec<ChatMessage> = Vec::new();
                let out: AskOutcome = kb.answer(pool, &kb_id, &question, &empty_history).await?;
                to_text(&serde_json::to_value(&out).map_err(|e| e.to_string())?)
            }
            "kb_build_index" => {
                let kb_id = req_str(args, "kbId")?;
                if indexing::has_running_task(pool, &kb_id).await? {
                    return Err("该知识库已有进行中的构建任务".to_string());
                }
                // 提前暴露绑定问题（与命令层 kb_build_index 同语义）
                embed::bound_binding(pool, &kb_id).await?;
                let task_id = indexing::create_task(pool, &kb_id, "embed_index").await?;
                let pool2 = pool.clone();
                let dir = self.ctx.index_dir.clone();
                let task_id2 = task_id.clone();
                tokio::spawn(async move {
                    match indexing::run_embed_index_task(&pool2, &task_id2, &dir, None).await {
                        Ok(()) => eprintln!("[mcp] 索引任务 {task_id2} 完成"),
                        Err(e) => eprintln!("[mcp] 索引任务 {task_id2} 失败: {e}"),
                    }
                });
                to_text(&json!({ "taskId": task_id }))
            }
            "kb_list_tasks" => {
                let kb_id = req_str(args, "kbId")?;
                let limit = opt_i64(args, "limit").unwrap_or(10);
                let tasks = indexing::list_tasks(pool, &kb_id, limit).await?;
                let list: Vec<Value> = tasks
                    .iter()
                    .map(|t| {
                        json!({
                            "id": t.id,
                            "taskType": t.task_type,
                            "status": t.status,
                            "progress": t.progress,
                            "doneItems": t.done_items,
                            "totalItems": t.total_items,
                            "errorMessage": t.error_message,
                            "createdAt": t.created_at,
                            "completedAt": t.completed_at,
                        })
                    })
                    .collect();
                to_text(&json!({ "tasks": list }))
            }
            _ => Err(format!("未知工具: {name}")),
        }
    }
}

// ---- 参数校验（JSON Schema draft-07 子集：type/required/enum）----

/// 校验：必填存在 + 类型匹配（string/integer/number/boolean/object；未知键宽容忽略）
fn validate_args(def: &ToolDef, args: &Value) -> Result<(), String> {
    if !args.is_object() {
        return Err(format!("工具 {} 的参数应为 JSON 对象", def.name));
    }
    let obj = args.as_object().expect("上面已校验");
    for name in def.required {
        if !obj.contains_key(*name) {
            return Err(format!("缺少必填参数: {name}"));
        }
    }
    let props = &def.schema["properties"];
    for (key, val) in obj {
        let Some(t) = props.get(key).and_then(|p| p.get("type")).and_then(Value::as_str) else {
            continue; // 未声明类型/未知键：宽容
        };
        let ok = match t {
            "string" => val.is_string(),
            "integer" => val.is_i64() || val.is_u64(),
            "number" => val.is_f64() || val.is_i64(),
            "boolean" => val.is_boolean(),
            "object" => val.is_object(),
            "array" => val.is_array(),
            "null" => val.is_null(),
            _ => true,
        };
        if !ok {
            return Err(format!("参数 {key} 应为 {t} 类型"));
        }
        // enum 约束
        if let Some(enumv) = props.get(key).and_then(|p| p.get("enum")) {
            if let Some(arr) = enumv.as_array() {
                if !arr.iter().any(|e| e == val) {
                    let allowed: Vec<String> = arr.iter().filter_map(|e| e.as_str().map(String::from)).collect();
                    return Err(format!("参数 {key} 取值非法，允许: {}", allowed.join(" / ")));
                }
            }
        }
    }
    Ok(())
}

/// 删除类工具的显式确认（不可撤销操作的形式约束）
fn require_confirm(args: &Value) -> Result<(), String> {
    if args.get("confirm").and_then(Value::as_bool) != Some(true) {
        return Err("该操作不可撤销：请在参数中显式传入 confirm=true 确认".to_string());
    }
    Ok(())
}

// ---- 参数提取 helpers ----

fn req_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("参数 {key} 缺失或为空"))
}

fn opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn opt_i64(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(Value::as_i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::kb::{data as kdata, rag as krag, schema};
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        schema::apply_schema_direct(&pool).await.unwrap();
        sqlx::query(
            "CREATE TABLE channels (id TEXT PRIMARY KEY, name TEXT, type TEXT, protocol TEXT NOT NULL DEFAULT '', base_url TEXT, \
             model TEXT, status TEXT, api_key_encrypted TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // 配置读取（services.mcp.*）与准入判定需要 settings 表
        sqlx::query("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    async fn set(pool: &SqlitePool, key: &str, value: &str) {
        sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)")
            .bind(key)
            .bind(value)
            .execute(pool)
            .await
            .unwrap();
    }

    fn new_tools(pool: SqlitePool) -> Tools {
        let dir = std::env::temp_dir().join(format!("kb-mcp-tools-{}", std::process::id()));
        Tools::new(pool, dir)
    }

    fn args(v: serde_json::Value) -> serde_json::Value {
        v
    }

    #[test]
    fn tool_infos_expose_kind_params_and_required() {
        let cfg = McpConfig::default();
        let infos = tool_infos(&cfg);
        assert_eq!(infos.len(), known_names().len());
        assert!(infos.iter().all(|t| t.enabled), "缺省全部启用");

        let search = infos.iter().find(|t| t.name == "kb_search").unwrap();
        assert_eq!(search.kind, ToolKind::Read);
        assert_eq!(search.kind_label, "只读");
        assert!(!search.costly && !search.audit);
        assert_eq!(search.required, vec!["kbId", "query"]);
        let mode = search.params.iter().find(|p| p.name == "mode").unwrap();
        assert_eq!(mode.type_name, "string");
        assert_eq!(mode.enum_values, vec!["keyword", "vector", "hybrid"]);
        assert!(!mode.required);
        let kb_id = search.params.iter().find(|p| p.name == "kbId").unwrap();
        assert!(kb_id.required);

        let del = infos.iter().find(|t| t.name == "kb_delete_base").unwrap();
        assert_eq!(del.kind, ToolKind::Dangerous);
        assert!(del.audit, "删除要留痕");
        let ask = infos.iter().find(|t| t.name == "kb_ask").unwrap();
        assert!(ask.costly, "问答会调用上游模型，标为有外部费用");
        let import = infos.iter().find(|t| t.name == "kb_import_file").unwrap();
        assert_eq!(import.kind, ToolKind::Write);

        // 禁用后信息里的 enabled 立即反映
        let cfg = McpConfig {
            disabled_tools: ["kb_search".to_string()].into_iter().collect(),
            ..McpConfig::default()
        };
        let infos = tool_infos(&cfg);
        assert!(!infos.iter().find(|t| t.name == "kb_search").unwrap().enabled);
    }

    #[test]
    fn dangerous_kind_matches_confirm_required_tools() {
        // 危险类 = 必须显式 confirm 的工具（两者必须同步，防止新增工具漏配）
        let dangerous: Vec<&str> = known_names()
            .into_iter()
            .filter(|n| kind_of(n) == ToolKind::Dangerous)
            .collect();
        assert_eq!(dangerous, vec!["kb_delete_base", "kb_delete_document"]);
    }

    #[tokio::test]
    async fn disabled_tool_and_read_only_are_enforced_in_run_tool() {
        let pool = test_pool().await;
        let tools = new_tools(pool.clone());

        set(&pool, "services.mcp.tools.list_knowledge_bases.enabled", "false").await;
        let err = tools.run_tool("list_knowledge_bases", &args(json!({}))).await.unwrap_err();
        assert!(err.contains("禁用"), "{err}");

        set(&pool, "services.mcp.readOnly", "true").await;
        let err = tools
            .run_tool("kb_import_text", &args(json!({ "kbId": "kb_x", "title": "t", "text": "x" })))
            .await
            .unwrap_err();
        assert!(err.contains("只读模式"), "{err}");
        // 只读模式不拦只读工具
        let r = tools.run_tool("kb_list_documents", &args(json!({ "kbId": "kb_x" }))).await;
        assert!(r.is_ok() || !r.unwrap_err().contains("只读模式"));
    }

    #[tokio::test]
    async fn basic_management_and_keyword_search_roundtrip() {
        let pool = test_pool().await;
        let tools = new_tools(pool.clone());

        // 空列表
        let r = tools.run_tool("list_knowledge_bases", &args(json!({}))).await.unwrap();
        assert!(r.contains("\"bases\":[]"), "空库列表: {r}");

        // 建库 → 导入 → 检索（keyword）
        let r = tools
            .run_tool(
                "kb_create_base",
                &args(json!({ "name": "网关笔记", "description": "测试", "chunkSize": 800 })),
            )
            .await
            .unwrap();
        let created: Value = serde_json::from_str(&r).unwrap();
        let kb_id = created["base"]["id"].as_str().unwrap().to_string();

        tools
            .run_tool(
                "kb_import_text",
                &args(json!({ "kbId": kb_id, "title": "故障转移", "format": "txt",
                              "text": "网关在主渠道失败时按顺序转移到备选渠道，4xx 错误不转移。" })),
            )
            .await
            .unwrap();

        let r = tools
            .run_tool("kb_search", &args(json!({ "kbId": kb_id, "query": "备选渠道", "mode": "keyword", "topK": 5 })))
            .await
            .unwrap();
        let hits: Value = serde_json::from_str(&r).unwrap();
        assert_eq!(hits["hits"].as_array().unwrap().len(), 1);
        assert_eq!(hits["hits"][0]["title"], "故障转移");

        // 文档列表可见
        let r = tools.run_tool("kb_list_documents", &args(json!({ "kbId": kb_id }))).await.unwrap();
        assert!(r.contains("故障转移"));

        // 参数校验：缺 kbId → Err；非法 mode 枚举 → Err
        assert!(tools.run_tool("kb_search", &args(json!({ "query": "x" }))).await.is_err());
        assert!(
            tools
                .run_tool("kb_search", &args(json!({ "kbId": kb_id, "query": "x", "mode": "wat" })))
                .await
                .is_err()
        );

        // 删除文档需要 confirm；带 confirm 后成功
        let doc_id = hits["hits"][0]["documentId"].as_str().unwrap();
        let err = tools
            .run_tool("kb_delete_document", &args(json!({ "documentId": doc_id })))
            .await
            .unwrap_err();
        assert!(err.contains("confirm"), "缺 confirm 应拒绝: {err}");
        tools
            .run_tool("kb_delete_document", &args(json!({ "documentId": doc_id, "confirm": true })))
            .await
            .unwrap();

        // 删除库（confirm）
        tools
            .run_tool("kb_delete_base", &args(json!({ "id": kb_id, "confirm": true })))
            .await
            .unwrap();
        let r = tools.run_tool("list_knowledge_bases", &args(json!({}))).await.unwrap();
        assert!(r.contains("\"bases\":[]"));
    }

    #[tokio::test]
    async fn kb_ask_guides_when_unbound_and_answers_via_mock() {
        let pool = test_pool().await;
        // mock 问答渠道（仅 chat 端点；检索走关键词降级，无需 embedding）
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(x) => x,
                    Err(_) => break,
                };
                tokio::spawn(async move {
                    let _ = serve_mock_chat(&mut stream).await;
                });
            }
        });
        let base_url = format!("http://127.0.0.1:{port}/v1");

        // 建库 + 导入 + 渠道 + 绑定问答
        let b = kdata::create_base(&pool, "问答库", "", 800, 150).await.unwrap();
        kdata::import_text(&pool, &b.id, "透传", "txt", "网关只改写 URL 与鉴权头，请求体原样转发到上游。")
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO channels (id, name, type, base_url, model, status, api_key_encrypted) \
             VALUES ('ch_ans', 'mock', 'OpenAI', ?1, 'mock-chat', 'active', 'k')",
        )
        .bind(&base_url)
        .execute(&pool)
        .await
        .unwrap();
        krag::bind_answer(&pool, &b.id, "ch_ans", None).await.unwrap();

        let tools = new_tools(pool.clone());
        // 未绑定问答渠道的另一个库 → 引导错误
        let b2 = kdata::create_base(&pool, "无绑定库", "", 800, 150).await.unwrap();
        let err = tools
            .run_tool("kb_ask", &args(json!({ "kbId": b2.id, "question": "你好" })))
            .await
            .unwrap_err();
        assert!(err.contains("绑定问答渠道"), "应引导绑定: {err}");

        // kb_ask 全链路（mock 回答）。注：关键词(trigram phrase)检索要求查询与
        // 文档存在连续重叠子串，故用文档原文片段作为问题以稳定命中。
        let r = tools
            .run_tool(
                "kb_ask",
                &args(json!({ "kbId": b.id, "question": "请求体原样转发到上游" })),
            )
            .await
            .unwrap();
        let out: Value = serde_json::from_str(&r).unwrap();
        assert_eq!(out["answer"], "mock 回答：原样透传。");
        assert_eq!(out["citations"].as_array().unwrap().len(), 1);
        assert_eq!(out["model"], "mock-chat");
        assert!(out["latencyMs"].as_u64().unwrap_or(0) < 60_000);
        // 问答不落会话（无状态工具调用）
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM kb_conversations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "kb_ask 不应产生会话消息");
    }

    /// 简化 mock：/v1/chat/completions 固定应答（只按路径分发）
    async fn serve_mock_chat(stream: &mut tokio::net::TcpStream) -> std::io::Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut raw = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            stream.read_exact(&mut byte).await?;
            raw.push(byte[0]);
            if raw.len() >= 4 && &raw[raw.len() - 4..] == b"\r\n\r\n" {
                break;
            }
        }
        let head = String::from_utf8_lossy(&raw);
        let clen: usize = head
            .lines()
            .find_map(|l| {
                let mut it = l.split(':');
                if it.next().unwrap_or("").trim().eq_ignore_ascii_case("content-length") {
                    it.next().and_then(|v| v.trim().parse().ok())
                } else {
                    None
                }
            })
            .unwrap_or(0);
        let mut body = vec![0u8; clen];
        if clen > 0 {
            stream.read_exact(&mut body).await?;
        }
        let payload = serde_json::to_vec(&serde_json::json!({
            "id": "cmpl-mock",
            "object": "chat.completion",
            "model": "mock-chat",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "mock 回答：原样透传。" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 6, "total_tokens": 16 }
        }))
        .unwrap();
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        );
        stream.write_all(header.as_bytes()).await?;
        stream.write_all(&payload).await?;
        stream.shutdown().await?;
        Ok(())
    }
}
