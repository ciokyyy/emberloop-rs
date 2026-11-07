use prompts::PinnedPrompts;
use serde_json::{json, Value};

use crate::server::{ProxyCall, PROXY_TOOLS};
use crate::turnkeys;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireKind {
    Google,
    OpenAiChat,
    Anthropic,
    OpenAiResponses,
}

impl WireKind {
    pub fn from_rest(rest: &str) -> Option<(Self, Option<String>)> {
        if rest.ends_with("/chat/completions") {
            return Some((Self::OpenAiChat, Some("openai/v1/chat/completions".into())));
        }
        if rest.ends_with("/responses") {
            return Some((Self::OpenAiResponses, Some("openai/v1/responses".into())));
        }
        if rest.ends_with("/messages") {
            return Some((Self::Anthropic, Some("anthropic/v1/messages".into())));
        }
        if let Some(model) = rest.strip_prefix("v1/models/") {
            return Some((Self::Google, Some(format!("google/v1beta/models/{model}"))));
        }
        if rest.contains(":streamGenerateContent") || rest.contains(":generateContent") {
            return Some((Self::Google, None));
        }
        None
    }
}

pub fn prompt_text(pins: &PinnedPrompts, codebase_memory: bool) -> String {
    let mut ids = vec![prompts::POLICY, prompts::PONYTAIL, prompts::CAVEMAN];
    if codebase_memory {
        ids.push(prompts::SKILL_CODEBASE_MEMORY);
    }
    let mut text = String::new();
    for id in ids {
        let Some(part) = pins.text(id).filter(|t| !t.trim().is_empty()) else {
            continue;
        };
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(part);
    }
    text
}

pub fn inject_prompts_for_wire(
    wire: WireKind,
    request: &mut Value,
    pins: &PinnedPrompts,
    codebase_memory: bool,
) {
    let text = prompt_text(pins, codebase_memory);
    if text.is_empty() {
        return;
    }
    match wire {
        WireKind::Google => inject_google_prompt(request, &text),
        WireKind::OpenAiChat => inject_chat_prompt(request, &text),
        WireKind::Anthropic => inject_anthropic_prompt(request, &text),
        WireKind::OpenAiResponses => inject_responses_prompt(request, &text),
    }
}

fn inject_google_prompt(request: &mut Value, text: &str) {
    let part = json!({ "text": text });
    match request.get_mut("systemInstruction") {
        Some(Value::Object(instruction)) => match instruction.get_mut("parts") {
            Some(Value::Array(parts)) => parts.push(part),
            _ => {
                instruction.insert("parts".into(), json!([part]));
            }
        },
        _ => {
            request["systemInstruction"] = json!({ "parts": [part] });
        }
    }
}

fn inject_chat_prompt(request: &mut Value, text: &str) {
    let Some(messages) = request.get_mut("messages").and_then(Value::as_array_mut) else {
        request["messages"] = json!([{ "role": "system", "content": text }]);
        return;
    };
    if let Some(sys) = messages.iter_mut().find(|m| {
        matches!(
            m.get("role").and_then(Value::as_str),
            Some("system" | "developer")
        )
    }) {
        append_text_content(sys, text);
    } else {
        messages.insert(0, json!({ "role": "system", "content": text }));
    }
}

fn append_text_content(msg: &mut Value, text: &str) {
    match msg.get_mut("content") {
        Some(Value::String(existing)) => {
            if !existing.is_empty() {
                existing.push_str("\n\n");
            }
            existing.push_str(text);
        }
        Some(Value::Array(parts)) => {
            parts.push(json!({ "type": "text", "text": text }));
        }
        _ => {
            msg["content"] = json!(text);
        }
    }
}

fn inject_anthropic_prompt(request: &mut Value, text: &str) {
    match request.get_mut("system") {
        Some(Value::String(existing)) => {
            if !existing.is_empty() {
                existing.push_str("\n\n");
            }
            existing.push_str(text);
        }
        Some(Value::Array(blocks)) => {
            blocks.push(json!({ "type": "text", "text": text }));
        }
        _ => {
            request["system"] = json!(text);
        }
    }
}

fn inject_responses_prompt(request: &mut Value, text: &str) {
    match request.get_mut("instructions") {
        Some(Value::String(existing)) => {
            if !existing.is_empty() {
                existing.push_str("\n\n");
            }
            existing.push_str(text);
        }
        _ => {
            request["instructions"] = json!(text);
        }
    }
}

