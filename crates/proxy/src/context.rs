//! Request-side text estimates. Provider totals remain authoritative and separate.
use serde::Serialize;
use serde_json::Value;

use crate::marker;
use crate::tokens::{json_tokens, text_tokens};
use crate::wire::WireKind;

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextBreakdown {
    pub system: u64,
    pub tool_definitions: u64,
    pub tool_calls: u64,
    pub tool_results: u64,
    pub compression: u64,
    pub conversation: u64,
    pub images: u64,
    pub image_count: u64,
    pub estimated_tokens: u64,
}

#[derive(Clone, Copy)]
enum Category {
    System,
    Conversation,
    ToolResult,
}

impl ContextBreakdown {
    pub fn from_request(wire: WireKind, request: &Value) -> Self {
        let body = request
            .get("request")
            .filter(|v| v.is_object())
            .unwrap_or(request);
        let mut out = Self::default();
        if let Some(tools) = body.get("tools") {
            out.tool_definitions = json_tokens(tools);
        }
        match wire {
            WireKind::Google => {
                out.content(&body["systemInstruction"], Category::System);
                out.content(&body["contents"], Category::Conversation);
            }
            WireKind::Anthropic => {
                out.content(&body["system"], Category::System);
                out.content(&body["messages"], Category::Conversation);
            }
            WireKind::OpenAiChat => out.content(&body["messages"], Category::Conversation),
            WireKind::OpenAiResponses => {
                out.content(&body["instructions"], Category::System);
                out.content(&body["input"], Category::Conversation);
            }
        }
        out.estimated_tokens = out.system
            + out.tool_definitions
            + out.tool_calls
            + out.tool_results
            + out.compression
            + out.conversation;
        out
    }

    fn text(&mut self, text: &str, category: Category) {
        let tokens = text_tokens(text);
        match category {
            Category::System => self.system += tokens,
            Category::ToolResult => self.tool_results += tokens,
            Category::Conversation if marker::is_summary(text) => self.compression += tokens,
            Category::Conversation => self.conversation += tokens,
        }
    }

    fn content(&mut self, value: &Value, category: Category) {
        if matches!(category, Category::ToolResult) {
            let (tokens, images) = result_tokens(value);
            self.tool_results += tokens;
            self.image_count += images;
            return;
        }
        match value {
            Value::String(text) => self.text(text, category),
            Value::Array(items) => {
                for item in items {
                    self.content(item, category);
                }
            }
            Value::Object(_) => self.block(value, category),
            _ => {}
        }
    }

    fn block(&mut self, value: &Value, category: Category) {
        let kind = value["type"].as_str().unwrap_or("");
        if is_image(value, kind) {
            self.image_count += 1;
            return;
        }
        if matches!(kind, "tool_use" | "function_call") || value.get("functionCall").is_some() {
            self.tool_calls += json_tokens(value);
            return;
        }
        if let Some(response) = value.get("functionResponse") {
            self.content(&response["response"], Category::ToolResult);
            return;
        }
        if matches!(kind, "tool_result" | "function_call_output") {
            self.content(&value["content"], Category::ToolResult);
            self.content(&value["output"], Category::ToolResult);
            return;
        }
        self.message(value, category);
    }

    fn message(&mut self, value: &Value, category: Category) {
        let category = match value["role"].as_str() {
            Some("system" | "developer") => Category::System,
            Some("tool") => Category::ToolResult,
            _ => category,
        };
        if let Some(calls) = value.get("tool_calls") {
            self.tool_calls += json_tokens(calls);
        }
        // Traverse content fields only: roles, signatures, ids, and image data are not text.
        for key in [
            "content", "parts", "text", "result", "output", "response", "thinking",
        ] {
            if let Some(part) = value.get(key) {
                self.content(part, category);
            }
        }
    }
}

fn is_image(value: &Value, kind: &str) -> bool {
    matches!(kind, "image" | "input_image" | "image_url")
        || value.get("inlineData").is_some()
        || value.get("fileData").is_some()
}

// Account arbitrary structured tool output in tokens, excluding embedded image payloads.
fn result_tokens(value: &Value) -> (u64, u64) {
    match value {
        Value::Null => (0, 0),
        Value::Array(items) => items.iter().map(result_tokens).fold((0, 0), sum_counts),
        Value::Object(fields) => {
            if is_image(value, value["type"].as_str().unwrap_or("")) {
                return (0, 1);
            }
            fields
                .iter()
                .map(|(key, value)| {
                    let (tokens, images) = result_tokens(value);
                    (text_tokens(key) + tokens, images)
                })
                .fold((0, 0), sum_counts)
        }
        Value::String(text) => (text_tokens(text), 0),
        other => (text_tokens(&other.to_string()), 0),
    }
}

fn sum_counts(a: (u64, u64), b: (u64, u64)) -> (u64, u64) {
    (a.0 + b.0, a.1 + b.1)
}
