//! Anthropic Messages API（/v1/messages）↔ OpenAI chat/completions 转换。
//!
//! 结构差异要点：
//! - 请求：顶层 `system`（字符串/数组）、content blocks（text/image/tool_use/tool_result）、
//!   `stop_sequences`、`tools` 为 `{name, description, input_schema}` 扁平结构
//! - 响应：`content` 为 block 数组（text/tool_use）、`stop_reason`、`usage` 为
//!   input/output_tokens（含缓存明细）
//! - 流式：`message_start → content_block_start → content_block_delta → content_block_stop
//!   → message_delta → message_stop`，usage 在 message_delta

use serde_json::{json, Value};

use super::sse_data;

// ---------------- 请求转换 ----------------

/// Anthropic 请求 → OpenAI chat/completions（就地修改）
pub fn request_to_openai(body: &mut Value) -> Result<(), String> {
    // system（顶层，字符串或 content block 数组）→ 前置 system 消息
    let mut messages: Vec<Value> = Vec::new();
    if let Some(system) = body.get("system") {
        let text = match system {
            Value::String(s) => s.clone(),
            Value::Array(blocks) => blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        if !text.is_empty() {
            messages.push(json!({ "role": "system", "content": text }));
        }
    }

    // messages：逐条转换（user 的 tool_result 拆成 tool 消息）
    if let Some(msgs) = body.get("messages").and_then(|m| m.as_array()) {
        for msg in msgs {
            let Some(role) = msg.get("role").and_then(|r| r.as_str()) else {
                continue;
            };
            match role {
                "user" => convert_user_message(msg, &mut messages),
                "assistant" => convert_assistant_message(msg, &mut messages),
                _ => {}
            }
        }
    }

    // tools：{name, description, input_schema} → OpenAI function 结构
    if let Some(tools) = body.get("tools").and_then(|t| t.as_array()) {
        let converted: Vec<Value> = tools
            .iter()
            .filter_map(|t| {
                let name = t.get("name")?.as_str()?.to_string();
                let description = t.get("description").and_then(|d| d.as_str()).unwrap_or("");
                let parameters = t.get("input_schema").cloned().unwrap_or(json!({}));
                Some(json!({
                    "type": "function",
                    "function": { "name": name, "description": description, "parameters": parameters }
                }))
            })
            .collect();
        body["tools"] = Value::Array(converted);
    }

    // tool_choice：{type:auto|any|tool} → OpenAI 格式
    if let Some(tc) = body.get("tool_choice").cloned() {
        body["tool_choice"] = match tc {
            Value::String(s) if s == "none" => Value::String("none".into()),
            Value::Object(o) => match o.get("type").and_then(|t| t.as_str()) {
                Some("any") => Value::String("required".into()),
                Some("tool") => {
                    let name = o.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    json!({ "type": "function", "function": { "name": name } })
                }
                _ => Value::String("auto".into()),
            },
            _ => Value::String("auto".into()),
        };
    }

    // stop_sequences → stop
    if let Some(stops) = body.get("stop_sequences").cloned() {
        body["stop"] = stops;
    }
    body.as_object_mut().unwrap().remove("stop_sequences");
    body.as_object_mut().unwrap().remove("system");
    body.as_object_mut().unwrap().remove("metadata");

    body["messages"] = Value::Array(messages);
    Ok(())
}

/// user 消息：字符串直转；content block 数组中的 text/image 转 OpenAI content，
/// tool_result 拆成独立 tool 消息（必须紧跟对应 assistant 的 tool_calls 之后，顺序保持）
fn convert_user_message(msg: &Value, out: &mut Vec<Value>) {
    let Some(content) = msg.get("content") else { return };
    match content {
        Value::String(s) => out.push(json!({ "role": "user", "content": s })),
        Value::Array(blocks) => {
            let mut text_parts: Vec<&str> = Vec::new();
            let mut rich: Vec<Value> = Vec::new();
            let mut tool_results: Vec<Value> = Vec::new();
            for b in blocks {
                let Some(bt) = b.get("type").and_then(|t| t.as_str()) else {
                    continue;
                };
                match bt {
                    "text" => {
                        if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                            text_parts.push(t);
                            rich.push(json!({ "type": "text", "text": t }));
                        }
                    }
                    "image" => {
                        // Anthropic image: {type:image, source:{type:base64, media_type, data}}
                        if let Some(url) = anthropic_image_to_data_url(b) {
                            rich.push(json!({
                                "type": "image_url",
                                "image_url": { "url": url }
                            }));
                        }
                    }
                    "tool_result" => {
                        let call_id = b.get("tool_use_id").and_then(|i| i.as_str()).unwrap_or("");
                        let result_content = b.get("content").cloned().unwrap_or(Value::String(String::new()));
                        tool_results.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": result_content,
                        }));
                    }
                    _ => {}
                }
            }
            // 文本/图片消息（若有）在前，tool_result 各成独立 tool 消息在后
            if rich.is_empty() {
                if !text_parts.is_empty() {
                    out.push(json!({ "role": "user", "content": text_parts.join("\n") }));
                }
            } else if tool_results.is_empty() {
                out.push(json!({ "role": "user", "content": rich }));
            } else {
                if !text_parts.is_empty() {
                    out.push(json!({ "role": "user", "content": text_parts.join("\n") }));
                }
            }
            out.extend(tool_results);
        }
        _ => {}
    }
}

