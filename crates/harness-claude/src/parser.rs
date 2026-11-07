//! `claude`'s newline-delimited JSON as `HarnessEvent`s: one pure function over
//! one line, so the recorded fixtures pin the mapping without a process.
use harness::HarnessEvent;
use serde_json::Value;

/// What the CLI reports once at startup: the conversation id and what it loaded.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Init {
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub tools: Vec<String>,
    pub mcp_servers: Vec<Value>,
}

impl Init {
    /// True when the server `name` finished connecting; anything else is a refusal.
    pub fn connected(&self, name: &str) -> bool {
        self.mcp_servers
            .iter()
            .any(|server| server["name"] == name && server["status"] == "connected")
    }
}

/// One parsed line: startup facts, or events for the Session.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Event(HarnessEvent),
    Init(Init),
}

/// Unknown types, unknown subtypes and malformed lines yield nothing, never an error.
pub fn parse_line(line: &str) -> Vec<Output> {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    match value["type"].as_str() {
        Some("system") => system(&value),
        Some("stream_event") => stream(&value["event"]),
        Some("assistant") => assistant(&value),
        Some("user") => user(&value),
        Some("result") => result(&value),
        _ => Vec::new(),
    }
}

fn system(value: &Value) -> Vec<Output> {
    if value["subtype"] != "init" {
        return Vec::new();
    }
    vec![Output::Init(Init {
        session_id: value["session_id"].as_str().map(str::to_string),
        model: value["model"].as_str().map(str::to_string),
        tools: names(&value["tools"]),
        mcp_servers: list(&value["mcp_servers"]),
    })]
}

fn stream(event: &Value) -> Vec<Output> {
    if event["type"] != "content_block_delta" {
        return Vec::new();
    }
    let delta = &event["delta"];
    let text = delta["text"].as_str().or(delta["thinking"].as_str());
    match (delta["type"].as_str(), text) {
        (Some("text_delta"), Some(text)) => event_out(HarnessEvent::AssistantChunk(text.into())),
        (Some("thinking_delta"), Some(text)) => event_out(HarnessEvent::ThoughtChunk(text.into())),
        _ => Vec::new(),
    }
}

fn assistant(value: &Value) -> Vec<Output> {
    let message = &value["message"];
    let content = list(&message["content"]);
    let error = value["error"].as_str().or(message["error"].as_str());
    if error.is_some() || value["is_api_error_message"] == true {
        let text = text_blocks(&content).unwrap_or_else(|| error.unwrap_or("api error").into());
        return event_out(HarnessEvent::Error(text));
    }
    content
        .iter()
        .filter(|block| block["type"] == "tool_use")
        .filter_map(|block| {
            Some(Output::Event(HarnessEvent::ToolStarted {
                id: block["id"].as_str()?.into(),
                name: block["name"].as_str()?.into(),
                input: block["input"].clone(),
            }))
        })
        .collect()
}

fn user(value: &Value) -> Vec<Output> {
    list(&value["message"]["content"])
        .iter()
        .filter(|block| block["type"] == "tool_result")
        .filter_map(|block| {
            Some(Output::Event(HarnessEvent::ToolFinished {
                id: block["tool_use_id"].as_str()?.into(),
                output: block["content"].clone(),
                status: (block["is_error"] == true).then(|| "error".to_string()),
            }))
        })
        .collect()
}

fn result(value: &Value) -> Vec<Output> {
    let mut out = Vec::new();
    if value["is_error"] == true {
        let text = value["result"]
            .as_str()
            .unwrap_or("claude reported an error");
        out.extend(event_out(HarnessEvent::Error(text.into())));
    }
    out.push(Output::Event(HarnessEvent::TurnFinished {
        stop_reason: value["stop_reason"].as_str().map(str::to_string),
        usage: (!value["usage"].is_null()).then(|| value["usage"].clone()),
    }));
    out
}

fn event_out(event: HarnessEvent) -> Vec<Output> {
    vec![Output::Event(event)]
}

fn list(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}

fn names(value: &Value) -> Vec<String> {
    list(value)
        .iter()
        .filter_map(|name| name.as_str().map(str::to_string))
        .collect()
}

fn text_blocks(content: &[Value]) -> Option<String> {
    let text: Vec<&str> = content
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect();
    (!text.is_empty()).then(|| text.join(""))
}
