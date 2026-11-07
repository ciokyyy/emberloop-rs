#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Nudge engine tests: the first-nudge gate, pressure and emergency bands, the
//! baseline reset after a compress, and decision-level replay of the reference
//! goldens in `parity/golden/` (generated from billion-context daa082ea).
//! Golden tests compare decisions (inject, over-limit, emergency), never reason
//! strings, because the reference's reason text carries its own growth numbers.

use proxy::nudge::{Nudge, NudgeConfig, NudgeKind, NudgeState};
use serde_json::Value;

const MIN_COMPRESS_RANGE: u64 = 5_000;

fn golden(name: &str) -> Value {
    let path = format!("{}/parity/golden/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).expect("golden file exists");
    serde_json::from_str(&text).expect("golden file is JSON")
}

fn observe(state: &mut NudgeState, tokens: u64, window: u64) -> Option<Nudge> {
    state.observe(tokens, window, tokens)
}

#[test]
fn first_nudge_gate_is_min_of_75000_and_half_the_window() {
    let cases = [
        (1_000_000, 75_000),
        (200_000, 75_000),
        (150_000, 75_000),
        (128_000, 64_000),
        (64_000, 32_000),
    ];
    for (window, threshold) in cases {
        let mut below = NudgeState::default();
        assert_eq!(
            observe(&mut below, threshold - 1, window),
            None,
            "window {window} below gate"
        );
        let mut at = NudgeState::default();
        let fired = observe(&mut at, threshold, window).expect("gate fires at threshold");
        assert_eq!(fired.kind, NudgeKind::First, "window {window}");
    }
}

#[test]
fn first_nudge_needs_compressible_mass() {
    let mut state = NudgeState::default();
    assert_eq!(state.observe(80_000, 200_000, MIN_COMPRESS_RANGE - 1), None);
    assert!(state.observe(80_000, 200_000, MIN_COMPRESS_RANGE).is_some());
}

#[test]
fn a_nudge_repeats_once_the_context_grew_by_fifty_thousand_tokens() {
    let window = 1_000_000;
    let mut state = NudgeState::default();
    assert_eq!(
        observe(&mut state, 75_000, window).map(|n| n.kind),
        Some(NudgeKind::First)
    );
    assert_eq!(
        observe(&mut state, 100_000, window),
        None,
        "inside the growth step"
    );
    let again = observe(&mut state, 125_000, window).expect("growth nudge");
    assert_eq!(again.kind, NudgeKind::First);
    assert_eq!(again.since_last, 50_000);
    assert!(!again.over_limit);
    assert_eq!(
        observe(&mut state, 150_000, window),
        None,
        "step starts again"
    );
}

#[test]
fn a_growth_nudge_still_needs_compressible_mass() {
    let window = 1_000_000;
    let mut state = NudgeState::default();
    assert!(state.observe(75_000, window, 75_000).is_some());
    assert_eq!(state.observe(125_000, window, MIN_COMPRESS_RANGE - 1), None);
}

#[test]
fn the_growth_reference_falls_when_the_context_shrinks() {
    let window = 1_000_000;
    let mut state = NudgeState::default();
    observe(&mut state, 75_000, window);
    assert_eq!(
        observe(&mut state, 125_000, window).map(|n| n.kind),
        Some(NudgeKind::First)
    );
    // The context shrank (a fold outside a compress, or a new conversation): no nudge,
    // but the next growth is measured from here, not from the old peak.
    assert_eq!(observe(&mut state, 40_000, window), None);
    let again = observe(&mut state, 90_000, window).expect("growth from the shrunk context");
    assert_eq!(again.since_last, 50_000);
}

#[test]
fn first_nudge_fires_once_then_pressure_fires_every_turn_while_over() {
    let window = 64_000;
    let mut state = NudgeState::default();
    assert_eq!(
        observe(&mut state, 32_000, window).map(|n| n.kind),
        Some(NudgeKind::First)
    );
    for tokens in [50_000, 51_000, 52_000] {
        let fired = observe(&mut state, tokens, window).expect("pressure fires while over");
        assert_eq!(fired.kind, NudgeKind::Pressure);
        assert!(fired.over_limit);
        assert!(!fired.emergency);
    }
    assert_eq!(
        observe(&mut state, 40_000, window),
        None,
        "stops when under 75 percent"
    );
}

#[test]
fn emergency_band_starts_at_95_percent() {
    let window = 64_000;
    let mut state = NudgeState::default();
    observe(&mut state, 32_000, window);
    let at_94 = observe(&mut state, 60_000, window).expect("pressure at 94 percent");
    assert_eq!(at_94.kind, NudgeKind::Pressure);
    let at_95 = observe(&mut state, 60_800, window).expect("emergency at 95 percent");
    assert_eq!(at_95.kind, NudgeKind::Emergency);
    assert!(at_95.emergency && at_95.over_limit);
}

#[test]
fn the_bands_win_over_the_first_nudge_when_mass_allows_it() {
    let window = 64_000;
    let mut state = NudgeState::default();
    // Compressible mass is too small for any nudge, so the gate stays shut even
    // though usage is far over the emergency band.
    assert_eq!(state.observe(63_000, window, MIN_COMPRESS_RANGE - 1), None);
    // With enough mass the band nudge fires first: the gentle first nudge never
    // comes ahead of the emergency that the usage already calls for.
    let fired = state
        .observe(63_000, window, MIN_COMPRESS_RANGE)
        .expect("gate fires");
    assert_eq!(fired.kind, NudgeKind::Emergency);
}

#[test]
fn compress_closes_the_gate_and_the_bands_win_over_the_first_nudge_again() {
    let window = 64_000;
    let mut state = NudgeState::default();
    observe(&mut state, 32_000, window);
    state.reset_after_compress();
    // 62000 is 97 percent of the window: emergency, not the gentle first nudge.
    let fired = observe(&mut state, 62_000, window).expect("gate reopens");
    assert_eq!(fired.kind, NudgeKind::Emergency);
    assert_eq!(
        observe(&mut state, 62_500, window).map(|n| n.kind),
        Some(NudgeKind::Emergency)
    );
}

#[test]
fn after_a_compress_the_first_nudge_still_fires_below_the_bands() {
    let window = 64_000;
    let mut state = NudgeState::default();
    observe(&mut state, 32_000, window);
    state.reset_after_compress();
    // 40000 is 62 percent of the window: under the pressure band, so the first nudge.
    let fired = observe(&mut state, 40_000, window).expect("gate reopens");
    assert_eq!(fired.kind, NudgeKind::First);
    assert!(!fired.over_limit);
}

#[test]
fn state_round_trips_through_json_so_a_restart_keeps_the_gate() {
    let window = 64_000;
    let mut state = NudgeState::default();
    observe(&mut state, 32_000, window);
    let text = serde_json::to_string(&state).expect("serialize");
    let mut restored: NudgeState = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(
        observe(&mut restored, 50_000, window).map(|n| n.kind),
        Some(NudgeKind::Pressure)
    );
}

/// Replays one golden through the engine and checks every reference frame that
/// the reference marks over the limit. Compressible mass is the token count,
/// except in the protected scenario where the reference has no compressible mass.
fn replay(name: &str, compressible_is_zero: bool) {
    let g = golden(name);
    let window = g["windowTokens"].as_u64().expect("windowTokens");
    let mut state = NudgeState::default();
    for frame in g["frames"].as_array().expect("frames") {
        let tokens = frame["tokenCount"].as_u64().expect("tokenCount");
        let mass = if compressible_is_zero { 0 } else { tokens };
        let ours = state.observe(tokens, window, mass);
        let reference_over = frame["overLimit"].as_u64().unwrap_or(0) == 1;
        let reference_emergency = frame["emergencyOverride"].as_u64().unwrap_or(0) == 1;
        if compressible_is_zero {
            assert_eq!(
                ours, None,
                "{name} step {}: protected tool must never nudge",
                frame["step"]
            );
            continue;
        }
        if reference_over {
            let n = ours.unwrap_or_else(|| {
                panic!(
                    "{name} step {}: reference over limit, we stayed silent",
                    frame["step"]
                )
            });
            assert!(n.over_limit, "{name} step {}", frame["step"]);
            assert_eq!(
                n.emergency, reference_emergency,
                "{name} step {}",
                frame["step"]
            );
        }
    }
}

#[test]
fn golden_pressure_decisions_match_the_reference() {
    replay("window-64k-pressure", false);
}

#[test]
fn golden_compress_reset_decisions_match_the_reference() {
    replay("window-64k-compress-reset", false);
}

#[test]
fn golden_blocks_decisions_match_the_reference() {
    replay("window-64k-blocks", false);
}

#[test]
fn tiers_fire_once_when_the_block_count_reaches_their_trigger() {
    let window = 64_000;
    let mut state = NudgeState::default();
    state.observe(32_000, window, 32_000);
    let config = NudgeConfig::default();
    assert_eq!(
        state
            .observe_with(40_000, window, 20_000, 1_000, &config)
            .map(|nudge| nudge.kind),
        Some(NudgeKind::Tier2)
    );
    assert_eq!(
        state.observe_with(40_000, window, 20_000, 1_500, &config),
        None,
        "tier 2 fires once"
    );
    assert_eq!(
        state
            .observe_with(40_000, window, 20_000, 2_000, &config)
            .map(|nudge| nudge.kind),
        Some(NudgeKind::Tier3)
    );
}

#[test]
fn settings_move_the_pressure_and_emergency_bands() {
    let window = 100_000;
    let config = NudgeConfig {
        max_context_limit_pct: 50,
        emergency_threshold_pct: 80,
        min_pressure_benefit_tokens: 100,
        ..NudgeConfig::default()
    };
    let mut state = NudgeState::default();
    state.observe_with(50_000, window, 50_000, 0, &config);
    assert_eq!(
        state
            .observe_with(60_000, window, 10_000, 0, &config)
            .map(|nudge| nudge.kind),
        Some(NudgeKind::Pressure)
    );
    assert_eq!(
        state
            .observe_with(80_000, window, 10_000, 0, &config)
            .map(|nudge| nudge.kind),
        Some(NudgeKind::Emergency)
    );
    assert_eq!(
        state.observe_with(80_001, window, 99, 0, &config),
        None,
        "the configured floor is enforced"
    );
}

#[test]
fn golden_protected_tool_never_nudges() {
    replay("window-64k-protected", true);
}

#[test]
fn golden_steady_scenarios_have_no_pressure_frames() {
    // The reference never reaches 75 percent in these, so the engine must not
    // report an over-limit nudge for any frame of them.
    for name in ["window-64k-steady"] {
        let g = golden(name);
        let window = g["windowTokens"].as_u64().expect("windowTokens");
        let mut state = NudgeState::default();
        for frame in g["frames"].as_array().expect("frames") {
            let tokens = frame["tokenCount"].as_u64().expect("tokenCount");
            if let Some(n) = state.observe(tokens, window, tokens) {
                assert!(
                    !n.over_limit,
                    "{name} step {}: over-limit nudge below 75 percent",
                    frame["step"]
                );
            }
        }
    }
}

#[test]
fn a_nudge_reports_the_growth_since_the_previous_nudge() {
    let window = 64_000;
    let mut state = NudgeState::default();
    assert_eq!(
        observe(&mut state, 32_000, window).map(|n| n.since_last),
        Some(32_000)
    );
    assert_eq!(
        observe(&mut state, 50_000, window).map(|n| n.since_last),
        Some(18_000)
    );
}

#[test]
fn state_is_saved_by_rename_and_a_missing_or_broken_file_loads_the_default() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("nudge.json");
    assert_eq!(proxy::nudge::load(&path), NudgeState::default());
    let mut state = NudgeState::default();
    observe(&mut state, 32_000, 64_000);
    proxy::nudge::save(&path, &state).expect("save");
    assert_eq!(proxy::nudge::load(&path), state);
    assert!(
        !path.with_extension("json.tmp").exists(),
        "the temp file is renamed away"
    );
    std::fs::write(&path, "{not json").expect("write");
    assert_eq!(proxy::nudge::load(&path), NudgeState::default());
}

