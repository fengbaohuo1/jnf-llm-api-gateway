//! MCP 协议层：JSON-RPC 2.0 子集 + MCP 方法分发。
//!
//! 实现范围（自研、无外部 SDK）：`initialize` / `notifications/initialized` /
//! `notifications/cancelled` / `ping` / `tools/list` / `tools/call`。
//!
//! 取舍（单机单用户的服务形态）：
//! - **无状态**：不维护会话、不校验 `Mcp-Session-Id`、不做 GET SSE——客户端能力
//!   收敛在 tools 上，协议层只负责 JSON-RPC 正确性；
//! - 工具执行错误按 MCP 惯例放进结果 `isError: true`（而非 JSON-RPC error），
//!   参数结构错误用 `-32602`，未知方法用 `-32601`。

use serde_json::{json, Value};

/// 本服务声明的 MCP 协议版本（与客户端 capabilities 协商值）
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

/// 工具执行抽象：协议层不感知具体工具，只消费"列表"与"按名执行"。
#[async_trait::async_trait]
pub trait ToolSet: Send + Sync {
    /// tools/list 返回的完整工具定义（name/description/inputSchema）
    fn list_tools(&self) -> Vec<Value>;
    /// 执行工具：Ok(文本) 成功；Err(中文错误) → 协议层放进 isError 结果
    async fn run_tool(&self, name: &str, args: &Value) -> Result<String, String>;
}

/// 服务器信息（initialize 响应 serverInfo）
pub fn server_info() -> Value {
    json!({
        "name": "llm-api-gateway",
        "version": env!("CARGO_PKG_VERSION"),
    })
}

/// 处理一条 JSON-RPC 消息（含 MCP 方法分发）。返回响应体；
/// 通知类消息（无 id 或 initialized 等）返回 None（调用方回 202 空响应）。
pub async fn handle_message(msg: &Value, tools: &dyn ToolSet) -> Option<Value> {
    let id = msg.get("id").cloned();
    let rpc_ok = msg.get("jsonrpc").and_then(Value::as_str) == Some("2.0");
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return Some(error_response(id, -32600, "请求不是合法的 JSON-RPC 2.0 消息".to_string()));
    };
    if !rpc_ok {
        return Some(error_response(id, -32600, "请求不是合法的 JSON-RPC 2.0 消息".to_string()));
    }

    match method {
        "initialize" => Some(success(
            id,
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": server_info(),
            }),
        )),
        // 通知：无响应（客户端在 initialize 后通知 initialized）
        "notifications/initialized" | "notifications/cancelled" => None,
        "ping" => Some(success(id, Value::Object(Default::default()))),
        "tools/list" => {
            let tools = tools.list_tools();
            Some(success(id, json!({ "tools": tools })))
        }
        "tools/call" => {
            let Some(name) = msg.pointer("/params/name").and_then(Value::as_str) else {
                return Some(error_response(id, -32602, "tools/call 缺少参数 name".to_string()));
            };
            let args = msg
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match tools.run_tool(name, &args).await {
                Ok(text) => Some(success(
                    id,
                    json!({
                        "content": [{"type": "text", "text": text}],
                        "isError": false,
                    }),
                )),
                Err(err) => Some(success(
                    id,
                    json!({
                        "content": [{"type": "text", "text": err}],
                        "isError": true,
                    }),
                )),
            }
        }
        _ => Some(error_response(id, -32601, format!("未知方法: {method}"))),
    }
}

/// 成功响应
pub fn success(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result })
}

/// 错误响应
pub fn error_response(id: Option<Value>, code: i64, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": { "code": code, "message": message },
    })
}

