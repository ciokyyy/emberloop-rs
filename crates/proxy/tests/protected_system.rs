#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Protected content never folds, on every wire. System and developer items are not conversation
//! turns, so no fold can select them; a user message that carries a `<system-reminder>` text is a
//! protected turn, so a fold that covers it is refused and an unrelated fold leaves its bytes
//! alone. The property test drives random compress and decompress requests and checks that every
//! protected item comes back byte-identical.

use prompts::{PinnedPrompts, PromptStore, PromptVars};
use proptest::prelude::*;
use proxy::compressor::{
    CompressCall, CompressPolicy, Compressor, DecompressCall, NativeCompressor,
};
use proxy::wire::{extract_core_contents, inject_prompts_for_wire, write_core_contents, WireKind};
use serde_json::{json, Value};

const REMINDER: &str = "<system-reminder>\nCLAUDE.md: always use tabs.\n</system-reminder>";
const WIRES: [WireKind; 4] = [
    WireKind::OpenAiChat,
    WireKind::OpenAiResponses,
    WireKind::Anthropic,
    WireKind::Google,
];

fn wire_name(wire: WireKind) -> &'static str {
    match wire {
        WireKind::Google => "google",
        WireKind::OpenAiChat => "chat",
        WireKind::Anthropic => "anthropic",
        WireKind::OpenAiResponses => "responses",
    }
}

fn pins() -> PinnedPrompts {
    let dir = tempfile::tempdir().unwrap();
    PinnedPrompts::pin(&PromptStore::new(dir.path()), &PromptVars::default()).unwrap()
}

/// A system-role item in the wire's own shape. Anthropic has no system role in its messages, so
/// its item is the one a malformed request could carry; the rule must hold for it all the same.
fn system_item(wire: WireKind, text: &str) -> Value {
    match wire {
        WireKind::OpenAiChat => json!({"role": "developer", "content": text}),
        WireKind::OpenAiResponses => {
            json!({"type": "message", "role": "developer", "content": [{"type": "input_text", "text": text}]})
        }
        WireKind::Anthropic => json!({"role": "system", "content": text}),
        WireKind::Google => json!({"role": "system", "parts": [{"text": text}]}),
    }
}

/// A user message carrying a system reminder before the user's own text.
fn reminder_item(wire: WireKind, reminder: &str, text: &str) -> Value {
    match wire {
        WireKind::OpenAiChat => json!({"role": "user", "content": format!("{reminder}\n\n{text}")}),
        WireKind::OpenAiResponses => json!({"type": "message", "role": "user", "content": [
            {"type": "input_text", "text": reminder},
            {"type": "input_text", "text": text}
        ]}),
        WireKind::Anthropic => json!({"role": "user", "content": [
            {"type": "text", "text": reminder},
            {"type": "text", "text": text}
        ]}),
        WireKind::Google => json!({"role": "user", "parts": [{"text": reminder}, {"text": text}]}),
    }
}

fn text_item(wire: WireKind, user: bool, text: &str) -> Value {
    match wire {
        WireKind::OpenAiChat => {
            json!({"role": if user {"user"} else {"assistant"}, "content": text})
        }
        WireKind::OpenAiResponses => {
            json!({"type": "message", "role": if user {"user"} else {"assistant"}, "content": text})
        }
        WireKind::Anthropic => json!({"role": if user {"user"} else {"assistant"},
            "content": [{"type": "text", "text": text}]}),
        WireKind::Google => {
            json!({"role": if user {"user"} else {"model"}, "parts": [{"text": text}]})
        }
    }
}

/// A tool call and its result, two wire items.
fn call_items(wire: WireKind, id: &str, tool: &str) -> Vec<Value> {
    match wire {
        WireKind::OpenAiChat => vec![
            json!({"role": "assistant", "content": null, "tool_calls": [{"id": id, "type": "function",
                "function": {"name": tool, "arguments": "{\"path\":\"a.rs\"}"}}]}),
            json!({"role": "tool", "tool_call_id": id, "content": "a.rs"}),
        ],
        WireKind::OpenAiResponses => vec![
            json!({"type": "function_call", "call_id": id, "name": tool, "arguments": "{\"path\":\"a.rs\"}"}),
            json!({"type": "function_call_output", "call_id": id, "output": "a.rs"}),
        ],
        WireKind::Anthropic => vec![
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": tool,
                "input": {"path": "a.rs"}}]}),
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id,
                "content": "a.rs"}]}),
        ],
        WireKind::Google => vec![
            json!({"role": "model", "parts": [{"functionCall": {"id": id, "name": tool,
                "args": {"path": "a.rs"}}}]}),
            json!({"role": "user", "parts": [{"functionResponse": {"id": id, "name": tool,
                "response": {"result": "a.rs"}}}]}),
        ],
    }
}

