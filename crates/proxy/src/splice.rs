use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Google's documented stand-in for a function call the model never signed.
/// It costs some reasoning context, so a real signature always wins.
pub const UNSIGNED_CALL_SIGNATURE: &str = "skip_thought_signature_validator";

/// A byte-exact set of turns the proxy added to history, keyed by the id of the
/// tool-result turn it attaches to. Values stay as parsed: `thoughtSignature`
/// and every other opaque field ride along untouched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Splice {
    pub id: String,
    pub tool_result_id: String,
    /// Exact string whose presence means this splice is already in the history.
    pub marker: String,
    pub turns: Vec<Value>,
    #[serde(default)]
    pub append_parts: Vec<Value>,
}

impl Splice {
    pub fn new(tool_result_id: impl Into<String>, turns: Vec<Value>) -> Self {
        let tool_result_id = tool_result_id.into();
        Self {
            id: tool_result_id.clone(),
            marker: format!("emberloop-splice:{tool_result_id}"),
            tool_result_id,
            turns,
            append_parts: Vec::new(),
        }
    }

    pub fn with_marker(mut self, marker: impl Into<String>) -> Self {
        self.marker = marker.into();
        self
    }

    pub fn with_append_parts(mut self, parts: Vec<Value>) -> Self {
        self.append_parts = parts;
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpliceReport {
    pub applied: usize,
    pub dropped: usize,
}

/// Append-only store at `sessions/<id>/splices.jsonl`.
#[derive(Default)]
pub struct SpliceStore {
    splices: Vec<Splice>,
    path: Option<PathBuf>,
}

impl SpliceStore {
    /// Load every splice already recorded for a Session. A malformed line is
    /// skipped, so one bad append cannot lose the rest of the history.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut splices = Vec::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                if let Ok(splice) = serde_json::from_str::<Splice>(line) {
                    splices.push(splice);
                }
            }
        }
        Self {
            splices,
            path: Some(path),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn insert(&mut self, splice: Splice) -> std::io::Result<bool> {
        if self.splices.iter().any(|existing| existing.id == splice.id) {
            return Ok(false);
        }
        if let Some(path) = &self.path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = OpenOptions::new().create(true).append(true).open(path)?;
            let line = serde_json::to_string(&splice).map_err(std::io::Error::other)?;
            writeln!(file, "{line}")?;
        }
        self.splices.push(splice);
        Ok(true)
    }

    pub fn len(&self) -> usize {
        self.splices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.splices.is_empty()
    }

    pub fn clear(&mut self) {
        self.splices.clear();
    }

    /// Forget every splice, on disk too: the conversation they belong to is gone.
    pub fn reset(&mut self) -> std::io::Result<()> {
        self.splices.clear();
        match &self.path {
            Some(path) if path.exists() => std::fs::write(path, ""),
            _ => Ok(()),
        }
    }

    /// Re-attach every stored splice. An applied splice is detected by its
    /// marker. A harness-anchored splice whose turn is gone is dropped with it.
    /// A self-anchored splice (one the harness never saw, such as a proxy tool
    /// pair) attaches before the newest turn, because its own tool-result turn
    /// is not part of the harness history.
    pub fn apply(&self, contents: &mut Vec<Value>) -> SpliceReport {
        let mut report = SpliceReport::default();
        for splice in &self.splices {
            if marker_present(contents, &splice.marker) {
                continue;
            }
            match tool_result_index(contents, &splice.tool_result_id) {
                Some(index) => {
                    append_parts(&mut contents[index], &splice.append_parts);
                    insert_turns(contents, index + 1, &splice.turns);
                    report.applied += 1;
                }
                None if self_anchored(splice) => match replay_index(contents) {
                    Some(index) => {
                        insert_turns(contents, index, &splice.turns);
                        report.applied += 1;
                    }
                    None => report.dropped += 1,
                },
                None => report.dropped += 1,
            }
        }
        report
    }
}

fn marker_present(contents: &[Value], marker: &str) -> bool {
    !marker.is_empty()
        && contents
            .iter()
            .any(|turn| serde_json::to_string(turn).is_ok_and(|text| text.contains(marker)))
}

fn self_anchored(splice: &Splice) -> bool {
    splice.turns.iter().any(|turn| {
        turn.get("parts")
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                parts.iter().any(|part| {
                    part.get("functionResponse")
                        .and_then(|response| response.get("id"))
                        .and_then(Value::as_str)
                        == Some(splice.tool_result_id.as_str())
                })
            })
    })
}

fn tool_result_index(contents: &[Value], tool_result_id: &str) -> Option<usize> {
    contents.iter().position(|turn| {
        turn.get("parts")
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                parts.iter().any(|part| {
                    part.get("functionResponse")
                        .and_then(|response| response.get("id"))
                        .and_then(Value::as_str)
                        == Some(tool_result_id)
                })
            })
    })
}

