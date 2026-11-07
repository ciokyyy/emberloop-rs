#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Characterization of the compression engine through its public API: turn matching
//! (`turn_eq`, `find_span`, `is_transient`), refolding (`apply`, `apply_skipping`), `fold_report`,
//! the summary marker, `token_count`, the policy and batch validation. Every expectation is what
//! the code does today. `fixtures/compressor/conversation.json` is a 12-turn chat history with
//! `_raw` copies, a thought signature, a steer, a proxy splice pair and a transient nudge.

use proxy::compressor::{
    CompressCall, CompressPolicy, Compressor, DecompressCall, NativeCompressor,
};
use serde_json::{json, Value};
use tempfile::{tempdir, TempDir};

fn conversation() -> Vec<Value> {
    let path = format!(
        "{}/tests/fixtures/compressor/conversation.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).expect("fixture exists");
    serde_json::from_str(&text).expect("fixture is a JSON array")
}

fn fresh() -> (TempDir, NativeCompressor) {
    let dir = tempdir().expect("temp dir");
    let compressor = NativeCompressor::new(dir.path()).expect("fresh compressor");
    (dir, compressor)
}

fn fold(start: usize, end: usize) -> CompressCall {
    CompressCall {
        start,
        end,
        summary: format!("summary of {start}..{end}"),
        topic: None,
    }
}

fn text(turn: &Value) -> &str {
    turn["parts"][0]["text"].as_str().unwrap_or_default()
}

fn nudge() -> Value {
    json!({"role": "user", "parts": [{"text": "nudge: first"}], "_nudge": true})
}

fn without(turns: &[Value], drop: &[usize]) -> Vec<Value> {
    turns
        .iter()
        .enumerate()
        .filter(|(index, _)| !drop.contains(index))
        .map(|(_, turn)| turn.clone())
        .collect()
}

/// What the harness resends: the nudge is never in its history, and the chat wire's `_raw`
/// copies and the Google thought signatures are not in its turns either.
fn harness_view(turns: &[Value]) -> Vec<Value> {
    turns
        .iter()
        .filter(|turn| turn.get("_nudge").is_none())
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

/// The estimate the status totals use: compact JSON bytes of the turn, divided by four.
fn estimate(turn: &Value) -> u64 {
    u64::try_from(turn.to_string().len() / 4).unwrap()
}

#[test]
fn a_turn_that_differs_only_in_raw_or_thought_signature_still_matches() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    assert!(contents[1].get("_raw").is_some(), "stored turns carry _raw");
    compressor.compress(&mut contents, fold(1, 5)).unwrap();

    let mut view = harness_view(&conversation());
    assert!(conversation()[2]["parts"][0]
        .get("thoughtSignature")
        .is_some());
    assert!(view[2]["parts"][0].get("thoughtSignature").is_none());
    assert_eq!(compressor.apply(&mut view), 1);
    assert_eq!(
        text(&view[1]),
        "📦 [acp] Compressed turns 1–4 into block b1\n\nsummary of 1..5"
    );
}

#[test]
fn a_turn_whose_text_changed_does_not_match_the_stored_fold() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();

    let mut view = harness_view(&conversation());
    view[1]["parts"][0]["text"] = json!("a different answer");
    assert_eq!(compressor.apply(&mut view), 0);
    assert_eq!(view.len(), 11);
}

#[test]
fn an_extra_field_other_than_raw_or_thought_signature_stops_a_match() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();

    let mut view = harness_view(&conversation());
    view[3]["meta"] = json!(1);
    assert_eq!(compressor.apply(&mut view), 0);
}

#[test]
fn reapplying_a_fold_to_its_own_result_changes_nothing() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();

    let mut view = harness_view(&conversation());
    assert_eq!(compressor.apply(&mut view), 1);
    let once = view.clone();
    assert_eq!(compressor.apply(&mut view), 0);
    assert_eq!(view, once);
}

