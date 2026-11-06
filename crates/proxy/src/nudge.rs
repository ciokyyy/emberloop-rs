//! Billion-context nudge gate (ticket 20). The state machine is pure: no clock, no IO.
//! `load` and `save` are the only IO, for the Session's `nudge.json`.
//!
//! The first nudge fires when context tokens reach `min(75000, window / 2)` and the
//! Session has compressible mass. Until one has fired, no pressure, emergency or tier
//! nudge can fire. After it, another nudge fires as soon as the context has grown by
//! `NUDGE_GROWTH_TOKENS` and nothing was folded, so a model that skipped one nudge is
//! asked again. Pressure fires every turn while usage is at or over
//! `maxContextLimit`, emergency over `emergencyThresholdPercent`, both only when
//! compressible mass is at least `minPressureBenefitTokens` (default
//! `max(5000, window / 100)`). Tiers fire once when the active block count reaches
//! their trigger and usage is under the pressure band. A successful compress closes
//! the gate again.
//!
//! Bands and minimums follow the reference (billion-context, `maxContextLimit` 75%,
//! `emergencyThresholdPercent` 95%, `minCompressRange` 5000, `nudgeGrowthTokens`
//! 50000). The first nudge's threshold is this project's own (ticket 20).

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Tokens at which the first nudge fires for any window of 150000 or more.
pub const FIRST_NUDGE_TOKENS: u64 = 75_000;
/// Smallest compressible mass that can carry a nudge (reference `minCompressRange`).
pub const MIN_COMPRESS_RANGE: u64 = 5_000;
/// Context growth that earns another nudge while nothing is folded (reference
/// `nudgeGrowthTokens`).
pub const NUDGE_GROWTH_TOKENS: u64 = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NudgeConfig {
    pub max_context_limit_pct: u64,
    pub emergency_threshold_pct: u64,
    /// 0 means `max(MIN_COMPRESS_RANGE, window / 100)`.
    pub min_pressure_benefit_tokens: u64,
    pub tier2_trigger: u64,
    pub tier3_trigger: u64,
}

impl Default for NudgeConfig {
    fn default() -> Self {
        Self {
            max_context_limit_pct: 75,
            emergency_threshold_pct: 95,
            min_pressure_benefit_tokens: 0,
            tier2_trigger: 1_000,
            tier3_trigger: 2_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NudgeKind {
    First,
    Pressure,
    Emergency,
    Tier2,
    Tier3,
}

impl NudgeKind {
    /// Event and prompt suffix, e.g. `tier2` or `first`.
    pub fn name(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Pressure => "pressure",
            Self::Emergency => "emergency",
            Self::Tier2 => "tier2",
            Self::Tier3 => "tier3",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nudge {
    pub kind: NudgeKind,
    /// Usage is at or over the pressure band.
    pub over_limit: bool,
    /// Usage is at or over the emergency band.
    pub emergency: bool,
    /// Tokens added since the previous nudge (all tokens for the first nudge).
    pub since_last: u64,
}
/// Per-Session nudge state. `first_fired` is the gate: it is open after the first
/// nudge and closed again by a compress.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NudgeState {
    first_fired: bool,
    #[serde(default)]
    last_nudge_tokens: u64,
    #[serde(default)]
    tier2_fired: bool,
    #[serde(default)]
    tier3_fired: bool,
}

impl NudgeState {
    /// Decides whether a nudge is due with the default bands and no blocks.
    pub fn observe(&mut self, tokens: u64, window: u64, compressible: u64) -> Option<Nudge> {
        self.observe_with(tokens, window, compressible, 0, &NudgeConfig::default())
    }

    /// Decides whether a nudge is due for this turn. `tokens` is the context size the
    /// model will see, `window` the model's context window, `compressible` the token
    /// mass the model could still fold, `active_blocks` the folds already stored.
    pub fn observe_with(
        &mut self,
        tokens: u64,
        window: u64,
        compressible: u64,
        active_blocks: u64,
        config: &NudgeConfig,
    ) -> Option<Nudge> {
        if window == 0 {
            return None;
        }
        let over_limit = at_percent(tokens, window, config.max_context_limit_pct);
        let emergency = at_percent(tokens, window, config.emergency_threshold_pct);
        let floor = pressure_floor(window, config.min_pressure_benefit_tokens);
        let kind = if !self.first_fired {
            if compressible < MIN_COMPRESS_RANGE || tokens < first_threshold(window) {
                return None;
            }
            NudgeKind::First
        } else if emergency && compressible >= floor {
            NudgeKind::Emergency
        } else if over_limit && compressible >= floor {
            NudgeKind::Pressure
        } else if compressible >= floor
            && tokens.saturating_sub(self.last_nudge_tokens) >= NUDGE_GROWTH_TOKENS
        {
            NudgeKind::First
        } else if compressible >= floor
            && active_blocks >= config.tier3_trigger
            && !self.tier3_fired
        {
            self.tier3_fired = true;
            NudgeKind::Tier3
        } else if compressible >= floor
            && active_blocks >= config.tier2_trigger
            && !self.tier2_fired
        {
            self.tier2_fired = true;
            NudgeKind::Tier2
        } else {
            return None;
        };
        let since_last = tokens.saturating_sub(self.last_nudge_tokens);
        self.first_fired = true;
        self.last_nudge_tokens = tokens;
        Some(Nudge {
            kind,
            over_limit,
            emergency,
            since_last,
        })
    }

    /// A successful compress closes the gate; the next first nudge comes before any
    /// pressure, emergency or tier nudge again.
    pub fn reset_after_compress(&mut self) {
        self.first_fired = false;
        self.tier2_fired = false;
        self.tier3_fired = false;
    }
}

/// `min(FIRST_NUDGE_TOKENS, window / 2)`, computed in integers.
fn first_threshold(window: u64) -> u64 {
    FIRST_NUDGE_TOKENS.min(window / 2)
}

/// `max(MIN_COMPRESS_RANGE, window / 100)` unless Settings pins a value.
fn pressure_floor(window: u64, configured: u64) -> u64 {
    if configured > 0 {
        configured
    } else {
        MIN_COMPRESS_RANGE.max(window / 100)
    }
}

fn at_percent(tokens: u64, window: u64, percent: u64) -> bool {
    u128::from(tokens) * 100 >= u128::from(window) * u128::from(percent)
}

/// Loads the Session nudge state. A missing or unreadable file means no nudge yet.
pub fn load(path: &Path) -> NudgeState {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Writes the state to a temp file next to `path`, then renames it over `path`, so a
/// crash never leaves a half-written file.
pub fn save(path: &Path, state: &NudgeState) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string(state).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}
