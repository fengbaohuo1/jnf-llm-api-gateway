//! 转发代理：本地 HTTP 服务（OpenAI 兼容入口）。
//!
//! `POST http://127.0.0.1:{port}/v1/chat/completions` 完整处理链路：
//! ① 密钥鉴权（auth）→ ② 请求侧安全扫描/脱敏 → ③ 调度器选有序故障转移队列 →
//! ④ 透传转发（forward）+ 故障转移循环 → ⑤ 响应侧安全扫描/脱敏 →
//! ⑥ 安全结果合并落库 → ⑦ 记账（requests/daily_stats/used_tokens）
//!
//! 模块划分：
//! - `auth`：密钥鉴权与配额检查
//! - `settings`：代理运行设置（server/failover/security）
//! - `handlers`：各路由处理器（非流式 / 流式 / models / health）
//! - `forward`：向上游透传转发与错误分类
//! - `stream`：SSE 流式透传、token 统计与流结束记账
//!
//! 设计取向：
//! - **透传而非字段映射**：请求体原样转发给上游（只改 URL 与 Authorization），
//!   保证 tools / response_format 等 OpenAI 扩展字段 100% 兼容；
//!   适配器层保留给渠道测试与未来的协议转换。
//! - **没有故障就不要负载均衡**：固定走主渠道，失败才按序转移（failover 配置）。
//! - 转移可追溯：每次尝试（含失败）都写 requests 日志。

pub mod auth;
pub mod forward;
pub mod handlers;
pub mod settings;
pub mod stream;

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use sqlx::SqlitePool;

use crate::security::SecurityScanner;

/// 代理运行状态（axum 注入）
#[derive(Clone)]
pub struct ProxyState {
    pub(crate) pool: SqlitePool,
    pub(crate) scanner: Arc<dyn SecurityScanner>,
    /// 非流式请求客户端（30s 总超时）
    pub(crate) client: reqwest::Client,
    /// 流式请求客户端（长超时，SSE 长连接用）
    pub(crate) stream_client: reqwest::Client,
}

/// 流式请求总超时（SSE 长连接，模型生成几分钟内应完成）
const STREAM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// 公共前置处理的结果（非流式/流式共用）
pub(crate) struct PreparedRequest {
    pub(crate) auth: auth::AuthKey,
    pub(crate) settings: settings::ProxySettings,
    /// 脱敏后的请求体（透传 JSON）
    pub(crate) body: serde_json::Value,
    pub(crate) model: String,
    pub(crate) queue: Vec<crate::scheduler::ChannelChoice>,
    pub(crate) request_findings: Vec<crate::security::Finding>,
    /// 请求侧 high 命中的原文片段（内存态，响应侧上下文泄露比对用，不落库）
    pub(crate) request_secrets: Vec<String>,
    /// 请求侧消息总字符数（流式无精确 usage 时的估算输入）
    pub(crate) prompt_chars: (u64, u64),
}

/// 启动本地 HTTP 服务（地址取自 settings `server.host`/`server.port`，默认 127.0.0.1:8787）。
/// 随应用常驻；`server.enabled=false` 时仍监听但返回 503（设置即时生效）。
pub async fn serve(pool: SqlitePool, scanner: Arc<dyn SecurityScanner>) {
    let (host, port) = settings::read_listen_addr(&pool).await;
    let state = ProxyState {
        pool,
        scanner,
        client: reqwest::Client::builder()
            .timeout(crate::adapters::REQUEST_TIMEOUT)
            .build()
            .expect("failed to build proxy http client"),
        stream_client: reqwest::Client::builder()
            .timeout(STREAM_TIMEOUT)
            .build()
            .expect("failed to build proxy stream http client"),
    };

    let app = Router::new()
        .route("/v1/chat/completions", post(handlers::handle_chat_completion))
        .route("/v1/models", get(handlers::handle_models))
        .route("/health", get(handlers::handle_health))
        .fallback(handlers::handle_not_found)
        .with_state(state);

    let addr = format!("{host}:{port}");
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[proxy] 监听 {addr} 失败: {e}");
            return;
        }
    };
    eprintln!("[proxy] 转发代理已启动: http://{addr}/v1/chat/completions");
    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("[proxy] 服务异常退出: {e}");
    }
}

// ---------------- 测试公共设施 ----------------

#[cfg(test)]
pub(crate) mod test_util {
    use std::str::FromStr;

    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use axum::Router;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use sqlx::SqlitePool;

    use crate::commands::util::hash_key;
    use crate::scheduler::ChannelChoice;

    /// 内存 SQLite（单连接），带 keys 表
    pub async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE keys (
                id TEXT PRIMARY KEY, name TEXT, key_hash TEXT UNIQUE, key_prefix TEXT,
                status TEXT, quota_limit INTEGER, used_tokens INTEGER, expires_at TEXT, created_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    pub async fn insert_key(
        pool: &SqlitePool,
        id: &str,
        secret: &str,
        status: &str,
        quota_limit: i64,
        used_tokens: i64,
        expires_at: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO keys (id, name, key_hash, key_prefix, status, quota_limit, used_tokens, expires_at, created_at)
             VALUES (?1, '测试密钥', ?2, 'sk-llmapi-', ?3, ?4, ?5, ?6, '2025-01-01 00:00:00')",
        )
        .bind(id)
        .bind(hash_key(secret))
        .bind(status)
        .bind(quota_limit)
        .bind(used_tokens)
        .bind(expires_at)
        .execute(pool)
        .await
        .unwrap();
    }

    /// 起一个 mock 上游（返回固定状态码 + 固定 body）
    pub async fn mock_upstream(status: u16, body: &'static str) -> String {
        let app = Router::new()
            .route("/chat/completions", get(mock_respond).post(mock_respond))
            .with_state((status, body));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    async fn mock_respond(State((status, body)): State<(u16, &'static str)>) -> Response {
        (StatusCode::from_u16(status).unwrap(), Bytes::from_static(body.as_bytes())).into_response()
    }

    pub fn choice(base_url: &str) -> ChannelChoice {
        ChannelChoice {
            id: "c1".into(),
            name: "测试渠道".into(),
            channel_type: "OpenAI".into(),
            base_url: base_url.into(),
            api_key: "k".into(),
            model: "gpt-4o".into(),
            priority: 0,
            weight: 1,
            price_per_1k_tokens: 0.0,
            cache_price_per_1k_tokens: 0.0,
            reasoning_price_per_1k_tokens: 0.0,
        }
    }
}
