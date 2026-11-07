//! Arrival replacement for oversized tool results (the reference's CCR, `kernel/src/ccr.ts`).
//!
//! The original is stored once under the Session spill dir, where `read ctx:<handle>` and
//! `ctx_search` page it back; the visible turn keeps a deterministic placeholder naming that
//! handle. Replace-once: the placeholder is recomputed from the recorded entry, never from
//! fresh bytes, so its text never changes for the life of the Session and the upstream prompt
//! cache stays intact. Only `functionResponse` content is touched, so the call/result pairing
//! the wires need is never broken.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tokens::text_tokens;
use crate::turnkeys;
use crate::wire::{response_text, set_response_text};

/// Smallest result, in estimated tokens, that is replaced by a handle: the reference's
/// `minToolTokens`. The tools spill their own output at 10 KiB with a head/tail preview, so
/// this only catches what a foreign tool sent back whole.
pub const MIN_TOKENS: u64 = 4_000;
const HEAD_CHARS: usize = 96;
const PREVIEW: &str = "[ctx:";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    id: String,
    handle: String,
    kind: String,
    tokens: u64,
    title: String,
}

/// Per-Session record of the replacements, persisted next to `splices.jsonl`.
#[derive(Debug)]
pub struct CtxStore {
    dir: PathBuf,
    file: PathBuf,
    entries: HashMap<String, Entry>,
    counter: AtomicU64,
}

impl CtxStore {
    pub fn open(dir: impl Into<PathBuf>, file: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let file = file.into();
        let mut entries = HashMap::new();
        if let Ok(text) = fs::read_to_string(&file) {
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                if let Ok(entry) = serde_json::from_str::<Entry>(line) {
                    entries.insert(entry.id.clone(), entry);
                }
            }
        }
        Self {
            dir,
            file,
            entries,
            counter: AtomicU64::new(0),
        }
    }

    /// Replaces every oversized tool result in `contents` with its placeholder. Returns how
    /// many turns changed. Idempotent: a turn that already carries a placeholder is left
    /// alone, and a result seen before gets the bytes recorded at arrival.
    pub fn apply(&mut self, contents: &mut [Value]) -> usize {
        let args = call_args(contents);
        contents
            .iter_mut()
            .map(|turn| self.apply_turn(turn, &args))
            .sum()
    }

    /// Replaces the oversized results of one turn, in its parts and in the `_raw` chat
    /// message the chat wires send upstream (`core_to_chat` prefers `_raw`).
    fn apply_turn(&mut self, turn: &mut Value, args: &HashMap<String, Value>) -> usize {
        let mut replaced = 0;
        let mut raw_text = None;
        if let Some(parts) = turn.get_mut("parts").and_then(Value::as_array_mut) {
            for part in parts.iter_mut() {
                let Some(result) = large_result(part) else {
                    continue;
                };
                if args.get(&result.id).is_some_and(pages_a_handle) {
                    continue;
                }
                let title = args
                    .get(&result.id)
                    .and_then(subject)
                    .unwrap_or_else(|| head(&result.text));
                let kind = kind(&result.name);
                let Ok(text) = self.project(&result.id, kind, result.tokens, title, &result.text)
                else {
                    continue;
                };
                set_response_text(part, &text);
                raw_text = Some(text);
                replaced += 1;
            }
        }
        if let (Some(raw), Some(text)) = (turn.get_mut(turnkeys::RAW), raw_text) {
            set_raw_content(raw, &text);
        }
        replaced
    }
    /// Returns the placeholder for `id`, storing `original` first if this is its arrival.
    fn project(
        &mut self,
        id: &str,
        kind: &str,
        tokens: u64,
        title: String,
        original: &str,
    ) -> io::Result<String> {
        if let Some(entry) = self.entries.get(id) {
            return Ok(placeholder(entry));
        }
        let handle = self.write(original)?;
        let entry = Entry {
            id: id.to_string(),
            handle,
            kind: kind.to_string(),
            tokens,
            title,
        };
        self.append(&entry)?;
        let text = placeholder(&entry);
        self.entries.insert(entry.id.clone(), entry);
        Ok(text)
    }

    fn write(&self, content: &str) -> io::Result<String> {
        fs::create_dir_all(&self.dir)?;
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let seq = self.counter.fetch_add(1, Ordering::Relaxed);
        let handle = format!("c{millis:x}{seq:04x}");
        fs::write(self.dir.join(&handle), content)?;
        Ok(handle)
    }

    fn append(&self, entry: &Entry) -> io::Result<()> {
        let mut line = serde_json::to_string(entry).map_err(io::Error::other)?;
        line.push('\n');
        let mut existing = fs::read(&self.file).unwrap_or_default();
        existing.extend_from_slice(line.as_bytes());
        fs::write(&self.file, existing)
    }
}

struct LargeResult {
    id: String,
    name: String,
    tokens: u64,
    text: String,
}

/// The oversized, not-yet-replaced result of one part.
fn large_result(part: &Value) -> Option<LargeResult> {
    let response = part.get("functionResponse")?;
    let text = response_text(response);
    if text.starts_with(PREVIEW) {
        return None;
    }
    let tokens = text_tokens(&text);
    if tokens < MIN_TOKENS {
        return None;
    }
    let id = response.get("id").and_then(Value::as_str)?.to_string();
    let name = response
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Some(LargeResult {
        id,
        name,
        tokens,
        text,
    })
}