const SCENARIOS: [&str; 7] = [
    "window-64k-steady",
    "window-64k-pressure",
    "window-64k-blocks",
    "window-64k-compress-reset",
    "window-64k-protected",
    "window-200k-steady",
    "window-1m-steady",
];

#[derive(Default)]
struct Replay {
    /// The engine's first nudge: step, tokens, kind.
    first: Option<(u64, u64, &'static str)>,
    /// Frames where the engine nudged and the reference's `shouldInject` is false.
    early: Vec<(u64, &'static str)>,
}

/// Replays a golden through the engine. Compressible mass is the token count, zero for
/// the protected scenario. The golden records each compress, and its frames already show
/// the folded count, so the engine resets its gate before the frame of that step. Tier
/// nudges are not replayed: the goldens carry no block count per frame.
fn replay_frames(name: &str) -> Replay {
    let g = golden(name);
    let window = g["windowTokens"].as_u64().expect("windowTokens");
    let protected = name == "window-64k-protected";
    let compress_steps: Vec<u64> = g["compresses"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| c["step"].as_u64().expect("compress step"))
        .collect();
    let mut state = NudgeState::default();
    let mut replay = Replay::default();
    for frame in g["frames"].as_array().expect("frames") {
        let step = frame["step"].as_u64().expect("step");
        let tokens = frame["tokenCount"].as_u64().expect("tokenCount");
        if compress_steps.contains(&step) {
            state.reset_after_compress();
        }
        let mass = if protected { 0 } else { tokens };
        let Some(nudge) = state.observe(tokens, window, mass) else {
            continue;
        };
        replay
            .first
            .get_or_insert((step, tokens, nudge.kind.name()));
        if frame["shouldInject"] == Value::Bool(false) {
            replay.early.push((step, nudge.kind.name()));
        }
    }
    replay
}

/// The reference's first injection in a golden: step and tokens.
fn reference_first(name: &str) -> Option<(u64, u64)> {
    golden(name)["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .find(|frame| frame["shouldInject"] == Value::Bool(true))
        .map(|frame| {
            (
                frame["step"].as_u64().expect("step"),
                frame["tokenCount"].as_u64().expect("tokenCount"),
            )
        })
}

/// Frames where the engine nudges and the reference stays silent, as `(scenario, step,
/// kind)`. Every one is the first-nudge threshold this project keeps on purpose (ticket
/// 20): the engine fires at `min(75000, window / 2)`, so at 64k at 32000 tokens, the
/// session's first nudge or the first after a compress. The reference waits for its own
/// first at 48055 (64k), 51855 (200k) or 50255 (1M). Each frame has compressible mass of
/// at least 30000, far above the floor, so the token substitution is not what fires it.
/// Any other early frame, or one of these going quiet, fails the test.
///
/// - `pressure` 42: the session's first nudge at 32555 tokens, before the reference's 48055.
/// - `blocks` 47, 52, 70 and `compress-reset` 42, 70: the gate reopens at 32000 tokens,
///   after a compress or as the session's first (`blocks` 47 follows the compress at 30,
///   52 the one at 50, 70 the one at 70; `compress-reset` 42 is its first, 70 follows its
///   compress at 70). The reference waits for 48000 tokens.
/// - `200k` 50: the first nudge at 76255 tokens; the reference fired at 51855 and 74730.
const KNOWN_EARLY: &[(&str, u64, &str)] = &[
    ("window-64k-pressure", 42, "first"),
    ("window-64k-blocks", 47, "first"),
    ("window-64k-blocks", 52, "first"),
    ("window-64k-blocks", 70, "first"),
    ("window-64k-compress-reset", 42, "first"),
    ("window-64k-compress-reset", 70, "first"),
    ("window-200k-steady", 50, "first"),
];

#[test]
fn the_engine_stays_silent_on_every_reference_silent_frame() {
    let mut early = Vec::new();
    for name in SCENARIOS {
        for (step, kind) in replay_frames(name).early {
            early.push((name, step, kind));
        }
    }
    assert_eq!(early, KNOWN_EARLY);
}

/// The first nudge of each golden: the engine's `(step, tokens, kind)` beside the
/// reference's `(step, tokens)`. Pins both counts, so a golden or threshold change shows.
const FIRST_FIRES: &[(&str, Option<(u64, u64, &str)>, Option<(u64, u64)>)] = &[
    ("window-64k-steady", None, None),
    (
        "window-64k-pressure",
        Some((42, 32555, "first")),
        Some((62, 48055)),
    ),
    (
        "window-64k-blocks",
        Some((47, 32592, "first")),
        Some((77, 48096)),
    ),
    (
        "window-64k-compress-reset",
        Some((42, 32555, "first")),
        Some((62, 48055)),
    ),
    ("window-64k-protected", None, None),
    (
        "window-200k-steady",
        Some((50, 76255, "first")),
        Some((34, 51855)),
    ),
    (
        "window-1m-steady",
        Some((15, 75380, "first")),
        Some((10, 50255)),
    ),
];

#[test]
fn the_first_nudge_counts_are_recorded_against_the_reference() {
    let actual: Vec<_> = SCENARIOS
        .iter()
        .map(|name| (*name, replay_frames(name).first, reference_first(name)))
        .collect();
    let expected: Vec<_> = FIRST_FIRES
        .iter()
        .map(|(name, engine, reference)| (*name, *engine, *reference))
        .collect();
    assert_eq!(actual, expected);
}