pub fn inject_tools_for_wire(wire: WireKind, request: &mut Value, pins: &PinnedPrompts) {
    let specs = tool_specs(pins);
    match wire {
        WireKind::Google => inject_google_tools(request, &specs),
        WireKind::OpenAiChat => inject_chat_tools(request, &specs),
        WireKind::Anthropic => inject_anthropic_tools(request, &specs),
        WireKind::OpenAiResponses => inject_responses_tools(request, &specs),
    }
}

struct ToolSpec {
    name: &'static str,
    description: String,
    google_props: Value,
    json_props: Value,
    required: Value,
}

fn tool_specs(pins: &PinnedPrompts) -> Vec<ToolSpec> {
    let text = |id: &str| pins.text(id).unwrap_or("").to_string();
    vec![
        ToolSpec {
            name: "compress",
            description: text(prompts::COMPRESS),
            google_props: json!({
                "ranges": {
                    "type": "ARRAY",
                    "description": "Every range to fold, numbered against the conversation as it is now",
                    "items": {
                        "type": "OBJECT",
                        "properties": {
                            "start": {"type": "INTEGER", "description": "First turn index to fold"},
                            "end": {"type": "INTEGER", "description": "Turn index after the last folded turn"},
                            "summary": {"type": "STRING", "description": "Summary of the folded turns, the only record kept"},
                            "topic": {"type": "STRING", "description": "Optional short topic"}
                        },
                        "required": ["start", "end", "summary"]
                    }
                }
            }),
            json_props: json!({
                "ranges": {
                    "type": "array",
                    "description": "Every range to fold, numbered against the conversation as it is now",
                    "items": {
                        "type": "object",
                        "properties": {
                            "start": {"type": "integer", "description": "First turn index to fold"},
                            "end": {"type": "integer", "description": "Turn index after the last folded turn"},
                            "summary": {"type": "string", "description": "Summary of the folded turns, the only record kept"},
                            "topic": {"type": "string", "description": "Optional short topic"}
                        },
                        "required": ["start", "end", "summary"]
                    }
                }
            }),
            required: json!(["ranges"]),
        },
        ToolSpec {
            name: "decompress",
            description: text(prompts::DECOMPRESS),
            google_props: json!({
                "blockId": {"type": "STRING", "description": "Block id returned by compress"},
                "toFile": {"type": "STRING", "description": "Plain file name under the session spill dir"},
                "full": {"type": "BOOLEAN", "description": "Accepted for parity; phase 1 has one tier"}
            }),
            json_props: json!({
                "blockId": {"type": "string", "description": "Block id returned by compress"},
                "toFile": {"type": "string", "description": "Plain file name under the session spill dir"},
                "full": {"type": "boolean", "description": "Accepted for parity; phase 1 has one tier"}
            }),
            required: json!(["blockId"]),
        },
        ToolSpec {
            name: "search_context",
            description: text(prompts::SEARCH_CONTEXT),
            google_props: json!({
                "query": {"type": "STRING", "description": "Words to find, case-insensitive"},
                "limit": {"type": "INTEGER", "description": "Maximum results"}
            }),
            json_props: json!({
                "query": {"type": "string", "description": "Words to find, case-insensitive"},
                "limit": {"type": "integer", "description": "Maximum results"}
            }),
            required: json!(["query"]),
        },
        ToolSpec {
            name: "acp_status",
            description: text(prompts::ACP_STATUS_DESCRIPTION),
            google_props: json!({}),
            json_props: json!({}),
            required: json!([]),
        },
    ]
}

fn inject_google_tools(request: &mut Value, specs: &[ToolSpec]) {
    let declarations: Vec<Value> = specs
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "description": s.description,
                "parameters": {
                    "type": "OBJECT",
                    "properties": s.google_props,
                    "required": s.required
                }
            })
        })
        .collect();
    let Some(tools) = request.get_mut("tools") else {
        request["tools"] = json!([{ "functionDeclarations": declarations }]);
        return;
    };
    let Some(tools) = tools.as_array_mut() else {
        return;
    };
    if let Some(holder) = tools
        .iter_mut()
        .find(|tool| tool.get("functionDeclarations").is_some())
    {
        if let Some(arr) = holder
            .get_mut("functionDeclarations")
            .and_then(Value::as_array_mut)
        {
            push_missing_by_name(arr, declarations, |v| v.get("name")?.as_str());
        }
        return;
    }
    if tools.iter().all(|tool| tool.get("name").is_some()) {
        push_missing_by_name(tools, declarations, |v| v.get("name")?.as_str());
        return;
    }
    tools.push(json!({ "functionDeclarations": declarations }));
}

