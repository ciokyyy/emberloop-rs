use std::collections::{BTreeMap, HashMap};

use prompts::PinnedPrompts;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::protect::is_system_item;
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

/// The system text of one request: the pinned skill texts, then the Session's frozen catalog.
pub fn prompt_text(pins: &PinnedPrompts, codebase_memory: bool, catalog: &str) -> String {
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
    if !catalog.trim().is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(catalog.trim_end());
    }
    text
}

pub fn inject_prompts_for_wire(
    wire: WireKind,
    request: &mut Value,
    pins: &PinnedPrompts,
    codebase_memory: bool,
    catalog: &str,
) {
    let text = prompt_text(pins, codebase_memory, catalog);
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

/// Anthropic accepts at most this many `cache_control` breakpoints in one request.
const MAX_BREAKPOINTS: usize = 4;

/// Counts the `cache_control` marks anywhere in a request.
fn breakpoints(value: &Value) -> usize {
    match value {
        Value::Object(map) => {
            usize::from(map.contains_key("cache_control"))
                + map.values().map(breakpoints).sum::<usize>()
        }
        Value::Array(items) => items.iter().map(breakpoints).sum(),
        _ => 0,
    }
}

/// The breakpoint a block appended after `last` takes over: a copy of `last`'s, only while the
/// request has room for one more. A proxy block never moves an existing breakpoint.
fn carried_breakpoint(request: &Value, last: Option<&Value>) -> Option<Value> {
    let cache = last?.get("cache_control")?;
    (breakpoints(request) < MAX_BREAKPOINTS).then(|| cache.clone())
}

fn inject_anthropic_prompt(request: &mut Value, text: &str) {
    let last_system = request
        .get("system")
        .and_then(Value::as_array)
        .and_then(|blocks| blocks.last());
    let carry = carried_breakpoint(request, last_system);
    match request.get_mut("system") {
        Some(Value::String(existing)) => {
            if !existing.is_empty() {
                existing.push_str("\n\n");
            }
            existing.push_str(text);
        }
        Some(Value::Array(blocks)) => {
            let mut block = json!({ "type": "text", "text": text });
            if let Some(cache) = carry {
                block["cache_control"] = cache;
            }
            blocks.push(block);
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

/// Remove only the edit declaration; reviewers still have shell and check tools.
pub fn remove_edit_tool(request: &mut Value) {
    let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) else {
        return;
    };
    for tool in tools.iter_mut() {
        if let Some(declarations) = tool
            .get_mut("functionDeclarations")
            .and_then(Value::as_array_mut)
        {
            declarations.retain(|declaration| !is_edit_tool(declaration));
        }
    }
    tools.retain(|tool| {
        !is_edit_tool(tool)
            && !tool
                .get("functionDeclarations")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
    });
}

fn is_edit_tool(tool: &Value) -> bool {
    let name = tool
        .get("name")
        .or_else(|| tool.pointer("/function/name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    matches!(
        name.rsplit("__").next().unwrap_or(name),
        "edit" | "el_edit" | "Edit"
    )
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
    let mut items: Vec<Value> = specs
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
    let last_tool = request
        .get("tools")
        .and_then(Value::as_array)
        .and_then(|tools| tools.last());
    let carry = carried_breakpoint(request, last_tool);
    if let (Some(cache), Some(last)) = (carry, items.last_mut()) {
        last["cache_control"] = cache;
    }
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

/// The wire's own key for its conversation items.
fn items_key(wire: WireKind) -> &'static str {
    match wire {
        WireKind::Google => "contents",
        WireKind::OpenAiChat | WireKind::Anthropic => "messages",
        WireKind::OpenAiResponses => "input",
    }
}

/// The conversation turns, without system and developer items: those are protected and never
/// fold, so they are not turns at all. `write_core_contents` writes them back.
pub fn extract_core_contents(wire: WireKind, request: &Value) -> Option<Vec<Value>> {
    match wire {
        WireKind::Google => request
            .get("contents")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|item| !is_system_item(item))
                    .cloned()
                    .collect()
            }),
        WireKind::OpenAiChat => chat_to_core(request.get("messages")?.as_array()?),
        WireKind::Anthropic => anthropic_to_core(request.get("messages")?.as_array()?),
        WireKind::OpenAiResponses => responses_to_core(request.get("input")?.as_array()?),
    }
}

/// Writes the turns back in the wire's shape. The request's system and developer items come first,
/// in their original order and byte for byte, so no write can drop or fold them.
pub fn write_core_contents(wire: WireKind, request: &mut Value, contents: &[Value]) {
    // Only a turn carrying the nudge mark needs a copy; the common request is written from the
    // caller's slice as it is.
    let owned;
    let contents: &[Value] = if contents.iter().any(turnkeys::is_transient) {
        owned = contents.iter().cloned().map(unmarked).collect::<Vec<_>>();
        &owned
    } else {
        contents
    };
    let key = items_key(wire);
    let mut out: Vec<Value> = request
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| is_system_item(item))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    out.extend(match wire {
        WireKind::Google => contents.to_vec(),
        WireKind::OpenAiChat => core_to_chat(contents),
        WireKind::Anthropic => core_to_anthropic(contents),
        WireKind::OpenAiResponses => core_to_responses(contents),
    });
    request[key] = Value::Array(out);
}

