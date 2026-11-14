use crate::marker;
use crate::protect::is_protected;
use crate::tokens;
use crate::turnkeys::{self, is_transient};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// Snippet budget in characters, and how many of them come before the match.
const SNIPPET_TOTAL: usize = 200;
const SNIPPET_AHEAD: usize = 80;
/// Directory under a Session's spill dir holding one `<block id>.json` per fold.
const BLOCKS_DIR: &str = "blocks";

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
    /// Index of the first folded turn when the fold was made. A refold prefers the span that
    /// starts nearest here, so identical turns elsewhere in the history are not folded instead.
    #[serde(default)]
    start: usize,
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
    /// The hit is in the block's original turns, not in its summary or topic.
    pub from_original: bool,
    /// The block was restored, so its turns are back in the history.
    pub inactive: bool,
    /// The number of turns the block folded.
    pub turns: usize,
}

/// A search result: the hits in the order shown, with what `render` needs for the model text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchReport {
    pub query: String,
    pub total_blocks: usize,
    /// Summary matches before the limit was applied.
    pub summary_candidates: usize,
    pub hits: Vec<SearchHit>,
    /// A hint naming a query term the shown blocks lack but lower-ranked blocks have.
    pub narrowing: Option<String>,
}

impl SearchReport {
    /// The model-visible text of the result.
    pub fn render(&self) -> String {
        if self.total_blocks == 0 {
            return "No compressed blocks exist yet — nothing to search.".into();
        }
        if self.hits.is_empty() {
            return format!(
                "No blocks matched \"{}\" among {} block(s). Scope = compressed topics and \
                 summaries plus the original text of compressed blocks, NOT visible messages. \
                 Absence here does not mean the topic was never discussed.",
                self.query, self.total_blocks
            );
        }
        let originals: Vec<&SearchHit> = self.hits.iter().filter(|hit| hit.from_original).collect();
        let summaries: Vec<&SearchHit> =
            self.hits.iter().filter(|hit| !hit.from_original).collect();
        let mut out = format!("Found {} block(s) for \"{}\":", summaries.len(), self.query);
        if !summaries.is_empty() {
            out.push_str("\n\n");
            out.push_str(&render_entries(&summaries));
        }
        if self.summary_candidates > summaries.len() {
            out.push_str(&format!(
                "\n\n[truncated: showing {} of {} candidates by relevance — pass limit:{} to see the rest]",
                summaries.len(),
                self.summary_candidates,
                self.summary_candidates
            ));
        }
        if let Some(tip) = &self.narrowing {
            out.push_str("\n\n");
            out.push_str(tip);
        }
        if !originals.is_empty() {
            out.push_str("\n\nOriginal text, not in any summary:\n\n");
            out.push_str(&render_entries(&originals));
        }
        out
    }
}