#[test]
fn contiguous_folded_turns_are_replaced_by_the_stored_summary_turn_byte_for_byte() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();
    let stored_summary = contents[1].clone();

    let mut view = harness_view(&conversation());
    assert_eq!(compressor.apply(&mut view), 1);
    assert_eq!(view.len(), 8);
    assert_eq!(view[1], stored_summary);
}

#[test]
fn up_to_four_unrelated_turns_between_folded_neighbours_still_match() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();

    let mut view = harness_view(&conversation());
    for n in 0..4 {
        view.insert(
            3,
            json!({"role": "user", "parts": [{"text": format!("steer {n}")}]}),
        );
    }
    assert_eq!(compressor.apply(&mut view), 1);
    // Only the folded turns are replaced; the steers between them are not in the stored fold,
    // so they stay in the request after the summary.
    assert_eq!(view.len(), 12);
    assert!(text(&view[1]).starts_with("📦"), "{}", text(&view[1]));
    assert_eq!(
        view.iter()
            .filter(|turn| text(turn).starts_with("steer"))
            .count(),
        4
    );
}

#[test]
fn five_unrelated_turns_between_folded_neighbours_break_the_match() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();

    let mut view = harness_view(&conversation());
    for n in 0..5 {
        view.insert(
            3,
            json!({"role": "user", "parts": [{"text": format!("steer {n}")}]}),
        );
    }
    assert_eq!(compressor.apply(&mut view), 0);
}

#[test]
fn folded_turns_out_of_their_stored_order_do_not_match() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();

    let mut view = harness_view(&conversation());
    view.swap(2, 3);
    assert_eq!(compressor.apply(&mut view), 0);
}

#[test]
fn a_nudge_in_the_stored_fold_is_not_needed_for_a_later_match() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    // Turns 10..12 hold the transient nudge at 10; the fold stores it. Turn 9, the latest user
    // message, is outside the fold.
    compressor.compress(&mut contents, fold(10, 12)).unwrap();
    assert!(contents[10]["parts"][0]["text"]
        .as_str()
        .unwrap()
        .starts_with("📦"));

    let mut view = harness_view(&conversation());
    assert_eq!(compressor.apply(&mut view), 1);
    assert_eq!(view.len(), 11);
}

#[test]
fn a_nudge_in_the_request_counts_as_one_unrelated_turn_between_folded_turns() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();

    let mut view = harness_view(&conversation());
    view.insert(3, nudge());
    assert_eq!(compressor.apply(&mut view), 1);
}

#[test]
fn apply_without_skip_misses_a_fold_whose_splice_pair_the_harness_does_not_resend() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(4, 9)).unwrap();

    // The harness has no steer (5) and no proxy pair (6, 7); the splice store puts them back.
    let mut view = harness_view(&without(&conversation(), &[5, 6, 7]));
    assert_eq!(compressor.apply(&mut view), 0);
}

#[test]
fn apply_skipping_matches_a_fold_once_the_splice_turns_are_skipped_even_with_a_changed_shape() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(4, 9)).unwrap();

    // The skipped pair is matched by call id: the response's name and marker field differ.
    let mut response = conversation()[7].clone();
    response["parts"][0]["functionResponse"]["name"] = json!("compress");
    let skip = vec![
        conversation()[5].clone(),
        conversation()[6].clone(),
        response,
    ];

    let mut view = harness_view(&without(&conversation(), &[5, 6, 7]));
    assert_eq!(compressor.apply_skipping(&mut view, &skip), 1);
    assert_eq!(view.len(), 7);
    assert_eq!(
        text(&view[4]),
        "📦 [acp] Compressed turns 4–8 into block b1\n\nsummary of 4..9"
    );
}

#[test]
fn apply_skipping_an_unrelated_turn_leaves_the_match_alone() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();

    let mut view = harness_view(&conversation());
    assert_eq!(
        compressor.apply_skipping(&mut view, &[conversation()[9].clone()]),
        1
    );
}

