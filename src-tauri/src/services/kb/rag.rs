//! RAG 问答：绑定 → 会话 → 检索（策略层见 `retrieve.rs`）→ 引用上下文
//! → 渠道 chat（复用适配器）→ user/assistant 成对落库。
//!
//! 关键决策：
//! - **问答绑定与 embedding 绑定解耦**（answer_channel_id/answer_model，
//!   0004 字段）：问答模型由 `adapters::create_adapter` 的 chat 契约调用，
//!   检索仍走 embedding 绑定（hybrid 时才需要，缺失即降级关键词）；
//! - **成功才落库**：渠道调用失败不写任何行，前端可安全重发（避免半截会话）；
//! - 引用溯源：正文以 `[n]` 编号对应 `assistant.sources` JSON，可逐条跳转文档；
//! - 检索策略（RRF 权重、相关度阈值、每文档去重、邻居扩展）全部在
//!   `retrieve.rs`，参数由 settings 覆盖；
//! - 用量记 `kb_conversations.tokens_used`（不经代理转发，不进 requests 记账表）；
//! - 问答内容不过安全脱敏引擎：知识库是本地自有内容，位于信任边界内
//!   （安全引擎聚焦代理流量的外部输入/输出）。
//! - 明确不做：流式（前端轮再做）、rerank 模型、历史摘要。

use std::time::Instant;

use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::adapters::{create_adapter, ChatMessage, ChatRequest};
use crate::commands::util::{new_id, now_iso};

use super::data::{self, KbBase};
use super::retrieve;
use super::search::SearchHit;
use super::KnowledgeBaseService;

// ---- 常量（上下文组装预算与检索档位）----

/// 系统提示：只依据引用材料回答、以 [n] 标注、缺料明说
pub(crate) const KB_SYSTEM_PROMPT: &str =
    "你是本地知识库的问答助手。请只依据「引用材料」回答用户的问题，引用出处用 [n] 标注；\
     引用材料中没有的信息请明确说明「知识库中未找到相关信息」，不要编造。";

/// 引用材料总预算（字符）
pub(crate) const CONTEXT_MAX_CHARS: usize = 6000;
/// 单块引用内容上限（超出截断，防单块吞掉全部预算）
const CHUNK_EXCERPT_MAX_CHARS: usize = 1500;
/// 会话历史保留条数（≤ 3 轮往返；再早的由引用上下文承载）
const HISTORY_LIMIT: usize = 6;
/// 历史消息单条内容截断（防超大答案撑爆上下文）
const HISTORY_MSG_MAX_CHARS: usize = 2000;
/// 单问字符上限（超出截断）
const MAX_QUESTION_CHARS: usize = 4096;
/// 会话自动标题字符上限
const TITLE_MAX_CHARS: usize = 30;
/// 会话默认标题（首问后自动替换为摘要）
const DEFAULT_TITLE: &str = "新对话";

// ---- DTO（snake_case 匹配列 / 表；序列化 camelCase 给前端）----

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbSession {
    pub id: String,
    pub kb_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

/// 引用条目（正文 [n] ↔ sources JSON 一一对应，可跳转原文档）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    /// 正文引用序号（1 起，与「引用材料」[n] 对齐）
    pub n: usize,
    pub chunk_id: i64,
    pub document_id: String,
    pub title: String,
    pub heading: String,
}

/// 会话消息（sources 为引用的数组形态；user 行为空数组）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub sources: Vec<Citation>,
    pub model: Option<String>,
    pub tokens_used: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReply {
    pub session_id: String,
    pub answer: String,
    /// 引用条目（与回答正文 [n] 对应）
    pub citations: Vec<Citation>,
    pub model: String,
    pub tokens_used: i64,
    /// 本次问答耗时（毫秒）
    pub latency_ms: u64,
}

/// 无状态问答结果（MCP 等无会话调用方用；由 [`KnowledgeBaseService::answer`]
/// 产出，不落库——审计与留痕由调用方负责）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskOutcome {
    pub answer: String,
    /// 引用条目（与回答正文 [n] 对应）
    pub citations: Vec<Citation>,
    pub model: String,
    pub tokens_used: i64,
    /// 本次问答耗时（毫秒）
    pub latency_ms: u64,
}

