use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

/// One folded range: the summary survives, the original turns live in `spill/`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Block {
    #[serde(rename = "blockId")]
    id: String,
    topic: Option<String>,
    summary: String,
    originals: Vec<Value>,
    #[serde(default)]
    summary_turn: Value,
    #[serde(default)]
    restored: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressCall {
    pub start: usize,
    pub end: usize,
    pub summary: String,
    pub topic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecompressCall {
    pub block_id: String,
    pub to_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchHit {
    pub block_id: String,
    pub topic: Option<String>,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressorStatus {
    pub blocks: usize,
    pub active_blocks: usize,
    pub restored_blocks: usize,
    pub spilled_turns: usize,
    pub tokens_compressed: u64,
    pub spill_dir: String,
}

#[derive(Debug, Clone, Default)]
pub struct CompressPolicy {
    /// `compress.preserveRecentMessages`: turns at the tail that never fold.
    pub preserve_recent_messages: usize,
    /// `compress.protectedTools`: results of these tools never fold.
    pub protected_tools: Vec<String>,
}

impl CompressPolicy {
    pub fn with_recent(mut self, preserve_recent_messages: usize) -> Self {
        self.preserve_recent_messages = preserve_recent_messages;
        self
    }

    pub fn with_protected_tools(mut self, protected_tools: Vec<String>) -> Self {
        self.protected_tools = protected_tools;
        self
    }
}

/// Billion-context backend. Phase 1 ships the native implementation; a future
/// proxy-backed compressor implements the same operations.
pub trait Compressor: Send + Sync {
    fn compress(&mut self, contents: &mut Vec<Value>, call: CompressCall)
        -> Result<String, String>;
    /// Fold several ranges in one call. Every range is numbered against the history as it is
    /// now; they are validated together and either all fold or none does.
    fn compress_batch(
        &mut self,
        contents: &mut Vec<Value>,
        calls: Vec<CompressCall>,
    ) -> Result<String, String>;
    fn decompress(
        &mut self,
        contents: &mut Vec<Value>,
        call: DecompressCall,
    ) -> Result<String, String>;
    fn search_context(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, String>;
    fn status(&self) -> CompressorStatus;

    /// Recent-turn and protected-tool policy for the next folds. Backends without folds ignore it.
    fn set_policy(&mut self, _policy: CompressPolicy) {}

    /// Why no stored fold matched `contents`: one line for the session log. Backends without
    /// folds say nothing.
    fn fold_report(&self, _contents: &[Value], _skip: &[Value]) -> Option<String> {
        None
    }

    /// Re-attach stored folds to a request that the harness rebuilt from its
    /// own history. Backends without folds do nothing.
    fn apply(&mut self, contents: &mut Vec<Value>) -> usize {
        self.apply_skipping(contents, &[])
    }

    /// Like `apply`, ignoring `skip` turns when matching a fold's stored turns: steers and proxy
    /// pairs the splice store puts back after the fold is applied.
    fn apply_skipping(&mut self, _contents: &mut Vec<Value>, _skip: &[Value]) -> usize {
        0
    }
}

pub struct NativeCompressor {
    blocks: Vec<Block>,
    next_id: usize,
    spill_dir: PathBuf,
    policy: CompressPolicy,
}

impl NativeCompressor {
    pub fn new(spill_dir: impl Into<PathBuf>) -> Result<Self, String> {
        let mut compressor = Self {
            blocks: Vec::new(),
            next_id: 1,
            spill_dir: spill_dir.into(),
            policy: CompressPolicy::default(),
        };
        let entries = match std::fs::read_dir(compressor.spill_dir.join("blocks")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(compressor),
            Err(error) => return Err(format!("load compression blocks: {error}")),
        };
        for entry in entries {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let bytes =
                std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            let mut block: Block = serde_json::from_slice(&bytes)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            let next = block
                .id
                .strip_prefix('b')
                .and_then(|id| id.parse::<usize>().ok())
                .filter(|id| *id > 0)
                .and_then(|id| id.checked_add(1))
                .ok_or_else(|| format!("invalid compression block id {:?}", block.id))?;
            if path.file_stem().and_then(|s| s.to_str()) != Some(block.id.as_str()) {
                return Err(format!(
                    "compression block id does not match {}",
                    path.display()
                ));
            }
            if block.summary_turn.is_null() {
                // Older files kept the summary but not its exact on-wire marker.
                block.summary_turn = Self::summary_turn(format!(
                    "📦 [acp] Compressed block {}\n\n{}",
                    block.id, block.summary
                ));
            }
            compressor.next_id = compressor.next_id.max(next);
            compressor.blocks.push(block);
        }
        compressor.blocks.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(compressor)
    }

    fn spill_originals(&self, block: &Block) -> Result<(), String> {
        let dir = self.spill_dir.join("blocks");
        std::fs::create_dir_all(&dir).map_err(|err| format!("spill dir: {err}"))?;
        let bytes = serde_json::to_vec(block).map_err(|err| err.to_string())?;
        let temporary = dir.join(format!("{}.tmp", block.id));
        std::fs::write(&temporary, bytes).map_err(|err| format!("spill write: {err}"))?;
        std::fs::rename(temporary, dir.join(format!("{}.json", block.id)))
            .map_err(|err| format!("spill rename: {err}"))
    }

    fn marker(&self, call: &CompressCall, block_id: &str) -> String {
        let topic = call
            .topic
            .as_deref()
            .map(|topic| format!(" · {topic}"))
            .unwrap_or_default();
        format!(
            "📦 [acp] Compressed turns {}–{} into block {}{topic}\n\n{}",
            call.start,
            call.end.saturating_sub(1),
            block_id,
            call.summary
        )
    }

    fn summary_turn(marker: String) -> Value {
        json!({"role": "user", "parts": [{"text": marker}]})
    }

    fn fold_range(
        &mut self,
        contents: &mut Vec<Value>,
        call: CompressCall,
        id: &str,
    ) -> Result<String, String> {
        let originals = contents[call.start..call.end].to_vec();
        let summary_turn = Self::summary_turn(self.marker(&call, id));
        let block = Block {
            id: id.to_string(),
            topic: call.topic.clone(),
            summary: call.summary.clone(),
            originals,
            summary_turn: summary_turn.clone(),
            restored: false,
        };
        self.spill_originals(&block)?;
        contents.splice(call.start..call.end, [summary_turn]);
        self.blocks.push(block);
        Ok(format!(
            "Compressed turns {}–{} into block {id}. Originals kept in the session spill dir.",
            call.start,
            call.end.saturating_sub(1)
        ))
    }

    fn block_index(&self, id: &str) -> Option<usize> {
        self.blocks.iter().position(|block| block.id == id)
    }

    fn sanitize_file_name(name: &str) -> Result<&str, String> {
        let plain = !name.trim().is_empty()
            && name == name.trim()
            && !name.contains(['/', '\\'])
            && name != "."
            && name != "..";
        if plain {
            Ok(name)
        } else {
            Err("toFile must be a plain file name inside the session spill dir".into())
        }
    }
}

impl Compressor for NativeCompressor {
    fn compress(
        &mut self,
        contents: &mut Vec<Value>,
        call: CompressCall,
    ) -> Result<String, String> {
        self.compress_batch(contents, vec![call])
    }

    fn compress_batch(
        &mut self,
        contents: &mut Vec<Value>,
        mut calls: Vec<CompressCall>,
    ) -> Result<String, String> {
        if calls.is_empty() {
            return Err("ranges must not be empty".into());
        }
        calls.sort_by_key(|call| call.start);
        let several = calls.len() > 1;
        for (index, call) in calls.iter().enumerate() {
            let named = |error: String| {
                if several {
                    format!("range {}: {error}", index + 1)
                } else {
                    error
                }
            };
            validate_range(call, contents, &self.policy).map_err(named)?;
            if index > 0 && call.start < calls[index - 1].end {
                return Err(format!(
                    "range {} overlaps range {}: turns {}–{} and {}–{}",
                    index + 1,
                    index,
                    calls[index - 1].start,
                    calls[index - 1].end.saturating_sub(1),
                    call.start,
                    call.end.saturating_sub(1)
                ));
            }
        }

        // Ids follow the position in the history, but ranges are folded from the highest down
        // so folding one never moves the numbers of the ones before it.
        let first_id = self.next_id;
        self.next_id += calls.len();
        let mut lines = vec![String::new(); calls.len()];
        for (index, call) in calls.into_iter().enumerate().rev() {
            let id = format!("b{}", first_id + index);
            lines[index] = self.fold_range(contents, call, &id)?;
        }
        Ok(lines.join("\n"))
    }

    fn decompress(
        &mut self,
        contents: &mut Vec<Value>,
        call: DecompressCall,
    ) -> Result<String, String> {
        let index = self
            .block_index(&call.block_id)
            .ok_or_else(|| format!("unknown block {}", call.block_id))?;
        let block = self.blocks[index].clone();
        if block.restored {
            return Ok(format!("block {} is already restored", block.id));
        }

        if let Some(name) = call.to_file.as_deref() {
            let name = Self::sanitize_file_name(name)?;
            std::fs::create_dir_all(&self.spill_dir).map_err(|err| err.to_string())?;
            let path = self.spill_dir.join(name);
            let bytes =
                serde_json::to_vec_pretty(&block.originals).map_err(|err| err.to_string())?;
            std::fs::write(&path, bytes).map_err(|err| err.to_string())?;
            return Ok(format!(
                "Restored block {} to {}",
                block.id,
                path.to_string_lossy()
            ));
        }

        let position = contents.iter().position(|turn| turn == &block.summary_turn);
        if position.is_none() && !contains_window(contents, &block.originals) {
            return Err(format!(
                "block {} is not present in this conversation",
                block.id
            ));
        }
        let mut restored = block.clone();
        restored.restored = true;
        self.spill_originals(&restored)?;
        if let Some(position) = position {
            contents.splice(position..position + 1, block.originals.clone());
        }
        self.blocks[index] = restored;
        Ok(format!(
            "Restored {} turns from block {}",
            block.originals.len(),
            block.id
        ))
    }

    fn search_context(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, String> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Err("query must not be empty".into());
        }
        let hits = self
            .blocks
            .iter()
            .filter(|block| {
                block.summary.to_lowercase().contains(&needle)
                    || block
                        .topic
                        .as_deref()
                        .is_some_and(|topic| topic.to_lowercase().contains(&needle))
            })
            .take(limit.max(1))
            .map(|block| SearchHit {
                block_id: block.id.clone(),
                topic: block.topic.clone(),
                snippet: snippet_for(&block.summary, &needle),
            })
            .collect();
        Ok(hits)
    }

    fn status(&self) -> CompressorStatus {
        CompressorStatus {
            blocks: self.blocks.len(),
            active_blocks: self.blocks.iter().filter(|block| !block.restored).count(),
            restored_blocks: self.blocks.iter().filter(|block| block.restored).count(),
            spilled_turns: self
                .blocks
                .iter()
                .filter(|block| !block.restored)
                .map(|block| block.originals.len())
                .sum(),
            tokens_compressed: self
                .blocks
                .iter()
                .filter(|block| !block.restored)
                .map(|block| token_count(&block.originals))
                .sum(),
            spill_dir: self.spill_dir.to_string_lossy().to_string(),
        }
    }

    fn set_policy(&mut self, policy: CompressPolicy) {
        self.policy = policy;
    }

    fn apply_skipping(&mut self, contents: &mut Vec<Value>, skip: &[Value]) -> usize {
        let skip_ids = turn_ids(skip);
        let mut applied = 0;
        // Widest first: a block folded over an earlier block's raw turns must win, or the
        // smaller fold lands first and the wider one can never match again. Then repeat until
        // nothing moves, because every fold over a fold's marker needs the previous one applied:
        // a chain of n folds takes n rounds.
        let mut blocks: Vec<&Block> = self
            .blocks
            .iter()
            .filter(|block| !block.restored && !block.originals.is_empty())
            .collect();
        blocks.sort_by_key(|block| std::cmp::Reverse(block.originals.len()));
        let cap = blocks.len() * 2 + 4;
        loop {
            let mut moved = false;
            for block in &blocks {
                let wanted: Vec<Value> = block
                    .originals
                    .iter()
                    .filter(|turn| !is_transient(turn) && !is_skipped(turn, skip, &skip_ids))
                    .cloned()
                    .collect();
                if let Some((start, end)) = find_span(contents, &wanted) {
                    let replacement = [block.summary_turn.clone()];
                    contents.splice(start..end, replacement);
                    applied += 1;
                    moved = true;
                }
            }
            if !moved || applied >= cap {
                break;
            }
        }
        applied
    }
    fn fold_report(&self, contents: &[Value], skip: &[Value]) -> Option<String> {
        let skip_ids = turn_ids(skip);
        let block = self
            .blocks
            .iter()
            .filter(|block| !block.restored && !block.originals.is_empty())
            .max_by_key(|block| block.originals.len())?;
        let wanted: Vec<Value> = block
            .originals
            .iter()
            .filter(|turn| !is_transient(turn) && !is_skipped(turn, skip, &skip_ids))
            .cloned()
            .collect();
        let head = wanted.first()?;
        let Some(start) = contents.iter().position(|turn| turn_eq(turn, head)) else {
            return Some(format!(
                "{}: first stored turn `{}` is not in the request",
                block.id,
                brief(head)
            ));
        };
        for (offset, turn) in wanted.iter().enumerate().skip(1) {
            let found = contents.get(start + offset);
            if found.is_none_or(|found| !turn_eq(turn, found)) {
                return Some(format!(
                    "{}: turns 0..{offset} matched at {start}, turn {offset} differs: stored `{}` vs request `{}`",
                    block.id,
                    brief(turn),
                    found.map_or("<none>".to_string(), brief)
                ));
            }
        }
        None
    }
}

/// One short line for a stored or requested turn, used only by `fold_report`.
fn brief(turn: &Value) -> String {
    let text = turn
        .get("parts")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .map(|part| {
                    match part
                        .get("functionCall")
                        .or_else(|| part.get("functionResponse"))
                    {
                        Some(call) => format!(
                            "{}[{}]",
                            call.get("name").and_then(Value::as_str).unwrap_or("?"),
                            call.get("id").and_then(Value::as_str).unwrap_or("?")
                        ),
                        None => part
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .chars()
                            .take(40)
                            .collect(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .unwrap_or_default();
    text.chars().take(80).collect()
}

/// The nudge is injected into one request only; a fold that spanned it stored a turn no later
/// request carries, which would otherwise stop the fold from ever matching again.
fn is_transient(turn: &Value) -> bool {
    turn.get("_nudge").is_some()
}

fn validate_range(
    call: &CompressCall,
    contents: &[Value],
    policy: &CompressPolicy,
) -> Result<(), String> {
    let turns = contents.len();
    if call.start >= call.end || call.end > turns {
        return Err(format!(
            "invalid range {}..{} for {turns} turns",
            call.start, call.end
        ));
    }
    if call.summary.trim().is_empty() {
        return Err("summary must not be empty".into());
    }
    let keep = policy.preserve_recent_messages.min(turns);
    if call.end > turns - keep {
        return Err(format!(
            "range {}..{} includes the last {keep} preserved turns",
            call.start, call.end
        ));
    }
    if let Some(name) = policy.protected_tools.iter().find(|name| {
        contents[call.start..call.end]
            .iter()
            .any(|turn| turn_tool_names(turn).any(|tool| tool == name.as_str()))
    }) {
        return Err(format!(
            "range {}..{} includes protected tool {name}",
            call.start, call.end
        ));
    }
    Ok(())
}

/// Names of every tool call and result in a turn, whatever the wire's shape.
fn turn_tool_names(turn: &Value) -> impl Iterator<Item = &str> {
    turn.get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| {
            part.get("functionCall")
                .or_else(|| part.get("functionResponse"))
                .and_then(|call| call.get("name"))
                .and_then(Value::as_str)
        })
}

fn token_count(turns: &[Value]) -> u64 {
    turns
        .iter()
        .map(|turn| u64::try_from(turn.to_string().len() / 4).unwrap_or(u64::MAX))
        .sum()
}

/// Most turns a stored steer or a proxy pair may add between two folded turns. They are put back
/// into the history before a block is looked for, so the harness's own turns are no longer
/// contiguous; a bigger gap is a different conversation, not a splice.
const MAX_SPLICE_GAP: usize = 4;

/// Where the folded turns sit in `contents`: the range from the first to the last of them, found
/// in order with at most `MAX_SPLICE_GAP` unrelated turns between neighbours. The fast path is the
/// exact contiguous window.
fn find_span(contents: &[Value], originals: &[Value]) -> Option<(usize, usize)> {
    if originals.is_empty() || originals.len() > contents.len() {
        return None;
    }
    if let Some(start) = contents
        .windows(originals.len())
        .position(|turns| turns.iter().zip(originals).all(|(a, b)| turn_eq(a, b)))
    {
        return Some((start, start + originals.len()));
    }
    (0..contents.len())
        .filter(|&start| turn_eq(&contents[start], &originals[0]))
        .find_map(|start| span_from(contents, originals, start))
}

fn span_from(contents: &[Value], originals: &[Value], start: usize) -> Option<(usize, usize)> {
    let mut at = start;
    for original in &originals[1..] {
        let next = (at + 1..contents.len().min(at + 2 + MAX_SPLICE_GAP))
            .find(|&index| turn_eq(&contents[index], original))?;
        at = next;
    }
    Some((start, at + 1))
}

/// Turn equality that ignores `thoughtSignature` and `_raw`: the Google wire signs unsigned calls
/// after a fold, and the chat wires wrap every turn in a `_raw` copy the proxy's own turns lack.
fn turn_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let kept = |m: &serde_json::Map<String, Value>| {
                m.keys()
                    .filter(|k| *k != "thoughtSignature" && *k != "_raw")
                    .count()
            };
            kept(x) == kept(y)
                && x.iter()
                    .filter(|(k, _)| *k != "thoughtSignature" && *k != "_raw")
                    .all(|(k, v)| y.get(k).is_some_and(|w| turn_eq(v, w)))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| turn_eq(p, q))
        }
        _ => a == b,
    }
}

/// Every function-call id the stored turns carry. A splice pair changes shape on the wire round
/// trip (`_raw`, the tool name, the marker's field), so the id is what identifies it.
fn turn_ids(turns: &[Value]) -> Vec<&str> {
    turns
        .iter()
        .filter_map(|turn| turn.get("parts").and_then(Value::as_array))
        .flatten()
        .filter_map(|part| {
            part.get("functionCall")
                .or_else(|| part.get("functionResponse"))
        })
        .filter_map(|call| call.get("id").and_then(Value::as_str))
        .collect()
}

fn is_skipped(turn: &Value, skip: &[Value], skip_ids: &[&str]) -> bool {
    let id_matches = turn
        .get("parts")
        .and_then(Value::as_array)
        .is_some_and(|parts| {
            parts.iter().any(|part| {
                part.get("functionCall")
                    .or_else(|| part.get("functionResponse"))
                    .and_then(|call| call.get("id"))
                    .and_then(Value::as_str)
                    .is_some_and(|id| skip_ids.contains(&id))
            })
        });
    id_matches || skip.iter().any(|spliced| turn_eq(turn, spliced))
}

fn contains_window(contents: &[Value], window: &[Value]) -> bool {
    find_span(contents, window).is_some()
}

fn snippet_for(summary: &str, needle: &str) -> String {
    let lower = summary.to_lowercase();
    let Some(byte) = lower.find(needle) else {
        return summary.chars().take(160).collect();
    };
    let start = byte.saturating_sub(60);
    let end = (byte + needle.len() + 100).min(summary.len());
    let start = floor_char_boundary(summary, start);
    let end = ceil_char_boundary(summary, end);
    summary[start..end].trim().to_string()
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn turns() -> Vec<Value> {
        vec![
            json!({"role": "user", "parts": [{"text": "first"}]}),
            json!({"role": "model", "parts": [{"text": "second"}]}),
            json!({"role": "user", "parts": [{"text": "third"}]}),
            json!({"role": "model", "parts": [{"text": "fourth"}]}),
        ]
    }

    fn call() -> CompressCall {
        CompressCall {
            start: 1,
            end: 3,
            summary: "**second** and **third** agreed on `x=1`".into(),
            topic: Some("plan".into()),
        }
    }

    #[test]
    fn restart_recovers_folds_search_and_originals_without_reusing_ids() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        compressor.compress(&mut contents, call()).unwrap();
        let folded = contents.clone();
        let saved = std::fs::read(dir.path().join("blocks/b1.json")).unwrap();
        drop(compressor);

        let mut reloaded = NativeCompressor::new(dir.path()).unwrap();
        assert_eq!(reloaded.search_context("x=1", 3).unwrap().len(), 1);
        let mut replay = turns();
        assert_eq!(reloaded.apply(&mut replay), 1);
        assert_eq!(replay, folded);
        reloaded
            .decompress(
                &mut replay,
                DecompressCall {
                    block_id: "b1".into(),
                    to_file: None,
                },
            )
            .unwrap();
        assert_eq!(replay, turns());
        let mut restarted = NativeCompressor::new(dir.path()).unwrap();
        assert_eq!(restarted.status().restored_blocks, 1);
        assert_eq!(restarted.apply(&mut replay), 0);
        restarted
            .compress(
                &mut contents,
                CompressCall {
                    start: 0,
                    end: 1,
                    summary: "new fold".into(),
                    topic: None,
                },
            )
            .unwrap();
        assert!(dir.path().join("blocks/b2.json").exists());
        let old: Value = serde_json::from_slice(&saved).unwrap();
        let current: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("blocks/b1.json")).unwrap())
                .unwrap();
        assert_eq!(old["originals"], current["originals"]);
    }

    #[test]
    fn restart_replays_nested_folds_byte_exactly() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        compressor.compress(&mut contents, call()).unwrap();
        compressor
            .compress(
                &mut contents,
                CompressCall {
                    start: 0,
                    end: 2,
                    summary: "nested plan".into(),
                    topic: None,
                },
            )
            .unwrap();
        let mut reloaded = NativeCompressor::new(dir.path()).unwrap();
        let mut replay = turns();
        assert_eq!(reloaded.apply(&mut replay), 2);
        assert_eq!(replay, contents);
    }

    #[test]
    fn legacy_blocks_remain_recoverable_and_corrupt_files_are_not_overwritten() {
        let dir = tempdir().unwrap();
        let blocks = dir.path().join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        std::fs::write(
            blocks.join("b7.json"),
            serde_json::to_vec(&json!({
                "blockId": "b7", "summary": "legacy plan", "topic": null, "originals": turns(),
            }))
            .unwrap(),
        )
        .unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        assert_eq!(
            compressor.search_context("legacy", 1).unwrap()[0].block_id,
            "b7"
        );
        compressor
            .decompress(
                &mut Vec::new(),
                DecompressCall {
                    block_id: "b7".into(),
                    to_file: Some("legacy.json".into()),
                },
            )
            .unwrap();
        let restored: Vec<Value> =
            serde_json::from_slice(&std::fs::read(dir.path().join("legacy.json")).unwrap())
                .unwrap();
        assert_eq!(restored, turns());
        compressor.compress(&mut turns(), call()).unwrap();
        assert!(blocks.join("b8.json").exists());
        std::fs::write(blocks.join("b9.json"), b"broken json").unwrap();
        assert!(NativeCompressor::new(dir.path()).is_err());
        assert_eq!(
            std::fs::read(blocks.join("b9.json")).unwrap(),
            b"broken json"
        );
    }

    #[test]
    fn preserve_recent_messages_rejects_a_range_that_reaches_the_tail() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        compressor.set_policy(CompressPolicy::default().with_recent(2));
        let mut contents = turns();
        let error = compressor.compress(&mut contents, call()).unwrap_err();
        assert!(error.contains("last 2 preserved turns"), "{error}");
        assert_eq!(contents, turns());
    }

    #[test]
    fn protected_tools_never_fold_and_are_named_in_the_error() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        compressor.set_policy(CompressPolicy::default().with_protected_tools(vec!["read".into()]));
        let mut contents = vec![
            json!({"role": "user", "parts": [{"text": "first"}]}),
            json!({"role": "model", "parts": [{"functionCall": {"id": "c1", "name": "read", "args": {}}}]}),
            json!({"role": "user", "parts": [{"functionResponse": {"id": "c1", "name": "read", "response": {"result": "x"}}}]}),
            json!({"role": "model", "parts": [{"text": "fourth"}]}),
        ];
        let error = compressor.compress(&mut contents, call()).unwrap_err();
        assert!(error.contains("protected tool read"), "{error}");
    }

    #[test]
    fn status_reports_compressed_tokens() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        compressor.compress(&mut contents, call()).unwrap();
        let status = compressor.status();
        assert_eq!(status.active_blocks, 1);
        assert_eq!(status.spilled_turns, 2);
        assert!(status.tokens_compressed > 0);
    }

    #[test]
    fn parity_fold_swaps_range_for_block_and_spills_originals() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        let originals = contents[1..3].to_vec();

        compressor.compress(&mut contents, call()).unwrap();

        assert_eq!(contents.len(), 3);
        assert!(contents[1]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("block b1"));
        let spill = std::fs::read_to_string(dir.path().join("blocks/b1.json")).unwrap();
        let spill: Value = serde_json::from_str(&spill).unwrap();
        assert_eq!(spill["originals"], json!(originals));
        assert_eq!(compressor.status().active_blocks, 1);
    }

    #[test]
    fn parity_decompress_restores_the_exact_turns() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        let originals = contents[1..3].to_vec();
        compressor.compress(&mut contents, call()).unwrap();

        compressor
            .decompress(
                &mut contents,
                DecompressCall {
                    block_id: "b1".into(),
                    to_file: None,
                },
            )
            .unwrap();

        assert_eq!(contents[1..3], originals);
        assert_eq!(compressor.status().restored_blocks, 1);
    }

    #[test]
    fn parity_apply_refolds_a_conversation_the_harness_rebuilt() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        compressor.compress(&mut contents, call()).unwrap();

        let mut rebuilt = turns();
        assert_eq!(compressor.apply(&mut rebuilt), 1);
        assert_eq!(rebuilt.len(), 3);
        assert!(rebuilt[1]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("block b1"));
    }

    #[test]
    fn apply_refolds_when_spliced_turns_sit_inside_the_folded_range() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        compressor.compress(&mut contents, call()).unwrap();

        // The harness resends the original turns; a stored steer and a proxy pair were put
        // back between them before the block is looked for.
        let mut rebuilt = turns();
        rebuilt.insert(
            2,
            json!({"role": "user", "parts": [{"text": "steer: spliced"}]}),
        );
        rebuilt.insert(3, json!({"role": "user", "parts": [{"text": "pair"}]}));
        assert_eq!(compressor.apply(&mut rebuilt), 1);
        assert!(rebuilt.iter().any(|turn| turn["parts"][0]["text"]
            .as_str()
            .is_some_and(|t| t.contains("block b1"))));
        assert!(!rebuilt
            .iter()
            .any(|turn| turn["parts"][0]["text"].as_str() == Some("steer: spliced")));
    }

    #[test]
    fn apply_does_not_swallow_a_distant_lookalike() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        compressor.compress(&mut contents, call()).unwrap();

        // The first folded turn is present, the rest is far away behind unrelated turns.
        let originals = turns();
        let mut rebuilt = vec![originals[1].clone()];
        for n in 0..12 {
            rebuilt.push(json!({"role": "user", "parts": [{"text": format!("other {n}")}]}));
        }
        rebuilt.push(originals[2].clone());
        let before = rebuilt.clone();
        assert_eq!(compressor.apply(&mut rebuilt), 0);
        assert_eq!(rebuilt, before);
    }

    /// Turns as the OpenAI wire extracts them: every turn carries the exact wire message in `_raw`.
    fn raw_turns() -> Vec<Value> {
        (0..6)
            .map(|n| {
                json!({
                    "role": "user",
                    "parts": [{"text": format!("t{n}")}],
                    "_raw": {"role": "user", "content": format!("t{n}")}
                })
            })
            .collect()
    }

    #[test]
    fn apply_refolds_a_fold_whose_marker_came_back_with_raw() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = raw_turns();
        compressor.compress(&mut contents, range(1, 3)).unwrap();
        // The wire round trip gives the inserted marker a `_raw` field; a later fold stores it.
        for turn in &mut contents {
            turn.as_object_mut()
                .unwrap()
                .insert("_raw".into(), json!({"role": "user"}));
        }
        compressor.compress(&mut contents, range(1, 4)).unwrap();

        // The harness resends only its own turns: neither marker is in them.
        let mut rebuilt = raw_turns();
        assert_eq!(compressor.apply(&mut rebuilt), 2);
        assert_eq!(rebuilt.len(), 3);
        assert!(rebuilt[1]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("block b2"));
    }

    #[test]
    fn apply_refolds_a_fold_whose_originals_carry_a_transient_nudge() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = raw_turns();
        // The proxy inserts a nudge in every request while its gate holds, so a fold over the
        // history stores one; the harness never saw it and its next request lacks the turn.
        contents.insert(
            2,
            json!({"role": "user", "parts": [{"text": "nudge: first"}], "_nudge": true}),
        );
        compressor.compress(&mut contents, range(1, 4)).unwrap();

        let mut rebuilt = raw_turns();
        assert_eq!(compressor.apply(&mut rebuilt), 1);
        assert!(rebuilt.iter().any(|turn| {
            turn["parts"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .contains("block b1")
        }));
    }

    #[test]
    fn apply_skipping_removes_a_proxy_pair_by_its_call_id() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = raw_turns();
        let pair = [
            json!({"role": "model", "parts": [{"functionCall": {"id": "call_p", "name": "compress", "args": {}}}]}),
            json!({"role": "user", "parts": [{"functionResponse": {"id": "call_p", "name": "tool", "response": {"result": "folded\n[emberloop-splice:call_p]"}}}]}),
        ];
        contents.splice(2..2, pair.clone());
        compressor.compress(&mut contents, range(1, 4)).unwrap();

        // The stored pair differs from the request copy: no `_raw`, the splice marker in its own
        // response field, and the call's name instead of the wire's "tool".
        let skip = vec![
            pair[0].clone(),
            json!({"role": "user", "parts": [{"functionResponse": {"id": "call_p", "name": "compress", "response": {"result": "folded", "splice": "emberloop-splice:call_p"}}}]}),
        ];
        let mut rebuilt = raw_turns();
        assert_eq!(compressor.apply_skipping(&mut rebuilt, &skip), 1);
        assert_eq!(rebuilt.len(), 6);
        assert!(rebuilt[1]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("block b1"));
    }

    #[test]
    fn a_chain_of_folds_collapses_to_the_outermost_summary() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = eight();
        compressor.compress(&mut contents, range(1, 3)).unwrap();
        compressor.compress(&mut contents, range(1, 4)).unwrap();
        compressor.compress(&mut contents, range(1, 5)).unwrap();

        // Each fold wrapped the previous marker, so the chain needs three rounds.
        let mut rebuilt = eight();
        assert_eq!(compressor.apply(&mut rebuilt), 3);
        let marker = rebuilt[1]["parts"][0]["text"].as_str().unwrap();
        assert!(marker.contains("summary of 1..5"), "{marker}");
    }

    #[test]
    fn apply_prefers_the_widest_matching_fold_so_a_nested_block_does_not_shadow_it() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let wide = |n: usize| -> Vec<Value> {
            (0..n)
                .map(|i| json!({"role": "user", "parts": [{"text": format!("turn {i}")}]}))
                .collect()
        };
        let mut first = wide(12);
        compressor
            .compress(
                &mut first,
                CompressCall {
                    start: 1,
                    end: 3,
                    summary: "narrow fold".into(),
                    topic: None,
                },
            )
            .unwrap();
        let mut second = wide(12);
        compressor
            .compress(
                &mut second,
                CompressCall {
                    start: 1,
                    end: 9,
                    summary: "wide fold".into(),
                    topic: None,
                },
            )
            .unwrap();

        let mut rebuilt = wide(12);
        assert_eq!(compressor.apply(&mut rebuilt), 1);
        assert!(rebuilt.iter().any(|turn| turn["parts"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("wide fold"))));
    }

    fn eight() -> Vec<Value> {
        (0..8)
            .map(|n| json!({"role": "user", "parts": [{"text": format!("t{n}")}]}))
            .collect()
    }

    fn range(start: usize, end: usize) -> CompressCall {
        CompressCall {
            start,
            end,
            summary: format!("summary of {start}..{end}"),
            topic: None,
        }
    }

    fn texts(contents: &[Value]) -> Vec<String> {
        contents
            .iter()
            .map(|turn| turn["parts"][0]["text"].as_str().unwrap_or("").to_string())
            .collect()
    }

    #[test]
    fn batch_folds_every_range_against_the_original_numbering() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = eight();

        let result = compressor
            .compress_batch(&mut contents, vec![range(1, 3), range(4, 6)])
            .unwrap();

        // Two folds, each replacing two turns by one: 8 - 2 = 6 turns, in place.
        let got = texts(&contents);
        assert_eq!(got.len(), 6);
        assert_eq!(got[0], "t0");
        assert!(got[1].contains("block b1"), "{got:?}");
        assert_eq!(got[2], "t3");
        assert!(got[3].contains("block b2"), "{got:?}");
        assert_eq!(&got[4..], ["t6", "t7"]);
        assert_eq!(result.lines().count(), 2);
        assert!(result.contains("turns 1–2") && result.contains("block b1"));
        assert!(result.contains("turns 4–5") && result.contains("block b2"));
    }

    #[test]
    fn batch_order_given_does_not_matter() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = eight();
        compressor
            .compress_batch(&mut contents, vec![range(4, 6), range(1, 3)])
            .unwrap();
        let got = texts(&contents);
        assert!(got[1].contains("block b1"), "lowest range is b1: {got:?}");
        assert!(got[3].contains("block b2"), "{got:?}");
    }

    #[test]
    fn batch_with_overlapping_ranges_changes_nothing() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = eight();
        let before = contents.clone();
        let error = compressor
            .compress_batch(&mut contents, vec![range(1, 4), range(3, 6)])
            .unwrap_err();
        assert!(error.contains("overlap"), "{error}");
        assert_eq!(contents, before);
        assert_eq!(compressor.status().blocks, 0);
    }

    #[test]
    fn batch_with_one_bad_range_changes_nothing_and_names_it() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = eight();
        let before = contents.clone();
        let mut bad = range(4, 6);
        bad.summary = "  ".into();
        let error = compressor
            .compress_batch(&mut contents, vec![range(1, 3), bad])
            .unwrap_err();
        assert!(error.contains("range 2"), "{error}");
        assert_eq!(contents, before);
        assert_eq!(compressor.status().blocks, 0);

        let out_of_bounds = compressor
            .compress_batch(&mut contents, vec![range(1, 3), range(6, 9)])
            .unwrap_err();
        assert!(out_of_bounds.contains("range 2"), "{out_of_bounds}");
        assert_eq!(contents, before);
    }

    #[test]
    fn batch_blocks_each_restore_their_own_turns() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = eight();
        let original = contents.clone();
        compressor
            .compress_batch(&mut contents, vec![range(1, 3), range(4, 6)])
            .unwrap();
        for id in ["b1", "b2"] {
            compressor
                .decompress(
                    &mut contents,
                    DecompressCall {
                        block_id: id.into(),
                        to_file: None,
                    },
                )
                .unwrap();
        }
        assert_eq!(contents, original);
    }

    #[test]
    fn a_single_range_batch_keeps_the_single_call_wording() {
        let dir = tempdir().unwrap();
        let mut single = NativeCompressor::new(dir.path()).unwrap();
        let mut a = eight();
        let one = single.compress(&mut a, range(1, 3)).unwrap();
        let dir2 = tempdir().unwrap();
        let mut batch = NativeCompressor::new(dir2.path()).unwrap();
        let mut b = eight();
        let many = batch.compress_batch(&mut b, vec![range(1, 3)]).unwrap();
        assert_eq!(one, many);
        assert_eq!(a, b);
    }

    #[test]
    fn search_finds_a_block_case_insensitively() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        compressor.compress(&mut contents, call()).unwrap();

        let hits = compressor.search_context("PLAN", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].block_id, "b1");
        assert!(compressor.search_context("absent", 5).unwrap().is_empty());
        assert!(compressor.search_context("  ", 5).is_err());
    }

    #[test]
    fn to_file_rejects_a_path_outside_the_spill_dir() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        compressor.compress(&mut contents, call()).unwrap();

        let err = compressor
            .decompress(
                &mut contents,
                DecompressCall {
                    block_id: "b1".into(),
                    to_file: Some("../../evil.json".into()),
                },
            )
            .unwrap_err();
        assert!(err.contains("plain file name"));
        assert!(!dir.path().parent().unwrap().join("evil.json").exists());
    }

    #[test]
    fn compress_rejects_invalid_ranges_and_empty_summaries() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();

        let mut bad_range = call();
        bad_range.end = 99;
        assert!(compressor.compress(&mut contents, bad_range).is_err());

        let mut empty = call();
        empty.summary = "  ".into();
        assert!(compressor.compress(&mut contents, empty).is_err());
        assert_eq!(compressor.status().blocks, 0);
    }

    #[test]
    fn unknown_block_is_an_error() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        let err = compressor
            .decompress(
                &mut contents,
                DecompressCall {
                    block_id: "b9".into(),
                    to_file: None,
                },
            )
            .unwrap_err();
        assert!(err.contains("unknown block"));
    }

    #[test]
    fn fold_report_names_the_turn_that_stopped_a_fold() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = raw_turns();
        compressor.compress(&mut contents, range(1, 4)).unwrap();

        let mut rebuilt = raw_turns();
        rebuilt[2]["parts"][0]["text"] = json!("changed");
        // `rebuilt[2]` is the second turn of the fold: the first one at index 1 is `turn 0`.
        let report = compressor.fold_report(&rebuilt, &[]).expect("a report");
        assert!(report.contains("turn 1 differs"), "{report}");
    }
}