fn inject_chat_tools(request: &mut Value, specs: &[ToolSpec]) {
    let items: Vec<Value> = specs
        .iter()
        .map(|s| {
            json!({
                "type": "function",
                "function": {
                    "name": s.name,
                    "description": s.description,
                    "parameters": {
                        "type": "object",
                        "properties": s.json_props,
                        "required": s.required
                    }
                }
            })
        })
        .collect();
    ensure_tool_array(request, items, |v| v.get("function")?.get("name")?.as_str());
}

fn inject_anthropic_tools(request: &mut Value, specs: &[ToolSpec]) {
    let items: Vec<Value> = specs
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "description": s.description,
                "input_schema": {
                    "type": "object",
                    "properties": s.json_props,
                    "required": s.required
                }
            })
        })
        .collect();
    ensure_tool_array(request, items, |v| v.get("name")?.as_str());
}

fn inject_responses_tools(request: &mut Value, specs: &[ToolSpec]) {
    let items: Vec<Value> = specs
        .iter()
        .map(|s| {
            json!({
                "type": "function",
                "name": s.name,
                "description": s.description,
                "parameters": {
                    "type": "object",
                    "properties": s.json_props,
                    "required": s.required
                }
            })
        })
        .collect();
    ensure_tool_array(request, items, |v| v.get("name")?.as_str());
}

fn ensure_tool_array<F>(request: &mut Value, items: Vec<Value>, get_name: F)
where
    F: Fn(&Value) -> Option<&str>,
{
    let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) else {
        request["tools"] = Value::Array(items);
        return;
    };
    push_missing_by_name(tools, items, get_name);
}

fn push_missing_by_name<F>(existing: &mut Vec<Value>, items: Vec<Value>, get_name: F)
where
    F: Fn(&Value) -> Option<&str>,
{
    for item in items {
        let Some(name) = get_name(&item) else {
            continue;
        };
        if !existing.iter().any(|t| get_name(t) == Some(name)) {
            existing.push(item);
        }
    }
}

pub fn extract_core_contents(wire: WireKind, request: &Value) -> Option<Vec<Value>> {
    match wire {
        WireKind::Google => request.get("contents").and_then(Value::as_array).cloned(),
        WireKind::OpenAiChat => chat_to_core(request.get("messages")?.as_array()?),
        WireKind::Anthropic => anthropic_to_core(request.get("messages")?.as_array()?),
        WireKind::OpenAiResponses => responses_to_core(request.get("input")?.as_array()?),
    }
}