fn call_args(contents: &[Value]) -> HashMap<String, Value> {
    let mut args = HashMap::new();
    for call in contents
        .iter()
        .filter_map(|turn| turn.get("parts"))
        .flat_map(|parts| {
            parts
                .as_array()
                .map(|parts| parts.as_slice())
                .unwrap_or_default()
        })
    {
        let Some(function_call) = call.get("functionCall") else {
            continue;
        };
        let Some(id) = function_call.get("id").and_then(Value::as_str) else {
            continue;
        };
        if let Some(arguments) = function_call.get("args") {
            args.insert(id.to_string(), arguments.clone());
        }
    }
    args
}

/// A `read ctx:<handle>` call: its result is a page of an original already stored, and
/// replacing it would send the model round in circles.
fn pages_a_handle(arguments: &Value) -> bool {
    arguments
        .get("path")
        .and_then(Value::as_str)
        .is_some_and(|path| path.trim_start().starts_with("ctx:"))
}

/// The call's subject (command, path, query), so the placeholder says what was run.
fn subject(arguments: &Value) -> Option<String> {
    ["command", "cmd", "script", "query", "path", "url"]
        .iter()
        .find_map(|field| {
            let value = arguments.get(*field)?.as_str()?.trim();
            (!value.is_empty()).then(|| clamp(value))
        })
}

fn head(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    clamp(line)
}

fn clamp(text: &str) -> String {
    let single = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if single.chars().count() <= HEAD_CHARS {
        return single;
    }
    let mut out: String = single.chars().take(HEAD_CHARS).collect();
    out.push('…');
    out
}

fn kind(name: &str) -> &'static str {
    let name = name
        .rsplit("__")
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    match name.as_str() {
        "bash" | "shell" | "exec" | "execute" | "background" => "shell output",
        "read" => "file read",
        "grep" | "find" | "search" | "ctx_search" | "glob" | "websearch" => "search output",
        "webfetch" | "fetch" | "curl" => "web fetch",
        _ => "tool result",
    }
}

fn placeholder(entry: &Entry) -> String {
    format!(
        "[ctx:{}] {} · {} tokens · {}\n   → read ctx:{}:<start>-<end> pages it; ctx_search finds snippets",
        entry.handle,
        entry.kind,
        entry.tokens,
        entry.title,
        entry.handle
    )
}

/// True for a result this module already replaced, so nothing re-stores it.
pub fn is_placeholder(text: &str) -> bool {
    text.starts_with(PREVIEW)
}

/// A tool result's raw chat message: its `content` is the text upstream reads.
fn set_raw_content(raw: &mut Value, text: &str) {
    if let Some(map) = raw.as_object_mut() {
        map.insert("content".to_string(), Value::String(text.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    fn contents(text: &str) -> Vec<Value> {
        vec![
            json!({"role": "model", "parts": [{"functionCall": {
                "name": "bash", "id": "call-1", "args": {"command": "cargo test"}}}]}),
            json!({"role": "user", "parts": [{"functionResponse": {
                "name": "bash", "id": "call-1", "response": {"result": text}}}]}),
        ]
    }

    fn result_text(contents: &[Value]) -> String {
        let response = contents[1]["parts"][0]["functionResponse"].clone();
        response_text(&response)
    }

    fn big() -> String {
        "line of test output\n".repeat(900)
    }

    #[test]
    fn a_result_under_the_threshold_is_left_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = store(dir.path());
        let mut turns = contents(&"line of test output\n".repeat(700));
        assert_eq!(store.apply(&mut turns), 0);
    }

    #[test]
    fn a_page_read_back_from_a_handle_is_never_replaced_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = store(dir.path());
        let mut turns = contents(&big());
        turns[0]["parts"][0]["functionCall"]["args"] = json!({"path": "ctx:c1f0:1-900"});
        turns[0]["parts"][0]["functionCall"]["name"] = json!("read");
        turns[1]["parts"][0]["functionResponse"]["name"] = json!("read");
        assert_eq!(store.apply(&mut turns), 0);
        assert_eq!(result_text(&turns), big());
    }

    fn store(dir: &Path) -> CtxStore {
        CtxStore::open(dir.join("spill"), dir.join("ctx.jsonl"))
    }

    #[test]
    fn a_large_result_is_replaced_once_and_paged_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = store(dir.path());
        let original = big();
        let mut turns = contents(&original);

        assert_eq!(store.apply(&mut turns), 1);

        let text = result_text(&turns);
        assert!(is_placeholder(&text), "{text}");
        assert!(text.contains("shell output"), "{text}");
        assert!(text.contains("cargo test"), "{text}");
        let handle = text
            .split("[ctx:")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .expect("handle");
        let stored = std::fs::read_to_string(dir.path().join("spill").join(handle)).expect("spill");
        assert_eq!(stored, original);
    }

    #[test]
    fn the_placeholder_never_changes_when_the_original_comes_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = store(dir.path());
        let mut first = contents(&big());
        store.apply(&mut first);

        let mut again = contents(&big());
        store.apply(&mut again);

        assert_eq!(result_text(&first), result_text(&again));
    }

    #[test]
    fn a_small_result_and_an_existing_placeholder_are_left_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = store(dir.path());
        let mut small = contents("ok");
        assert_eq!(store.apply(&mut small), 0);
        assert_eq!(result_text(&small), "ok");

        let mut replaced = contents(&big());
        store.apply(&mut replaced);
        assert_eq!(store.apply(&mut replaced), 0);
    }

    #[test]
    fn a_restarted_store_reuses_the_recorded_handle() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut first = contents(&big());
        store(dir.path()).apply(&mut first);

        let mut again = contents(&big());
        store(dir.path()).apply(&mut again);

        assert_eq!(result_text(&first), result_text(&again));
    }
}
