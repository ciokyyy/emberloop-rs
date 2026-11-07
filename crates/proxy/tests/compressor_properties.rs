#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Properties of the compression engine over generated conversations, through the public
//! `NativeCompressor`/`Compressor` API: refolding a harness-rebuilt history, the fold and
//! decompress round trip, and all-or-none batches. Conversations are 2 to 29 turns of user and
//! model text and function-call/response pairs, always ending with the latest user message, so
//! every generated range is valid unless the test breaks it on purpose.

use proptest::prelude::*;
use proptest::test_runner::TestCaseError;
use proxy::compressor::{
    CompressCall, CompressPolicy, Compressor, DecompressCall, NativeCompressor,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use tempfile::tempdir;

const WORDS: [&str; 3] = ["ok", "go on", "done"];
const TOOLS: [&str; 3] = ["read", "grep", "bash"];

fn config() -> ProptestConfig {
    ProptestConfig {
        failure_persistence: None,
        cases: 64,
        ..ProptestConfig::default()
    }
}

#[derive(Debug, Clone, Copy)]
enum Unit {
    Text { user: bool, word: usize, raw: bool },
    Call { tool: usize, signed: bool },
}

#[derive(Debug, Clone, Copy)]
enum Fault {
    OutOfBounds,
    EmptyRange,
    BlankSummary,
    IncludesLatestUser,
    SpillObstacle,
}

fn unit() -> impl Strategy<Value = Unit> {
    prop_oneof![
        (any::<bool>(), 0..WORDS.len(), any::<bool>()).prop_map(|(user, word, raw)| Unit::Text {
            user,
            word,
            raw
        }),
        (0..TOOLS.len(), any::<bool>()).prop_map(|(tool, signed)| Unit::Call { tool, signed }),
    ]
}

fn fault() -> impl Strategy<Value = Fault> {
    prop_oneof![
        Just(Fault::OutOfBounds),
        Just(Fault::EmptyRange),
        Just(Fault::BlankSummary),
        Just(Fault::IncludesLatestUser),
        Just(Fault::SpillObstacle),
    ]
}

/// Turns built from the units, then the latest user message. With `distinct`, every text carries
/// its position, so no two turns repeat; without it, texts come from a three-word pool and repeat.
fn build(units: &[Unit], distinct: bool) -> Vec<Value> {
    let mut turns = Vec::new();
    for (index, unit) in units.iter().enumerate() {
        let tag = if distinct {
            format!(" #{index}")
        } else {
            String::new()
        };
        match *unit {
            Unit::Text { user, word, raw } => {
                let role = if user { "user" } else { "model" };
                let text = format!("{}{tag}", WORDS[word]);
                let mut turn = json!({"role": role, "parts": [{"text": text}]});
                if raw {
                    turn["_raw"] = json!({"role": role, "content": text});
                }
                turns.push(turn);
            }
            Unit::Call { tool, signed } => {
                let id = format!("call_{index}");
                let name = TOOLS[tool];
                let mut call = json!({"functionCall": {
                    "id": id,
                    "name": name,
                    "args": {"path": format!("src/{name}.rs")}
                }});
                if signed {
                    call["thoughtSignature"] = json!("sig");
                }
                turns.push(json!({"role": "model", "parts": [call]}));
                turns.push(json!({"role": "user", "parts": [{"functionResponse": {
                    "id": id,
                    "name": name,
                    "response": {"result": format!("{name} output{tag}")}
                }}]}));
            }
        }
    }
    let end = if distinct { " #end" } else { "" };
    turns.push(json!({"role": "user", "parts": [{"text": format!("final question{end}")}]}));
    turns
}

fn history(distinct: bool) -> impl Strategy<Value = Vec<Value>> {
    proptest::collection::vec(unit(), 1..=14).prop_map(move |units| build(&units, distinct))
}

/// Up to two non-overlapping ranges, every end before the latest user message.
fn folds(len: usize) -> impl Strategy<Value = Vec<(usize, usize)>> {
    let last = len - 1;
    proptest::collection::btree_set(0..=last, 2..=(last + 1).min(4)).prop_map(|cuts| {
        let cuts: Vec<usize> = cuts.into_iter().collect();
        cuts.as_chunks::<2>()
            .0
            .iter()
            .map(|&[start, end]| (start, end))
            .collect()
    })
}

fn history_with_folds(distinct: bool) -> impl Strategy<Value = (Vec<Value>, Vec<(usize, usize)>)> {
    history(distinct).prop_flat_map(|turns| {
        let folds = folds(turns.len());
        (Just(turns), folds)
    })
}

fn calls(folds: &[(usize, usize)]) -> Vec<CompressCall> {
    folds
        .iter()
        .map(|&(start, end)| CompressCall {
            start,
            end,
            summary: format!("summary of {start}..{end}"),
            topic: None,
        })
        .collect()
}

/// The turns the harness resends: the chat wire's `_raw` copies and the Google thought
/// signatures are not in its history.
fn harness_view(turns: &[Value]) -> Vec<Value> {
    turns
        .iter()
        .map(|turn| {
            let mut turn = turn.clone();
            if let Some(object) = turn.as_object_mut() {
                object.remove("_raw");
            }
            if let Some(parts) = turn.get_mut("parts").and_then(Value::as_array_mut) {
                for part in parts {
                    if let Some(object) = part.as_object_mut() {
                        object.remove("thoughtSignature");
                    }
                }
            }
            turn
        })
        .collect()
}

fn bad_call(fault: Fault, len: usize) -> Option<CompressCall> {
    let call = |start, end, summary: &str| CompressCall {
        start,
        end,
        summary: summary.into(),
        topic: None,
    };
    match fault {
        Fault::OutOfBounds => Some(call(0, len + 1, "past the end")),
        Fault::EmptyRange => Some(call(0, 0, "no turns")),
        Fault::BlankSummary => Some(call(0, 1, "  ")),
        Fault::IncludesLatestUser => Some(call(len - 1, len, "the latest message")),
        Fault::SpillObstacle => None,
    }
}

/// Every file and directory under `root`: a directory maps to `None`, a file to its bytes.
fn snapshot(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn collect(root: &Path, dir: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        for entry in std::fs::read_dir(dir).expect("readable directory") {
            let path = entry.expect("directory entry").path();
            let key = path
                .strip_prefix(root)
                .expect("under the root")
                .display()
                .to_string();
            if path.is_dir() {
                out.insert(key, None);
                collect(root, &path, out);
            } else {
                out.insert(key, Some(std::fs::read(&path).expect("readable file")));
            }
        }
    }
    let mut out = BTreeMap::new();
    collect(root, root, &mut out);
    out
}

fn engine_error(error: String) -> TestCaseError {
    TestCaseError::fail(error)
}

proptest! {
    #![proptest_config(config())]

    /// Refolding the harness's unfolded history reproduces the folded history, and refolding
    /// again changes nothing.
    #[test]
    fn refolding_the_unfolded_history_reproduces_the_fold_and_a_second_pass_changes_nothing(
        (turns, folds) in history_with_folds(true)
    ) {
        let dir = tempdir().expect("temp dir");
        let mut compressor = NativeCompressor::new(dir.path()).map_err(engine_error)?;
        let mut folded = turns.clone();
        compressor
            .compress_batch(&mut folded, calls(&folds), &CompressPolicy::default())
            .map_err(engine_error)?;

        let mut view = harness_view(&turns);
        let applied = compressor.apply_skipping(&mut view, &[]);
        prop_assert_eq!(applied, folds.len());
        prop_assert_eq!(&view, &harness_view(&folded));

        let once = view.clone();
        prop_assert_eq!(compressor.apply_skipping(&mut view, &[]), 0);
        prop_assert_eq!(view, once);
    }

    /// The same refold property with repeated texts (a history of "ok", "done" and so on).
    ///
    /// A fold matches the span nearest its recorded start, and a block whose summary is already
    /// in the history is not applied again. The minimal case, `[ok, ok, final]` with fold `1..2`,
    /// is pinned by a characterization test.
    #[test]
    fn refolding_a_history_with_repeated_text_reproduces_the_fold(
        (turns, folds) in history_with_folds(false)
    ) {
        let dir = tempdir().expect("temp dir");
        let mut compressor = NativeCompressor::new(dir.path()).map_err(engine_error)?;
        let mut folded = turns.clone();
        compressor
            .compress_batch(&mut folded, calls(&folds), &CompressPolicy::default())
            .map_err(engine_error)?;

        let mut view = harness_view(&turns);
        prop_assert_eq!(compressor.apply_skipping(&mut view, &[]), folds.len());
        prop_assert_eq!(&view, &harness_view(&folded));
    }

    /// Folding and then decompressing every block gives back the original turns exactly.
    #[test]
    fn fold_then_decompress_every_block_restores_the_original_turns(
        (turns, folds) in history_with_folds(true)
    ) {
        let dir = tempdir().expect("temp dir");
        let mut compressor = NativeCompressor::new(dir.path()).map_err(engine_error)?;
        let mut contents = turns.clone();
        compressor
            .compress_batch(&mut contents, calls(&folds), &CompressPolicy::default())
            .map_err(engine_error)?;

        for id in 1..=folds.len() {
            compressor
                .decompress(
                    &mut contents,
                    DecompressCall {
                        block_id: format!("b{id}"),
                        to_file: None,
                    },
                )
                .map_err(engine_error)?;
        }
        prop_assert_eq!(compressor.status().active_blocks, 0);
        prop_assert_eq!(contents, turns);
    }

    /// A batch that fails, for any reason, leaves the history, the status and every file under
    /// the spill directory as they were.
    #[test]
    fn a_failed_batch_leaves_history_status_and_spill_dir_unchanged(
        (turns, folds, fault) in history_with_folds(true)
            .prop_flat_map(|(turns, folds)| (Just(turns), Just(folds), fault()))
    ) {
        let dir = tempdir().expect("temp dir");
        let mut compressor = NativeCompressor::new(dir.path()).map_err(engine_error)?;
        let mut batch = calls(&folds);
        if let Some(bad) = bad_call(fault, turns.len()) {
            batch.push(bad);
        }
        if matches!(fault, Fault::SpillObstacle) {
            // The lowest block is b1; a directory where its file goes makes its rename fail
            // after the higher blocks were already written.
            std::fs::create_dir_all(dir.path().join("blocks/b1.json")).expect("obstacle");
        }

        let mut contents = turns.clone();
        let status_before = compressor.status();
        let files_before = snapshot(dir.path());
        let result = compressor.compress_batch(&mut contents, batch, &CompressPolicy::default());

        prop_assert!(result.is_err(), "{fault:?} must refuse the batch");
        prop_assert_eq!(contents, turns);
        prop_assert_eq!(compressor.status(), status_before);
        prop_assert_eq!(snapshot(dir.path()), files_before);
    }
}