/// 请求体无法解析为 JSON 时的标准响应（id 为 null）
pub fn parse_error_response() -> Value {
    error_response(None, -32700, "请求体不是合法 JSON".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 测试工具集：一个 echo 工具 + 一个会失败的工具
    struct MockTools {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ToolSet for MockTools {
        fn list_tools(&self) -> Vec<Value> {
            vec![json!({ "name": "echo", "description": "回显", "inputSchema": { "type": "object" } })]
        }
        async fn run_tool(&self, name: &str, args: &Value) -> Result<String, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match name {
                "echo" => Ok(format!("echo:{}", args["text"].as_str().unwrap_or(""))),
                "boom" => Err("工具执行失败（中文）".to_string()),
                _ => Err(format!("未知工具: {name}")),
            }
        }
    }

    fn msg(id: Option<i64>, method: &str, params: Value) -> Value {
        let mut m = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        if let Some(i) = id {
            m["id"] = json!(i);
        }
        m
    }

    #[tokio::test]
    async fn initialize_returns_capabilities_and_server_info() {
        let tools = MockTools { calls: AtomicUsize::new(0) };
        let resp = handle_message(&msg(Some(1), "initialize", json!({})), &tools)
            .await
            .expect("initialize 应有响应");
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(resp["result"]["serverInfo"]["name"], "llm-api-gateway");
        assert_eq!(resp["result"]["capabilities"]["tools"]["listChanged"], false);
    }

    #[tokio::test]
    async fn notifications_get_no_response_but_ping_does() {
        let tools = MockTools { calls: AtomicUsize::new(0) };
        // initialized 通知（无 id）→ None
        assert!(handle_message(&msg(None, "notifications/initialized", json!({})), &tools).await.is_none());
        // ping → 空 result
        let resp = handle_message(&msg(Some(2), "ping", json!({})), &tools).await.unwrap();
        assert_eq!(resp["id"], 2);
        assert!(resp.get("result").is_some());
    }

    #[tokio::test]
    async fn tools_list_and_call_shape() {
        let tools = MockTools { calls: AtomicUsize::new(0) };
        let resp = handle_message(&msg(Some(3), "tools/list", json!({})), &tools).await.unwrap();
        assert_eq!(resp["result"]["tools"][0]["name"], "echo");

        let resp = handle_message(
            &msg(Some(4), "tools/call", json!({ "name": "echo", "arguments": { "text": "hi" } })),
            &tools,
        )
        .await
        .unwrap();
        assert_eq!(resp["result"]["content"][0]["text"], "echo:hi");
        assert_eq!(resp["result"]["isError"], false);

        // 工具执行错误 → isError true（不是 JSON-RPC error）
        let resp = handle_message(&msg(Some(5), "tools/call", json!({ "name": "boom" })), &tools).await.unwrap();
        assert_eq!(resp["result"]["isError"], true);
        assert!(resp["result"]["content"][0]["text"].as_str().unwrap().contains("中文"));
        assert!(resp.get("error").is_none(), "工具错误不应进 JSON-RPC error");
    }

    #[tokio::test]
    async fn unknown_method_and_bad_rpc_and_missing_args() {
        let tools = MockTools { calls: AtomicUsize::new(0) };
        // 未知方法 → -32601
        let resp = handle_message(&msg(Some(6), "foo/bar", json!({})), &tools).await.unwrap();
        assert_eq!(resp["error"]["code"], -32601);
        // 非法 rpc 版本 → -32600
        let bad = json!({ "jsonrpc": "1.0", "id": 7, "method": "ping" });
        let resp = handle_message(&bad, &tools).await.unwrap();
        assert_eq!(resp["error"]["code"], -32600);
        // tools/call 缺 name → -32602
        let resp = handle_message(&msg(Some(8), "tools/call", json!({ "arguments": {} })), &tools).await.unwrap();
        assert_eq!(resp["error"]["code"], -32602);
        // 缺 jsonrpc 字段 → -32600
        let no_rpc = json!({ "id": 9, "method": "ping" });
        let resp = handle_message(&no_rpc, &tools).await.unwrap();
        assert_eq!(resp["error"]["code"], -32600);
    }

    #[test]
    fn parse_error_response_shape() {
        let r = parse_error_response();
        assert_eq!(r["error"]["code"], -32700);
        assert!(r["id"].is_null());
    }
}