#[test]
fn a_fold_over_an_earlier_fold_marker_resolves_in_one_apply() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();
    // Folding turns 0 and 1 folds the first block's marker into block b2.
    compressor.compress(&mut contents, fold(0, 2)).unwrap();

    let mut view = harness_view(&conversation());
    assert_eq!(compressor.apply(&mut view), 2);
    assert_eq!(view.len(), 7);
    assert!(text(&view[0]).contains("block b2"), "{}", text(&view[0]));
}

#[test]
fn a_restored_block_is_no_longer_refolded() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();
    let restored = compressor
        .decompress(
            &mut contents,
            DecompressCall {
                block_id: "b1".into(),
                to_file: None,
            },
        )
        .unwrap();
    assert_eq!(restored, "Restored 4 turns from block b1");

    let mut view = harness_view(&conversation());
    assert_eq!(compressor.apply(&mut view), 0);
}

#[test]
fn the_summary_marker_names_the_range_block_and_topic() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    let call = CompressCall {
        topic: Some("billing".into()),
        ..fold(1, 5)
    };
    compressor.compress(&mut contents, call).unwrap();

    assert_eq!(contents[1]["role"], "user");
    assert_eq!(contents[1]["parts"].as_array().unwrap().len(), 1);
    assert_eq!(
        text(&contents[1]),
        "📦 [acp] Compressed turns 1–4 into block b1 · billing\n\nsummary of 1..5"
    );
}

#[test]
fn the_summary_marker_has_no_topic_separator_without_a_topic() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();
    assert_eq!(
        text(&contents[1]),
        "📦 [acp] Compressed turns 1–4 into block b1\n\nsummary of 1..5"
    );
}

#[test]
fn compress_returns_the_one_line_report_for_a_single_range() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    let report = compressor.compress(&mut contents, fold(1, 5)).unwrap();
    assert_eq!(
        report,
        "Compressed turns 1–4 into block b1. Originals kept in the session spill dir."
    );
}

#[test]
fn a_batch_returns_one_report_line_per_range_in_history_order() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    let report = compressor
        .compress_batch(
            &mut contents,
            vec![fold(10, 12), fold(1, 3)],
            &CompressPolicy::default(),
        )
        .unwrap();
    let lines: Vec<&str> = report.lines().collect();
    assert_eq!(lines.len(), 2, "{report}");
    assert!(lines[0].contains("turns 1–2 into block b1"), "{report}");
    assert!(lines[1].contains("turns 10–11 into block b2"), "{report}");
}

#[test]
fn tokens_compressed_estimates_each_folded_turn_as_its_json_bytes_over_four() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();

    let expected: u64 = conversation()[1..5].iter().map(estimate).sum();
    let status = compressor.status();
    assert_eq!(status.tokens_compressed, expected);
    assert_eq!(status.spilled_turns, 4);
    assert_eq!(
        (status.blocks, status.active_blocks, status.restored_blocks),
        (1, 1, 0)
    );
}

#[test]
fn a_restored_block_stops_counting_toward_the_status_totals() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();
    compressor
        .decompress(
            &mut contents,
            DecompressCall {
                block_id: "b1".into(),
                to_file: None,
            },
        )
        .unwrap();

    let status = compressor.status();
    assert_eq!(
        (status.blocks, status.active_blocks, status.restored_blocks),
        (1, 0, 1)
    );
    assert_eq!((status.spilled_turns, status.tokens_compressed), (0, 0));
}

#[test]
fn fold_report_is_silent_when_the_stored_fold_matches() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();
    assert_eq!(
        compressor.fold_report(&harness_view(&conversation()), &[]),
        None
    );
}

#[test]
fn fold_report_is_silent_with_no_stored_fold() {
    let (_dir, compressor) = fresh();
    assert_eq!(compressor.fold_report(&conversation(), &[]), None);
}