/// Drops the proxy's own turn marks; upstream must not see them.
fn unmarked(mut turn: Value) -> Value {
    turnkeys::strip_transient(&mut turn);
    turn
}

fn chat_to_core(messages: &[Value]) -> Option<Vec<Value>> {
    let mut turns = Vec::new();
    for msg in messages {
        if is_system_item(msg) {
            continue;
        }
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
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
    let mut images = Vec::new();
    for part in parts {
        if let Some(fr) = part.get("functionResponse") {
            let id = fr.get("id").and_then(Value::as_str).unwrap_or("call_1");
            let content = response_text(fr);
            out.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": content
            }));
        } else if let Some((mime, data)) = inline_image(part) {
            images.push(json!({ "type": "image_url", "image_url": { "url": format!("data:{mime};base64,{data}") } }));
        } else if let Some(t) = part.get("text").and_then(Value::as_str) {
            texts.push(t);
        }
    }
    if !images.is_empty() {
        // Text-only turns keep the joined string form; a turn with images needs the part array.
        let mut content: Vec<Value> = texts
            .iter()
            .map(|t| json!({ "type": "text", "text": t }))
            .collect();
        content.extend(images);
        out.push(json!({ "role": "user", "content": content }));
    } else if !texts.is_empty() {
        out.push(json!({
            "role": "user",
            "content": texts.join("\n\n")
        }));
    }
    out
}

fn inline_image(part: &Value) -> Option<(&str, &str)> {
    let image = part.get("inlineData")?;
    Some((
        image.get("mimeType")?.as_str()?,
        image.get("data")?.as_str()?,
    ))
}

fn anthropic_to_core(messages: &[Value]) -> Option<Vec<Value>> {
    let mut turns = Vec::new();
    // A tool result names no tool on the wire; it takes the name of the call it answers.
    let mut call_names = HashMap::new();
    for msg in messages {
        if is_system_item(msg) {
            continue;
        }
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
        let core_role = if role == "assistant" { "model" } else { "user" };
        let parts = anthropic_msg_parts(msg, &mut call_names);
        turns.push(json!({
            "role": core_role,
            "parts": parts,
            (turnkeys::RAW): msg
        }));
    }
    Some(turns)
}

fn anthropic_msg_parts(msg: &Value, call_names: &mut HashMap<String, String>) -> Vec<Value> {
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
                        call_names.insert(id.to_string(), name.to_string());
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
                        let name = call_names.get(id).map_or("tool", String::as_str);
                        let text = content_as_string(block.get("content"));
                        parts.push(json!({
                            "functionResponse": {
                                "id": id,
                                "name": name,
                                "response": { "result": text }
                            }
                        }));
                    }
                    _ => parts.push(opaque_part(block)),
                }
            }
        }
        _ => {}
    }
    parts
}

/// The content blocks of one Anthropic response in stream order, as the model sent them. A stream
/// is folded event by event: deltas append to their block (thinking, signature, text), and a
/// tool_use's streamed JSON is parsed when its block stops.
#[derive(Debug, Default)]
pub struct AnthropicBlocks {
    blocks: BTreeMap<u64, Value>,
    partial_json: BTreeMap<u64, String>,
}