pub fn write_core_contents(wire: WireKind, request: &mut Value, contents: Vec<Value>) {
    let contents = contents.into_iter().map(unmarked).collect::<Vec<_>>();
    match wire {
        WireKind::Google => {
            request["contents"] = Value::Array(contents);
        }
        WireKind::OpenAiChat => {
            let sys_prefix: Vec<Value> = request
                .get("messages")
                .and_then(Value::as_array)
                .map(|msgs| {
                    msgs.iter()
                        .filter(|m| {
                            matches!(
                                m.get("role").and_then(Value::as_str),
                                Some("system" | "developer")
                            )
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            let mut out = sys_prefix;
            out.extend(core_to_chat(&contents));
            request["messages"] = Value::Array(out);
        }
        WireKind::Anthropic => {
            request["messages"] = Value::Array(core_to_anthropic(&contents));
        }
        WireKind::OpenAiResponses => {
            request["input"] = Value::Array(core_to_responses(&contents));
        }
    }
}

/// Drops the proxy's own turn marks; upstream must not see them.
fn unmarked(mut turn: Value) -> Value {
    turnkeys::strip_transient(&mut turn);
    turn
}

fn chat_to_core(messages: &[Value]) -> Option<Vec<Value>> {
    let mut turns = Vec::new();
    for msg in messages {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
        if matches!(role, "system" | "developer") {
            continue;
        }
        let core_role = if role == "assistant" { "model" } else { "user" };
        let parts = chat_msg_parts(msg, role);
        turns.push(json!({
            "role": core_role,
            "parts": parts,
            (turnkeys::RAW): msg
        }));
    }
    Some(turns)
}

fn chat_msg_parts(msg: &Value, role: &str) -> Vec<Value> {
    let mut parts = Vec::new();
    if role == "tool" {
        let id = msg
            .get("tool_call_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let text = content_as_string(msg.get("content"));
        parts.push(json!({
            "functionResponse": {
                "id": id,
                "name": msg.get("name").and_then(Value::as_str).unwrap_or("tool"),
                "response": { "result": text }
            }
        }));
        return parts;
    }
    let text = content_as_string(msg.get("content"));
    if !text.is_empty() {
        parts.push(json!({ "text": text }));
    }
    if let Some(calls) = msg.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let id = call.get("id").and_then(Value::as_str).unwrap_or("call_1");
            let func = call.get("function").unwrap_or(call);
            let name = func.get("name").and_then(Value::as_str).unwrap_or("");
            let args = parse_args_field(func.get("arguments"));
            parts.push(json!({
                "functionCall": { "id": id, "name": name, "args": args }
            }));
        }
    }
    parts
}

fn core_to_chat(contents: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for turn in contents {
        if let Some(raw) = turn.get(turnkeys::RAW) {
            out.push(raw.clone());
            continue;
        }
        let role = turn.get("role").and_then(Value::as_str).unwrap_or("user");
        let Some(parts) = turn.get("parts").and_then(Value::as_array) else {
            continue;
        };
        if role == "model" {
            out.push(core_model_to_chat(parts));
        } else {
            out.extend(core_user_to_chat(parts));
        }
    }
    out
}

fn core_model_to_chat(parts: &[Value]) -> Value {
    let mut texts = Vec::new();
    let mut calls = Vec::new();
    for part in parts {
        if let Some(t) = part.get("text").and_then(Value::as_str) {
            texts.push(t);
        }
        if let Some(fc) = part.get("functionCall") {
            let id = fc.get("id").and_then(Value::as_str).unwrap_or("call_1");
            let name = fc.get("name").and_then(Value::as_str).unwrap_or("");
            let args = serde_json::to_string(fc.get("args").unwrap_or(&json!({})))
                .unwrap_or_else(|_| "{}".into());
            calls.push(json!({
                "id": id,
                "type": "function",
                "function": { "name": name, "arguments": args }
            }));
        }
    }
    let mut msg = json!({ "role": "assistant" });
    if !texts.is_empty() {
        msg["content"] = json!(texts.join("\n"));
    }
    if !calls.is_empty() {
        msg["tool_calls"] = Value::Array(calls);
    }
    msg
}

fn core_user_to_chat(parts: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    let mut texts = Vec::new();
    for part in parts {
        if let Some(fr) = part.get("functionResponse") {
            let id = fr.get("id").and_then(Value::as_str).unwrap_or("call_1");
            let content = response_text(fr);
            out.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": content
            }));
        } else if let Some(t) = part.get("text").and_then(Value::as_str) {
            texts.push(t);
        }
    }
    if !texts.is_empty() {
        out.push(json!({
            "role": "user",
            "content": texts.join("\n\n")
        }));
    }
    out
}

fn anthropic_to_core(messages: &[Value]) -> Option<Vec<Value>> {
    let mut turns = Vec::new();
    for msg in messages {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
        let core_role = if role == "assistant" { "model" } else { "user" };
        let parts = anthropic_msg_parts(msg);
        turns.push(json!({
            "role": core_role,
            "parts": parts,
            (turnkeys::RAW): msg
        }));
    }
    Some(turns)
}