/// The request body of one wire, with its own top-level system field: a system-role item first,
/// a reminder-bearing first user message, two tool rounds, then the latest user message.
fn request(wire: WireKind) -> Value {
    let mut items = vec![
        system_item(wire, "Base system prompt"),
        reminder_item(wire, REMINDER, "fix the bug"),
    ];
    items.extend(call_items(wire, "c1", "read"));
    items.push(text_item(wire, false, "done"));
    items.push(text_item(wire, true, "next"));
    items.push(text_item(wire, false, "ok"));
    items.push(text_item(wire, true, "latest"));
    match wire {
        WireKind::OpenAiChat => json!({"model": "m", "messages": items}),
        WireKind::OpenAiResponses => {
            json!({"model": "m", "instructions": "Top level", "input": items})
        }
        WireKind::Anthropic => json!({"model": "m", "system": "Top level", "messages": items}),
        WireKind::Google => json!({
            "systemInstruction": {"parts": [{"text": "Top level"}]},
            "contents": items
        }),
    }
}

fn items_of(wire: WireKind, body: &Value) -> Vec<Value> {
    let key = match wire {
        WireKind::OpenAiChat | WireKind::Anthropic => "messages",
        WireKind::OpenAiResponses => "input",
        WireKind::Google => "contents",
    };
    body[key].as_array().unwrap().clone()
}

fn compressor() -> (tempfile::TempDir, NativeCompressor) {
    let dir = tempfile::tempdir().unwrap();
    let compressor = NativeCompressor::new(dir.path()).unwrap();
    (dir, compressor)
}

fn call(start: usize, end: usize) -> CompressCall {
    CompressCall {
        start,
        end,
        summary: "folded".into(),
        topic: None,
    }
}

fn policy() -> CompressPolicy {
    CompressPolicy::default()
}

/// The protected items of a request, as the bytes they must keep.
fn protected_bytes(wire: WireKind, body: &Value) -> Vec<String> {
    items_of(wire, body)
        .iter()
        .filter(|item| {
            let role = item.get("role").and_then(Value::as_str).unwrap_or_default();
            matches!(role, "system" | "developer") || item.to_string().contains("<system-reminder>")
        })
        .map(|item| serde_json::to_string(item).unwrap())
        .collect()
}

#[test]
fn system_items_are_never_conversation_turns() {
    for wire in WIRES {
        let body = request(wire);
        let core = extract_core_contents(wire, &body).unwrap();
        assert_eq!(
            core.len(),
            7,
            "{}: system item leaked into the foldable turns",
            wire_name(wire)
        );
        let text = serde_json::to_string(&core).unwrap();
        assert!(!text.contains("Base system prompt"), "{}", wire_name(wire));
    }
}

#[test]
fn a_range_covering_a_reminder_turn_is_refused_and_changes_nothing() {
    for wire in WIRES {
        let body = request(wire);
        let (_dir, mut compressor) = compressor();
        let mut contents = extract_core_contents(wire, &body).unwrap();
        let before = contents.clone();
        let error = compressor
            .compress_batch(&mut contents, vec![call(0, 2)], &policy())
            .unwrap_err();
        assert!(error.contains("protected"), "{}: {error}", wire_name(wire));
        assert_eq!(contents, before, "{}", wire_name(wire));
    }
}

#[test]
fn a_range_of_only_protected_content_folds_nothing_and_says_why() {
    for wire in WIRES {
        let body = request(wire);
        let (_dir, mut compressor) = compressor();
        let mut contents = extract_core_contents(wire, &body).unwrap();
        let error = compressor
            .compress_batch(&mut contents, vec![call(0, 1)], &policy())
            .unwrap_err();
        assert!(error.contains("0..1"), "{}: {error}", wire_name(wire));
        assert!(
            error.contains("never folds"),
            "{}: {error}",
            wire_name(wire)
        );
        assert_eq!(contents.len(), 7, "{}", wire_name(wire));
    }
}

#[test]
fn an_unrelated_fold_keeps_every_protected_byte_and_the_top_level_system_field() {
    for wire in WIRES {
        let body = request(wire);
        let (_dir, mut compressor) = compressor();
        let mut contents = extract_core_contents(wire, &body).unwrap();
        compressor
            .compress_batch(&mut contents, vec![call(1, 3)], &policy())
            .unwrap();
        let mut out = body.clone();
        write_core_contents(wire, &mut out, contents);

        assert_eq!(
            protected_bytes(wire, &out),
            protected_bytes(wire, &body),
            "{}: a protected item changed bytes or moved out of the request",
            wire_name(wire)
        );
        for key in ["system", "instructions", "systemInstruction"] {
            assert_eq!(out.get(key), body.get(key), "{}: {key}", wire_name(wire));
        }
    }
}

#[test]
fn the_proxy_policy_text_never_becomes_a_foldable_turn() {
    let pins = pins();
    for wire in WIRES {
        let body = request(wire);
        let before = extract_core_contents(wire, &body).unwrap();
        let mut injected = body.clone();
        inject_prompts_for_wire(wire, &mut injected, &pins, true);
        assert_eq!(
            extract_core_contents(wire, &injected).unwrap(),
            before,
            "{}: injected text became a turn",
            wire_name(wire)
        );
    }
}

