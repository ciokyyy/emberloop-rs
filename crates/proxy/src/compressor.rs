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

/// Billion-context backend. Phase 1 ships the native implementation; a future
/// proxy-backed compressor implements the same operations.
pub trait Compressor: Send + Sync {
    /// Fold several ranges in one call, checked against `policy` as given for this call. Every
    /// range is numbered against the history as it is now and validated before anything changes.
    /// Either all of them fold, changing the history, the blocks and the spill files, or none
    /// does: a failure removes the spill files it wrote and leaves the history, the blocks and
    /// the next block id as they were.
    fn compress_batch(
        &mut self,
        contents: &mut Vec<Value>,
        calls: Vec<CompressCall>,
        policy: &CompressPolicy,
    ) -> Result<String, String>;
    fn decompress(
        &mut self,
        contents: &mut Vec<Value>,
        call: DecompressCall,
    ) -> Result<String, String>;
    fn search_context(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, String>;
    fn status(&self) -> CompressorStatus;

    /// Why no stored fold matched `contents`: one line for the session log. Backends without
    /// folds say nothing.
    fn fold_report(&self, _contents: &[Value], _skip: &[Value]) -> Option<String> {
        None
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
}

impl NativeCompressor {
    pub fn new(spill_dir: impl Into<PathBuf>) -> Result<Self, String> {
        let mut compressor = Self {
            blocks: Vec::new(),
            next_id: 1,
            spill_dir: spill_dir.into(),
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
            let block: Block = serde_json::from_slice(&bytes)
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
            compressor.next_id = compressor.next_id.max(next);
            compressor.blocks.push(block);
        }
        compressor.blocks.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(compressor)
    }

    /// Writes the block's file and returns its path. A failed rename leaves no temporary file.
    fn spill_originals(&self, block: &Block) -> Result<PathBuf, String> {
        let dir = self.spill_dir.join("blocks");
        std::fs::create_dir_all(&dir).map_err(|err| format!("spill dir: {err}"))?;
        let bytes = serde_json::to_vec(block).map_err(|err| err.to_string())?;
        let temporary = dir.join(format!("{}.tmp", block.id));
        std::fs::write(&temporary, bytes).map_err(|err| format!("spill write: {err}"))?;
        let path = dir.join(format!("{}.json", block.id));
        if let Err(err) = std::fs::rename(&temporary, &path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!("spill rename: {err}"));
        }
        Ok(path)
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

    /// Folds one range of `working` into a new block without touching the compressor's state.
    fn stage_fold(
        &self,
        working: &mut Vec<Value>,
        call: &CompressCall,
        id: &str,
    ) -> (String, Block) {
        let originals = working[call.start..call.end].to_vec();
        let summary_turn = Self::summary_turn(self.marker(call, id));
        working.splice(call.start..call.end, [summary_turn.clone()]);
        let line = format!(
            "Compressed turns {}–{} into block {id}. Originals kept in the session spill dir.",
            call.start,
            call.end.saturating_sub(1)
        );
        let block = Block {
            id: id.to_string(),
            topic: call.topic.clone(),
            summary: call.summary.clone(),
            originals,
            summary_turn,
            restored: false,
        };
        (line, block)
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
    fn compress_batch(
        &mut self,
        contents: &mut Vec<Value>,
        mut calls: Vec<CompressCall>,
        policy: &CompressPolicy,
    ) -> Result<String, String> {
        if calls.is_empty() {
            return Err("ranges must not be empty".into());
        }
        calls.sort_by_key(|call| call.start);
        let several = calls.len() > 1;
        for (index, call) in calls.iter().enumerate() {
            validate_range(call, contents, policy).map_err(|error| {
                if several {
                    format!("range {}: {error}", index + 1)
                } else {
                    error
                }
            })?;
        }
        // In history order the earlier range wins: a range that starts inside the last kept one is
        // not folded, and the report says so, because the model must know the turns are still
        // there.
        let mut kept: Vec<CompressCall> = Vec::with_capacity(calls.len());
        let mut warnings = Vec::new();
        for call in calls {
            match kept.last() {
                Some(last) if call.start < last.end => warnings.push(format!(
                    "Warning: turns {}–{} overlap turns {}–{} and were not folded.",
                    call.start,
                    call.end.saturating_sub(1),
                    last.start,
                    last.end.saturating_sub(1)
                )),
                _ => kept.push(call),
            }
        }

        // Ids follow the position in the history, but ranges are folded from the highest down
        // so folding one never moves the numbers of the ones before it. Everything is staged
        // on a copy first: the history, the blocks and the next id change only once every
        // spill has been written.
        let first_id = self.next_id;
        let count = kept.len();
        let mut working = contents.clone();
        let mut lines = vec![String::new(); count];
        let mut staged = Vec::with_capacity(count);
        for (index, call) in kept.into_iter().enumerate().rev() {
            let id = format!("b{}", first_id + index);
            let (line, block) = self.stage_fold(&mut working, &call, &id);
            lines[index] = line;
            staged.push(block);
        }
        let mut written = Vec::with_capacity(count);
        for block in &staged {
            match self.spill_originals(block) {
                Ok(path) => written.push(path),
                Err(error) => {
                    for path in written {
                        let _ = std::fs::remove_file(path);
                    }
                    return Err(error);
                }
            }
        }
        *contents = working;
        self.next_id = first_id + count;
        self.blocks.extend(staged.into_iter().rev());
        lines.extend(warnings);
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
        let terms = search_terms(query)?;
        let mut ranked: Vec<(usize, usize)> = self
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(index, block)| {
                let haystack = format!(
                    "{}\n{}",
                    block.topic.as_deref().unwrap_or(""),
                    block.summary
                )
                .to_lowercase();
                let score = term_score(&haystack, &terms);
                (score > 0).then_some((score, index))
            })
            .collect();
        // Best score first; equal scores keep history order.
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let hits = ranked
            .into_iter()
            .take(limit.max(1))
            .map(|(_, index)| {
                let block = &self.blocks[index];
                let term = terms
                    .iter()
                    .find(|term| block.summary.to_lowercase().contains(term.as_str()))
                    .map_or("", String::as_str);
                SearchHit {
                    block_id: block.id.clone(),
                    topic: block.topic.clone(),
                    snippet: snippet_for(&block.summary, term),
                }
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
                if let Some(matched) = find_span(contents, &wanted) {
                    fold_matched(
                        contents,
                        &matched,
                        block.summary_turn.clone(),
                        skip,
                        &skip_ids,
                    );
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

/// Index of the latest message the user typed, the one no fold may include.
pub fn latest_user_turn(contents: &[Value]) -> Option<usize> {
    contents.iter().rposition(is_user_message)
}

/// A message the user typed: a user-role turn that is neither a tool result nor the transient
/// nudge. Tool results carry the user role on the Google wire too, so they are told apart by their
/// function response.
fn is_user_message(turn: &Value) -> bool {
    turn.get("role").and_then(Value::as_str) == Some("user")
        && !is_transient(turn)
        && !turn
            .get("parts")
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                parts
                    .iter()
                    .any(|part| part.get("functionResponse").is_some())
            })
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
    if let Some(latest) = latest_user_turn(contents) {
        if (call.start..call.end).contains(&latest) {
            return Err(format!(
                "range {}..{} includes the latest user message",
                call.start, call.end
            ));
        }
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

/// Indices in `contents` of the folded turns, in order, with at most `MAX_SPLICE_GAP` unrelated
/// turns between neighbours. The fast path is the exact contiguous window.
fn find_span(contents: &[Value], originals: &[Value]) -> Option<Vec<usize>> {
    if originals.is_empty() || originals.len() > contents.len() {
        return None;
    }
    if let Some(start) = contents
        .windows(originals.len())
        .position(|turns| turns.iter().zip(originals).all(|(a, b)| turn_eq(a, b)))
    {
        return Some((start..start + originals.len()).collect());
    }
    (0..contents.len())
        .filter(|&start| turn_eq(&contents[start], &originals[0]))
        .find_map(|start| span_from(contents, originals, start))
}

fn span_from(contents: &[Value], originals: &[Value], start: usize) -> Option<Vec<usize>> {
    let mut matched = vec![start];
    let mut at = start;
    for original in &originals[1..] {
        let next = (at + 1..contents.len().min(at + 2 + MAX_SPLICE_GAP))
            .find(|&index| turn_eq(&contents[index], original))?;
        matched.push(next);
        at = next;
    }
    Some(matched)
}

/// Replaces the matched turns with `replacement` where the first one stood. Turns between them
/// that the fold did not hold are kept, except the transient and skipped ones: a steer or a
/// proxy pair the harness will not resend must not be deleted with the fold.
fn fold_matched(
    contents: &mut Vec<Value>,
    matched: &[usize],
    replacement: Value,
    skip: &[Value],
    skip_ids: &[&str],
) {
    let (Some(&first), Some(&last)) = (matched.first(), matched.last()) else {
        return;
    };
    let old = std::mem::take(contents);
    for (index, turn) in old.into_iter().enumerate() {
        if index == first {
            contents.push(replacement.clone());
            continue;
        }
        let between = index > first && index < last;
        let dropped = matched.contains(&index)
            || (between && (is_transient(&turn) || is_skipped(&turn, skip, skip_ids)));
        if !dropped {
            contents.push(turn);
        }
    }
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

/// Lowercased whitespace-separated query terms; an empty query is an error.
fn search_terms(query: &str) -> Result<Vec<String>, String> {
    let terms: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if terms.is_empty() {
        return Err("query must not be empty".into());
    }
    Ok(terms)
}

/// Sum over the terms of their non-overlapping occurrence counts in `haystack` (lowercase).
fn term_score(haystack: &str, terms: &[String]) -> usize {
    terms
        .iter()
        .map(|term| haystack.matches(term.as_str()).count())
        .sum()
}

fn snippet_for(summary: &str, needle: &str) -> String {
    let Some((match_start, match_end)) = find_lowercase(summary, needle) else {
        return summary.chars().take(160).collect();
    };
    let start = match_start.saturating_sub(60);
    let end = (match_end + 100).min(summary.len());
    let start = floor_char_boundary(summary, start);
    let end = ceil_char_boundary(summary, end);
    summary[start..end].trim().to_string()
}

/// Byte range in `text` of the first case-insensitive match of `needle_lower` (already lowercase).
/// An empty needle matches nothing. Lowercasing can change the byte length, so the match is
/// located in the lowercased text and mapped back to the original characters that produced it.
fn find_lowercase(text: &str, needle_lower: &str) -> Option<(usize, usize)> {
    if needle_lower.is_empty() {
        return None;
    }
    let mut lower = String::with_capacity(text.len());
    let mut spans = Vec::with_capacity(text.len());
    for (start, ch) in text.char_indices() {
        let span = (start, start + ch.len_utf8());
        for lowered in ch.to_lowercase() {
            lower.push(lowered);
            spans.extend(std::iter::repeat_n(span, lowered.len_utf8()));
        }
    }
    let at = lower.find(needle_lower)?;
    let last = at + needle_lower.len() - 1;
    Some((spans[at].0, spans[last].1))
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
            // The last user message: `call()` folds turns 1 and 2 and must leave it alone.
            json!({"role": "user", "parts": [{"text": "fourth"}]}),
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
        compressor
            .compress_batch(&mut contents, vec![call()], &CompressPolicy::default())
            .unwrap();
        let folded = contents.clone();
        let saved = std::fs::read(dir.path().join("blocks/b1.json")).unwrap();
        drop(compressor);

        let mut reloaded = NativeCompressor::new(dir.path()).unwrap();
        assert_eq!(reloaded.search_context("x=1", 3).unwrap().len(), 1);
        let mut replay = turns();
        assert_eq!(reloaded.apply_skipping(&mut replay, &[]), 1);
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
        assert_eq!(restarted.apply_skipping(&mut replay, &[]), 0);
        restarted
            .compress_batch(
                &mut contents,
                vec![CompressCall {
                    start: 0,
                    end: 1,
                    summary: "new fold".into(),
                    topic: None,
                }],
                &CompressPolicy::default(),
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
        compressor
            .compress_batch(&mut contents, vec![call()], &CompressPolicy::default())
            .unwrap();
        compressor
            .compress_batch(
                &mut contents,
                vec![CompressCall {
                    start: 0,
                    end: 2,
                    summary: "nested plan".into(),
                    topic: None,
                }],
                &CompressPolicy::default(),
            )
            .unwrap();
        let mut reloaded = NativeCompressor::new(dir.path()).unwrap();
        let mut replay = turns();
        assert_eq!(reloaded.apply_skipping(&mut replay, &[]), 2);
        assert_eq!(replay, contents);
    }

    #[test]
    fn a_block_without_its_summary_turn_fails_to_load_and_is_not_overwritten() {
        let dir = tempdir().unwrap();
        let blocks = dir.path().join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        let legacy = serde_json::to_vec(&json!({
            "blockId": "b7", "summary": "legacy plan", "topic": null, "originals": turns(),
        }))
        .unwrap();
        std::fs::write(blocks.join("b7.json"), &legacy).unwrap();
        assert!(NativeCompressor::new(dir.path()).is_err());
        assert_eq!(std::fs::read(blocks.join("b7.json")).unwrap(), legacy);
    }

    #[test]
    fn a_corrupt_block_file_fails_to_load_and_is_not_overwritten() {
        let dir = tempdir().unwrap();
        let blocks = dir.path().join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
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
        let policy = CompressPolicy {
            preserve_recent_messages: 2,
            ..CompressPolicy::default()
        };
        let mut contents = turns();
        let error = compressor
            .compress_batch(&mut contents, vec![call()], &policy)
            .unwrap_err();
        assert!(error.contains("last 2 preserved turns"), "{error}");
        assert_eq!(contents, turns());
    }

    #[test]
    fn protected_tools_never_fold_and_are_named_in_the_error() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let policy = CompressPolicy {
            protected_tools: vec!["read".into()],
            ..CompressPolicy::default()
        };
        let mut contents = vec![
            json!({"role": "user", "parts": [{"text": "first"}]}),
            json!({"role": "model", "parts": [{"functionCall": {"id": "c1", "name": "read", "args": {}}}]}),
            json!({"role": "user", "parts": [{"functionResponse": {"id": "c1", "name": "read", "response": {"result": "x"}}}]}),
            json!({"role": "model", "parts": [{"text": "fourth"}]}),
        ];
        let error = compressor
            .compress_batch(&mut contents, vec![call()], &policy)
            .unwrap_err();
        assert!(error.contains("protected tool read"), "{error}");
    }

    #[test]
    fn status_reports_compressed_tokens() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        compressor
            .compress_batch(&mut contents, vec![call()], &CompressPolicy::default())
            .unwrap();
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

        compressor
            .compress_batch(&mut contents, vec![call()], &CompressPolicy::default())
            .unwrap();

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
        compressor
            .compress_batch(&mut contents, vec![call()], &CompressPolicy::default())
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

        assert_eq!(contents[1..3], originals);
        assert_eq!(compressor.status().restored_blocks, 1);
    }

    #[test]
    fn parity_apply_refolds_a_conversation_the_harness_rebuilt() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        compressor
            .compress_batch(&mut contents, vec![call()], &CompressPolicy::default())
            .unwrap();

        let mut rebuilt = turns();
        assert_eq!(compressor.apply_skipping(&mut rebuilt, &[]), 1);
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
        compressor
            .compress_batch(&mut contents, vec![call()], &CompressPolicy::default())
            .unwrap();

        // The harness resends the original turns; a steer and a proxy pair the stored fold never
        // held sit between them. They are not transient or skipped, so they must survive.
        let mut rebuilt = turns();
        rebuilt.insert(
            2,
            json!({"role": "user", "parts": [{"text": "steer: spliced"}]}),
        );
        rebuilt.insert(3, json!({"role": "user", "parts": [{"text": "pair"}]}));
        assert_eq!(compressor.apply_skipping(&mut rebuilt, &[]), 1);
        assert_eq!(rebuilt.len(), 5, "{rebuilt:?}");
        assert!(rebuilt[1]["parts"][0]["text"]
            .as_str()
            .is_some_and(|t| t.contains("block b1")));
        assert_eq!(rebuilt[2]["parts"][0]["text"], "steer: spliced");
        assert_eq!(rebuilt[3]["parts"][0]["text"], "pair");
    }

    #[test]
    fn apply_does_not_swallow_a_distant_lookalike() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        compressor
            .compress_batch(&mut contents, vec![call()], &CompressPolicy::default())
            .unwrap();

        // The first folded turn is present, the rest is far away behind unrelated turns.
        let originals = turns();
        let mut rebuilt = vec![originals[1].clone()];
        for n in 0..12 {
            rebuilt.push(json!({"role": "user", "parts": [{"text": format!("other {n}")}]}));
        }
        rebuilt.push(originals[2].clone());
        let before = rebuilt.clone();
        assert_eq!(compressor.apply_skipping(&mut rebuilt, &[]), 0);
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
        compressor
            .compress_batch(&mut contents, vec![range(1, 3)], &CompressPolicy::default())
            .unwrap();
        // The wire round trip gives the inserted marker a `_raw` field; a later fold stores it.
        for turn in &mut contents {
            turn.as_object_mut()
                .unwrap()
                .insert("_raw".into(), json!({"role": "user"}));
        }
        compressor
            .compress_batch(&mut contents, vec![range(1, 4)], &CompressPolicy::default())
            .unwrap();

        // The harness resends only its own turns: neither marker is in them.
        let mut rebuilt = raw_turns();
        assert_eq!(compressor.apply_skipping(&mut rebuilt, &[]), 2);
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
        compressor
            .compress_batch(&mut contents, vec![range(1, 4)], &CompressPolicy::default())
            .unwrap();

        let mut rebuilt = raw_turns();
        assert_eq!(compressor.apply_skipping(&mut rebuilt, &[]), 1);
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
        compressor
            .compress_batch(&mut contents, vec![range(1, 4)], &CompressPolicy::default())
            .unwrap();

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
        // Two turns after the eight keep the latest user message out of the chain's ranges.
        let with_prompt = || {
            let mut turns = eight();
            turns.push(json!({"role": "model", "parts": [{"text": "answer"}]}));
            turns.push(json!({"role": "user", "parts": [{"text": "prompt"}]}));
            turns
        };
        let mut contents = with_prompt();
        compressor
            .compress_batch(&mut contents, vec![range(1, 3)], &CompressPolicy::default())
            .unwrap();
        compressor
            .compress_batch(&mut contents, vec![range(1, 4)], &CompressPolicy::default())
            .unwrap();
        compressor
            .compress_batch(&mut contents, vec![range(1, 5)], &CompressPolicy::default())
            .unwrap();

        // Each fold wrapped the previous marker, so the chain needs three rounds.
        let mut rebuilt = with_prompt();
        assert_eq!(compressor.apply_skipping(&mut rebuilt, &[]), 3);
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
            .compress_batch(
                &mut first,
                vec![CompressCall {
                    start: 1,
                    end: 3,
                    summary: "narrow fold".into(),
                    topic: None,
                }],
                &CompressPolicy::default(),
            )
            .unwrap();
        let mut second = wide(12);
        compressor
            .compress_batch(
                &mut second,
                vec![CompressCall {
                    start: 1,
                    end: 9,
                    summary: "wide fold".into(),
                    topic: None,
                }],
                &CompressPolicy::default(),
            )
            .unwrap();

        let mut rebuilt = wide(12);
        assert_eq!(compressor.apply_skipping(&mut rebuilt, &[]), 1);
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
            .compress_batch(
                &mut contents,
                vec![range(1, 3), range(4, 6)],
                &CompressPolicy::default(),
            )
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
            .compress_batch(
                &mut contents,
                vec![range(4, 6), range(1, 3)],
                &CompressPolicy::default(),
            )
            .unwrap();
        let got = texts(&contents);
        assert!(got[1].contains("block b1"), "lowest range is b1: {got:?}");
        assert!(got[3].contains("block b2"), "{got:?}");
    }

    #[test]
    fn batch_with_overlapping_ranges_folds_the_earlier_and_skips_the_later() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = eight();
        let report = compressor
            .compress_batch(
                &mut contents,
                vec![range(1, 4), range(3, 6)],
                &CompressPolicy::default(),
            )
            .unwrap();
        assert_eq!(compressor.status().blocks, 1);
        assert_eq!(contents.len(), 6);
        assert!(
            report.contains("Warning:") && report.contains("not folded"),
            "{report}"
        );
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
            .compress_batch(
                &mut contents,
                vec![range(1, 3), bad],
                &CompressPolicy::default(),
            )
            .unwrap_err();
        assert!(error.contains("range 2"), "{error}");
        assert_eq!(contents, before);
        assert_eq!(compressor.status().blocks, 0);

        let out_of_bounds = compressor
            .compress_batch(
                &mut contents,
                vec![range(1, 3), range(6, 9)],
                &CompressPolicy::default(),
            )
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
            .compress_batch(
                &mut contents,
                vec![range(1, 3), range(4, 6)],
                &CompressPolicy::default(),
            )
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
    fn search_finds_a_block_case_insensitively() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        compressor
            .compress_batch(&mut contents, vec![call()], &CompressPolicy::default())
            .unwrap();

        let hits = compressor.search_context("PLAN", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].block_id, "b1");
        assert!(compressor.search_context("absent", 5).unwrap().is_empty());
        assert!(compressor.search_context("  ", 5).is_err());
    }

    #[test]
    fn snippet_stays_on_the_match_when_lowercasing_changes_the_byte_length() {
        // 'İ' lowercases to two code points (3 bytes against 2), 'ẞ' shrinks (2 bytes against 3):
        // the match offset must come from the text the snippet is cut from.
        for prefix in ["İ".repeat(100), "ẞ".repeat(100)] {
            let dir = tempdir().unwrap();
            let mut compressor = NativeCompressor::new(dir.path()).unwrap();
            let mut contents = turns();
            let call = CompressCall {
                summary: format!("{prefix} target {}", "x".repeat(300)),
                ..call()
            };
            compressor
                .compress_batch(&mut contents, vec![call], &CompressPolicy::default())
                .unwrap();

            let hits = compressor.search_context("target", 5).unwrap();
            assert_eq!(hits.len(), 1);
            assert!(hits[0].snippet.contains("target"), "{}", hits[0].snippet);
        }
    }

    fn six_turns() -> Vec<Value> {
        (0..6)
            .map(|i| {
                let role = if i == 5 { "user" } else { "model" };
                json!({"role": role, "parts": [{"text": format!("turn {i}")}]})
            })
            .collect()
    }

    #[test]
    fn search_ranks_the_block_matching_more_terms_first_without_needing_the_phrase() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = six_turns();
        let calls = vec![
            CompressCall {
                start: 1,
                end: 2,
                summary: "alpha only".into(),
                topic: None,
            },
            CompressCall {
                start: 3,
                end: 5,
                summary: "beta and alpha".into(),
                topic: None,
            },
        ];
        compressor
            .compress_batch(&mut contents, calls, &CompressPolicy::default())
            .unwrap();

        // "alpha beta" is not a phrase in either summary: both match, the one with both terms first.
        let hits = compressor.search_context("alpha beta", 5).unwrap();
        let ids: Vec<&str> = hits.iter().map(|hit| hit.block_id.as_str()).collect();
        assert_eq!(ids, ["b2", "b1"]);

        // The limit applies after ranking, so the best block survives a limit of one.
        let top = compressor.search_context("alpha beta", 1).unwrap();
        assert_eq!(top[0].block_id, "b2");
        assert!(compressor.search_context("  ", 5).is_err());
    }

    #[test]
    fn to_file_rejects_a_path_outside_the_spill_dir() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        compressor
            .compress_batch(&mut contents, vec![call()], &CompressPolicy::default())
            .unwrap();

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
        assert!(compressor
            .compress_batch(&mut contents, vec![bad_range], &CompressPolicy::default())
            .is_err());

        let mut empty = call();
        empty.summary = "  ".into();
        assert!(compressor
            .compress_batch(&mut contents, vec![empty], &CompressPolicy::default())
            .is_err());
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
        compressor
            .compress_batch(&mut contents, vec![range(1, 4)], &CompressPolicy::default())
            .unwrap();

        let mut rebuilt = raw_turns();
        rebuilt[2]["parts"][0]["text"] = json!("changed");
        // `rebuilt[2]` is the second turn of the fold: the first one at index 1 is `turn 0`.
        let report = compressor.fold_report(&rebuilt, &[]).expect("a report");
        assert!(report.contains("turn 1 differs"), "{report}");
    }
}