impl AnthropicBlocks {
    pub fn observe(&mut self, event: &Value) {
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => *self = Self::default(),
            Some("content_block_start") => {
                if let Some(block) = event.get("content_block") {
                    self.blocks.insert(index, block.clone());
                }
            }
            Some("content_block_delta") => self.apply_delta(index, event.get("delta")),
            Some("content_block_stop") => self.close(index),
            _ => {}
        }
    }

    fn apply_delta(&mut self, index: u64, delta: Option<&Value>) {
        let Some(delta) = delta else {
            return;
        };
        let (field, fragment) = match delta.get("type").and_then(Value::as_str) {
            Some("input_json_delta") => {
                if let Some(text) = delta.get("partial_json").and_then(Value::as_str) {
                    self.partial_json.entry(index).or_default().push_str(text);
                }
                return;
            }
            Some("text_delta") => ("text", delta.get("text")),
            Some("thinking_delta") => ("thinking", delta.get("thinking")),
            Some("signature_delta") => ("signature", delta.get("signature")),
            _ => return,
        };
        let (Some(fragment), Some(map)) = (
            fragment.and_then(Value::as_str),
            self.blocks.get_mut(&index).and_then(Value::as_object_mut),
        ) else {
            return;
        };
        let slot = map
            .entry(field)
            .or_insert_with(|| Value::String(String::new()));
        if let Value::String(text) = slot {
            text.push_str(fragment);
        }
    }

    fn close(&mut self, index: u64) {
        let Some(json) = self.partial_json.remove(&index) else {
            return;
        };
        let input = serde_json::from_str(&json).unwrap_or_else(|_| json!({}));
        if let Some(map) = self.blocks.get_mut(&index).and_then(Value::as_object_mut) {
            map.insert("input".to_string(), input);
        }
    }

    pub fn content(&self) -> Vec<Value> {
        self.blocks.values().cloned().collect()
    }
}

/// The model turn an Anthropic proxy round adds. The response's blocks are kept as the turn's raw
/// message, so upstream receives them as the model sent them (thinking signatures included); a
/// harness call is left out, and a proxy call the blocks miss is added as its tool_use.
pub fn anthropic_model_turn(blocks: &[Value], calls: &[&ProxyCall]) -> Value {
    let mut content: Vec<Value> = blocks
        .iter()
        .filter(|block| !is_harness_call(block))
        .cloned()
        .collect();
    for call in calls {
        let present = content
            .iter()
            .any(|block| block.get("id").and_then(Value::as_str) == Some(call.id.as_str()));
        if !present {
            content.push(json!({
                "type": "tool_use",
                "id": call.id,
                "name": call.name,
                "input": call.args
            }));
        }
    }
    let message = json!({ "role": "assistant", "content": content });
    json!({
        "role": "model",
        "parts": anthropic_msg_parts(&message, &mut HashMap::new()),
        (turnkeys::RAW): message
    })
}

fn is_harness_call(block: &Value) -> bool {
    block.get("type").and_then(Value::as_str) == Some("tool_use")
        && block
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| !PROXY_TOOLS.contains(&name))
}