// ---- 问答渠道绑定（0004 字段，与 embedding 绑定解耦）----

/// 绑定问答渠道与模型。校验：渠道存在、active、类型支持 chat（同适配器工厂
/// 约束，不发网络请求）；模型未显式指定时取渠道默认模型。
pub async fn bind_answer(
    pool: &SqlitePool,
    kb_id: &str,
    channel_id: &str,
    model: Option<&str>,
) -> Result<KbBase, String> {
    if data::read_base(pool, kb_id).await?.is_none() {
        return Err("知识库不存在".to_string());
    }
    let row = sqlx::query_as::<_, (String, String, String, String, String)>(
        "SELECT type, protocol, model, models, status FROM channels WHERE id = ?1",
    )
    .bind(channel_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;
    let Some((ctype, cprotocol, channel_model, channel_models, status)) = row else {
        return Err(format!("问答渠道不存在：{channel_id}"));
    };
    if status != "active" {
        return Err(format!("问答渠道已停用：{channel_id}"));
    }
    // 协议须能被适配器工厂接受（Claude/Gemini 原生协议尚未实现会在绑定期即暴露）
    let protocol = crate::adapters::effective_protocol(&ctype, &cprotocol);
    create_adapter(&protocol, "http://placeholder.invalid", "k")
        .map_err(|e| format!("问答渠道协议暂不支持问答（{ctype}）: {e}"))?;
    let models = crate::models::channel_models(&channel_models, &channel_model);
    let model = match model.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => m.to_string(),
        None => {
            if models.is_empty() {
                return Err(format!(
                    "渠道 {channel_id} 未配置默认模型，请显式指定问答模型"
                ));
            }
            if models.len() > 1 {
                return Err(format!(
                    "渠道 {channel_id} 配置了多个模型（{}），请显式指定问答模型",
                    models.join(" / ")
                ));
            }
            models[0].clone()
        }
    };
    let now = now_iso();
    sqlx::query(
        "UPDATE kb_knowledge_bases SET answer_channel_id=?1, answer_model=?2, updated_at=?3 \
         WHERE id=?4",
    )
    .bind(channel_id)
    .bind(&model)
    .bind(&now)
    .bind(kb_id)
    .execute(pool)
    .await
    .map_err(|e| format!("绑定失败: {e}"))?;
    Ok(data::read_base(pool, kb_id).await?.expect("绑定后读取失败"))
}

/// 解除问答绑定（问答退化为不可用；不影响 embedding 绑定与检索）
pub async fn unbind_answer(pool: &SqlitePool, kb_id: &str) -> Result<KbBase, String> {
    if data::read_base(pool, kb_id).await?.is_none() {
        return Err("知识库不存在".to_string());
    }
    let now = now_iso();
    sqlx::query(
        "UPDATE kb_knowledge_bases SET answer_channel_id=NULL, answer_model=NULL, \
         updated_at=?1 WHERE id=?2",
    )
    .bind(&now)
    .bind(kb_id)
    .execute(pool)
    .await
    .map_err(|e| format!("解绑失败: {e}"))?;
    Ok(data::read_base(pool, kb_id).await?.expect("解绑后读取失败"))
}

/// 读取库的问答绑定（channel_id, model）；未绑定给出引导性错误
pub async fn bound_answer(pool: &SqlitePool, kb_id: &str) -> Result<(String, String), String> {
    let Some(base) = data::read_base(pool, kb_id).await? else {
        return Err("知识库不存在".to_string());
    };
    let channel_id = base.answer_channel_id.ok_or_else(|| {
        "知识库尚未绑定问答渠道，请先绑定（渠道 + 问答模型）".to_string()
    })?;
    let model = base
        .answer_model
        .ok_or("知识库缺少问答模型配置，请重新绑定")?;
    Ok((channel_id, model))
}

// ---- 会话 CRUD（kb_sessions；删除级联清消息）----

pub async fn create_session(
    pool: &SqlitePool,
    kb_id: &str,
    title: Option<&str>,
) -> Result<KbSession, String> {
    if data::read_base(pool, kb_id).await?.is_none() {
        return Err("知识库不存在".to_string());
    }
    let title = title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| truncate_chars(t, 50))
        .unwrap_or_else(|| DEFAULT_TITLE.to_string());
    let id = new_id("ses");
    let now = now_iso();
    sqlx::query(
        "INSERT INTO kb_sessions (id, kb_id, title, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?4)",
    )
    .bind(&id)
    .bind(kb_id)
    .bind(&title)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| format!("创建会话失败: {e}"))?;
    Ok(read_session(pool, &id).await?.expect("新建会话读取失败"))
}