fn render_entries(hits: &[&SearchHit]) -> String {
    hits.iter()
        .map(|hit| {
            let topic = hit.topic.as_deref().unwrap_or("(no topic)");
            let inactive = if hit.inactive {
                " [inactive — restored to full text; its turns are back in the history]"
            } else {
                ""
            };
            format!(
                "{} \"{topic}\" [{} turns]{inactive}\n  {}",
                hit.block_id, hit.turns, hit.snippet
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
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
    fn search_context(&self, query: &str, limit: usize) -> Result<SearchReport, String>;
    fn status(&self) -> CompressorStatus;

    /// Call ids covered by active folds; their proxy exchanges must not be replayed.
    fn folded_call_ids(&self) -> HashSet<String> {
        HashSet::new()
    }

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
        let entries = match std::fs::read_dir(compressor.spill_dir.join(BLOCKS_DIR)) {
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

    /// Moves the spill dir's blocks directory aside as `blocks-reset-N` (N the first free number),
    /// so a new compressor starts empty. The originals stay on disk. A missing directory moves
    /// nothing.
    pub fn archive_blocks(spill_dir: &Path) -> std::io::Result<()> {
        let blocks = spill_dir.join(BLOCKS_DIR);
        if !blocks.exists() {
            return Ok(());
        }
        let mut generation = 1;
        while spill_dir
            .join(format!("blocks-reset-{generation}"))
            .exists()
        {
            generation += 1;
        }
        std::fs::rename(blocks, spill_dir.join(format!("blocks-reset-{generation}")))
    }

    /// Writes the block's file and returns its path. A failed rename leaves no temporary file.
    fn spill_originals(&self, block: &Block) -> Result<PathBuf, String> {
        let dir = self.spill_dir.join(BLOCKS_DIR);
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

    fn summary_turn(text: String) -> Value {
        json!({"role": "user", "parts": [{"text": text}]})
    }

    /// Folds one range of `working` into a new block without touching the compressor's state.
    fn stage_fold(
        &self,
        working: &mut Vec<Value>,
        call: &CompressCall,
        id: &str,
    ) -> (String, Block) {
        let originals = working[call.start..call.end].to_vec();
        let summary_turn = Self::summary_turn(marker::summary(
            call.start,
            call.end.saturating_sub(1),
            id,
            call.topic.as_deref(),
            &call.summary,
        ));
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
            start: call.start,
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
        if position.is_none() && find_span(contents, &block.originals, block.start).is_none() {
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

    fn search_context(&self, query: &str, limit: usize) -> Result<SearchReport, String> {
        let terms = search_terms(query)?;
        let limit = limit.max(1);
        let summaries: Vec<String> = self
            .blocks
            .iter()
            .map(|block| {
                format!(
                    "{}\n{}",
                    block.topic.as_deref().unwrap_or(""),
                    block.summary
                )
            })
            .collect();
        let summary_refs: Vec<&str> = summaries.iter().map(String::as_str).collect();
        let summary_ranked = rank_by_terms(summary_refs.iter().copied(), &terms);
        let summary_freq = doc_frequency(&summary_refs, &terms);
        let shown: Vec<usize> = summary_ranked
            .iter()
            .take(limit)
            .map(|&(_, index)| index)
            .collect();
        let shown_texts: Vec<&str> = shown.iter().map(|&index| summary_refs[index]).collect();
        let lower_texts: Vec<&str> = summary_ranked
            .iter()
            .skip(limit)
            .map(|&(_, index)| summary_refs[index])
            .collect();
        let narrowing = narrowing_hint(&terms, &shown_texts, &lower_texts);
        let mut hits: Vec<SearchHit> = shown
            .iter()
            .map(|&index| {
                let block = &self.blocks[index];
                let anchor = anchor_term(&block.summary, &terms, &summary_freq);
                search_hit(
                    block,
                    snippet_around(&block.summary, anchor.as_deref()),
                    false,
                )
            })
            .collect();
        // Original text only adds blocks whose summary was not shown above.
        let candidates: Vec<(usize, String)> = self
            .blocks
            .iter()
            .enumerate()
            .filter(|(index, _)| !shown.contains(index))
            .map(|(index, block)| (index, original_text(block)))
            .collect();
        let texts: Vec<&str> = candidates.iter().map(|(_, text)| text.as_str()).collect();
        let original_freq = doc_frequency(&texts, &terms);
        let original_ranked = rank_by_terms(texts.iter().copied(), &terms);
        hits.extend(original_ranked.iter().take(limit).map(|&(_, pos)| {
            let (index, text) = &candidates[pos];
            let anchor = anchor_term(text, &terms, &original_freq);
            search_hit(
                &self.blocks[*index],
                snippet_around(text, anchor.as_deref()),
                true,
            )
        }));
        Ok(SearchReport {
            query: query.to_string(),
            total_blocks: self.blocks.len(),
            summary_candidates: summary_ranked.len(),
            hits,
            narrowing,
        })
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
                .map(|block| tokens::turns_tokens(&block.originals))
                .sum(),
            spill_dir: self.spill_dir.to_string_lossy().to_string(),
        }
    }

    fn folded_call_ids(&self) -> HashSet<String> {
        self.blocks
            .iter()
            .filter(|block| !block.restored)
            .flat_map(|block| turn_ids(&block.originals))
            .map(str::to_owned)
            .collect()
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
        // (first index, turns removed) of every fold applied in this pass, so a later block's
        // stored start can be moved to where its turns are now.
        let mut shifts: Vec<(usize, usize)> = Vec::new();
        loop {
            let mut moved = false;
            for block in &blocks {
                // A fold already in the history is never folded again: its summary turn is the
                // proof, and it keeps a stored fold from matching its own result.
                if contents
                    .iter()
                    .any(|turn| turnkeys::same_turn(turn, &block.summary_turn))
                {
                    continue;
                }
                let wanted = wanted_turns(block, skip, &skip_ids);
                let near = shifted_start(block.start, &shifts);
                if let Some(matched) = find_span(contents, &wanted, near) {
                    let before = contents.len();
                    fold_matched(
                        contents,
                        &matched,
                        block.summary_turn.clone(),
                        skip,
                        &skip_ids,
                    );
                    shifts.push((matched[0], before - contents.len()));
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
        let live = || {
            self.blocks
                .iter()
                .filter(|block| !block.restored && !block.originals.is_empty())
        };
        // The newest block is the one the agent just asked for. A fold of the same start or later
        // in the request means it, or a wider twin, applied; otherwise report it even when older
        // blocks still apply, because that miss silently undoes the agent's last compress.
        let block = live()
            .max_by_key(|block| block.id.trim_start_matches('b').parse::<u64>().unwrap_or(0))?;
        let applied = |other: &Block| {
            contents
                .iter()
                .any(|turn| turnkeys::same_turn(turn, &other.summary_turn))
        };
        if live().any(|other| other.start >= block.start && applied(other)) {
            return None;
        }
        let wanted = wanted_turns(block, skip, &skip_ids);
        let head = wanted.first()?;
        let Some(start) = contents
            .iter()
            .position(|turn| turnkeys::same_turn(turn, head))
        else {
            return Some(format!(
                "{}: first stored turn `{}` is not in the request",
                block.id,
                brief(head)
            ));
        };
        for (offset, turn) in wanted.iter().enumerate().skip(1) {
            let found = contents.get(start + offset);
            if found.is_none_or(|found| !turnkeys::same_turn(turn, found)) {
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

/// Where a fold's stored start is now, given the folds applied before it in the same pass: each
/// one that began before it removed its turns from the history.
fn shifted_start(start: usize, shifts: &[(usize, usize)]) -> usize {
    let removed: usize = shifts
        .iter()
        .filter(|(first, _)| *first < start)
        .map(|(_, count)| count)
        .sum();
    start.saturating_sub(removed)
}

/// The stored turns a fold must find in the request: its originals without the transient nudge
/// and without the turns the splice store puts back.
fn wanted_turns(block: &Block, skip: &[Value], skip_ids: &[&str]) -> Vec<Value> {
    block
        .originals
        .iter()
        .filter(|turn| !is_transient(turn) && !is_skipped(turn, skip, skip_ids))
        .cloned()
        .collect()
}

/// One short line for a stored or requested turn, used only by `fold_report`.
fn brief(turn: &Value) -> String {
    let text = turn
        .get("parts")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .map(|part| match part_call(part) {
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
                })
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .unwrap_or_default();
    text.chars().take(80).collect()
}

/// The maximal runs of foldable turns in `1..end`, oldest first. A protected turn ends a run, and
/// turn 0 is never a start, because ranges begin at 1. The nudge and the preflight backstop count
/// and fold these runs, never a protected turn.
pub fn foldable_runs(contents: &[Value], end: usize) -> Vec<Range<usize>> {
    let end = end.min(contents.len());
    let mut runs = Vec::new();
    let mut start = None;
    for (index, turn) in contents.iter().enumerate().take(end).skip(1) {
        match (is_protected(turn) || is_summary_turn(turn), start) {
            (false, None) => start = Some(index),
            (true, Some(first)) => {
                runs.push(first..index);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(first) = start {
        runs.push(first..end);
    }
    runs.into_iter()
        .flat_map(|run| refine_pair_safe(contents, run))
        .collect()
}

fn part_ids<'a>(turn: &'a Value, key: &str) -> Vec<&'a str> {
    turn.get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| part.get(key)?.get("id")?.as_str())
        .filter(|id| !id.is_empty())
        .collect()
}

/// Shrink or split a candidate run so no sub-run strands half of a tool-call/result pair
/// across an edge (`validate_range` rejects `split_pair`).
fn refine_pair_safe(contents: &[Value], run: Range<usize>) -> Vec<Range<usize>> {
    let mut sub_runs = vec![run];
    loop {
        let mut next = Vec::with_capacity(sub_runs.len());
        let mut changed = false;
        for r in sub_runs {
            let outer_calls: HashSet<&str> = contents[..r.start]
                .iter()
                .chain(&contents[r.end..])
                .flat_map(|turn| part_ids(turn, "functionCall"))
                .collect();
            let outer_results: HashSet<&str> = contents[..r.start]
                .iter()
                .chain(&contents[r.end..])
                .flat_map(|turn| part_ids(turn, "functionResponse"))
                .collect();
            let inside_calls: HashSet<&str> = contents[r.start..r.end]
                .iter()
                .flat_map(|turn| part_ids(turn, "functionCall"))
                .collect();
            let inside_results: HashSet<&str> = contents[r.start..r.end]
                .iter()
                .flat_map(|turn| part_ids(turn, "functionResponse"))
                .collect();

            let mut cur_start: Option<usize> = None;
            for (offset, turn) in contents[r.clone()].iter().enumerate() {
                let idx = r.start + offset;
                let straddles = part_ids(turn, "functionCall")
                    .into_iter()
                    .any(|id| !inside_results.contains(id) && outer_results.contains(id))
                    || part_ids(turn, "functionResponse")
                        .into_iter()
                        .any(|id| !inside_calls.contains(id) && outer_calls.contains(id));
                match (straddles, cur_start) {
                    (false, None) => cur_start = Some(idx),
                    (true, Some(first)) => {
                        next.push(first..idx);
                        cur_start = None;
                        changed = true;
                    }
                    (true, None) => changed = true,
                    (false, Some(_)) => {}
                }
            }
            if let Some(first) = cur_start {
                next.push(first..r.end);
            }
        }
        sub_runs = next;
        if !changed {
            return sub_runs;
        }
    }
}

/// Index of the latest message the user typed, the one no fold may include.
pub fn latest_user_turn(contents: &[Value]) -> Option<usize> {
    contents.iter().rposition(is_user_message)
}

/// A message the user typed: a user-role turn that is neither a tool result, the transient
/// nudge, nor the summary a fold left (the proxy's own text; forged markers are stripped from
/// real input). Tool results carry the user role on the Google wire too, so they are told apart
/// by their function response.
fn is_user_message(turn: &Value) -> bool {
    turn.get("role").and_then(Value::as_str) == Some("user")
        && !is_transient(turn)
        && !is_summary_turn(turn)
        && !turn
            .get("parts")
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                parts
                    .iter()
                    .any(|part| part.get("functionResponse").is_some())
            })
}

pub fn is_summary_turn(turn: &Value) -> bool {
    turn.pointer("/parts/0/text")
        .and_then(Value::as_str)
        .is_some_and(marker::is_summary)
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
    if contents[call.start..call.end].iter().any(is_protected) {
        return Err(format!(
            "range {}..{} includes protected system content (a system prompt or <system-reminder>), which never folds",
            call.start, call.end
        ));
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
    if let Some(id) = split_pair(call, contents) {
        return Err(format!(
            "range {}..{} splits tool call {id} from its result; move an edge so both stay on one side",
            call.start, call.end
        ));
    }
    Ok(())
}

/// An id whose call and result are on opposite sides of the range's edge: folding it would leave
/// the other half orphaned, which upstreams reject.
fn split_pair<'a>(call: &CompressCall, contents: &'a [Value]) -> Option<&'a str> {
    let ids = |turns: &'a [Value], key: &str| -> Vec<&'a str> {
        turns
            .iter()
            .filter_map(|turn| turn.get("parts").and_then(Value::as_array))
            .flatten()
            .filter_map(|part| part.get(key)?.get("id")?.as_str())
            .filter(|id| !id.is_empty())
            .collect()
    };
    let (inside, before, after) = (
        &contents[call.start..call.end],
        &contents[..call.start],
        &contents[call.end..],
    );
    let outside = |key: &str| [ids(before, key), ids(after, key)].concat();
    let (calls, results) = (ids(inside, "functionCall"), ids(inside, "functionResponse"));
    let (outer_calls, outer_results) = (outside("functionCall"), outside("functionResponse"));
    calls
        .iter()
        .find(|id| !results.contains(id) && outer_results.contains(id))
        .or_else(|| {
            results
                .iter()
                .find(|id| !calls.contains(id) && outer_calls.contains(id))
        })
        .copied()
}

/// Names of every tool call and result in a turn, whatever the wire's shape.
fn turn_tool_names(turn: &Value) -> impl Iterator<Item = &str> {
    turn.get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| part_call(part)?.get("name").and_then(Value::as_str))
}

/// Most turns a stored steer or a proxy pair may add between two folded turns. They are put back
/// into the history before a block is looked for, so the harness's own turns are no longer
/// contiguous; a bigger gap is a different conversation, not a splice.
const MAX_SPLICE_GAP: usize = 4;

/// Indices in `contents` of the folded turns, in order, with at most `MAX_SPLICE_GAP` unrelated
/// turns between neighbours. Contiguous windows come first, then gapped ones. Among the
/// candidates of a kind, the one whose first index is nearest `near` (the fold's recorded start)
/// wins, the earlier one on a tie, so repeated turns resolve to the position the fold was made at.
fn find_span(contents: &[Value], originals: &[Value], near: usize) -> Option<Vec<usize>> {
    if originals.is_empty() || originals.len() > contents.len() {
        return None;
    }
    let contiguous: Vec<Vec<usize>> = (0..=contents.len() - originals.len())
        .filter(|&start| {
            contents[start..start + originals.len()]
                .iter()
                .zip(originals)
                .all(|(a, b)| turnkeys::same_turn(a, b))
        })
        .map(|start| (start..start + originals.len()).collect())
        .collect();
    if let Some(span) = nearest_span(contiguous, near) {
        return Some(span);
    }
    // A gapped span must not cover protected content: folding across it would move it.
    let gapped: Vec<Vec<usize>> = (0..contents.len())
        .filter(|&start| turnkeys::same_turn(&contents[start], &originals[0]))
        .filter_map(|start| span_from(contents, originals, start))
        .filter(|span| !covers_protected(contents, span))
        .collect();
    nearest_span(gapped, near)
}

/// Whether a span's turns, or any turn between its first and last, is protected content.
fn covers_protected(contents: &[Value], span: &[usize]) -> bool {
    let (Some(&first), Some(&last)) = (span.first(), span.last()) else {
        return false;
    };
    contents[first..=last].iter().any(is_protected)
}

fn nearest_span(spans: Vec<Vec<usize>>, near: usize) -> Option<Vec<usize>> {
    spans
        .into_iter()
        .min_by_key(|span| (span[0].abs_diff(near), span[0]))
}

fn span_from(contents: &[Value], originals: &[Value], start: usize) -> Option<Vec<usize>> {
    let mut matched = vec![start];
    let mut at = start;
    for original in &originals[1..] {
        let next = (at + 1..contents.len().min(at + 2 + MAX_SPLICE_GAP))
            .find(|&index| turnkeys::same_turn(&contents[index], original))?;
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

/// The function call or the function result a part carries, whichever it is.
fn part_call(part: &Value) -> Option<&Value> {
    part.get("functionCall")
        .or_else(|| part.get("functionResponse"))
}

/// Every function-call id the stored turns carry. A splice pair changes shape on the wire round
/// trip (`_raw`, the tool name, the marker's field), so the id is what identifies it.
fn turn_ids(turns: &[Value]) -> Vec<&str> {
    turns
        .iter()
        .filter_map(|turn| turn.get("parts").and_then(Value::as_array))
        .flatten()
        .filter_map(part_call)
        .filter_map(|call| call.get("id").and_then(Value::as_str))
        .collect()
}

fn is_skipped(turn: &Value, skip: &[Value], skip_ids: &[&str]) -> bool {
    let id_matches = turn
        .get("parts")
        .and_then(Value::as_array)
        .is_some_and(|parts| {
            parts.iter().any(|part| {
                part_call(part)
                    .and_then(|call| call.get("id"))
                    .and_then(Value::as_str)
                    .is_some_and(|id| skip_ids.contains(&id))
            })
        });
    id_matches
        || skip
            .iter()
            .any(|spliced| turnkeys::same_turn(turn, spliced))
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

/// `(score, position)` of every text with a positive term score, best first; equal scores keep
/// their position order.
fn rank_by_terms<'a>(
    texts: impl Iterator<Item = &'a str>,
    terms: &[String],
) -> Vec<(usize, usize)> {
    let mut ranked: Vec<(usize, usize)> = texts
        .enumerate()
        .filter_map(|(position, text)| {
            let score = term_score(&text.to_lowercase(), terms);
            (score > 0).then_some((score, position))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    ranked
}

/// The text a block's folded turns carried: message text, call names and arguments, and the
/// strings of tool results. The fold's own marker lines are not content.
fn original_text(block: &Block) -> String {
    let mut pieces = Vec::new();
    for turn in &block.originals {
        for part in turn
            .get("parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                pieces.push(text.to_string());
            } else if let Some(call) = part.get("functionCall") {
                pieces.push(format!(
                    "{} {}",
                    call.get("name").and_then(Value::as_str).unwrap_or(""),
                    call.get("args").unwrap_or(&Value::Null)
                ));
            } else if let Some(result) = part.get("functionResponse") {
                json_strings(result.get("response").unwrap_or(&Value::Null), &mut pieces);
            }
        }
    }
    pieces
        .join("\n")
        .lines()
        .filter(|line| !marker::is_tool_text(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn json_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) => out.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| json_strings(item, out)),
        Value::Object(map) => map.values().for_each(|item| json_strings(item, out)),
        _ => {}
    }
}

/// Sum over the terms of their non-overlapping occurrence counts in `haystack` (lowercase).
fn term_score(haystack: &str, terms: &[String]) -> usize {
    terms
        .iter()
        .map(|term| haystack.matches(term.as_str()).count())
        .sum()
}

fn search_hit(block: &Block, snippet: String, from_original: bool) -> SearchHit {
    SearchHit {
        block_id: block.id.clone(),
        topic: block.topic.clone(),
        snippet,
        from_original,
        inactive: block.restored,
        turns: block.originals.len(),
    }
}

/// For each term, the number of texts that contain it.
fn doc_frequency(texts: &[&str], terms: &[String]) -> Vec<usize> {
    let lowered: Vec<String> = texts.iter().map(|text| text.to_lowercase()).collect();
    terms
        .iter()
        .map(|term| {
            lowered
                .iter()
                .filter(|text| text.contains(term.as_str()))
                .count()
        })
        .collect()
}

/// The term that occurs in `text` and is rarest across the searched texts; ties go to the earlier
/// occurrence in `text`.
fn anchor_term(text: &str, terms: &[String], freq: &[usize]) -> Option<String> {
    terms
        .iter()
        .zip(freq)
        .filter_map(|(term, &frequency)| {
            find_lowercase(text, term).map(|(at, _)| (frequency, at, term))
        })
        .min_by_key(|&(frequency, at, _)| (frequency, at))
        .map(|(_, _, term)| term.clone())
}

/// A window of `SNIPPET_TOTAL` characters with the anchor term in 【】, starting `SNIPPET_AHEAD`
/// characters before it. Without an anchor, the summary's head.
fn snippet_around(text: &str, anchor: Option<&str>) -> String {
    let Some((match_start, match_end)) = anchor.and_then(|term| find_lowercase(text, term)) else {
        let head: String = text.chars().take(SNIPPET_TOTAL).collect();
        let more = if text.chars().count() > SNIPPET_TOTAL {
            "..."
        } else {
            ""
        };
        return collapse_whitespace(&format!("{head}{more}"));
    };
    let total = text.chars().count();
    let match_char = text[..match_start].chars().count();
    let start_char = match_char.saturating_sub(SNIPPET_AHEAD);
    let end_char = (start_char + SNIPPET_TOTAL).min(total);
    let start = byte_of_char(text, start_char);
    let end = byte_of_char(text, end_char).max(match_end);
    let mut out = String::new();
    if start_char > 0 {
        out.push_str("... ");
    }
    out.push_str(&text[start..match_start]);
    out.push('【');
    out.push_str(&text[match_start..match_end]);
    out.push('】');
    out.push_str(&text[match_end..end]);
    if end_char < total {
        out.push_str(" ...");
    }
    collapse_whitespace(&out)
}

fn byte_of_char(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(at, _)| at)
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// When the query has three or more terms, a term no shown block contains but a lower-ranked
/// block does: the model is told to search that term alone.
fn narrowing_hint(terms: &[String], shown: &[&str], lower: &[&str]) -> Option<String> {
    if terms.len() < 3 || lower.is_empty() {
        return None;
    }
    let mut best: Option<(&String, usize)> = None;
    for term in terms {
        if shown
            .iter()
            .any(|text| text.to_lowercase().contains(term.as_str()))
        {
            continue;
        }
        let count = lower
            .iter()
            .filter(|text| text.to_lowercase().contains(term.as_str()))
            .count();
        if count == 0 {
            continue;
        }
        let better = match best {
            None => true,
            Some((best_term, best_count)) => {
                count < best_count || (count == best_count && term.len() > best_term.len())
            }
        };
        if better {
            best = Some((term, count));
        }
    }
    best.map(|(term, count)| {
        format!(
            "[tip: none of the top {} results contain \"{term}\" but {count} lower-ranked candidate(s) do — try searching \"{term}\" alone to surface its block directly]",
            shown.len()
        )
    })
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
        assert_eq!(reloaded.search_context("x=1", 3).unwrap().hits.len(), 1);
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
    fn a_range_that_splits_a_call_from_its_result_is_rejected() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let contents = vec![
            json!({"role": "user", "parts": [{"text": "first"}]}),
            json!({"role": "model", "parts": [{"text": "second"}]}),
            json!({"role": "model", "parts": [{"functionCall": {"id": "c1", "name": "acp_status", "args": {}}}]}),
            json!({"role": "user", "parts": [{"functionResponse": {"id": "c1", "name": "acp_status", "response": {"result": "x"}}}]}),
            json!({"role": "model", "parts": [{"text": "fifth"}]}),
        ];
        for (start, end) in [(1, 3), (3, 4)] {
            let mut turns = contents.clone();
            let call = CompressCall {
                start,
                end,
                summary: "folded".into(),
                topic: None,
            };
            let error = compressor
                .compress_batch(&mut turns, vec![call], &CompressPolicy::default())
                .unwrap_err();
            assert!(
                error.contains("splits tool call c1"),
                "{start}..{end}: {error}"
            );
            assert_eq!(turns, contents);
        }
        let mut whole = contents.clone();
        let call = CompressCall {
            start: 2,
            end: 4,
            summary: "folded".into(),
            topic: None,
        };
        compressor
            .compress_batch(&mut whole, vec![call], &CompressPolicy::default())
            .unwrap();
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
    fn a_range_covering_protected_content_is_refused_and_named() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = vec![
            json!({"role": "user", "parts": [{"text": "<system-reminder>CLAUDE.md</system-reminder>\n\nfix"}]}),
            json!({"role": "model", "parts": [{"text": "second"}]}),
            json!({"role": "user", "parts": [{"text": "third"}]}),
            json!({"role": "user", "parts": [{"text": "fourth"}]}),
        ];
        let before = contents.clone();
        let call = CompressCall {
            start: 0,
            end: 2,
            summary: "x".into(),
            topic: None,
        };
        let error = compressor
            .compress_batch(&mut contents, vec![call], &CompressPolicy::default())
            .unwrap_err();
        assert!(error.contains("protected"), "{error}");
        assert_eq!(contents, before);
    }

    #[test]
    fn a_stored_fold_never_matches_across_protected_content() {
        let (_dir, mut compressor, _) = one_block();
        // The harness resends the folded turns with a reminder between them. Folding across it
        // would move that reminder, so the stored fold must not apply.
        let mut resent = vec![
            json!({"role": "user", "parts": [{"text": "first"}]}),
            json!({"role": "model", "parts": [{"text": "second"}]}),
            json!({"role": "user", "parts": [{"text": "<system-reminder>r</system-reminder>"}]}),
            json!({"role": "user", "parts": [{"text": "third"}]}),
            json!({"role": "user", "parts": [{"text": "fourth"}]}),
        ];
        let before = resent.clone();
        assert_eq!(compressor.apply_skipping(&mut resent, &[]), 0);
        assert_eq!(resent, before);
    }

    #[test]
    fn foldable_runs_split_at_protected_turns_and_start_at_turn_one() {
        let contents = vec![
            json!({"role": "user", "parts": [{"text": "first"}]}),
            json!({"role": "user", "parts": [{"text": "<system-reminder>r</system-reminder>"}]}),
            json!({"role": "model", "parts": [{"text": "second"}]}),
            json!({"role": "user", "parts": [{"text": "third"}]}),
            json!({"role": "user", "parts": [{"text": "<system-reminder>s</system-reminder>"}]}),
            json!({"role": "model", "parts": [{"text": "fourth"}]}),
            json!({"role": "user", "parts": [{"text": "latest"}]}),
        ];
        assert_eq!(foldable_runs(&contents, 6), vec![2..4, 5..6]);
        assert!(foldable_runs(&contents, 1).is_empty());
    }

    #[test]
    fn foldable_runs_trim_and_split_around_straddling_tool_pairs() {
        let contents = vec![
            json!({"role": "user", "parts": [{"text": "first"}]}),
            json!({"role": "model", "parts": [{"functionCall": {"id": "c1", "name": "read", "args": {}}}]}),
            json!({"role": "model", "parts": [{"functionCall": {"id": "c2", "name": "read", "args": {}}}]}),
            json!({"role": "user", "parts": [{"functionResponse": {"id": "c1", "name": "read", "response": {}}}]}),
            json!({"role": "model", "parts": [{"text": "note"}]}),
            json!({"role": "user", "parts": [{"text": "later"}]}),
            json!({"role": "user", "parts": [{"text": "<system-reminder>r</system-reminder>"}]}),
            json!({"role": "user", "parts": [{"functionResponse": {"id": "c2", "name": "read", "response": {}}}]}),
            json!({"role": "user", "parts": [{"text": "latest"}]}),
        ];
        // Turn 2 (c2) pairs across the reminder at 6 with turn 7: drop turn 2, then turns 1 and 3
        // also straddle the gap around turn 2 so only the clean suffix 4..6 remains.
        let runs = foldable_runs(&contents, 8);
        assert_eq!(runs, vec![4..6]);
        for run in runs {
            let call = CompressCall {
                start: run.start,
                end: run.end,
                summary: "kept".into(),
                topic: None,
            };
            assert_eq!(split_pair(&call, &contents), None);
        }
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
                    (turnkeys::RAW): {"role": "user", "content": format!("t{n}")}
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
                .insert(turnkeys::RAW.into(), json!({"role": "user"}));
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
        contents.insert(2, turnkeys::nudge_turn("nudge: first"));
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
            json!({"role": "user", "parts": [{"functionResponse": {"id": "call_p", "name": "tool", "response": {"result": format!("folded\n[{}]", turnkeys::splice_marker("call_p"))}}}]}),
        ];
        contents.splice(2..2, pair.clone());
        compressor
            .compress_batch(&mut contents, vec![range(1, 4)], &CompressPolicy::default())
            .unwrap();

        // The stored pair differs from the request copy: no `_raw`, the splice marker in its own
        // response field, and the call's name instead of the wire's "tool".
        let skip = vec![
            pair[0].clone(),
            json!({"role": "user", "parts": [{"functionResponse": {"id": "call_p", "name": "compress", "response": {"result": "folded", "splice": turnkeys::splice_marker("call_p")}}}]}),
        ];
        let mut rebuilt = raw_turns();
        assert_eq!(compressor.apply_skipping(&mut rebuilt, &skip), 1);
        assert_eq!(rebuilt.len(), 6);
        assert!(rebuilt[1]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("block b1"));
    }

    // Regression: after a reset the splice store is empty, the request carries the proxy-shaped
    // pair, and the block stored the wire-shaped one; the fold must still apply.
    #[test]
    fn fold_applies_when_carried_proxy_pair_differs_from_stored_wire_pair() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = raw_turns();
        contents.splice(2..2, [
            json!({"role": "model", "parts": [{"functionCall": {"id": "call_p", "name": "compress", "args": {}}}]}),
            json!({"role": "user", "parts": [{"functionResponse": {"id": "call_p", "name": "tool", "response": {"result": format!("folded\n[{}]", turnkeys::splice_marker("call_p"))}}}]}),
        ]);
        compressor
            .compress_batch(&mut contents, vec![range(1, 4)], &CompressPolicy::default())
            .unwrap();
        let mut rebuilt = raw_turns();
        rebuilt.splice(2..2, [
            json!({"role": "model", "parts": [{"functionCall": {"id": "call_p", "name": "compress", "args": {}}}]}),
            json!({"role": "user", "parts": [{"functionResponse": {"id": "call_p", "name": "compress", "response": {"result": "folded", "splice": turnkeys::splice_marker("call_p")}}}]}),
        ]);
        assert_eq!(compressor.apply_skipping(&mut rebuilt, &[]), 1, "fold-miss");
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

        let hits = compressor.search_context("PLAN", 5).unwrap().hits;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].block_id, "b1");
        assert!(compressor
            .search_context("absent", 5)
            .unwrap()
            .hits
            .is_empty());
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

            let hits = compressor.search_context("target", 5).unwrap().hits;
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
        let hits = compressor.search_context("alpha beta", 5).unwrap().hits;
        let ids: Vec<&str> = hits.iter().map(|hit| hit.block_id.as_str()).collect();
        assert_eq!(ids, ["b2", "b1"]);

        // The limit applies after ranking, so the best block survives a limit of one.
        let top = compressor.search_context("alpha beta", 1).unwrap().hits;
        assert_eq!(top[0].block_id, "b2");
        assert!(compressor.search_context("  ", 5).is_err());
    }

    fn tool_turns() -> Vec<Value> {
        vec![
            json!({"role": "model", "parts": [{"functionCall": {"id": "c1", "name": "read", "args": {"path": "zoo.txt"}}}]}),
            json!({"role": "user", "parts": [{"functionResponse": {"id": "c1", "name": "read", "response": {"output": "the zebra was seen"}}}]}),
            json!({"role": "model", "parts": [{"text": "📦 [acp] Compressed turns 1–2 into block b0\n\nmarker words"}]}),
            json!({"role": "user", "parts": [{"text": "zebra in the visible message"}]}),
            json!({"role": "user", "parts": [{"text": "last"}]}),
        ]
    }

    #[test]
    fn search_finds_terms_only_in_the_original_turns_of_a_block() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = tool_turns();
        let call = CompressCall {
            start: 0,
            end: 3,
            summary: "the read finished".into(),
            topic: None,
        };
        compressor
            .compress_batch(&mut contents, vec![call], &CompressPolicy::default())
            .unwrap();

        // "zebra" is in the folded tool result only: found, and marked as original text.
        let hits = compressor.search_context("zebra", 5).unwrap().hits;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].block_id, "b1");
        assert!(hits[0].from_original);
        assert!(hits[0].snippet.contains("zebra"));

        // A summary match is not repeated as an original-text hit.
        let summary_hits = compressor.search_context("finished", 5).unwrap().hits;
        assert_eq!(summary_hits.len(), 1);
        assert!(!summary_hits[0].from_original);

        // Visible messages are not searched, and the marker text of a fold is not original content.
        assert!(compressor
            .search_context("visible", 5)
            .unwrap()
            .hits
            .is_empty());
        assert!(compressor
            .search_context("compressed", 5)
            .unwrap()
            .hits
            .is_empty());
    }

    fn one_block() -> (tempfile::TempDir, NativeCompressor, Vec<Value>) {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = turns();
        compressor
            .compress_batch(&mut contents, vec![call()], &CompressPolicy::default())
            .unwrap();
        (dir, compressor, contents)
    }

    #[test]
    fn render_marks_the_rarest_term_in_a_windowed_snippet() {
        let (_dir, compressor, _) = one_block();
        let report = compressor.search_context("agreed", 5).unwrap();
        assert_eq!(
            report.render(),
            "Found 1 block(s) for \"agreed\":\n\n\
             b1 \"plan\" [2 turns]\n  **second** and **third** 【agreed】 on `x=1`"
        );
    }

    #[test]
    fn render_shows_a_topic_only_hit_from_the_start_of_the_summary() {
        let (_dir, compressor, _) = one_block();
        let report = compressor.search_context("plan", 5).unwrap();
        assert_eq!(
            report.render(),
            "Found 1 block(s) for \"plan\":\n\n\
             b1 \"plan\" [2 turns]\n  **second** and **third** agreed on `x=1`"
        );
    }

    #[test]
    fn render_marks_a_restored_block_inactive() {
        let (_dir, mut compressor, mut contents) = one_block();
        compressor
            .decompress(
                &mut contents,
                DecompressCall {
                    block_id: "b1".into(),
                    to_file: None,
                },
            )
            .unwrap();
        let text = compressor.search_context("agreed", 5).unwrap().render();
        assert!(text.contains("b1 \"plan\" [2 turns] [inactive"), "{text}");
    }

    #[test]
    fn render_says_what_was_searched_when_nothing_matches() {
        let (_dir, compressor, _) = one_block();
        assert_eq!(
            compressor.search_context("absent", 5).unwrap().render(),
            "No blocks matched \"absent\" among 1 block(s). Scope = compressed topics and \
             summaries plus the original text of compressed blocks, NOT visible messages. \
             Absence here does not mean the topic was never discussed."
        );
        let empty = NativeCompressor::new(tempdir().unwrap().path()).unwrap();
        assert_eq!(
            empty.search_context("absent", 5).unwrap().render(),
            "No compressed blocks exist yet — nothing to search."
        );
    }

    #[test]
    fn render_notes_truncation_and_a_term_the_shown_blocks_lack() {
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

        // Both blocks score 2, so b1 leads; b2 is the candidate the limit leaves out.
        let text = compressor
            .search_context("alpha beta only", 1)
            .unwrap()
            .render();
        assert!(
            text.contains(
                "[truncated: showing 1 of 2 candidates by relevance — pass limit:2 to see the rest]"
            ),
            "{text}"
        );
        assert!(text.contains("try searching \"beta\" alone"), "{text}");
    }

    #[test]
    fn render_lists_original_text_hits_under_their_own_heading() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path()).unwrap();
        let mut contents = tool_turns();
        compressor
            .compress_batch(
                &mut contents,
                vec![CompressCall {
                    start: 0,
                    end: 3,
                    summary: "the read finished".into(),
                    topic: None,
                }],
                &CompressPolicy::default(),
            )
            .unwrap();
        assert_eq!(
            compressor.search_context("zebra", 5).unwrap().render(),
            "Found 0 block(s) for \"zebra\":\n\n\
             Original text, not in any summary:\n\n\
             b1 \"(no topic)\" [3 turns]\n  \
             read {\"path\":\"zoo.txt\"} the 【zebra】 was seen marker words"
        );
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

    #[test]
    fn archive_blocks_moves_the_directory_aside_under_the_first_free_generation() {
        let dir = tempdir().unwrap();
        let blocks = dir.path().join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        std::fs::write(blocks.join("b1.json"), b"old").unwrap();

        NativeCompressor::archive_blocks(dir.path()).unwrap();
        assert!(!blocks.exists());
        assert_eq!(
            std::fs::read(dir.path().join("blocks-reset-1/b1.json")).unwrap(),
            b"old"
        );

        std::fs::create_dir_all(&blocks).unwrap();
        NativeCompressor::archive_blocks(dir.path()).unwrap();
        assert!(dir.path().join("blocks-reset-2").exists());
    }

    #[test]
    fn archive_blocks_with_no_directory_moves_nothing() {
        let dir = tempdir().unwrap();
        NativeCompressor::archive_blocks(dir.path()).unwrap();
        assert!(!dir.path().join("blocks-reset-1").exists());
    }
}
