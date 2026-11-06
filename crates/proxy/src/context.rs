//! Request-side text estimates. Provider totals remain authoritative and separate.
use serde::Serialize;
use serde_json::Value;

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
            out.tool_definitions = estimate_json(tools);
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
        let tokens = (text.len() as u64).div_ceil(4);
        match category {
            Category::System => self.system += tokens,
            Category::ToolResult => self.tool_results += tokens,
            Category::Conversation if text.starts_with("📦 [acp] Compressed turns ") => {
                self.compression += tokens
            }
            Category::Conversation => self.conversation += tokens,
        }
    }

    fn content(&mut self, value: &Value, category: Category) {
        if matches!(category, Category::ToolResult) {
            let (bytes, images) = result_size(value);
            self.tool_results += bytes.div_ceil(4);
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
            self.tool_calls += estimate_json(value);
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
            self.tool_calls += estimate_json(calls);
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

fn estimate_json(value: &Value) -> u64 {
    serde_json::to_vec(value)
        .map(|bytes| (bytes.len() as u64).div_ceil(4))
        .unwrap_or(0)
}

// Account arbitrary structured tool output while excluding embedded image payloads.
fn result_size(value: &Value) -> (u64, u64) {
    match value {
        Value::Null => (0, 0),
        Value::Array(items) => items.iter().map(result_size).fold((0, 0), sum_sizes),
        Value::Object(fields) => {
            if is_image(value, value["type"].as_str().unwrap_or("")) {
                return (0, 1);
            }
            fields
                .iter()
                .map(|(key, value)| {
                    let (bytes, images) = result_size(value);
                    (key.len() as u64 + bytes + 3, images)
                })
                .fold((0, 0), sum_sizes)
        }
        Value::String(text) => (text.len() as u64, 0),
        other => (other.to_string().len() as u64, 0),
    }
}

fn sum_sizes(a: (u64, u64), b: (u64, u64)) -> (u64, u64) {
    (a.0 + b.0, a.1 + b.1)
}