fn append_parts(turn: &mut Value, parts: &[Value]) {
    if parts.is_empty() {
        return;
    }
    let Some(parts_array) = turn.get_mut("parts").and_then(Value::as_array_mut) else {
        return;
    };
    for part in parts {
        if !parts_array.contains(part) {
            parts_array.push(part.clone());
        }
    }
}

fn insert_turns(contents: &mut Vec<Value>, index: usize, turns: &[Value]) {
    for (offset, turn) in turns.iter().enumerate() {
        contents.insert(index + offset, signed_for_replay(turn));
    }
}
/// Where a stored model turn may go: before the newest model turn, where a
/// user turn precedes it, or in a gap between two user turns when folding left
/// no model turn. Anywhere else the wire rejects the turn order.
fn replay_index(contents: &[Value]) -> Option<usize> {
    let last_model = contents
        .iter()
        .rposition(|turn| turn.get("role").and_then(Value::as_str) == Some("model"))
        .filter(|index| *index > 0);
    last_model.or_else(|| {
        let role = |turn: &Value| turn.get("role").and_then(Value::as_str).map(str::to_string);
        (1..contents.len()).rev().find(|index| {
            role(&contents[*index - 1]).as_deref() == Some("user")
                && role(&contents[*index]).as_deref() == Some("user")
        })
    })
}

/// Google rejects a replayed model turn whose first function call carries no
/// thought signature, and a harness store may drop one. The documented
/// placeholder keeps the turn valid; a real signature is never overwritten.
pub fn sign_unsigned_calls(contents: &mut [Value]) {
    for turn in contents.iter_mut() {
        if turn.get("role").and_then(Value::as_str) != Some("model") {
            continue;
        }
        let Some(parts) = turn.get_mut("parts").and_then(Value::as_array_mut) else {
            continue;
        };
        let Some(first_call) = parts
            .iter_mut()
            .find(|part| part.get("functionCall").is_some())
        else {
            continue;
        };
        if first_call.get("thoughtSignature").is_none() {
            first_call["thoughtSignature"] = Value::String(UNSIGNED_CALL_SIGNATURE.to_string());
        }
    }
}