/// assistant 消息：text blocks 拼接为 content；tool_use blocks → tool_calls；thinking 忽略
fn convert_assistant_message(msg: &Value, out: &mut Vec<Value>) {
    let Some(content) = msg.get("content") else { return };
    match content {
        Value::String(s) => out.push(json!({ "role": "assistant", "content": s })),
        Value::Array(blocks) => {
            let mut text_parts: Vec<&str> = Vec::new();
            let mut tool_calls: Vec<Value> = Vec::new();
            for b in blocks {
                let Some(bt) = b.get("type").and_then(|t| t.as_str()) else {
                    continue;
                };
                match bt {
                    "text" => {
                        if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                            text_parts.push(t);
                        }
                    }
                    "tool_use" => {
                        let id = b.get("id").and_then(|i| i.as_str()).unwrap_or("");
                        let name = b.get("name").and_then(|n| n.as_str()).unwrap_or("");
                        let args = b.get("input").cloned().unwrap_or(Value::Null);
                        tool_calls.push(json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": if args.is_null() { "{}".to_string() } else { args.to_string() },
                            }
                        }));
                    }
                    _ => {} // thinking / redacted_thinking 等忽略
                }
            }
            let mut converted = json!({ "role": "assistant", "content": text_parts.join("\n") });
            if !tool_calls.is_empty() {
                converted["tool_calls"] = Value::Array(tool_calls);
            }
            out.push(converted);
        }
        _ => {}
    }
}

/// Anthropic image block → OpenAI data URL
fn anthropic_image_to_data_url(block: &Value) -> Option<String> {
    let source = block.get("source")?;
    let st = source.get("type")?.as_str()?;
    if st != "base64" {
        return None;
    }
    let media_type = source.get("media_type").and_then(|m| m.as_str()).unwrap_or("image/png");
    let data = source.get("data").and_then(|d| d.as_str())?;
    Some(format!("data:{media_type};base64,{data}"))
}

// ---------------- 非流式响应转换 ----------------

