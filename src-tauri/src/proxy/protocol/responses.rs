//! OpenAI Responses API（/v1/responses）↔ 内部 chat/completions 转换。
//!
//! 结构差异要点：
//! - 请求：顶层 `instructions`、`input`（字符串或 items 数组：message/function_call/
//!   function_call_output/reasoning）、`tools` 为 `{type:function, name, description, parameters}`
//!   扁平结构、`max_output_tokens`
//! - 响应：`output` items 数组（message 项 content 为 output_text parts；function_call 项）
//! - 流式：`response.created → response.in_progress → response.output_item.added →
//!   response.content_part.added → response.output_text.delta → ... → response.output_item.done
//!   → response.completed`（工具为 function_call_arguments.delta/done）

use serde_json::{json, Value};

use super::sse_data;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------- 请求转换 ----------------

/// Responses 请求 → OpenAI chat/completions（就地修改）
pub fn request_to_openai(body: &mut Value) -> Result<(), String> {
    let mut messages: Vec<Value> = Vec::new();

    // instructions（字符串）→ system 消息
    if let Some(inst) = body.get("instructions").and_then(|i| i.as_str()) {
        if !inst.is_empty() {
            messages.push(json!({ "role": "system", "content": inst }));
        }
    }

    // input：字符串 → user；数组 → items 逐项转换
    if let Some(input) = body.get("input").cloned() {
        match input {
            Value::String(s) => messages.push(json!({ "role": "user", "content": s })),
            Value::Array(items) => {
                for item in items {
                    let Some(it) = item.get("type").and_then(|t| t.as_str()) else {
                        continue;
                    };
                    match it {
                        "message" => {
                            let role = item.get("role").and_then(|r| r.as_str()).unwrap_or("user");
                            let content = item.get("content").cloned().unwrap_or(Value::Null);
                            let converted = match content {
                                Value::String(s) => Value::String(s),
                                Value::Array(parts) => {
                                    // input_text / input_image parts → chat content
                                    let mut text_parts: Vec<String> = Vec::new();
                                    let mut rich: Vec<Value> = Vec::new();
                                    for p in parts {
                                        match p.get("type").and_then(|t| t.as_str()) {
                                            Some("input_text") => {
                                                if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                                                    text_parts.push(t.to_string());
                                                    rich.push(json!({ "type": "text", "text": t }));
                                                }
                                            }
                                            Some("input_image") => {
                                                if let Some(url) = p.get("image_url").and_then(|u| u.as_str()) {
                                                    rich.push(json!({ "type": "image_url", "image_url": { "url": url } }));
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    if rich.iter().any(|r| r["type"] == "image_url") {
                                        Value::Array(rich)
                                    } else {
                                        Value::String(text_parts.join("\n"))
                                    }
                                }
                                other => other,
                            };
                            messages.push(json!({ "role": role, "content": converted }));
                        }
                        "function_call" => {
                            // 历史中的模型工具调用项 → assistant 消息带 tool_calls
                            let id = item.get("call_id").and_then(|i| i.as_str()).unwrap_or("");
                            let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
                            let args = item.get("arguments").cloned().unwrap_or(Value::String("{}".into()));
                            let args_str = args.as_str().map(|s| s.to_string()).unwrap_or_else(|| args.to_string());
                            messages.push(json!({
                                "role": "assistant",
                                "content": null,
                                "tool_calls": [{
                                    "id": id,
                                    "type": "function",
                                    "function": { "name": name, "arguments": args_str }
                                }]
                            }));
                        }
                        "function_call_output" => {
                            let call_id = item.get("call_id").and_then(|i| i.as_str()).unwrap_or("");
                            let output = item.get("output").cloned().unwrap_or(Value::Null);
                            messages.push(json!({
                                "role": "tool",
                                "tool_call_id": call_id,
                                "content": output,
                            }));
                        }
                        _ => {} // reasoning 等忽略
                    }
                }
            }
            _ => {}
        }
    }

    // tools：{type:function, name, description, parameters} → OpenAI 结构
    if let Some(tools) = body.get("tools").and_then(|t| t.as_array()) {
        let converted: Vec<Value> = tools
            .iter()
            .filter_map(|t| {
                let name = t.get("name")?.as_str()?.to_string();
                let description = t.get("description").and_then(|d| d.as_str()).unwrap_or("");
                let parameters = t.get("parameters").cloned().unwrap_or(json!({}));
                Some(json!({
                    "type": "function",
                    "function": { "name": name, "description": description, "parameters": parameters }
                }))
            })
            .collect();
        body["tools"] = Value::Array(converted);
    }

    // tool_choice：字符串或 {type:function, name}
    if let Some(tc) = body.get("tool_choice").cloned() {
        body["tool_choice"] = match tc {
            Value::Object(o) => {
                let name = o.get("name").and_then(|n| n.as_str()).unwrap_or("");
                json!({ "type": "function", "function": { "name": name } })
            }
            other => other,
        };
    }

    // max_output_tokens → max_tokens
    if let Some(mot) = body.get("max_output_tokens").cloned() {
        body["max_tokens"] = mot;
    }

    body.as_object_mut().unwrap().remove("instructions");
    body.as_object_mut().unwrap().remove("input");
    body.as_object_mut().unwrap().remove("max_output_tokens");
    body.as_object_mut().unwrap().remove("store");
    body.as_object_mut().unwrap().remove("previous_response_id");
    body.as_object_mut().unwrap().remove("metadata");
    body.as_object_mut().unwrap().remove("reasoning");
    body.as_object_mut().unwrap().remove("parallel_tool_calls");

    body["messages"] = Value::Array(messages);
    Ok(())
}

// ---------------- 非流式响应转换 ----------------

/// OpenAI 非流式响应 → Responses 响应
pub fn response_from_openai(openai: &Value) -> Value {
    let id = crate::commands::util::new_id("resp");
    let model = openai.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let choice = openai.get("choices").and_then(|c| c.as_array()).and_then(|a| a.first());
    let msg = choice.and_then(|c| c.get("message"));

    let mut output: Vec<Value> = Vec::new();
    // message item
    let mut text_parts: Vec<String> = Vec::new();
    if let Some(content) = msg.and_then(|m| m.get("content")) {
        match content {
            Value::String(s) => text_parts.push(s.clone()),
            Value::Array(parts) => {
                for p in parts {
                    if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                        text_parts.push(t.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    let text = text_parts.join("");
    if !text.is_empty() || msg.and_then(|m| m.get("tool_calls")).is_none() {
        output.push(json!({
            "type": "message",
            "id": crate::commands::util::new_id("msg"),
            "status": "completed",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": text,
                "annotations": []
            }]
        }));
    }
    // function_call items
    if let Some(calls) = msg.and_then(|m| m.get("tool_calls")).and_then(|c| c.as_array()) {
        for call in calls {
            let call_id = call.get("id").and_then(|i| i.as_str()).unwrap_or("");
            let name = call.pointer("/function/name").and_then(|n| n.as_str()).unwrap_or("");
            let args = call.pointer("/function/arguments").and_then(|a| a.as_str()).unwrap_or("{}").to_string();
            output.push(json!({
                "type": "function_call",
                "id": crate::commands::util::new_id("fc"),
                "call_id": call_id,
                "name": name,
                "arguments": args,
            }));
        }
    }

    // usage
    let (input_tokens, output_tokens, cached, reasoning) = if let Some(u) = openai.get("usage") {
        (
            u.get("prompt_tokens").and_then(|t| t.as_i64()).unwrap_or(0),
            u.get("completion_tokens").and_then(|t| t.as_i64()).unwrap_or(0),
            u.get("prompt_cache_hit_tokens").and_then(|t| t.as_i64()).unwrap_or(0),
            u.get("completion_tokens_details").and_then(|d| d.get("reasoning_tokens")).and_then(|t| t.as_i64()).unwrap_or(0),
        )
    } else {
        (0, 0, 0, 0)
    };

    json!({
        "id": id,
        "object": "response",
        "created_at": now_secs(),
        "status": "completed",
        "model": model,
        "output": output,
        "parallel_tool_calls": true,
        "usage": {
            "input_tokens": input_tokens,
            "input_tokens_details": { "cached_tokens": cached },
            "output_tokens": output_tokens,
            "output_tokens_details": { "reasoning_tokens": reasoning },
            "total_tokens": input_tokens + output_tokens,
        }
    })
}

// ---------------- 流式转换（状态机） ----------------

/// Responses 流式翻译器：把 OpenAI delta 事件翻译成 Responses 事件序列。
pub struct ResponsesStream {
    model: String,
    input_tokens: i64,
    id: String,
    created_at: u64,
    started: bool,
    completed_sent: bool,
    text: String,
    text_item_id: String,
    text_open: bool,
    tool_states: Vec<RespToolState>,
    next_output_index: usize,
}

struct RespToolState {
    oai_index: usize,
    output_index: usize,
    item_id: String,
    call_id: String,
    name: String,
    args: String,
    open: bool,
}

impl ResponsesStream {
    pub fn new(model: &str, input_tokens: i64) -> Self {
        Self {
            model: model.to_string(),
            input_tokens,
            id: crate::commands::util::new_id("resp"),
            created_at: now_secs(),
            started: false,
            completed_sent: false,
            text: String::new(),
            text_item_id: String::new(),
            text_open: false,
            tool_states: Vec::new(),
            next_output_index: 0,
        }
    }

    pub fn on_event(&mut self, event: &Value) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.started {
            out.extend_from_slice(&self.response_created("in_progress"));
            self.started = true;
        }

        // 流尾 usage：response.completed
        if let Some(usage) = event.get("usage") {
            self.completed_sent = true;
            out.extend_from_slice(&self.response_completed(usage));
            return out;
        }

        let Some(choice) = event.get("choices").and_then(|c| c.as_array()).and_then(|a| a.first()) else {
            return out;
        };
        let delta = choice.get("delta").unwrap_or(&Value::Null);

        // 文本 delta（空串跳过：上游首 chunk 常带空 content）
        if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
            if !text.is_empty() {
                if !self.text_open {
                    self.ensure_text_item(&mut out);
                }
                self.text.push_str(text);
                out.extend_from_slice(&sse_data(&json!({
                    "type": "response.output_text.delta",
                    "item_id": self.text_item_id,
                    "output_index": 0,
                    "content_index": 0,
                    "delta": text
                })));
            }
        }

        // 工具 delta
        if let Some(calls) = delta.get("tool_calls").and_then(|c| c.as_array()) {
            for call in calls {
                let Some(index) = call.get("index").and_then(|i| i.as_u64()) else {
                    continue;
                };
                let idx = index as usize;
                if self.tool_states.iter().all(|t| t.oai_index != idx) {
                    self.tool_states.push(RespToolState {
                        oai_index: idx,
                        output_index: 0,
                        item_id: String::new(),
                        call_id: String::new(),
                        name: String::new(),
                        args: String::new(),
                        open: false,
                    });
                }
                let state = self.tool_states.iter_mut().find(|t| t.oai_index == idx).unwrap();
                if let Some(id) = call.get("id").and_then(|i| i.as_str()) {
                    state.call_id = id.to_string();
                }
                if let Some(name) = call.pointer("/function/name").and_then(|n| n.as_str()) {
                    state.name = name.to_string();
                }
                if let Some(args) = call.pointer("/function/arguments").and_then(|a| a.as_str()) {
                    state.args.push_str(args);
                }
                if !state.open {
                    state.open = true;
                    state.output_index = self.next_output_index;
                    self.next_output_index += 1;
                    state.item_id = crate::commands::util::new_id("fc");
                    out.extend_from_slice(&sse_data(&json!({
                        "type": "response.output_item.added",
                        "output_index": state.output_index,
                        "item": {
                            "type": "function_call",
                            "id": state.item_id,
                            "call_id": state.call_id,
                            "name": state.name,
                            "arguments": ""
                        }
                    })));
                }
                out.extend_from_slice(&sse_data(&json!({
                    "type": "response.function_call_arguments.delta",
                    "item_id": state.item_id,
                    "output_index": state.output_index,
                    "delta": state.args
                })));
            }
        }

        // finish_reason：关闭打开的 part / item
        if let Some(fr) = choice.get("finish_reason").and_then(|f| f.as_str()) {
            if !fr.is_empty() && !self.completed_sent {
                if self.text_open {
                    out.extend_from_slice(&self.text_item_done());
                }
                for t in self.tool_states.iter_mut().filter(|t| t.open) {
                    out.extend_from_slice(&sse_data(&json!({
                        "type": "response.function_call_arguments.done",
                        "item_id": t.item_id,
                        "output_index": t.output_index,
                        "arguments": t.args
                    })));
                    out.extend_from_slice(&sse_data(&json!({
                        "type": "response.output_item.done",
                        "output_index": t.output_index,
                        "item": {
                            "type": "function_call",
                            "id": t.item_id,
                            "call_id": t.call_id,
                            "name": t.name,
                            "arguments": t.args
                        }
                    })));
                    t.open = false;
                }
            }
        }

        out
    }

    /// 上游 [DONE]：补发 response.completed（若 usage 事件未到）
    pub fn on_done(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.started {
            out.extend_from_slice(&self.response_created("in_progress"));
            self.started = true;
        }
        if !self.completed_sent {
            // 关闭未完成的 part/item（异常收尾兜底）
            if self.text_open {
                out.extend_from_slice(&self.text_item_done());
            }
            for t in self.tool_states.iter_mut().filter(|t| t.open) {
                t.open = false;
            }
            self.completed_sent = true;
            out.extend_from_slice(&self.response_completed(&Value::Null));
        }
        out
    }

    fn response_created(&self, status: &str) -> Vec<u8> {
        sse_data(&json!({
            "type": "response.created",
            "response": {
                "id": self.id,
                "object": "response",
                "created_at": self.created_at,
                "status": status,
                "model": self.model,
                "output": [],
                "parallel_tool_calls": true,
                "usage": null
            }
        }))
    }

    fn ensure_text_item(&mut self, out: &mut Vec<u8>) {
        self.text_open = true;
        self.text_item_id = crate::commands::util::new_id("msg");
        out.extend_from_slice(&sse_data(&json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "message",
                "id": self.text_item_id,
                "status": "in_progress",
                "role": "assistant",
                "content": []
            }
        })));
        out.extend_from_slice(&sse_data(&json!({
            "type": "response.content_part.added",
            "item_id": self.text_item_id,
            "output_index": 0,
            "content_index": 0,
            "part": { "type": "output_text", "text": "", "annotations": [] }
        })));
    }

    fn text_item_done(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        // 注意：不清空 self.text —— response.completed 的 output 需要完整文本
        let text = self.text.clone();
        self.text_open = false;
        out.extend_from_slice(&sse_data(&json!({
            "type": "response.output_text.done",
            "item_id": self.text_item_id,
            "output_index": 0,
            "content_index": 0,
            "text": text
        })));
        out.extend_from_slice(&sse_data(&json!({
            "type": "response.content_part.done",
            "item_id": self.text_item_id,
            "output_index": 0,
            "content_index": 0,
            "part": { "type": "output_text", "text": text, "annotations": [] }
        })));
        out.extend_from_slice(&sse_data(&json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {
                "type": "message",
                "id": self.text_item_id,
                "status": "completed",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": text, "annotations": [] }]
            }
        })));
        out
    }

    fn response_completed(&self, usage: &Value) -> Vec<u8> {
        let mut output: Vec<Value> = Vec::new();
        if self.text_open || !self.text.is_empty() {
            let text = self.text.clone();
            output.push(json!({
                "type": "message",
                "id": self.text_item_id,
                "status": "completed",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": text, "annotations": [] }]
            }));
        }
        for t in &self.tool_states {
            if !t.args.is_empty() {
                output.push(json!({
                    "type": "function_call",
                    "id": t.item_id,
                    "call_id": t.call_id,
                    "name": t.name,
                    "arguments": t.args
                }));
            }
        }
        let (input_tokens, output_tokens, cached) = if usage.is_null() {
            (self.input_tokens, 0, 0)
        } else {
            (
                usage.get("prompt_tokens").and_then(|t| t.as_i64()).unwrap_or(0),
                usage.get("completion_tokens").and_then(|t| t.as_i64()).unwrap_or(0),
                usage.get("prompt_cache_hit_tokens").and_then(|t| t.as_i64()).unwrap_or(0),
            )
        };
        sse_data(&json!({
            "type": "response.completed",
            "response": {
                "id": self.id,
                "object": "response",
                "created_at": self.created_at,
                "status": "completed",
                "model": self.model,
                "output": output,
                "parallel_tool_calls": true,
                "usage": {
                    "input_tokens": input_tokens,
                    "input_tokens_details": { "cached_tokens": cached },
                    "output_tokens": output_tokens,
                    "output_tokens_details": { "reasoning_tokens": 0 },
                    "total_tokens": input_tokens + output_tokens
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_string_input_and_instructions() {
        let mut body = json!({
            "model": "gpt-5",
            "instructions": "你是助手",
            "input": "你好",
            "max_output_tokens": 100,
        });
        request_to_openai(&mut body).unwrap();
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "你是助手");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["max_tokens"], 100);
        assert!(body.get("instructions").is_none());
        assert!(body.get("input").is_none());
        assert!(body.get("max_output_tokens").is_none());
    }

    #[test]
    fn request_items_with_function_history() {
        let mut body = json!({
            "model": "m",
            "input": [
                { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "查天气" }] },
                { "type": "function_call", "call_id": "call_1", "name": "get_weather", "arguments": "{\"city\":\"北京\"}" },
                { "type": "function_call_output", "call_id": "call_1", "output": "25度" }
            ],
            "tools": [
                { "type": "function", "name": "get_weather", "description": "查天气", "parameters": { "type": "object" } }
            ],
            "tool_choice": { "type": "function", "name": "get_weather" }
        });
        request_to_openai(&mut body).unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "查天气");
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["tool_calls"][0]["id"], "call_1");
        assert_eq!(msgs[1]["tool_calls"][0]["function"]["arguments"], "{\"city\":\"北京\"}");
        assert_eq!(msgs[2]["role"], "tool");
        assert_eq!(msgs[2]["content"], "25度");
        assert_eq!(body["tools"][0]["function"]["name"], "get_weather");
        assert_eq!(body["tool_choice"]["function"]["name"], "get_weather");
    }

    #[test]
    fn response_text_and_tool_items() {
        let openai = json!({
            "id": "chatcmpl-1",
            "model": "m",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "你好",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "get_weather", "arguments": "{}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 10, "completion_tokens": 5, "prompt_cache_hit_tokens": 3,
                "completion_tokens_details": { "reasoning_tokens": 2 }
            }
        });
        let resp = response_from_openai(&openai);
        assert_eq!(resp["object"], "response");
        assert_eq!(resp["status"], "completed");
        let out = resp["output"].as_array().unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["type"], "message");
        assert_eq!(out[0]["content"][0]["text"], "你好");
        assert_eq!(out[1]["type"], "function_call");
        assert_eq!(out[1]["call_id"], "call_1");
        assert_eq!(resp["usage"]["input_tokens_details"]["cached_tokens"], 3);
        assert_eq!(resp["usage"]["output_tokens_details"]["reasoning_tokens"], 2);
    }

    #[test]
    fn stream_text_sequence() {
        let mut s = ResponsesStream::new("m", 10);
        let mut all = Vec::new();
        for e in [
            json!({ "id": "x", "choices": [{ "delta": { "content": "你" }, "finish_reason": null }] }),
            json!({ "id": "x", "choices": [{ "delta": { "content": "好" }, "finish_reason": null }] }),
            json!({ "id": "x", "choices": [{ "delta": {}, "finish_reason": "stop" }] }),
            json!({ "id": "x", "choices": [], "usage": { "prompt_tokens": 10, "completion_tokens": 2 } }),
        ] {
            all.extend_from_slice(&s.on_event(&e));
        }
        all.extend_from_slice(&s.on_done());
        let out = String::from_utf8(all).unwrap();
        assert!(out.contains("\"type\":\"response.created\""));
        assert!(out.contains("\"type\":\"response.output_item.added\""));
        assert!(out.contains("\"type\":\"response.output_text.delta\""));
        assert!(out.contains("\"delta\":\"你\""));
        assert!(out.contains("\"type\":\"response.output_text.done\""));
        assert!(out.contains("\"type\":\"response.output_item.done\""));
        assert!(out.contains("\"type\":\"response.completed\""));
        assert!(out.contains("\"output_tokens\":2"));
        // completed 只发一次
        assert_eq!(out.matches("\"type\":\"response.completed\"").count(), 1);
    }

    #[test]
    fn stream_tool_call_sequence() {
        let mut s = ResponsesStream::new("m", 0);
        let mut all = Vec::new();
        for e in [
            json!({ "id": "x", "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "id": "call_1", "type": "function", "function": { "name": "get_weather", "arguments": "" } }
            ]}, "finish_reason": null }] }),
            json!({ "id": "x", "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "function": { "arguments": "{}" } }
            ]}, "finish_reason": null }] }),
            json!({ "id": "x", "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] }),
            json!({ "id": "x", "choices": [], "usage": { "prompt_tokens": 1, "completion_tokens": 2 } }),
        ] {
            all.extend_from_slice(&s.on_event(&e));
        }
        all.extend_from_slice(&s.on_done());
        let out = String::from_utf8(all).unwrap();
        assert!(out.contains("\"type\":\"function_call\""));
        assert!(out.contains("\"type\":\"response.function_call_arguments.delta\""));
        assert!(out.contains("\"type\":\"response.function_call_arguments.done\""));
        assert!(out.contains("\"call_id\":\"call_1\""));
        assert!(out.contains("\"type\":\"response.completed\""));
    }
}
