#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Anthropic wire round trip for a Claude Code shaped request: an unspliced request changes only its
//! system array and tools, and a splice rewrites only the messages it spliced.
use prompts::{PinnedPrompts, PromptStore, PromptVars};
use proxy::wire::{
    extract_core_contents, inject_prompts_for_wire, inject_tools_for_wire, write_core_contents,
    WireKind,
};
use serde_json::{json, Value};

fn pins() -> PinnedPrompts {
    let dir = tempfile::tempdir().unwrap();
    PinnedPrompts::pin(&PromptStore::new(dir.path()), &PromptVars::default()).unwrap()
}

fn image() -> Value {
    json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}})
}

/// A Claude Code request: billing header first, breakpoints on system and messages, native and
/// MCP tools, extended thinking with a signature and a redacted block, a tool round with an image
/// result, and a block type this proxy does not know.
fn claude_code_request() -> Value {
    json!({
        "model": "claude-opus-4-1",
        "max_tokens": 32000,
        "stream": true,
        "thinking": {"type": "enabled", "budget_tokens": 31999},
        "metadata": {"user_id": "{\"device_id\":\"d1\"}"},
        "system": [
            {"type": "text", "text": "x-anthropic-billing-header: cc_version=2.0.1; cch=00000;"},
            {"type": "text", "text": "You are Claude Code.", "cache_control": {"type": "ephemeral"}},
            {"type": "text", "text": "Project notes.", "cache_control": {"type": "ephemeral"}}
        ],
        "tools": [
            {"name": "Bash", "description": "Run a command", "input_schema": {"type": "object"}},
            {"name": "mcp__el__read", "description": "read", "input_schema": {"type": "object"}}
        ],
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "fix the bug"}]},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "look first", "signature": "sig-abc=="},
                {"type": "redacted_thinking", "data": "EmYKAhgCIkC"},
                {"type": "text", "text": "Running it."},
                {"type": "tool_use", "id": "toolu_01", "name": "Bash", "input": {"command": "ls"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_01", "is_error": false,
                 "content": [{"type": "text", "text": "a.rs"}, image()]}
            ]},
            {"role": "assistant", "content": [
                {"type": "future_block", "payload": {"k": 1}},
                {"type": "text", "text": "Done.", "cache_control": {"type": "ephemeral"}}
            ]},
            {"role": "user", "content": [{"type": "text", "text": "look"}, image()]}
        ]
    })
}

/// The same request with a string system and no tools, the other common Anthropic shape.
fn plain_request() -> Value {
    json!({
        "model": "claude-sonnet-4-5",
        "system": "You are a helpful assistant.",
        "messages": [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": [{"type": "text", "text": "hello"}]},
            {"role": "user", "content": "again"}
        ]
    })
}

fn shapes() -> Vec<Value> {
    vec![claude_code_request(), plain_request()]
}

/// Runs the wire's own injection and the core round trip the way the proxy does.
fn round_trip(request: &mut Value) {
    let pins = pins();
    inject_tools_for_wire(WireKind::Anthropic, request, &pins);
    inject_prompts_for_wire(WireKind::Anthropic, request, &pins, false, "");
    let contents = extract_core_contents(WireKind::Anthropic, request).unwrap();
    write_core_contents(WireKind::Anthropic, request, contents);
}

#[test]
fn an_unspliced_request_changes_only_its_system_and_tools() {
    for original in shapes() {
        let mut request = original.clone();
        round_trip(&mut request);
        for key in original.as_object().unwrap().keys() {
            if key == "system" || key == "tools" {
                continue;
            }
            assert_eq!(request[key], original[key], "{key} changed");
        }
    }
}

#[test]
fn an_unspliced_request_keeps_its_system_blocks_and_tools_as_prefix() {
    for original in shapes() {
        let mut request = original.clone();
        round_trip(&mut request);
        let before = original["system"].as_array();
        let after = request["system"].as_array();
        if let (Some(before), Some(after)) = (before, after) {
            assert_eq!(&after[..before.len()], before.as_slice());
            assert_eq!(after.len(), before.len() + 1);
        }
        if let Some(tools) = original.get("tools").and_then(Value::as_array) {
            let after = request["tools"].as_array().unwrap();
            assert_eq!(&after[..tools.len()], tools.as_slice());
        }
    }
}

#[test]
fn claude_messages_keep_thinking_signatures_images_and_unknown_blocks_byte_exact() {
    let original = claude_code_request();
    let mut request = original.clone();
    round_trip(&mut request);
    assert_eq!(request["messages"], original["messages"]);
}

#[test]
fn a_splice_rewrites_only_the_messages_it_replaces() {
    let original = claude_code_request();
    let mut request = original.clone();
    let mut contents = extract_core_contents(WireKind::Anthropic, &request).unwrap();
    // Fold the tool round (messages 1 and 2) into one summary turn.
    let summary = json!({"role": "user", "parts": [{"text": "Folded: listed the files."}]});
    contents.splice(1..3, [summary]);
    write_core_contents(WireKind::Anthropic, &mut request, contents);

    let messages = request["messages"].as_array().unwrap();
    let before = original["messages"].as_array().unwrap();
    assert_eq!(messages.len(), before.len() - 1);
    assert_eq!(messages[0], before[0]);
    assert_eq!(
        messages[1],
        json!({"role": "user", "content": [{"type": "text", "text": "Folded: listed the files."}]})
    );
    assert_eq!(&messages[2..], &before[3..]);
}