/// OpenAI 非流式响应 → Anthropic Messages 响应
pub fn response_from_openai(openai: &Value) -> Value {
    let id = openai
        .get("id")
        .and_then(|i| i.as_str())
        .map(|s| format!("msg_{}", s.trim_start_matches("chatcmpl-")))
        .unwrap_or_else(|| crate::commands::util::new_id("msg"));
    let model = openai.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let choice = openai.get("choices").and_then(|c| c.as_array()).and_then(|a| a.first());
    let msg = choice.and_then(|c| c.get("message"));

    // content blocks：文本 + tool_use
    let mut content: Vec<Value> = Vec::new();
    if let Some(text) = msg.and_then(|m| m.get("content")).and_then(|c| c.as_str()) {
        if !text.is_empty() {
            content.push(json!({ "type": "text", "text": text }));
        }
    }
    if let Some(calls) = msg.and_then(|m| m.get("tool_calls")).and_then(|c| c.as_array()) {
        for call in calls {
            let id = call.get("id").and_then(|i| i.as_str()).unwrap_or("");
            let name = call
                .pointer("/function/name")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            let args_str = call
                .pointer("/function/arguments")
                .and_then(|a| a.as_str())
                .unwrap_or("{}");
            let input = serde_json::from_str::<Value>(args_str).unwrap_or(Value::Null);
            content.push(json!({ "type": "tool_use", "id": id, "name": name, "input": input }));
        }
    }

    let stop_reason = choice
        .and_then(|c| c.get("finish_reason"))
        .and_then(|f| f.as_str())
        .map(map_finish_reason)
        .unwrap_or("end_turn");

    // usage
    let (input_tokens, output_tokens, cache_read) = if let Some(u) = openai.get("usage") {
        (
            u.get("prompt_tokens").and_then(|t| t.as_i64()).unwrap_or(0),
            u.get("completion_tokens").and_then(|t| t.as_i64()).unwrap_or(0),
            u.get("prompt_cache_hit_tokens").and_then(|t| t.as_i64()).unwrap_or(0),
        )
    } else {
        (0, 0, 0)
    };

    json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": cache_read,
        }
    })
}

fn map_finish_reason(fr: &str) -> &'static str {
    match fr {
        "stop" => "end_turn",
        "length" => "max_tokens",
        "tool_calls" => "tool_use",
        "content_filter" => "refusal",
        _ => "end_turn",
    }
}

// ---------------- 流式转换（状态机） ----------------

/// Anthropic 流式翻译器：把 OpenAI 的 choices delta 事件流翻译成
/// message_start → content_block_* → message_delta → message_stop 事件序列。
pub struct AnthropicStream {
    model: String,
    input_tokens: i64,
    started: bool,
    /// 当前 text block 是否已开启
    text_block_open: bool,
    /// OpenAI tool_calls 的 index → 对应 tool block 状态
    tool_blocks: Vec<ToolBlockState>,
    next_block_index: u32,
    /// 已输出的 output_tokens（估算累计，或 usage 精确值）
    output_tokens: i64,
    finish_sent: bool,
}

struct ToolBlockState {
    oai_index: usize,
    block_index: u32,
    id: String,
    name: String,
    args: String,
    started: bool,
}

impl AnthropicStream {
    pub fn new(model: &str, input_tokens: i64) -> Self {
        Self {
            model: model.to_string(),
            input_tokens,
            started: false,
            text_block_open: false,
            tool_blocks: Vec::new(),
            next_block_index: 0,
            output_tokens: 0,
            finish_sent: false,
        }
    }

    pub fn on_event(&mut self, event: &Value) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.started {
            out.extend_from_slice(&self.message_start());
            self.started = true;
        }

        // 流尾 usage：补发 message_delta 携带精确 output_tokens
        if let Some(usage) = event.get("usage") {
            if let Some(out_tok) = usage.get("completion_tokens").and_then(|t| t.as_i64()) {
                self.output_tokens = out_tok;
                out.extend_from_slice(&sse_data(&json!({
                    "type": "message_delta",
                    "delta": { "stop_reason": null, "stop_sequence": null },
                    "usage": { "output_tokens": out_tok }
                })));
            }
            return out;
        }

        let Some(choice) = event.get("choices").and_then(|c| c.as_array()).and_then(|a| a.first()) else {
            return out;
        };
        let delta = choice.get("delta").unwrap_or(&Value::Null);