pub async fn list_sessions(pool: &SqlitePool, kb_id: &str) -> Result<Vec<KbSession>, String> {
    sqlx::query_as::<_, KbSession>(
        "SELECT id, kb_id, title, created_at, updated_at FROM kb_sessions \
         WHERE kb_id = ?1 ORDER BY updated_at DESC LIMIT 200",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())
}

pub async fn delete_session(pool: &SqlitePool, session_id: &str) -> Result<(), String> {
    let r = sqlx::query("DELETE FROM kb_sessions WHERE id = ?1")
        .bind(session_id)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    if r.rows_affected() == 0 {
        return Err("会话不存在".to_string());
    }
    Ok(())
}

pub(crate) async fn read_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<KbSession>, String> {
    sqlx::query_as::<_, KbSession>(
        "SELECT id, kb_id, title, created_at, updated_at FROM kb_sessions WHERE id = ?1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())
}

// ---- 消息读写（kb_conversations）----

/// 会话消息（created_at 升序；sources JSON 解析失败容错为空数组）
pub async fn list_messages(pool: &SqlitePool, session_id: &str) -> Result<Vec<KbMessage>, String> {
    if read_session(pool, session_id).await?.is_none() {
        return Err("会话不存在".to_string());
    }
    let rows: Vec<ConversationRow> = sqlx::query_as(
        "SELECT id, role, content, sources, model, tokens_used, created_at \
         FROM kb_conversations WHERE session_id = ?1 ORDER BY created_at, rowid",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(ConversationRow::into_message).collect())
}

#[derive(sqlx::FromRow)]
struct ConversationRow {
    id: String,
    role: String,
    content: String,
    sources: Option<String>,
    model: Option<String>,
    tokens_used: i64,
    created_at: String,
}

impl ConversationRow {
    fn into_message(self) -> KbMessage {
        let sources = self
            .sources
            .as_deref()
            .and_then(|s| serde_json::from_str::<Vec<Citation>>(s).ok())
            .unwrap_or_default();
        KbMessage {
            id: self.id,
            role: self.role,
            content: self.content,
            sources,
            model: self.model,
            tokens_used: self.tokens_used,
            created_at: self.created_at,
        }
    }
}

/// 会话最近 N 条历史（升序，供多轮 chat 上下文）——user/assistant 原样回放
async fn history_messages(pool: &SqlitePool, session_id: &str) -> Result<Vec<ChatMessage>, String> {
    let rows: Vec<ConversationRow> = sqlx::query_as(
        "SELECT id, role, content, sources, model, tokens_used, created_at \
         FROM kb_conversations WHERE session_id = ?1 \
         ORDER BY created_at DESC, rowid DESC LIMIT ?2",
    )
    .bind(session_id)
    .bind(HISTORY_LIMIT as i64)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    let mut out: Vec<ChatMessage> = rows
        .into_iter()
        .rev()
        .map(|r| ChatMessage {
            role: r.role,
            content: truncate_chars(&r.content, HISTORY_MSG_MAX_CHARS),
        })
        .collect();
    out.retain(|m| m.role == "user" || m.role == "assistant");
    Ok(out)
}

// ---- 纯函数：标题摘要 / 引用上下文组装 / 用户提示词 ----

/// 问题预处理：去空白 + 超长截断（chat 与 MCP 共用，保证生成与落库内容一致）。
/// 空问题给出中文错误。
pub(crate) fn normalize_question(question: &str) -> Result<String, String> {
    let trimmed = question.trim();
    if trimmed.is_empty() {
        return Err("问题不能为空".to_string());
    }
    Ok(truncate_chars(trimmed, MAX_QUESTION_CHARS))
}

/// 首问自动标题：首个非空行，压缩空白并截断
pub(crate) fn summarize_title(question: &str) -> String {
    let first = question.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let joined: String = first.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&joined, TITLE_MAX_CHARS)
}