/// Identity of a block the proxy does not model (thinking, images, unknown types): a digest of
/// the block without its breakpoint, so two turns differing only in such content are different
/// turns, while the breakpoint moving between resends is not.
fn opaque_part(block: &Value) -> Value {
    let mut block = block.clone();
    if let Some(map) = block.as_object_mut() {
        map.remove("cache_control");
    }
    let digest = Sha256::digest(block.to_string().as_bytes());
    json!({
        "opaque": {
            "type": block.get("type").and_then(Value::as_str).unwrap_or(""),
            "digest": hex::encode(digest)
        }
    })
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
            if let Some((mime, data)) = inline_image(part) {
                blocks.push(json!({ "type": "image", "source": { "type": "base64", "media_type": mime, "data": data } }));
            } else if let Some(t) = part.get("text").and_then(Value::as_str) {
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
        if is_system_item(item) {
            continue;
        }
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
            if let Some((mime, data)) = inline_image(part) {
                let url = format!("data:{mime};base64,{data}");
                out.push(json!({ "role": resp_role, "content": [{ "type": "input_image", "image_url": url }] }));
            } else if let Some(t) = part.get("text").and_then(Value::as_str) {
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

pub(crate) fn parse_args_field(val: Option<&Value>) -> Value {
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

/// Rewrites a raw tool result's text where its own wire keeps it: `output` on a Responses
/// `function_call_output`, a `tool_result` block on an Anthropic message, `content` on a chat
/// message. Adding `content` to a Responses item makes the backend reject the whole request.
pub(crate) fn set_raw_result_text(raw: &mut Value, id: &str, text: &str) {
    let Some(map) = raw.as_object_mut() else {
        return;
    };
    match map.get("type").and_then(Value::as_str) {
        Some("function_call_output") => {
            // An image-carrying output is a parts array; the core text carries the placeholder.
            if let Some(slot) = map.get_mut("output").filter(|output| output.is_string()) {
                *slot = json!(text);
            }
        }
        _ => {
            if map.get("tool_call_id").and_then(Value::as_str) == Some(id) {
                map.insert("content".into(), json!(text));
                return;
            }
            let Some(blocks) = map.get_mut("content").and_then(Value::as_array_mut) else {
                return;
            };
            for block in blocks {
                if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                    continue;
                }
                if block.get("tool_use_id").and_then(Value::as_str) == Some(id) {
                    block["content"] = json!(text);
                }
            }
        }
    }
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
    fn steer_images_reach_every_wire_as_image_parts() {
        let turn = json!({"role": "user", "parts": [
            {"text": "look"},
            {"inlineData": {"mimeType": "image/png", "data": "AAAA"}}
        ]});
        let contents = vec![turn];

        let mut chat = json!({"messages": []});
        write_core_contents(WireKind::OpenAiChat, &mut chat, &contents);
        assert_eq!(
            chat["messages"][0]["content"][1]["image_url"]["url"],
            "data:image/png;base64,AAAA"
        );

        let mut anthropic = json!({"messages": []});
        write_core_contents(WireKind::Anthropic, &mut anthropic, &contents);
        assert_eq!(
            anthropic["messages"][0]["content"][1]["source"]["data"],
            "AAAA"
        );

        let mut responses = json!({"input": []});
        write_core_contents(WireKind::OpenAiResponses, &mut responses, &contents);
        assert_eq!(
            responses["input"][1]["content"][0]["image_url"],
            "data:image/png;base64,AAAA"
        );
    }

    #[test]
    fn write_borrows_the_turns_and_strips_only_the_nudge_mark() {
        let nudge = turnkeys::nudge_turn("keep the budget");
        let contents = vec![
            json!({"role": "user", "parts": [{"text": "a"}]}),
            nudge.clone(),
        ];

        let mut chat = json!({"messages": []});
        write_core_contents(WireKind::OpenAiChat, &mut chat, &contents);
        assert_eq!(chat["messages"][0]["content"], "a");
        assert_eq!(chat["messages"][1]["content"], "keep the budget");
        assert!(!chat.to_string().contains(turnkeys::NUDGE));

        let mut google = json!({"contents": []});
        write_core_contents(WireKind::Google, &mut google, &contents);
        assert_eq!(google["contents"][1]["parts"][0]["text"], "keep the budget");
        assert!(!google.to_string().contains(turnkeys::NUDGE));

        assert_eq!(contents[1], nudge, "the caller's turns keep their mark");
    }

    #[test]
    fn text_only_chat_turns_keep_the_string_form() {
        let turn = json!({"role": "user", "parts": [{"text": "a"}, {"text": "b"}]});
        let mut chat = json!({"messages": []});
        write_core_contents(WireKind::OpenAiChat, &mut chat, &[turn]);
        assert_eq!(chat["messages"][0]["content"], "a\n\nb");
    }

    #[test]
    fn the_graph_skill_joins_the_system_prompt_only_when_the_graph_is_up() {
        let dir = tempfile::tempdir().unwrap();
        let pins =
            PinnedPrompts::pin(&PromptStore::new(dir.path()), &PromptVars::default()).unwrap();
        let without = prompt_text(&pins, false, "");
        let with = prompt_text(&pins, true, "");
        assert!(prompt_text(&pins, true, "- a: b").ends_with("\n\n- a: b"));
        assert!(!without.contains("Codebase memory"));
        assert!(with.starts_with(&without));
        assert_eq!(
            with.strip_prefix(&without)
                .unwrap()
                .trim_start_matches("\n\n"),
            pins.text(prompts::SKILL_CODEBASE_MEMORY).unwrap()
        );
    }

    fn breakpoint() -> Value {
        json!({ "type": "ephemeral" })
    }

    #[test]
    fn anthropic_prompt_block_follows_every_existing_system_block_untouched() {
        let mut request = json!({"system": [
            {"type": "text", "text": "x-anthropic-billing-header: cc_version=1"},
            {"type": "text", "text": "You are Claude Code.", "cache_control": breakpoint()}
        ]});
        let before = request["system"].as_array().unwrap().clone();
        inject_anthropic_prompt(&mut request, "OURS");
        let after = request["system"].as_array().unwrap();
        assert_eq!(after.len(), 3);
        assert_eq!(&after[..2], &before[..]);
        assert_eq!(after[2]["text"], json!("OURS"));
    }

    #[test]
    fn anthropic_prompt_block_carries_the_last_breakpoint_when_the_last_block_has_one() {
        let mut request = json!({"system": [
            {"type": "text", "text": "identity"},
            {"type": "text", "text": "claude code", "cache_control": breakpoint()}
        ]});
        inject_anthropic_prompt(&mut request, "OURS");
        assert_eq!(
            request["system"][2],
            json!({"type": "text", "text": "OURS", "cache_control": breakpoint()})
        );
    }

    #[test]
    fn anthropic_prompt_block_has_no_breakpoint_when_the_last_block_has_none() {
        let mut request = json!({"system": [
            {"type": "text", "text": "identity", "cache_control": breakpoint()},
            {"type": "text", "text": "claude code"}
        ]});
        inject_anthropic_prompt(&mut request, "OURS");
        assert_eq!(
            request["system"][2],
            json!({"type": "text", "text": "OURS"})
        );
    }

    #[test]
    fn anthropic_prompt_block_never_makes_a_fifth_breakpoint() {
        // Four breakpoints already: the last system block has one, but a fifth is refused upstream.
        let mut request = json!({
            "system": [{"type": "text", "text": "a", "cache_control": breakpoint()}],
            "tools": [{"name": "Bash", "input_schema": {}, "cache_control": breakpoint()}],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "b", "cache_control": breakpoint()}]},
                {"role": "assistant", "content": [{"type": "text", "text": "c", "cache_control": breakpoint()}]}
            ]
        });
        inject_anthropic_prompt(&mut request, "OURS");
        assert_eq!(
            request["system"][1],
            json!({"type": "text", "text": "OURS"})
        );
    }

    #[test]
    fn anthropic_prompt_as_a_string_is_appended_to_it() {
        let mut request = json!({"system": "identity"});
        inject_anthropic_prompt(&mut request, "OURS");
        assert_eq!(request["system"], json!("identity\n\nOURS"));
    }

    #[test]
    fn anthropic_tools_keep_claudes_tools_in_order_and_never_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let pins =
            PinnedPrompts::pin(&PromptStore::new(dir.path()), &PromptVars::default()).unwrap();
        let specs = tool_specs(&pins);
        let claude = json!([
            {"name": "Bash", "input_schema": {"type": "object"}},
            {"name": "mcp__el__read", "input_schema": {"type": "object"}}
        ]);
        let mut request = json!({ "tools": claude.clone() });
        inject_anthropic_tools(&mut request, &specs);
        inject_anthropic_tools(&mut request, &specs);
        let tools = request["tools"].as_array().unwrap();
        assert_eq!(&tools[..2], claude.as_array().unwrap().as_slice());
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names.len(), 2 + specs.len());
        assert_eq!(
            names.iter().filter(|n| **n == "compress").count(),
            1,
            "{names:?}"
        );
    }

    #[test]
    fn anthropic_proxy_tools_carry_the_last_breakpoint_of_claudes_tools() {
        let dir = tempfile::tempdir().unwrap();
        let pins =
            PinnedPrompts::pin(&PromptStore::new(dir.path()), &PromptVars::default()).unwrap();
        let specs = tool_specs(&pins);
        let mut request = json!({"tools": [
            {"name": "Bash", "input_schema": {}},
            {"name": "Read", "input_schema": {}, "cache_control": breakpoint()}
        ]});
        inject_anthropic_tools(&mut request, &specs);
        let tools = request["tools"].as_array().unwrap();
        assert!(tools[1].get("cache_control").is_some());
        let last = tools.last().unwrap();
        assert_eq!(last["cache_control"], breakpoint());
        assert_eq!(
            tools
                .iter()
                .filter(|t| t.get("cache_control").is_some())
                .count(),
            2
        );
    }

    #[test]
    fn anthropic_turns_with_only_thinking_keep_their_identity() {
        let a = anthropic_to_core(&[json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "plan A", "signature": "sig-a"}
        ]})])
        .unwrap();
        let b = anthropic_to_core(&[json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "plan B", "signature": "sig-b"}
        ]})])
        .unwrap();
        assert!(!turnkeys::same_turn(&a[0], &b[0]));
    }

    #[test]
    fn anthropic_turns_with_only_images_keep_their_identity() {
        let image = |data: &str| {
            json!({"role": "user", "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": data}}
            ]})
        };
        let a = anthropic_to_core(&[image("AAAA")]).unwrap();
        let b = anthropic_to_core(&[image("BBBB")]).unwrap();
        assert!(!turnkeys::same_turn(&a[0], &b[0]));
    }

    #[test]
    fn anthropic_turn_identity_ignores_where_the_breakpoint_sits() {
        let plain = json!({"role": "user", "content": [
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}}
        ]});
        let marked = json!({"role": "user", "content": [
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"},
             "cache_control": breakpoint()}
        ]});
        let a = anthropic_to_core(&[plain]).unwrap();
        let b = anthropic_to_core(&[marked]).unwrap();
        assert!(turnkeys::same_turn(&a[0], &b[0]));
    }

    #[test]
    fn anthropic_tool_results_take_the_name_of_their_tool_use() {
        let core = anthropic_to_core(&[
            json!({"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "read", "input": {"path": "a"}}
            ]}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "file"}
            ]}),
        ])
        .unwrap();
        assert_eq!(
            core[1]["parts"][0]["functionResponse"]["name"],
            json!("read")
        );
    }

    #[test]
    fn anthropic_call_in_finds_a_proxy_call_after_thinking() {
        let payload = json!({"type": "message", "content": [
            {"type": "thinking", "thinking": "plan", "signature": "sig"},
            {"type": "text", "text": "compacting"},
            {"type": "tool_use", "id": "toolu_9", "name": "compress", "input": {"ranges": []}}
        ]});
        let call = anthropic_call_in(&payload).unwrap();
        assert_eq!(call.name, "compress");
        assert_eq!(call.id, "toolu_9");
    }

    #[test]
    fn anthropic_call_in_ignores_harness_tools_and_prefixed_names() {
        let payload = json!({"content": [
            {"type": "tool_use", "id": "t1", "name": "mcp__el__compress", "input": {}},
            {"type": "tool_use", "id": "t2", "name": "Bash", "input": {}}
        ]});
        assert!(anthropic_call_in(&payload).is_none());
    }

    #[test]
    fn reviewer_filter_removes_only_edit_across_all_tool_shapes() {
        for tools in [
            json!([{"functionDeclarations":[{"name":"mcp__el__edit"},{"name":"bash"},{"name":"write"}]}]),
            json!([{"type":"function","function":{"name":"mcp__el__edit"}}, {"type":"function","function":{"name":"bash"}}, {"type":"function","function":{"name":"write"}}]),
            json!([{"type":"function","name":"el_edit"},{"name":"bash"},{"name":"write"}]),
            json!([{"name":"Edit","input_schema":{}},{"name":"Bash"},{"name":"Write"}]),
        ] {
            let mut request = json!({"tools":tools});
            remove_edit_tool(&mut request);
            let tools = request["tools"].to_string().to_lowercase();
            assert!(!tools.contains("edit"));
            assert!(tools.contains("bash"));
            assert!(tools.contains("write"));
        }
        let mut request = json!({"tools":[{"functionDeclarations":[{"name":"edit"}]}]});
        remove_edit_tool(&mut request);
        assert_eq!(request["tools"], json!([]));
    }
}