#[test]
fn fold_report_says_the_first_stored_turn_is_missing_from_the_request() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();

    let view = without(&harness_view(&conversation()), &[1]);
    assert_eq!(
        compressor.fold_report(&view, &[]).as_deref(),
        Some("b1: first stored turn `I will read the billing code first.` is not in the request")
    );
}

#[test]
fn fold_report_names_the_first_turn_that_differs_after_the_matched_head() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(1, 5)).unwrap();

    let mut view = harness_view(&conversation());
    view[4]["parts"][0]["text"] = json!("changed");
    let report = compressor.fold_report(&view, &[]).expect("a report");
    assert!(
        report.starts_with("b1: turns 0..3 matched at 1, turn 3 differs: "),
        "{report}"
    );
}

#[test]
fn fold_report_ignores_a_nudge_and_skipped_splice_turns() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    compressor.compress(&mut contents, fold(10, 12)).unwrap();
    assert_eq!(
        compressor.fold_report(&harness_view(&conversation()), &[]),
        None
    );

    let (_dir2, mut compressor2) = fresh();
    let mut contents2 = conversation();
    compressor2.compress(&mut contents2, fold(4, 9)).unwrap();
    let view = harness_view(&without(&conversation(), &[5, 6, 7]));
    let skip = vec![
        conversation()[5].clone(),
        conversation()[6].clone(),
        conversation()[7].clone(),
    ];
    assert!(compressor2
        .fold_report(&view, &[])
        .unwrap()
        .contains("matched at 4"));
    assert_eq!(compressor2.fold_report(&view, &skip), None);
}

#[test]
fn preserve_recent_messages_keeps_the_tail_out_of_every_fold() {
    let (_dir, mut compressor) = fresh();
    let policy = CompressPolicy::default().with_recent(3);
    let mut contents = conversation();
    assert!(compressor
        .compress_batch(&mut contents, vec![fold(0, 9)], &policy)
        .is_ok());

    let (_dir2, mut compressor2) = fresh();
    let mut contents2 = conversation();
    let before = contents2.clone();
    let error = compressor2
        .compress_batch(&mut contents2, vec![fold(0, 10)], &policy)
        .unwrap_err();
    assert_eq!(error, "range 0..10 includes the last 3 preserved turns");
    assert_eq!(contents2, before);
}

#[test]
fn preserving_more_turns_than_the_history_refuses_every_fold() {
    let (_dir, mut compressor) = fresh();
    let policy = CompressPolicy::default().with_recent(50);
    let error = compressor
        .compress_batch(&mut conversation(), vec![fold(0, 1)], &policy)
        .unwrap_err();
    assert_eq!(error, "range 0..1 includes the last 12 preserved turns");
}

#[test]
fn the_latest_user_message_is_never_folded_even_with_no_recent_tail() {
    let (_dir, mut compressor) = fresh();
    let policy = CompressPolicy::default();
    let mut contents = conversation();
    let before = contents.clone();
    // Turn 9 is the latest user message. Turn 10 is the transient nudge, which is not one.
    let error = compressor
        .compress_batch(&mut contents, vec![fold(8, 10)], &policy)
        .unwrap_err();
    assert_eq!(error, "range 8..10 includes the latest user message");
    assert_eq!(contents, before);
    assert!(compressor
        .compress_batch(&mut contents, vec![fold(8, 9)], &policy)
        .is_ok());
    assert!(compressor
        .compress_batch(&mut contents, vec![fold(9, 10)], &policy)
        .is_err());
}

#[test]
fn a_tool_result_sent_with_the_user_role_is_not_the_latest_user_message() {
    let (_dir, mut compressor) = fresh();
    // Turn 7 is a tool result (user role, no text); the latest user message is turn 9.
    let mut contents = conversation();
    assert!(compressor
        .compress_batch(&mut contents, vec![fold(6, 8)], &CompressPolicy::default())
        .is_ok());
}