fn anthropic_msg_parts(msg: &Value) -> Vec<Value> {
    let mut parts = Vec::new();
    match msg.get("content") {
        Some(Value::String(text)) => {
            parts.push(json!({ "text": text }));
        }
        Some(Value::Array(blocks)) => {
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = block.get("text").and_then(Value::as_str) {
                            parts.push(json!({ "text": t }));
                        }
                    }
                    Some("tool_use") => {
                        let id = block.get("id").and_then(Value::as_str).unwrap_or("toolu_1");
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                        let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                        parts.push(json!({
                            "functionCall": { "id": id, "name": name, "args": input }
                        }));
                    }
                    Some("tool_result") => {
                        let id = block
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let text = content_as_string(block.get("content"));
                        parts.push(json!({
                            "functionResponse": {
                                "id": id,
                                "name": "tool",
                                "response": { "result": text }
                            }
                        }));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    parts
}

fn core_to_anthropic(contents: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for turn in contents {
        if let Some(raw) = turn.get(turnkeys::RAW) {
            out.push(raw.clone());
            continue;
        }
        let role = turn.get("role").and_then(Value::as_str).unwrap_or("user");
        let anth_role = if role == "model" { "assistant" } else { "user" };
        let Some(parts) = turn.get("parts").and_then(Value::as_array) else {
            continue;
        };
        let mut blocks = Vec::new();
        for part in parts {
            if let Some(t) = part.get("text").and_then(Value::as_str) {
                blocks.push(json!({ "type": "text", "text": t }));
            } else if let Some(fc) = part.get("functionCall") {
                blocks.push(json!({
                    "type": "tool_use",
                    "id": fc.get("id").and_then(Value::as_str).unwrap_or("toolu_1"),
                    "name": fc.get("name").and_then(Value::as_str).unwrap_or(""),
                    "input": fc.get("args").cloned().unwrap_or_else(|| json!({}))
                }));
            } else if let Some(fr) = part.get("functionResponse") {
                blocks.push(json!({
                    "type": "tool_result",
                    "tool_use_id": fr.get("id").and_then(Value::as_str).unwrap_or("toolu_1"),
                    "content": response_text(fr)
                }));
            }
        }
        if !blocks.is_empty() {
            out.push(json!({ "role": anth_role, "content": blocks }));
        }
    }
    out
}

fn responses_to_core(items: &[Value]) -> Option<Vec<Value>> {
    let mut turns = Vec::new();
    for item in items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        if item_type == "function_call" {
            let id = item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("call_1");
            let name = item.get("name").and_then(Value::as_str).unwrap_or("");
            let args = parse_args_field(item.get("arguments"));
            turns.push(json!({
                "role": "model",
                "parts": [{ "functionCall": { "id": id, "name": name, "args": args } }],
                (turnkeys::RAW): item
            }));
        } else if item_type == "function_call_output" {
            let id = item
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or("call_1");
            let output = content_as_string(item.get("output"));
            turns.push(json!({
                "role": "user",
                "parts": [{
                    "functionResponse": {
                        "id": id,
                        "name": "tool",
                        "response": { "result": output }
                    }
                }],
                (turnkeys::RAW): item
            }));
        } else {
            let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
            let core_role = if role == "assistant" { "model" } else { "user" };
            let text = content_as_string(item.get("content"));
            turns.push(json!({
                "role": core_role,
                "parts": [{ "text": text }],
                (turnkeys::RAW): item
            }));
        }
    }
    Some(turns)
}

fn core_to_responses(contents: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for turn in contents {
        if let Some(raw) = turn.get(turnkeys::RAW) {
            out.push(raw.clone());
            continue;
        }
        let role = turn.get("role").and_then(Value::as_str).unwrap_or("user");
        let resp_role = if role == "model" { "assistant" } else { "user" };
        let Some(parts) = turn.get("parts").and_then(Value::as_array) else {
            continue;
        };
        for part in parts {
            if let Some(t) = part.get("text").and_then(Value::as_str) {
                out.push(json!({ "role": resp_role, "content": t }));
            } else if let Some(fc) = part.get("functionCall") {
                let args = serde_json::to_string(fc.get("args").unwrap_or(&json!({})))
                    .unwrap_or_else(|_| "{}".into());
                out.push(json!({
                    "type": "function_call",
                    "call_id": fc.get("id").and_then(Value::as_str).unwrap_or("call_1"),
                    "name": fc.get("name").and_then(Value::as_str).unwrap_or(""),
                    "arguments": args
                }));
            } else if let Some(fr) = part.get("functionResponse") {
                out.push(json!({
                    "type": "function_call_output",
                    "call_id": fr.get("id").and_then(Value::as_str).unwrap_or("call_1"),
                    "output": response_text(fr)
                }));
            }
        }
    }
    out
}

pub fn extract_proxy_call_from_payload(payload: &Value) -> Option<ProxyCall> {
    if let Some(call) = anthropic_call_in(payload) {
        return Some(call);
    }
    if let Some(call) = chat_call_in(payload) {
        return Some(call);
    }
    if let Some(call) = responses_call_in(payload) {
        return Some(call);
    }
    None
}

fn anthropic_call_in(payload: &Value) -> Option<ProxyCall> {
    let content = payload.get("content")?.as_array()?;
    for block in content {
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let name = block
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !PROXY_TOOLS.contains(&name) {
            continue;
        }
        let id = block
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("toolu_1")
            .to_string();
        let args = block.get("input").cloned().unwrap_or_else(|| json!({}));
        let part = json!({ "functionCall": { "id": id, "name": name, "args": args } });
        return Some(ProxyCall {
            name: name.to_string(),
            id,
            part,
            args,
        });
    }
    None
}

fn chat_call_in(payload: &Value) -> Option<ProxyCall> {
    let choices = payload.get("choices")?.as_array()?;
    for choice in choices {
        let msg = choice.get("message").or_else(|| choice.get("delta"))?;
        let calls = msg.get("tool_calls")?.as_array()?;
        for call in calls {
            let func = call.get("function").unwrap_or(call);
            let name = func.get("name").and_then(Value::as_str).unwrap_or_default();
            if !PROXY_TOOLS.contains(&name) {
                continue;
            }
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("call_1")
                .to_string();
            let args = parse_args_field(func.get("arguments"));
            let part = json!({ "functionCall": { "id": id, "name": name, "args": args } });
            return Some(ProxyCall {
                name: name.to_string(),
                id,
                part,
                args,
            });
        }
    }
    None
}

fn responses_call_in(payload: &Value) -> Option<ProxyCall> {
    let output = payload
        .get("response")
        .and_then(|r| r.get("output"))
        .or_else(|| payload.get("output"))?
        .as_array()?;
    for item in output {
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            continue;
        }
        let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
        if !PROXY_TOOLS.contains(&name) {
            continue;
        }
        let id = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("call_1")
            .to_string();
        let args = parse_args_field(item.get("arguments"));
        let part = json!({ "functionCall": { "id": id, "name": name, "args": args } });
        return Some(ProxyCall {
            name: name.to_string(),
            id,
            part,
            args,
        });
    }
    None
}