/// 截断到 max 字符（按 Unicode 字符计数）
pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

/// 一条带编号的引用材料（正文 [n] 与 citations 对齐）
#[derive(Debug, Clone)]
pub(crate) struct ContextChunk {
    pub n: usize,
    pub hit: SearchHit,
    /// 进上下文的文本（单块超长已截断）
    pub excerpt: String,
}

/// 从排序命中里挑选并截断引用材料：按序累加，超出总预算即停止
/// （保证至少含第一块；单块以 1500 字符为上限，防止个别大块吞掉全部预算）
pub(crate) fn build_context(hits: &[SearchHit], max_chars: usize) -> Vec<ContextChunk> {
    let mut ctx: Vec<ContextChunk> = Vec::new();
    let mut used = 0usize;
    for hit in hits {
        let excerpt = truncate_chars(&hit.content, CHUNK_EXCERPT_MAX_CHARS);
        let cost = excerpt.chars().count();
        if !ctx.is_empty() && used + cost > max_chars {
            break;
        }
        used += cost;
        ctx.push(ContextChunk {
            n: ctx.len() + 1,
            hit: hit.clone(),
            excerpt,
        });
    }
    ctx
}

/// 渲染用户提示词：编号材料 + 问题（无材料时明示空检索，交给系统提示应对）
pub(crate) fn render_user_prompt(context: &[ContextChunk], question: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    if context.is_empty() {
        lines.push("（本次检索未在知识库中找到相关内容）".to_string());
    } else {
        lines.push("以下是知识库检索到的相关材料：".to_string());
        for c in context {
            let heading = if c.hit.heading.is_empty() {
                String::new()
            } else {
                format!(" · {}", c.hit.heading)
            };
            lines.push(format!("[{}] 《{}》{}", c.n, c.hit.title, heading));
            lines.push(c.excerpt.clone());
            lines.push(String::new());
        }
        lines.push("（材料完，请基于上述材料回答，引用处以 [n] 标注）".to_string());
    }
    lines.push(format!("问题：{question}"));
    lines.join("\n")
}

// ---- 服务入口：chat ----

impl KnowledgeBaseService {
    /// RAG 会话问答（服务入口）。检索策略与校验见 [`Self::answer`]；
    /// **渠道调用成功才落库**（user + assistant 同一事务，来源与用量一并记录）。
    pub async fn chat(
        &self,
        pool: &SqlitePool,
        session_id: &str,
        question: &str,
    ) -> Result<ChatReply, String> {
        let question = normalize_question(question)?;
        let Some(session) = read_session(pool, session_id).await? else {
            return Err("会话不存在".to_string());
        };
        // 最近 N 条历史由会话方提供（answer 只负责拼接）
        let history = history_messages(pool, session_id).await?;
        let out = self
            .answer(pool, &session.kb_id, &question, &history)
            .await?;

        // 成功才落库：user + assistant 同事务；首问自动生成会话标题
        let sources_json = serde_json::to_string(&out.citations).map_err(|e| e.to_string())?;
        let now = now_iso();
        let title = summarize_title(&question);
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        insert_message(&mut tx, session_id, "user", &question, None, None, 0, &now).await?;
        insert_message(
            &mut tx,
            session_id,
            "assistant",
            &out.answer,
            Some(&sources_json),
            Some(&out.model),
            out.tokens_used,
            &now,
        )
        .await?;
        sqlx::query(
            "UPDATE kb_sessions SET updated_at=?2, \
             title = CASE WHEN title = ?3 THEN ?4 ELSE title END WHERE id = ?1",
        )
        .bind(session_id)
        .bind(&now)
        .bind(DEFAULT_TITLE)
        .bind(&title)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("更新会话失败: {e}"))?;
        tx.commit().await.map_err(|e| e.to_string())?;

