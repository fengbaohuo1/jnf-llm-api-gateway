//! 客户端协议翻译层：Anthropic Messages（/v1/messages）与 OpenAI Responses（/v1/responses）
//! 在**入口/出口边界**转成内部统一的 OpenAI chat/completions 格式，
//! 内部管线（鉴权 → 扫描 → 调度 → 转发 → 脱敏 → 记账）只认 OpenAI 格式，零分叉。
//!
//! 转换分四个方向：
//! - `to_openai`：请求（客户端协议 → OpenAI），就地修改
//! - `from_openai`：非流式响应（OpenAI → 客户端协议）
//! - `StreamTranslator`：流式响应逐事件翻译（OpenAI SSE 事件 → 客户端协议 SSE 事件）
//! - `error_body`：错误响应按客户端协议包装（Anthropic 客户端不认 OpenAI 错误结构）

pub mod anthropic;
pub mod responses;

use serde_json::{json, Value};

/// 客户端协议（路由路径决定）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientProtocol {
    OpenAi,
    Anthropic,
    Responses,
}

impl ClientProtocol {
    /// 协议名（存 requests.protocol 列用）
    pub fn as_str(self) -> &'static str {
        match self {
            ClientProtocol::OpenAi => "openai",
            ClientProtocol::Anthropic => "anthropic",
            ClientProtocol::Responses => "responses",
        }
    }
}

/// 请求转换：客户端协议 → 内部 OpenAI 格式（就地修改 body；返回错误消息）
pub fn to_openai(body: &mut Value, proto: ClientProtocol) -> Result<(), String> {
    match proto {
        ClientProtocol::OpenAi => Ok(()),
        ClientProtocol::Anthropic => anthropic::request_to_openai(body),
        ClientProtocol::Responses => responses::request_to_openai(body),
    }
}

/// 非流式响应转换：内部 OpenAI → 客户端协议
pub fn from_openai(body: Value, proto: ClientProtocol) -> Value {
    match proto {
        ClientProtocol::OpenAi => body,
        ClientProtocol::Anthropic => anthropic::response_from_openai(&body),
        ClientProtocol::Responses => responses::response_from_openai(&body),
    }
}

/// 错误响应体（按客户端协议包装）
pub fn error_body(proto: ClientProtocol, message: &str, code: &str) -> Value {
    match proto {
        ClientProtocol::OpenAi => json_error_openai(message, code),
        ClientProtocol::Anthropic => json!({
            "type": "error",
            "error": { "type": "api_error", "message": message }
        }),
        ClientProtocol::Responses => json_error_openai(message, code),
    }
}

fn json_error_openai(message: &str, code: &str) -> Value {
    json!({
        "error": {
            "message": message,
            "type": "invalid_request_error",
            "code": code,
        }
    })
}

/// 流式事件翻译器（Anthropic / Responses 需要；OpenAI 透传不走这里）
pub enum StreamTranslator {
    Anthropic(anthropic::AnthropicStream),
    Responses(responses::ResponsesStream),
}

impl StreamTranslator {
    pub fn new(proto: ClientProtocol, model: &str, input_tokens: i64) -> Self {
        match proto {
            ClientProtocol::Anthropic => {
                StreamTranslator::Anthropic(anthropic::AnthropicStream::new(model, input_tokens))
            }
            ClientProtocol::Responses => {
                StreamTranslator::Responses(responses::ResponsesStream::new(model, input_tokens))
            }
            ClientProtocol::OpenAi => unreachable!("OpenAI 流式走透传，不建翻译器"),
        }
    }

    /// 处理一个上游 OpenAI data 事件，返回要写出的 SSE 文本（含 data: 前缀与空行）。
    /// 返回空 vec 表示该事件不需要翻译输出（如纯统计的 usage 已并入其他事件）。
    pub fn on_event(&mut self, event: &Value) -> Vec<u8> {
        match self {
            StreamTranslator::Anthropic(s) => s.on_event(event),
            StreamTranslator::Responses(s) => s.on_event(event),
        }
    }

    /// 上游 [DONE]：补齐收尾事件（message_stop / response.completed）
    pub fn on_done(&mut self) -> Vec<u8> {
        match self {
            StreamTranslator::Anthropic(s) => s.on_done(),
            StreamTranslator::Responses(s) => s.on_done(),
        }
    }

    /// 安全拦截等中断：按客户端协议注入 error 事件
    pub fn error_event(&self, message: &str) -> Vec<u8> {
        let payload = match self {
            StreamTranslator::Anthropic(_) => json!({
                "type": "error",
                "error": { "type": "api_error", "message": message }
            }),
            StreamTranslator::Responses(_) => json!({
                "type": "error",
                "error": { "type": "api_error", "message": message }
            }),
        };
        format!("event: error\ndata: {}\n\n", payload).into_bytes()
    }
}

/// SSE 行：data: <json>\n\n
pub(crate) fn sse_data(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"data: ");
    out.extend_from_slice(serde_json::to_string(value).unwrap_or_default().as_bytes());
    out.extend_from_slice(b"\n\n");
    out
}

/// OpenAI 流式错误事件（原 stream.rs 行为，OpenAI 协议用）
pub(crate) fn openai_error_event(message: &str) -> Vec<u8> {
    format!(
        "event: error\ndata: {}\n\n",
        json!({
            "error": {
                "message": message,
                "type": "invalid_response",
                "code": "response_security_blocked",
            }
        })
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_bodies_follow_protocol_shape() {
        let openai = error_body(ClientProtocol::OpenAi, "x", "code1");
        assert_eq!(openai["error"]["message"], "x");
        let anth = error_body(ClientProtocol::Anthropic, "x", "code1");
        assert_eq!(anth["type"], "error");
        assert_eq!(anth["error"]["message"], "x");
        assert!(anth.get("error").unwrap().get("code").is_none());
        let resp = error_body(ClientProtocol::Responses, "x", "code1");
        assert_eq!(resp["error"]["code"], "code1");
    }
}