/// A stored model turn stands alone when replayed, so its first function call
/// needs a signature even if the model only signed an earlier call of its step.
fn signed_for_replay(turn: &Value) -> Value {
    let mut turn = turn.clone();
    if turn.get("role").and_then(Value::as_str) != Some("model") {
        return turn;
    }
    let first_call = turn
        .get_mut("parts")
        .and_then(Value::as_array_mut)
        .and_then(|parts| {
            parts
                .iter_mut()
                .find(|part| part.get("functionCall").is_some())
        });
    if let Some(part) = first_call {
        if part.get("thoughtSignature").is_none() {
            part["thoughtSignature"] = Value::String(UNSIGNED_CALL_SIGNATURE.to_string());
        }
    }
    turn
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_turn(id: &str) -> Value {
        json!({
            "role": "user",
            "parts": [{"functionResponse": {"name": "bash", "id": id, "response": {"output": "ok"}}}]
        })
    }

    fn thought_turn() -> Value {
        json!({
            "role": "model",
            "parts": [{
                "functionCall": {"name": "compress", "args": {"start": 1, "end": 2, "summary": "s"}},
                "thoughtSignature": "opaque-\u{1F600}-signature"
            }]
        })
    }

    #[test]
    fn an_unsigned_stored_call_gets_the_validator_bypass_when_replayed() {
        let unsigned = json!({
            "role": "model",
            "parts": [{"functionCall": {"id": "c1", "name": "compress", "args": {}}}]
        });
        let response = json!({
            "role": "user",
            "parts": [{"functionResponse": {"name": "compress", "id": "c1", "response": {}}}]
        });
        let mut store = SpliceStore::default();
        store
            .insert(Splice::new("c1", vec![unsigned, response]))
            .unwrap();
        let mut contents = vec![
            json!({"role": "user", "parts": [{"text": "hi"}]}),
            json!({"role": "model", "parts": [{"text": "ok"}]}),
        ];

        store.apply(&mut contents);

        assert_eq!(
            contents[1]["parts"][0]["thoughtSignature"],
            UNSIGNED_CALL_SIGNATURE
        );
    }

    #[test]
    fn splice_reattaches_byte_exact_and_keeps_thought_signature() {
        let mut contents = vec![
            json!({"role": "user", "parts": [{"text": "go"}]}),
            tool_turn("call_1"),
            json!({"role": "user", "parts": [{"text": "next"}]}),
        ];
        let model_turn = thought_turn();
        let response_turn = json!({
            "role": "user",
            "parts": [{"functionResponse": {"name": "compress", "id": "call_1", "response": {"result": "done"}}}]
        });
        let mut store = SpliceStore::default();
        store
            .insert(
                Splice::new("call_1", vec![model_turn.clone(), response_turn.clone()])
                    .with_marker("marker-call-1"),
            )
            .unwrap();
        let report = store.apply(&mut contents);
        assert_eq!(report.applied, 1);
        assert_eq!(contents[2], model_turn);
        assert_eq!(contents[3], response_turn);
        assert_eq!(
            contents[2]["parts"][0]["thoughtSignature"],
            "opaque-\u{1F600}-signature"
        );
    }

    #[test]
    fn double_apply_is_detected_by_marker() {
        let mut contents = vec![tool_turn("call_1")];
        let turns = vec![json!({
            "role": "model",
            "parts": [{"text": "marker-call-1"}]
        })];
        let mut store = SpliceStore::default();
        store
            .insert(Splice::new("call_1", turns).with_marker("marker-call-1"))
            .unwrap();
        assert_eq!(store.apply(&mut contents).applied, 1);
        let second = store.apply(&mut contents);
        assert_eq!(second.applied, 0);
        assert_eq!(contents.len(), 2);
    }

    #[test]
    fn dropped_anchor_drops_harness_splice() {
        let mut contents = vec![tool_turn("call_2")];
        let turns = vec![json!({"role": "user", "parts": [{"text": "steer"}]})];
        let mut store = SpliceStore::default();
        store
            .insert(Splice::new("call_1", turns).with_marker("steer-marker"))
            .unwrap();
        let report = store.apply(&mut contents);
        assert_eq!(report.dropped, 1);
        assert_eq!(contents, vec![tool_turn("call_2")]);
    }

    #[test]
    fn self_anchored_splice_attaches_before_the_newest_model_turn() {
        let mut contents = vec![
            json!({"role": "user", "parts": [{"text": "three"}]}),
            json!({"role": "model", "parts": [{"text": "four"}]}),
            json!({"role": "user", "parts": [{"text": "five"}]}),
        ];
        let pair = vec![
            json!({"role": "model", "parts": [{"functionCall": {"id": "p1", "name": "compress"}, "thoughtSignature": "sig"}]}),
            json!({"role": "user", "parts": [{"functionResponse": {"id": "p1", "name": "compress", "response": {"splice": "m"}}}]}),
        ];
        let mut store = SpliceStore::default();
        store.insert(Splice::new("p1", pair.clone())).unwrap();
        store.apply(&mut contents);
        assert_eq!(contents.len(), 5);
        assert_eq!(contents[1], pair[0]);
        assert_eq!(contents[2], pair[1]);
        assert_eq!(contents[3]["parts"][0]["text"], "four");
        assert_eq!(contents[4]["parts"][0]["text"], "five");
    }

    #[test]
    fn self_anchored_splice_is_dropped_when_no_model_turn_can_hold_it() {
        let mut contents = vec![json!({"role": "user", "parts": [{"text": "only"}]})];
        let pair = vec![
            json!({"role": "model", "parts": [{"functionCall": {"id": "p1", "name": "compress"}}]}),
            json!({"role": "user", "parts": [{"functionResponse": {"id": "p1", "name": "compress", "response": {}}}]}),
        ];
        let mut store = SpliceStore::default();
        store.insert(Splice::new("p1", pair)).unwrap();
        let report = store.apply(&mut contents);
        assert_eq!(report.dropped, 1);
        assert_eq!(contents.len(), 1);
    }

    #[test]
    fn an_unsigned_model_turn_gets_the_validator_bypass_and_a_signed_one_is_kept() {
        let mut contents = vec![
            json!({"role": "user", "parts": [{"text": "go"}]}),
            json!({"role": "model", "parts": [
                {"text": "thinking"},
                {"functionCall": {"id": "c1", "name": "bash", "args": {}}}
            ]}),
            json!({"role": "user", "parts": [{"functionResponse": {"id": "c1", "name": "bash", "response": {}}}]}),
            json!({"role": "model", "parts": [
                {"functionCall": {"id": "c2", "name": "bash", "args": {}}, "thoughtSignature": "real"}
            ]}),
        ];

        sign_unsigned_calls(&mut contents);

        assert_eq!(
            contents[1]["parts"][1]["thoughtSignature"],
            UNSIGNED_CALL_SIGNATURE
        );
        assert!(contents[1]["parts"][0].get("thoughtSignature").is_none());
        assert_eq!(contents[3]["parts"][0]["thoughtSignature"], "real");
    }

    #[test]
    fn persisted_splice_replays_byte_identical_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("splices.jsonl");
        let turns = vec![
            thought_turn(),
            json!({
                "role": "user",
                "parts": [{
                    "functionResponse": {
                        "name": "compress",
                        "id": "call_1",
                        "response": {"result": "done", "splice": "persisted-marker"}
                    }
                }]
            }),
        ];
        let expected = serde_json::to_vec(&turns).unwrap();
        let mut store = SpliceStore::open(&path);
        store
            .insert(Splice::new("call_1", turns).with_marker("persisted-marker"))
            .unwrap();
        drop(store);

        let reloaded = SpliceStore::open(&path);
        let mut contents = vec![tool_turn("call_1")];
        let report = reloaded.apply(&mut contents);
        assert_eq!(report.applied, 1);
        assert_eq!(serde_json::to_vec(&contents[1..]).unwrap(), expected);
        assert_eq!(reloaded.apply(&mut contents).applied, 0);
    }
}