        Ok(ChatReply {
            session_id: session_id.to_string(),
            answer: out.answer,
            citations: out.citations,
            model: out.model,
            tokens_used: out.tokens_used,
            latency_ms: out.latency_ms,
        })
    }

    /// 无状态问答核心（MCP 等调用方与 [`Self::chat`] 共用）：
    /// 校验库与问答绑定 → 检索（hybrid 自动降级关键词）→ 组装引用上下文与历史
    /// → 调用问答渠道（temperature 0.2、非流式）。**不落库、不写会话**——
    /// 留痕由调用方负责（chat 走本方法后落库，MCP 走工具审计）。
    /// `question` 须已通过 [`normalize_question`]；`history` 已按条截断。
    pub(crate) async fn answer(
        &self,
        pool: &SqlitePool,
        kb_id: &str,
        question: &str,
        history: &[ChatMessage],
    ) -> Result<AskOutcome, String> {
        let Some(base) = data::read_base(pool, kb_id).await? else {
            return Err("知识库不存在".to_string());
        };
        if base.status != "active" {
            return Err("知识库已停用，无法问答".to_string());
        }
        let (channel_id, model) = bound_answer(pool, kb_id).await?;
        let (ctype, cprotocol, base_url, api_key, status): (String, String, String, String, String) = sqlx::query_as(
            "SELECT type, protocol, base_url, api_key_encrypted, status FROM channels WHERE id = ?1",
        )
        .bind(&channel_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("问答渠道不存在：{channel_id}，请重新绑定"))?;
        if status != "active" {
            return Err(format!("问答渠道已停用：{channel_id}，请重新绑定或换绑"));
        }
        let protocol = crate::adapters::effective_protocol(&ctype, &cprotocol);
        let adapter = create_adapter(&protocol, &base_url, &api_key)
            .map_err(|e| format!("问答渠道配置错误: {e}"))?;

        // 检索上下文（策略层：重排 / 阈值 / 邻居扩展；hybrid 不可用降级关键词）
        let cfg = retrieve::RetrievalConfig::load(pool).await;
        let hits = retrieve::retrieve_for_chat(self, pool, kb_id, question, &cfg).await?;
        let context = build_context(&hits, CONTEXT_MAX_CHARS);

        // 组装 messages：system + 调用方历史 + 当前问题（带引用）
        let mut messages: Vec<ChatMessage> = Vec::with_capacity(HISTORY_LIMIT + 2);
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: KB_SYSTEM_PROMPT.to_string(),
        });
        messages.extend(history.iter().cloned());
        messages.push(ChatMessage {
            role: "user".to_string(),
            content: render_user_prompt(&context, question),
        });

        // 调用问答渠道（非流式）
        let started = Instant::now();
        let resp = adapter
            .chat_completion(ChatRequest {
                model: model.clone(),
                messages,
                temperature: Some(0.2),
                max_tokens: Some(1024),
                stream: false,
            })
            .await
            .map_err(|e| format!("调用问答渠道失败: {e}"))?;
        let latency_ms = started.elapsed().as_millis() as u64;
        let tokens_used = resp.usage.as_ref().map(|u| u.total_tokens as i64).unwrap_or(0);

        // 引用条目 = 上下文编号 → sources JSON
        let citations: Vec<Citation> = context
            .iter()
            .map(|c| Citation {
                n: c.n,
                chunk_id: c.hit.chunk_id,
                document_id: c.hit.document_id.clone(),
                title: c.hit.title.clone(),
                heading: c.hit.heading.clone(),
            })
            .collect();

        Ok(AskOutcome {
            answer: resp.content,
            citations,
            model,
            tokens_used,
            latency_ms,
        })
    }

}

