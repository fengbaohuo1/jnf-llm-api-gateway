//! MCP Streamable HTTP 传输：axum 路由 + 可选 Bearer 鉴权。
//!
//! 端点：`POST /mcp`（application/json 或 text/event-stream 两种响应协商；
//! 通知类消息回 202 空响应）。`GET /mcp` 回 405（不实现 GET SSE，无状态取舍）。
//! 鉴权：设置 `services.mcp.token` 非空时要求 `Authorization: Bearer <token>`。

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;

use serde_json::Value;

use super::protocol;
use super::tools::Tools;

/// MCP HTTP 服务的进程内状态（tools 注册表 + 可选令牌）
pub struct McpServerState {
    pub tools: Arc<Tools>,
    pub token: Option<String>,
}

impl McpServerState {
    pub fn new(tools: Arc<Tools>, token: Option<String>) -> Self {
        Self { tools, token }
    }
}

/// 组装路由（与工具集状态绑定）
pub fn router(state: Arc<McpServerState>) -> Router {
    Router::new()
        .route("/mcp", axum::routing::any(handle_mcp))
        .with_state(state)
}

/// 请求体大小上限（8MB；知识库问答的工具调用不涉及大上传）
const MAX_BODY: usize = 8 * 1024 * 1024;

async fn handle_mcp(
    State(state): State<Arc<McpServerState>>,
    headers: HeaderMap,
    req: Request,
) -> Response {
    if req.method() != Method::POST {
        let mut resp = Response::new(Body::from("仅支持 POST"));
        *resp.status_mut() = StatusCode::METHOD_NOT_ALLOWED;
        resp.headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("POST"));
        return resp;
    }

    // 可选 Bearer 鉴权
    if let Some(expected) = &state.token {
        let auth_ok = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == format!("Bearer {expected}"));
        if !auth_ok {
            return text_response(StatusCode::UNAUTHORIZED, "application/json", "{\"error\":\"未授权：需要正确的 Bearer Token\"}".to_string());
        }
    }

    let wants_sse = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/event-stream"));

    let bytes = match axum::body::to_bytes(req.into_body(), MAX_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return text_response(StatusCode::PAYLOAD_TOO_LARGE, "application/json", "{\"error\":\"请求体过大\"}".to_string())
        }
    };

    // 解析失败 → JSON-RPC parse error（-32700）
    let parsed: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return json_or_sse(wants_sse, vec![protocol::parse_error_response()]),
    };

    // 批量数组逐条处理（响应数组元素位置对应；通知项为 null）
    if let Some(items) = parsed.as_array() {
        if items.is_empty() {
            return text_response(StatusCode::BAD_REQUEST, "application/json", "{\"error\":\"空批量数组\"}".to_string());
        }
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            out.push(
                protocol::handle_message(item, state.tools.as_ref())
                    .await
                    .unwrap_or(Value::Null),
            );
        }
        return json_or_sse(wants_sse, out);
    }

    let resp = protocol::handle_message(&parsed, state.tools.as_ref()).await;
    match resp {
        // 通知类：无响应内容
        None => StatusCode::ACCEPTED.into_response(),
        Some(r) => json_or_sse(wants_sse, vec![r]),
    }
}

/// 单响应输出：SSE（Accept 协商）或纯 JSON
fn json_or_sse(wants_sse: bool, responses: Vec<Value>) -> Response {
    if wants_sse {
        let mut out = String::new();
        for r in &responses {
            out.push_str("event: message\n");
            out.push_str(&format!("data: {r}\n\n"));
        }
        text_response(StatusCode::OK, "text/event-stream", out)
    } else {
        let body = if responses.len() == 1 {
            responses.into_iter().next().unwrap().to_string()
        } else {
            serde_json::Value::Array(responses).to_string()
        };
        text_response(StatusCode::OK, "application/json", body)
    }
}