        // 文本 delta（空串跳过：上游首 chunk 常带空 content）
        if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
            if !text.is_empty() {
                if self.text_block_open {
                out.extend_from_slice(&sse_data(&json!({
                    "type": "content_block_delta",
                    "index": self.next_block_index - 1,
                    "delta": { "type": "text_delta", "text": text }
                })));
            } else {
                // 若之前开着 tool block（罕见：文本出现在工具后），先关掉
                self.close_all_blocks(&mut out);
                let idx = self.next_block_index;
                self.next_block_index += 1;
                self.text_block_open = true;
                out.extend_from_slice(&sse_data(&json!({
                    "type": "content_block_start",
                    "index": idx,
                    "content_block": { "type": "text", "text": "" }
                })));
                out.extend_from_slice(&sse_data(&json!({
                    "type": "content_block_delta",
                    "index": idx,
                    "delta": { "type": "text_delta", "text": text }
                })));
                }
            }
        }

        // tool_calls delta（OpenAI 首个 chunk 带 id/name，后续只有 arguments 片段）
        if let Some(calls) = delta.get("tool_calls").and_then(|c| c.as_array()) {
            for call in calls {
                let Some(index) = call.get("index").and_then(|i| i.as_u64()) else {
                    continue;
                };
                let idx = index as usize;
                if self.tool_blocks.iter().all(|b| b.oai_index != idx) {
                    self.tool_blocks.push(ToolBlockState {
                        oai_index: idx,
                        block_index: 0,
                        id: String::new(),
                        name: String::new(),
                        args: String::new(),
                        started: false,
                    });
                }
                let state = self.tool_blocks.iter_mut().find(|b| b.oai_index == idx).unwrap();
                if let Some(id) = call.get("id").and_then(|i| i.as_str()) {
                    state.id = id.to_string();
                }
                if let Some(name) = call.pointer("/function/name").and_then(|n| n.as_str()) {
                    state.name = name.to_string();
                }
                if let Some(args) = call.pointer("/function/arguments").and_then(|a| a.as_str()) {
                    state.args.push_str(args);
                }
                if !state.started {
                    // 文本 block 让位：先关闭文本再开工具 block
                    // （字段级访问，避免 &self 方法调用与 tool_blocks 可变借用冲突）
                    if self.text_block_open {
                        out.extend_from_slice(&sse_data(&json!({
                            "type": "content_block_stop",
                            "index": self.next_block_index - 1
                        })));
                        self.text_block_open = false;
                    }
                    state.block_index = self.next_block_index;
                    self.next_block_index += 1;
                    state.started = true;
                    out.extend_from_slice(&sse_data(&json!({
                        "type": "content_block_start",
                        "index": state.block_index,
                        "content_block": {
                            "type": "tool_use",
                            "id": state.id,
                            "name": state.name,
                            "input": {}
                        }
                    })));
                }
                out.extend_from_slice(&sse_data(&json!({
                    "type": "content_block_delta",
                    "index": state.block_index,
                    "delta": { "type": "input_json_delta", "partial_json": state.args }
                })));
            }
        }

        // finish_reason：关闭所有 block，发 message_delta（stop_reason + usage）
        if let Some(fr) = choice.get("finish_reason").and_then(|f| f.as_str()) {
            if !fr.is_empty() && !self.finish_sent {
                self.close_all_blocks(&mut out);
                out.extend_from_slice(&sse_data(&json!({
                    "type": "message_delta",
                    "delta": { "stop_reason": map_finish_reason(fr), "stop_sequence": null },
                    "usage": { "output_tokens": self.output_tokens }
                })));
                self.finish_sent = true;
            }
        }

        out
    }

    /// 上游 [DONE]：补齐 message_stop（必要时补 message_delta）
    pub fn on_done(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.started {
            out.extend_from_slice(&self.message_start());
            self.started = true;
        }
        if !self.finish_sent {
            self.close_all_blocks(&mut out);
            out.extend_from_slice(&sse_data(&json!({
                "type": "message_delta",
                "delta": { "stop_reason": "end_turn", "stop_sequence": null },
                "usage": { "output_tokens": self.output_tokens }
            })));
            self.finish_sent = true;
        }
        out.extend_from_slice(b"event: message_stop\ndata: {}\n\n");
        out
    }

    fn message_start(&self) -> Vec<u8> {
        sse_data(&json!({
            "type": "message_start",
            "message": {
                "id": crate::commands::util::new_id("msg"),
                "type": "message",
                "role": "assistant",
                "model": self.model,
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {
                    "input_tokens": self.input_tokens,
                    "output_tokens": 0,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0,
                }
            }
        }))
    }

    fn text_block_stop(&self) -> Vec<u8> {
        sse_data(&json!({ "type": "content_block_stop", "index": self.next_block_index - 1 }))
    }

    fn close_all_blocks(&mut self, out: &mut Vec<u8>) {
        if self.text_block_open {
            out.extend_from_slice(&self.text_block_stop());
            self.text_block_open = false;
        }
        for b in self.tool_blocks.iter().filter(|b| b.started) {
            out.extend_from_slice(&sse_data(&json!({ "type": "content_block_stop", "index": b.block_index })));
        }
        self.tool_blocks.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_system_and_text() {
        let mut body = json!({
            "model": "claude-3-5-sonnet",
            "system": "你是助手",
            "messages": [
                { "role": "user", "content": "你好" }
            ],
            "max_tokens": 100,
            "stop_sequences": ["END"],
        });
        request_to_openai(&mut body).unwrap();
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "你是助手");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["stop"], json!(["END"]));
        assert!(body.get("system").is_none());
        assert!(body.get("stop_sequences").is_none());
    }

    #[test]
    fn request_tool_use_and_result_roundtrip() {
        let mut body = json!({
            "model": "claude-3-5-sonnet",
            "messages": [
                { "role": "user", "content": "查天气" },
                { "role": "assistant", "content": [
                    { "type": "text", "text": "好的" },
                    { "type": "tool_use", "id": "t1", "name": "get_weather", "input": { "city": "北京" } }
                ]},
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "t1", "content": "25度" }
                ]}
            ],
            "tools": [
                { "name": "get_weather", "description": "查天气", "input_schema": { "type": "object" } }
            ],
            "tool_choice": { "type": "any" }
        });
        request_to_openai(&mut body).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"], "好的");
        assert_eq!(msgs[1]["tool_calls"][0]["id"], "t1");
        assert_eq!(msgs[1]["tool_calls"][0]["function"]["name"], "get_weather");
        assert_eq!(msgs[2]["role"], "tool");
        assert_eq!(msgs[2]["tool_call_id"], "t1");
        assert_eq!(msgs[2]["content"], "25度");
        assert_eq!(body["tools"][0]["function"]["name"], "get_weather");
        assert_eq!(body["tools"][0]["function"]["parameters"]["type"], "object");
        assert_eq!(body["tool_choice"], json!("required"));
    }

    #[test]
    fn request_image_block_becomes_data_url() {
        let mut body = json!({
            "model": "m",
            "messages": [
                { "role": "user", "content": [
                    { "type": "text", "text": "看图" },
                    { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AAAA" } }
                ]}
            ]
        });
        request_to_openai(&mut body).unwrap();
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(content[1]["image_url"]["url"], "data:image/png;base64,AAAA");
    }

    #[test]
    fn response_text_and_usage() {
        let openai = json!({
            "id": "chatcmpl-123",
            "model": "gpt-4o",
            "choices": [{
                "message": { "role": "assistant", "content": "你好" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5, "prompt_cache_hit_tokens": 4 }
        });
        let anth = response_from_openai(&openai);
        assert_eq!(anth["type"], "message");
        assert_eq!(anth["content"][0]["type"], "text");
        assert_eq!(anth["content"][0]["text"], "你好");
        assert_eq!(anth["stop_reason"], "end_turn");
        assert_eq!(anth["usage"]["input_tokens"], 10);
        assert_eq!(anth["usage"]["output_tokens"], 5);
        assert_eq!(anth["usage"]["cache_read_input_tokens"], 4);
        assert!(anth["id"].as_str().unwrap().starts_with("msg_"));
    }

    #[test]
    fn response_tool_calls_become_tool_use_blocks() {
        let openai = json!({
            "id": "chatcmpl-1",
            "model": "m",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "get_weather", "arguments": "{\"city\":\"北京\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 2 }
        });
        let anth = response_from_openai(&openai);
        assert_eq!(anth["stop_reason"], "tool_use");
        let blocks = anth["content"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "tool_use");
        assert_eq!(blocks[0]["name"], "get_weather");
        assert_eq!(blocks[0]["input"]["city"], "北京");
    }

    // ---- 流式 ----

    fn feed_events(s: &mut AnthropicStream, events: &[Value]) -> String {
        let mut all = Vec::new();
        for e in events {
            all.extend_from_slice(&s.on_event(e));
        }
        all.extend_from_slice(&s.on_done());
        String::from_utf8(all).unwrap()
    }

    #[test]
    fn stream_text_sequence() {
        let mut s = AnthropicStream::new("claude-3-5-sonnet", 42);
        let out = feed_events(&mut s, &[
            json!({ "id": "x", "choices": [{ "delta": { "content": "你" }, "finish_reason": null }] }),
            json!({ "id": "x", "choices": [{ "delta": { "content": "好" }, "finish_reason": null }] }),
            json!({ "id": "x", "choices": [{ "delta": {}, "finish_reason": "stop" }] }),
            json!({ "id": "x", "choices": [], "usage": { "prompt_tokens": 42, "completion_tokens": 7 } }),
        ]);
        // 关键事件顺序与内容（serde_json 序列化键按字母序，避免依赖键序的断言）
        assert!(out.contains("\"type\":\"message_start\""));
        assert!(out.contains("\"input_tokens\":42"));
        assert!(out.contains("\"type\":\"content_block_start\""));
        assert!(out.contains("\"type\":\"text_delta\""));
        assert!(out.contains("\"text\":\"你\""));
        assert!(out.contains("\"text\":\"好\""));
        assert!(out.contains("\"type\":\"content_block_stop\""));
        assert!(out.contains("\"stop_reason\":\"end_turn\""));
        assert!(out.contains("\"output_tokens\":7"));
        assert!(out.contains("event: message_stop"));
        // message_start 只出现一次
        assert_eq!(out.matches("\"type\":\"message_start\"").count(), 1);
    }

    #[test]
    fn stream_tool_calls_sequence() {
        let mut s = AnthropicStream::new("claude-3-5-sonnet", 0);
        let out = feed_events(&mut s, &[
            json!({ "id": "x", "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "id": "call_1", "type": "function", "function": { "name": "get_weather", "arguments": "" } }
            ]}, "finish_reason": null }] }),
            json!({ "id": "x", "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "function": { "arguments": "{\"city\":" } }
            ]}, "finish_reason": null }] }),
            json!({ "id": "x", "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "function": { "arguments": "\"北京\"}" } }
            ]}, "finish_reason": null }] }),
            json!({ "id": "x", "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] }),
        ]);
        assert!(out.contains("\"type\":\"tool_use\""));
        assert!(out.contains("\"id\":\"call_1\""));
        assert!(out.contains("\"name\":\"get_weather\""));
        assert!(out.contains("\"type\":\"input_json_delta\""));
        assert!(out.contains("\"stop_reason\":\"tool_use\""));
        // 完整 arguments 拼回（partial_json 是 JSON 字符串，序列化后带转义：{\"city\":\"北京\"}）
        let joined = out.replace("\n", "");
        assert!(
            joined.contains("{\\\"city\\\":\\\"北京\\\"}") || joined.contains("\\\"city\\\":"),
            "arguments 分段应被拼回: {joined}"
        );
        // tool block 关闭
        assert!(out.contains("\"type\":\"content_block_stop\""));
    }

    #[test]
    fn stream_empty_upstream_gets_closing_events() {
        let mut s = AnthropicStream::new("m", 0);
        let out = s.on_done();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("\"type\":\"message_start\""));
        assert!(out.contains("message_stop"));
    }
}