fn content_as_string(val: Option<&Value>) -> String {
    match val {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|b| {
                b.get("text")
                    .or_else(|| b.get("content"))
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) if !other.is_null() => other.to_string(),
        _ => String::new(),
    }
}

fn parse_args_field(val: Option<&Value>) -> Value {
    match val {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
        Some(v) if v.is_object() => v.clone(),
        _ => json!({}),
    }
}

pub(crate) fn response_text(fr: &Value) -> String {
    fr.get("response")
        .map(|r| {
            let res = r.get("result").and_then(Value::as_str).unwrap_or("");
            if let Some(splice) = r.get("splice").and_then(Value::as_str) {
                format!("{res}\n[{splice}]")
            } else {
                res.to_string()
            }
        })
        .unwrap_or_default()
}

/// Rewrites a tool result's visible text, keeping the response object's other keys.
pub(crate) fn set_response_text(part: &mut Value, text: &str) {
    let Some(response) = part
        .get_mut("functionResponse")
        .and_then(|response| response.get_mut("response"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    response.insert("result".to_string(), Value::String(text.to_string()));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use prompts::{PromptStore, PromptVars};

    #[test]
    fn the_graph_skill_joins_the_system_prompt_only_when_the_graph_is_up() {
        let dir = tempfile::tempdir().unwrap();
        let pins =
            PinnedPrompts::pin(&PromptStore::new(dir.path()), &PromptVars::default()).unwrap();
        let without = prompt_text(&pins, false);
        let with = prompt_text(&pins, true);
        assert!(!without.contains("Codebase memory"));
        assert!(with.starts_with(&without));
        assert_eq!(
            with.strip_prefix(&without)
                .unwrap()
                .trim_start_matches("\n\n"),
            pins.text(prompts::SKILL_CODEBASE_MEMORY).unwrap()
        );
    }
}