/// 事务内插入一条会话消息（id 由调用方分配前缀 msg）
async fn insert_message(
    tx: &mut Transaction<'_, Sqlite>,
    session_id: &str,
    role: &str,
    content: &str,
    sources: Option<&str>,
    model: Option<&str>,
    tokens_used: i64,
    now: &str,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO kb_conversations (id, session_id, role, content, sources, model, \
         tokens_used, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
    )
    .bind(new_id("msg"))
    .bind(session_id)
    .bind(role)
    .bind(content)
    .bind(sources)
    .bind(model)
    .bind(tokens_used)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(|e| format!("写入会话消息失败: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::kb::{data, embed, indexing, schema};
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;
    use std::sync::Arc;

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
        // 最小渠道表（绑定校验 + ChannelEmbedder/适配器运行时读取）
        sqlx::query(
            "CREATE TABLE channels (id TEXT PRIMARY KEY, name TEXT, type TEXT, protocol TEXT NOT NULL DEFAULT '', base_url TEXT, \
             model TEXT, models TEXT NOT NULL DEFAULT '', status TEXT, api_key_encrypted TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn insert_channel(
        pool: &SqlitePool,
        id: &str,
        ctype: &str,
        base_url: &str,
        model: &str,
    ) {
        sqlx::query(
            "INSERT INTO channels (id, name, type, base_url, model, status, api_key_encrypted) \
             VALUES (?1, ?2, ?3, ?4, ?5, 'active', 'k')",
        )
        .bind(id)
        .bind(id)
        .bind(ctype)
        .bind(base_url)
        .bind(model)
        .execute(pool)
        .await
        .unwrap();
    }

    // ---- 内嵌 mock HTTP 服务（/v1/embeddings + /v1/chat/completions）----
    // 向量语义与 DummyEmbedder 一致（同文本同向量），实现 hybrid 检索端到端。

    async fn spawn_mock(dims: usize) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(x) => x,
                    Err(_) => break,
                };
                tokio::spawn(async move {
                    let _ = handle_mock_conn(stream, dims).await;
                });
            }
        });
        format!("http://127.0.0.1:{port}/v1")
    }

    async fn handle_mock_conn(
        mut stream: tokio::net::TcpStream,
        dims: usize,
    ) -> std::io::Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // 读到空行为止（头部），再按 Content-Length 读请求体
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
        let first_line = head.lines().next().unwrap_or("").to_string();
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
        let payload: Vec<u8> = if first_line.contains("/v1/embeddings") {
            let j: serde_json::Value =
                serde_json::from_slice(&body).unwrap_or(serde_json::json!({"input": []}));
            let texts: Vec<&str> = j["input"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            let data: Vec<serde_json::Value> = texts
                .iter()
                .map(|t| serde_json::json!({ "embedding": embed::dummy_vector(t, dims) }))
                .collect();
            serde_json::to_vec(&serde_json::json!({ "data": data })).unwrap()
        } else {
            // /v1/chat/completions：固定回答（引用 [1] 便于断言溯源）
            serde_json::to_vec(&serde_json::json!({
                "id": "cmpl-mock",
                "object": "chat.completion",
                "model": "mock",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "固定回答：材料支持该结论，见 [1]。" },
                    "finish_reason": "stop"
                }],
                "usage": { "prompt_tokens": 10, "completion_tokens": 6, "total_tokens": 16 }
            }))
            .unwrap()
        };
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        );
        stream.write_all(header.as_bytes()).await?;
        stream.write_all(&payload).await?;
        stream.shutdown().await?;
        Ok(())
    }

    // ---- 纯函数 ----

    fn hit(id: i64, title: &str, heading: &str, content: &str) -> SearchHit {
        SearchHit {
            chunk_id: id,
            document_id: format!("doc_{id}"),
            title: title.to_string(),
            heading: heading.to_string(),
            seq: id,
            content: content.to_string(),
            score: 1.0,
            vec_score: None,
            kw_score: None,
        }
    }

    #[test]
    fn context_respects_budget_numbering_and_truncation() {
        let long = "长".repeat(2000);
        let hits = vec![
            hit(1, "A", "", "短内容一"),
            hit(2, "B", "", &long),
            hit(3, "C", "", "短内容三"),
        ];
        // 预算只够 1 + 2（2 截到 1500，第三块放不下即整体裁掉）
        let ctx = build_context(&hits, 1506);
        assert_eq!(ctx.len(), 2, "超预算的第三块应被裁掉");
        assert_eq!(ctx[0].n, 1);
        assert_eq!(ctx[1].n, 2);
        assert!(ctx[1].excerpt.ends_with('…'), "超长块应截断");
        assert_eq!(ctx[1].excerpt.chars().count(), 1501, "截断 + 省略号");
        // 空预算也必须保留第一块（至少一条引用）
        let ctx = build_context(&hits, 1);
        assert_eq!(ctx.len(), 1);
    }

    #[test]
    fn user_prompt_renders_citations_and_fallback() {
        let ctx = build_context(&[hit(1, "负载均衡", "第四章", "内容甲")], CONTEXT_MAX_CHARS);
        let p = render_user_prompt(&ctx, "网关怎么做负载均衡？");
        assert!(p.contains("[1] 《负载均衡》 · 第四章"), "标题与 heading 溯源: {p}");
        assert!(p.contains("内容甲"));
        assert!(p.contains("问题：网关怎么做负载均衡？"));
        let none = render_user_prompt(&[], "空检索问题");
        assert!(none.contains("未在知识库中找到相关内容"));
    }

    #[test]
    fn summarize_title_takes_first_line_and_truncates() {
        assert_eq!(summarize_title("  第一行 \n第二行  "), "第一行");
        assert_eq!(summarize_title(""), "");
        let long = summarize_title(&"很长的标题内容".repeat(10));
        assert_eq!(long.chars().count(), TITLE_MAX_CHARS + 1, "超长加省略号");
        assert!(long.ends_with('…'));
    }

    // ---- 绑定与会话数据层 ----

    #[tokio::test]
    async fn bind_answer_validates_channel_and_type() {
        let pool = test_pool().await;
        let b = data::create_base(&pool, "库", "", 800, 150).await.unwrap();
        insert_channel(&pool, "ch_good", "OpenAI", "http://mock/v1", "gpt-4o-mini").await;
        insert_channel(&pool, "ch_claude", "Claude", "http://mock/v1", "claude-x").await;

        // 默认模型取渠道列；显式模型覆盖
        let b = bind_answer(&pool, &b.id, "ch_good", None).await.unwrap();
        assert_eq!(b.answer_channel_id.as_deref(), Some("ch_good"));
        assert_eq!(b.answer_model.as_deref(), Some("gpt-4o-mini"));
        let b = bind_answer(&pool, &b.id, "ch_good", Some("custom-model")).await.unwrap();
        assert_eq!(b.answer_model.as_deref(), Some("custom-model"));
        // Claude 类型在绑定期即暴露（适配器尚未实现）
        let err = bind_answer(&pool, &b.id, "ch_claude", None).await.unwrap_err();
        assert!(err.contains("Claude"), "应提示不支持: {err}");
        // 解绑清空
        let b = unbind_answer(&pool, &b.id).await.unwrap();
        assert!(b.answer_channel_id.is_none() && b.answer_model.is_none());
        // 不存在的库
        assert!(bind_answer(&pool, "kb_xx", "ch_good", None).await.is_err());
    }

    #[tokio::test]
    async fn sessions_crud_and_cascade_delete() {
        let pool = test_pool().await;
        let b = data::create_base(&pool, "库", "", 800, 150).await.unwrap();
        let s = create_session(&pool, &b.id, None).await.unwrap();
        assert_eq!(s.title, "新对话");
        let s2 = create_session(&pool, &b.id, Some("  面试准备 ")).await.unwrap();
        assert_eq!(s2.title, "面试准备");

        let list = list_sessions(&pool, &b.id).await.unwrap();
        assert_eq!(list.len(), 2);

        // 会话不存在时报错
        assert!(list_messages(&pool, "ses_none").await.is_err());
        // 删除会话 → 消息级联清除（先插两条消息占位）
        let now = now_iso();
        let mut tx = pool.begin().await.unwrap();
        insert_message(&mut tx, &s.id, "user", "q", None, None, 0, &now)
            .await
            .unwrap();
        insert_message(&mut tx, &s.id, "assistant", "a", None, None, 0, &now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        delete_session(&pool, &s.id).await.unwrap();
        assert!(delete_session(&pool, &s.id).await.is_err(), "重复删除报错");
        assert!(
            list_messages(&pool, &s.id).await.is_err(),
            "会话已删，消息查询应报会话不存在"
        );
        let remain: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM kb_conversations WHERE session_id = ?1",
        )
        .bind(&s.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remain, 0);
    }

    // ---- 问答端到端 ----

    #[tokio::test]
    async fn chat_full_hybrid_pipeline_persists_pair_and_citations() {
        let pool = test_pool().await;
        let base = spawn_mock(1536).await;
        insert_channel(&pool, "ch_llm", "OpenAI", &base, "mock-chat").await;
        let b = data::create_base(&pool, "向量问答库", "", 800, 150).await.unwrap();
        let text = "机器学习是人工智能的核心分支，研究让计算机从数据中自动学习的方法与理论。";
        let doc = data::import_text(&pool, &b.id, "机器学习笔记", "txt", text)
            .await
            .unwrap();
        assert_eq!(doc.chunk_count, 1);

        // embedding 绑定 + Dummy 构建索引（1536 维与 mock/dummy 一致）
        let b = embed::bind_embedding(&pool, &b.id, "ch_llm", "text-embedding-3-small")
            .await
            .unwrap();
        let dir = std::env::temp_dir().join(format!("kb-rag-idx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let task_id = indexing::create_task(&pool, &b.id, "embed_index").await.unwrap();
        let e: Arc<dyn embed::Embedder> = Arc::new(embed::DummyEmbedder { dims: 1536 });
        indexing::run_embed_index_task(&pool, &task_id, &dir, Some(e))
            .await
            .unwrap();

        // 问答绑定 + 会话 + 提问（问题与分块文本一致 → 向量与关键词双命中）
        let b = bind_answer(&pool, &b.id, "ch_llm", None).await.unwrap();
        assert_eq!(b.answer_model.as_deref(), Some("mock-chat"));
        let s = create_session(&pool, &b.id, None).await.unwrap();
        let svc = KnowledgeBaseService::default();
        let reply = svc.chat(&pool, &s.id, text).await.unwrap();

        assert_eq!(reply.answer, "固定回答：材料支持该结论，见 [1]。");
        assert_eq!(reply.model, "mock-chat");
        assert_eq!(reply.tokens_used, 16);
        assert_eq!(reply.citations.len(), 1, "应命中唯一分块");
        assert_eq!(reply.citations[0].document_id, doc.id);
        assert_eq!(reply.citations[0].title, "机器学习笔记");
        assert_eq!(reply.citations[0].n, 1);
        assert!(reply.latency_ms < 60_000);

        // 会话内消息成对落库：user + assistant（顺序正确、来源与用量齐全）
        let msgs = list_messages(&pool, &s.id).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, text);
        assert!(msgs[0].sources.is_empty());
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].model.as_deref(), Some("mock-chat"));
        assert_eq!(msgs[1].tokens_used, 16);
        assert_eq!(msgs[1].sources.len(), 1);
        assert_eq!(msgs[1].sources[0].chunk_id, reply.citations[0].chunk_id);

        // 首问自动标题 + 会话更新时间刷新
        let sess = read_session(&pool, &s.id).await.unwrap().unwrap();
        assert_ne!(sess.title, "新对话", "首问后应自动摘要标题");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn chat_degrades_to_keyword_without_index() {
        let pool = test_pool().await;
        let base = spawn_mock(64).await;
        insert_channel(&pool, "ch_llm", "DeepSeek", &base, "deepseek-chat").await;
        let b = data::create_base(&pool, "纯文本问答库", "", 800, 150).await.unwrap();
        let text = "网关转发层会把请求体原样透传，仅改写 URL 与鉴权头，以保留扩展字段。";
        let doc = data::import_text(&pool, &b.id, "透传设计", "txt", text)
            .await
            .unwrap();
        // 只绑问答渠道：无 embedding 绑定 → hybrid 前置失败 → 关键词降级
        bind_answer(&pool, &b.id, "ch_llm", Some("deepseek-chat"))
            .await
            .unwrap();
        let s = create_session(&pool, &b.id, None).await.unwrap();
        let svc = KnowledgeBaseService::default();
        let reply = svc.chat(&pool, &s.id, text).await.unwrap();
        assert_eq!(reply.answer, "固定回答：材料支持该结论，见 [1]。");
        assert_eq!(reply.citations.len(), 1);
        assert_eq!(reply.citations[0].document_id, doc.id, "降级关键词仍应溯源");
    }

    #[tokio::test]
    async fn chat_fails_cleanly_without_answer_binding() {
        let pool = test_pool().await;
        let b = data::create_base(&pool, "库", "", 800, 150).await.unwrap();
        data::import_text(&pool, &b.id, "d", "txt", "内容。").await.unwrap();
        let s = create_session(&pool, &b.id, None).await.unwrap();
        let svc = KnowledgeBaseService::default();
        let err = svc.chat(&pool, &s.id, "问一个问题").await.unwrap_err();
        assert!(err.contains("绑定问答渠道"), "未绑定应引导: {err}");
        // 失败不落库
        assert!(list_messages(&pool, &s.id).await.unwrap().is_empty());
    }
}