#[test]
fn the_nudge_is_not_a_user_message_for_the_latest_user_protection() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    assert!(compressor
        .compress_batch(
            &mut contents,
            vec![fold(10, 12)],
            &CompressPolicy::default()
        )
        .is_ok());
}

#[test]
fn each_batch_is_checked_against_the_policy_it_is_given() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    let strict = CompressPolicy {
        protected_tools: vec!["read".into()],
        ..CompressPolicy::default()
    };
    let error = compressor
        .compress_batch(&mut contents, vec![fold(1, 5)], &strict)
        .unwrap_err();
    assert_eq!(error, "range 1..5 includes protected tool read");
    assert!(compressor
        .compress_batch(&mut contents, vec![fold(1, 5)], &CompressPolicy::default())
        .is_ok());
}

#[test]
fn a_protected_tool_call_inside_the_range_refuses_the_fold_and_names_the_tool() {
    let (_dir, mut compressor) = fresh();
    let policy = CompressPolicy::default().with_protected_tools(vec!["read".into()]);
    let error = compressor
        .compress_batch(&mut conversation(), vec![fold(1, 5)], &policy)
        .unwrap_err();
    assert_eq!(error, "range 1..5 includes protected tool read");
    assert!(compressor
        .compress_batch(&mut conversation(), vec![fold(4, 6)], &policy)
        .is_ok());
}

#[test]
fn a_protected_tool_result_alone_inside_the_range_refuses_the_fold() {
    let (_dir, mut compressor) = fresh();
    let policy = CompressPolicy::default().with_protected_tools(vec!["read".into()]);
    let error = compressor
        .compress_batch(&mut conversation(), vec![fold(3, 4)], &policy)
        .unwrap_err();
    assert_eq!(error, "range 3..4 includes protected tool read");
}

#[test]
fn a_bad_range_in_a_batch_refuses_the_whole_batch_and_names_it() {
    let (_dir, mut compressor) = fresh();
    let policy = CompressPolicy::default().with_protected_tools(vec!["tool".into()]);
    let mut contents = conversation();
    let before = contents.clone();
    let error = compressor
        .compress_batch(&mut contents, vec![fold(0, 2), fold(6, 8)], &policy)
        .unwrap_err();
    assert_eq!(error, "range 2: range 6..8 includes protected tool tool");
    assert_eq!(contents, before);
    assert_eq!(compressor.status().blocks, 0);
}

#[test]
fn overlapping_ranges_in_a_batch_keep_the_earlier_and_warn_about_the_later() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    let report = compressor
        .compress_batch(
            &mut contents,
            vec![fold(1, 4), fold(3, 5)],
            &CompressPolicy::default(),
        )
        .unwrap();
    // Turns 1..4 fold into b1; the later range 3..5 overlaps it and is not folded at all.
    assert_eq!(contents.len(), 10);
    assert!(
        text(&contents[1]).contains("block b1"),
        "{}",
        text(&contents[1])
    );
    let lines: Vec<&str> = report.lines().collect();
    assert_eq!(lines.len(), 2, "{report}");
    assert!(lines[0].contains("turns 1–3 into block b1"), "{report}");
    assert!(
        lines[1].starts_with("Warning:")
            && lines[1].contains("turns 3–4")
            && lines[1].contains("turns 1–3")
            && lines[1].contains("not folded"),
        "{report}"
    );
    assert_eq!(compressor.status().blocks, 1);
}

#[test]
fn the_earlier_range_in_history_wins_whatever_the_order_given() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    let report = compressor
        .compress_batch(
            &mut contents,
            vec![fold(3, 5), fold(1, 4)],
            &CompressPolicy::default(),
        )
        .unwrap();
    assert!(
        text(&contents[1]).contains("block b1"),
        "{}",
        text(&contents[1])
    );
    assert!(report.contains("turns 1–3 into block b1"), "{report}");
    assert!(report.contains("Warning:"), "{report}");
}

