//! The one owner of the keys a conversation turn carries beside its content. Nothing else names
//! them: the compressor compares turns through `same_turn` and reads the transient mark through
//! `is_transient`, the wire strips and writes the keys through these helpers, and every splice
//! marker comes from `splice_marker`.
//!
//! - `NUDGE` marks the transient nudge turn: proxy-only, carried by one request, never stored by a
//!   fold and never sent upstream.
//! - `RAW` is the chat wires' copy of the exact wire message; `THOUGHT_SIGNATURE` is the Google
//!   wire's signature on a part. Stored originals keep both, because decompress restores them, so
//!   they are ignored by comparison and never stripped from a stored turn.

use serde_json::{json, Value};

/// Marks the transient nudge turn.
pub const NUDGE: &str = "_nudge";
/// The chat wires' copy of the exact wire message a turn was read from.
pub const RAW: &str = "_raw";
/// The Google wire's signature on a function-call part.
pub const THOUGHT_SIGNATURE: &str = "thoughtSignature";
const SPLICE_PREFIX: &str = "emberloop-splice:";
/// Keys that do not take part in turn identity.
const IGNORED_KEYS: [&str; 2] = [RAW, THOUGHT_SIGNATURE];

/// The marker a splice leaves in its tool result, keyed by the tool-result id it attaches to.
pub fn splice_marker(tool_result_id: &str) -> String {
    format!("{SPLICE_PREFIX}{tool_result_id}")
}

/// The transient nudge turn with `text`.
pub fn nudge_turn(text: &str) -> Value {
    json!({ "role": "user", "parts": [{ "text": text }], (NUDGE): true })
}

/// Whether the turn is the transient nudge: only the request carrying it holds it.
pub fn is_transient(turn: &Value) -> bool {
    turn.get(NUDGE).is_some()
}

/// Removes the nudge mark, so the turn can go upstream as a plain turn.
pub fn strip_transient(turn: &mut Value) {
    if let Some(map) = turn.as_object_mut() {
        map.remove(NUDGE);
    }
}

/// The comparison view of a value: the ignored keys dropped at every depth.
pub fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| !is_ignored(key))
                .map(|(key, field)| (key.clone(), canonical(field)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// Whether two turns are the same conversation turn: equal canonical views.
pub fn same_turn(a: &Value, b: &Value) -> bool {
    canonical(a) == canonical(b)
}

fn is_ignored(key: &str) -> bool {
    IGNORED_KEYS.contains(&key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A function-call turn carrying every ignored key at a different depth.
    fn wired_call() -> Value {
        json!({
            "role": "model",
            "parts": [{
                "functionCall": { "id": "c1", "name": "read", "args": { "path": "a.rs" } },
                "thoughtSignature": "sig-1"
            }],
            "_raw": { "role": "assistant", "tool_calls": [{ "id": "c1" }] }
        })
    }

    fn bare_call() -> Value {
        json!({
            "role": "model",
            "parts": [{ "functionCall": { "id": "c1", "name": "read", "args": { "path": "a.rs" } } }]
        })
    }

    #[test]
    fn canonical_drops_raw_and_thought_signature_at_any_depth() {
        assert_eq!(canonical(&wired_call()), bare_call());
    }

    #[test]
    fn canonical_keeps_the_nudge_mark_and_every_other_field() {
        let nudge = nudge_turn("nudge: first");
        assert_eq!(canonical(&nudge), nudge);
    }

    #[test]
    fn turns_that_differ_only_in_raw_or_thought_signature_are_the_same() {
        assert!(same_turn(&wired_call(), &bare_call()));
        assert!(same_turn(&bare_call(), &wired_call()));
        assert_eq!(canonical(&wired_call()), canonical(&bare_call()));
    }

    #[test]
    fn any_other_differing_field_is_not_the_same_turn() {
        let mut renamed = bare_call();
        renamed["parts"][0]["functionCall"]["name"] = json!("write");
        let mut text = json!({"role": "user", "parts": [{"text": "a"}]});
        let other_text = json!({"role": "user", "parts": [{"text": "b"}]});
        let with_extra = json!({"role": "user", "parts": [{"text": "a"}], "extra": 1});
        assert!(!same_turn(&bare_call(), &renamed));
        assert!(!same_turn(&text, &other_text));
        assert!(!same_turn(&text, &with_extra));
        assert!(!same_turn(&with_extra, &text));
        text["parts"][0]["text"] = json!("b");
        assert!(same_turn(&text, &other_text));
    }

    #[test]
    fn a_nudge_is_not_the_same_turn_as_a_plain_turn_with_its_text() {
        let plain = json!({"role": "user", "parts": [{"text": "nudge: first"}]});
        assert!(!same_turn(&nudge_turn("nudge: first"), &plain));
    }

    #[test]
    fn a_nudge_turn_is_transient_and_a_plain_turn_is_not() {
        assert!(is_transient(&nudge_turn("nudge: first")));
        assert!(!is_transient(&bare_call()));
        assert!(!is_transient(&wired_call()));
    }

    #[test]
    fn strip_transient_removes_only_the_mark() {
        let mut turn = nudge_turn("nudge: first");
        strip_transient(&mut turn);
        assert_eq!(
            turn,
            json!({"role": "user", "parts": [{"text": "nudge: first"}]})
        );
    }

    #[test]
    fn splice_marker_carries_the_tool_result_id() {
        assert_eq!(splice_marker("call_p"), "emberloop-splice:call_p");
    }
}