#[test]
fn a_decompress_restores_the_fold_and_protected_bytes_stay() {
    for wire in WIRES {
        let body = request(wire);
        let (_dir, mut compressor) = compressor();
        let mut contents = extract_core_contents(wire, &body).unwrap();
        compressor
            .compress_batch(&mut contents, vec![call(1, 4)], &policy())
            .unwrap();
        compressor
            .decompress(
                &mut contents,
                DecompressCall {
                    block_id: "b1".into(),
                    to_file: None,
                },
            )
            .unwrap();
        let mut out = body.clone();
        write_core_contents(wire, &mut out, contents);
        assert_eq!(
            protected_bytes(wire, &out),
            protected_bytes(wire, &body),
            "{}",
            wire_name(wire)
        );
    }
}

/// The same items regardless of where the wire keeps them: the chat-family wires move system
/// items to the front, so the property compares them as a multiset of bytes.
fn sorted(mut items: Vec<String>) -> Vec<String> {
    items.sort();
    items
}

/// A conversation unit before it is written in a wire's shape.
#[derive(Debug, Clone, Copy)]
enum Unit {
    System,
    Reminder,
    Text { user: bool },
    Call,
}

fn unit() -> impl Strategy<Value = Unit> {
    prop_oneof![
        Just(Unit::System),
        Just(Unit::Reminder),
        any::<bool>().prop_map(|user| Unit::Text { user }),
        Just(Unit::Call),
        Just(Unit::Call),
    ]
}

fn any_wire() -> impl Strategy<Value = WireKind> {
    (0..WIRES.len()).prop_map(|index| WIRES[index])
}

/// Items for a unit list, in the wire's shape. The latest user message closes the list.
fn build(wire: WireKind, units: &[Unit]) -> Value {
    let mut items = Vec::new();
    for (index, unit) in units.iter().enumerate() {
        match *unit {
            Unit::System => items.push(system_item(wire, &format!("system {index}"))),
            Unit::Reminder => items.push(reminder_item(
                wire,
                &format!("<system-reminder>r{index}</system-reminder>"),
                &format!("user {index}"),
            )),
            Unit::Text { user } => items.push(text_item(wire, user, &format!("text {index}"))),
            Unit::Call => items.extend(call_items(wire, &format!("c{index}"), "read")),
        }
    }
    items.push(text_item(wire, true, "latest"));
    match wire {
        WireKind::OpenAiChat => json!({"model": "m", "messages": items}),
        WireKind::OpenAiResponses => json!({"model": "m", "instructions": "Top", "input": items}),
        WireKind::Anthropic => json!({"model": "m", "system": "Top", "messages": items}),
        WireKind::Google => {
            json!({"systemInstruction": {"parts": [{"text": "Top"}]}, "contents": items})
        }
    }
}

fn config() -> ProptestConfig {
    ProptestConfig {
        failure_persistence: None,
        cases: 64,
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(config())]

    /// Random compress, refold and decompress requests on random conversations, on any wire: a
    /// request either folds or changes nothing, and after it every protected item of the body is
    /// still there byte for byte.
    #[test]
    fn splice_requests_never_change_protected_bytes(
        wire in any_wire(),
        units in prop::collection::vec(unit(), 2..14),
        ranges in prop::collection::vec((0usize..16, 1usize..5), 1..4),
        preserve in 0usize..2,
        decompress_first in any::<bool>(),
    ) {
        let body = build(wire, &units);
        let (_dir, mut compressor) = compressor();
        let policy = CompressPolicy { preserve_recent_messages: preserve, protected_tools: vec![] };
        let mut out = body.clone();
        let mut contents = extract_core_contents(wire, &out).unwrap();
        let calls: Vec<CompressCall> = ranges
            .iter()
            .map(|(start, len)| call(*start, start + len))
            .collect();
        let before = contents.clone();
        match compressor.compress_batch(&mut contents, calls, &policy) {
            Ok(_) => {}
            Err(_) => prop_assert_eq!(&contents, &before, "a refused batch changed the history"),
        }
        if decompress_first {
            let _ = compressor.decompress(&mut contents, DecompressCall { block_id: "b1".into(), to_file: None });
        }
        write_core_contents(wire, &mut out, contents);
        prop_assert_eq!(sorted(protected_bytes(wire, &out)), sorted(protected_bytes(wire, &body)));

        // A refold of what the harness resends keeps the protected bytes too.
        let mut again = extract_core_contents(wire, &out).unwrap();
        compressor.apply_skipping(&mut again, &[]);
        write_core_contents(wire, &mut out, again);
        prop_assert_eq!(sorted(protected_bytes(wire, &out)), sorted(protected_bytes(wire, &body)));
    }
}