fn text_response(status: StatusCode, content_type: &'static str, body: String) -> Response {
    let mut resp = Response::new(Body::from(body));
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type),
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::kb::schema;
    use crate::services::mcp::tools::Tools;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use sqlx::SqlitePool;
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
        pool
    }

    /// 起真实端口并返回 base url（tools 用空库即可测协议形状）
    async fn spawn_server(pool: SqlitePool, token: Option<String>) -> String {
        let dir = std::env::temp_dir().join(format!("kb-mcp-srv-{}", std::process::id()));
        let tools = Arc::new(Tools::new(pool, dir));
        let state = Arc::new(McpServerState::new(tools, token));
        let app = router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://127.0.0.1:{port}/mcp")
    }

    async fn post_json(url: &str, token: Option<&str>, body: String) -> (u16, String) {
        let client = reqwest::Client::new();
        let mut req = client.post(url).header("content-type", "application/json");
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        let resp = req.body(body).send().await.unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        (status, text)
    }

    #[tokio::test]
    async fn handshake_tools_list_and_json_responses() {
        let pool = test_pool().await;
        let url = spawn_server(pool, None).await;

        // initialize 握手
        let (code, body) = post_json(
            &url,
            None,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#.to_string(),
        )
        .await;
        assert_eq!(code, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["serverInfo"]["name"], "llm-api-gateway");
        assert_eq!(v["result"]["protocolVersion"], protocol::MCP_PROTOCOL_VERSION);

        // tools/list：全部知识库工具（≥12）
        let (_, body) = post_json(&url, None, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#.to_string()).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        let names: Vec<&str> = v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"kb_search"));
        assert!(names.contains(&"kb_ask"));
        assert!(names.contains(&"kb_build_index"));
        assert!(!names.contains(&"kb_bind_answer"), "渠道绑定类工具不应暴露");

        // 通知 initialized → 202 空响应
        let (code, body) = post_json(&url, None, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_string()).await;
        assert_eq!(code, 202);
        assert!(body.is_empty(), "通知不应有响应体: {body}");

        // 工具调用空库 list_knowledge_bases
        let (_, body) = post_json(
            &url,
            None,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_knowledge_bases","arguments":{}}}"#.to_string(),
        )
        .await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["isError"], false);
        assert!(v["result"]["content"][0]["text"].as_str().unwrap().contains("bases"));

        // 未知方法 → -32601
        let (_, body) = post_json(&url, None, r#"{"jsonrpc":"2.0","id":4,"method":"bogus/x"}"#.to_string()).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], -32601);

        // 非 JSON body → -32700
        let (_, body) = post_json(&url, None, "not-json".to_string()).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], -32700);

        // GET → 405
        let client = reqwest::Client::new();
        let resp = client.get(&url).send().await.unwrap();
        assert_eq!(resp.status().as_u16(), 405);
    }

    #[tokio::test]
    async fn sse_response_when_accept_negotiated() {
        let pool = test_pool().await;
        let url = spawn_server(pool, None).await;
        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .header("accept", "text/event-stream")
            .body(r#"{"jsonrpc":"2.0","id":9,"method":"ping"}"#.to_string())
            .send()
            .await
            .unwrap();
        let text = resp.text().await.unwrap();
        assert!(text.starts_with("event: message\n"), "SSE 消息格式: {text}");
        assert!(text.contains("\"result\""), "SSE 应携带 ping 结果: {text}");
    }

    #[tokio::test]
    async fn bearer_token_gates_access() {
        let pool = test_pool().await;
        let url = spawn_server(pool, Some("secret-token".to_string())).await;
        // 无 token → 401
        let (code, _) = post_json(&url, None, r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#.to_string()).await;
        assert_eq!(code, 401);
        // 错误 token → 401
        let (code, _) = post_json(&url, Some("wrong"), r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#.to_string()).await;
        assert_eq!(code, 401);
        // 正确 token → 200
        let (code, body) = post_json(&url, Some("secret-token"), r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#.to_string()).await;
        assert_eq!(code, 200);
        assert!(body.contains("\"result\""));
    }
}
