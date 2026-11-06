use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;

/// One folded range: the summary survives, the original turns live in `spill/`.
#[derive(Debug, Clone)]
struct Block {
    id: String,
    topic: Option<String>,
    summary: String,
    originals: Vec<Value>,
    summary_turn: Value,
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
    pub spill_dir: String,
}

/// Billion-context backend. Phase 1 ships the native implementation; a future
/// proxy-backed compressor implements the same operations.
pub trait Compressor: Send + Sync {
    fn compress(&mut self, contents: &mut Vec<Value>, call: CompressCall)
        -> Result<String, String>;
    fn decompress(
        &mut self,
        contents: &mut Vec<Value>,
        call: DecompressCall,
    ) -> Result<String, String>;
    fn search_context(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, String>;
    fn status(&self) -> CompressorStatus;

    /// Re-attach stored folds to a request that the harness rebuilt from its
    /// own history. Backends without folds do nothing.
    fn apply(&mut self, _contents: &mut Vec<Value>) -> usize {
        0
    }
}

pub struct NativeCompressor {
    blocks: Vec<Block>,
    next_id: usize,
    spill_dir: PathBuf,
}

impl NativeCompressor {
    pub fn new(spill_dir: impl Into<PathBuf>) -> Self {
        Self {
            blocks: Vec::new(),
            next_id: 1,
            spill_dir: spill_dir.into(),
        }
    }

    fn spill_originals(&self, block: &Block) -> Result<(), String> {
        let dir = self.spill_dir.join("blocks");
        std::fs::create_dir_all(&dir).map_err(|err| format!("spill dir: {err}"))?;
        let payload = json!({
            "blockId": block.id,
            "topic": block.topic,
            "summary": block.summary,
            "originals": block.originals,
        });
        let bytes = serde_json::to_vec_pretty(&payload).map_err(|err| err.to_string())?;
        std::fs::write(dir.join(format!("{}.json", block.id)), bytes)
            .map_err(|err| format!("spill write: {err}"))
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
        if call.start >= call.end || call.end > contents.len() {
            return Err(format!(
                "invalid range {}..{} for {} turns",
                call.start,
                call.end,
                contents.len()
            ));
        }
        if call.summary.trim().is_empty() {
            return Err("summary must not be empty".into());
        }

        let id = format!("b{}", self.next_id);
        self.next_id += 1;
        let originals = contents[call.start..call.end].to_vec();
        let summary_turn = Self::summary_turn(self.marker(&call, &id));
        let block = Block {
            id: id.clone(),
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

        if let Some(position) = contents.iter().position(|turn| turn == &block.summary_turn) {
            contents.splice(position..position + 1, block.originals.clone());
        } else if !contains_window(contents, &block.originals) {
            return Err(format!(
                "block {} is not present in this conversation",
                block.id
            ));
        }
        self.blocks[index].restored = true;
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
            spill_dir: self.spill_dir.to_string_lossy().to_string(),
        }
    }

    fn apply(&mut self, contents: &mut Vec<Value>) -> usize {
        let mut applied = 0;
        for block in &self.blocks {
            if block.restored || block.originals.is_empty() {
                continue;
            }
            if let Some(position) = find_window(contents, &block.originals) {
                let replacement = [block.summary_turn.clone()];
                contents.splice(position..position + block.originals.len(), replacement);
                applied += 1;
            }
        }
        applied
    }
}

fn find_window(contents: &[Value], window: &[Value]) -> Option<usize> {
    if window.is_empty() || window.len() > contents.len() {
        return None;
    }
    contents
        .windows(window.len())
        .position(|turns| turns == window)
}

fn contains_window(contents: &[Value], window: &[Value]) -> bool {
    find_window(contents, window).is_some()
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
    fn parity_fold_swaps_range_for_block_and_spills_originals() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path());
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
        let mut compressor = NativeCompressor::new(dir.path());
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
        let mut compressor = NativeCompressor::new(dir.path());
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
    fn search_finds_a_block_case_insensitively() {
        let dir = tempdir().unwrap();
        let mut compressor = NativeCompressor::new(dir.path());
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
        let mut compressor = NativeCompressor::new(dir.path());
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
        let mut compressor = NativeCompressor::new(dir.path());
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
        let mut compressor = NativeCompressor::new(dir.path());
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
}