#[test]
fn an_invalid_range_refuses_the_batch_even_when_it_would_be_skipped_as_an_overlap() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    let before = contents.clone();
    let blank = CompressCall {
        summary: "  ".into(),
        ..fold(3, 5)
    };
    let error = compressor
        .compress_batch(
            &mut contents,
            vec![fold(1, 4), blank],
            &CompressPolicy::default(),
        )
        .unwrap_err();
    assert_eq!(error, "range 2: summary must not be empty");
    assert_eq!(contents, before);
    assert_eq!(compressor.status().blocks, 0);
}

#[test]
fn an_empty_batch_a_blank_summary_and_a_range_out_of_bounds_are_refused() {
    let (_dir, mut compressor) = fresh();
    let mut contents = conversation();
    assert_eq!(
        compressor
            .compress_batch(&mut contents, vec![], &CompressPolicy::default())
            .unwrap_err(),
        "ranges must not be empty"
    );
    let blank = CompressCall {
        summary: "  ".into(),
        ..fold(1, 5)
    };
    assert_eq!(
        compressor.compress(&mut contents, blank).unwrap_err(),
        "summary must not be empty"
    );
    assert_eq!(
        compressor.compress(&mut contents, fold(5, 5)).unwrap_err(),
        "invalid range 5..5 for 12 turns"
    );
    assert_eq!(
        compressor
            .compress(&mut contents, fold(10, 13))
            .unwrap_err(),
        "invalid range 10..13 for 12 turns"
    );
    assert_eq!(contents, conversation());
}

#[test]
fn a_spill_failure_on_the_first_fold_changes_nothing() {
    let (dir, mut compressor) = fresh();
    // A regular file where the blocks directory must be makes every spill fail.
    std::fs::write(dir.path().join("blocks"), b"not a directory").expect("blocker file");
    let mut contents = conversation();
    let before = contents.clone();
    let error = compressor
        .compress_batch(
            &mut contents,
            vec![fold(1, 3), fold(6, 8)],
            &CompressPolicy::default(),
        )
        .expect_err("spill fails");
    assert!(error.starts_with("spill dir"), "{error}");
    assert_eq!(contents, before);
    assert_eq!(compressor.status().blocks, 0);
}

#[test]
fn a_failed_spill_midway_through_a_batch_changes_nothing() {
    let (dir, mut compressor) = fresh();
    // The batch folds the higher range first, as block b2, then the lower one as b1. A
    // directory named b1.json makes b1's rename fail after b2 is already spilled and spliced.
    std::fs::create_dir_all(dir.path().join("blocks/b1.json")).expect("obstacle");
    let mut contents = conversation();
    let before = contents.clone();
    let error = compressor
        .compress_batch(
            &mut contents,
            vec![fold(1, 3), fold(6, 8)],
            &CompressPolicy::default(),
        )
        .expect_err("spill fails");
    assert!(error.starts_with("spill rename"), "{error}");
    assert_eq!(
        contents, before,
        "history is unchanged after a failed batch"
    );
    assert_eq!(
        compressor.status().blocks,
        0,
        "no block is stored after a failed batch"
    );
    assert!(
        !dir.path().join("blocks/b2.json").exists(),
        "the block spilled before the failure is removed"
    );
    // The obstacle goes away: the same batch now folds as if the failure never happened.
    std::fs::remove_dir(dir.path().join("blocks/b1.json")).expect("remove obstacle");
    let reloaded = NativeCompressor::new(dir.path()).expect("reload");
    assert_eq!(reloaded.status().blocks, 0, "no block survives a restart");
    let mut retry = conversation();
    let report = compressor
        .compress_batch(
            &mut retry,
            vec![fold(1, 3), fold(6, 8)],
            &CompressPolicy::default(),
        )
        .expect("retry succeeds");
    assert!(
        report.contains("block b1") && report.contains("block b2"),
        "{report}"
    );
}
