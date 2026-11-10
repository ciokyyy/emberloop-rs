//! When a Session's harness may be closed for idleness. The clock is injected so tests move
//! time by hand instead of sleeping.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a Session may sit idle before its harness and tool server are closed.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// A clock that only moves when a test advances it.
#[derive(Debug)]
pub struct ManualClock {
    now: Mutex<Instant>,
}

impl ManualClock {
    pub fn new() -> Self {
        Self {
            now: Mutex::new(Instant::now()),
        }
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *now += by;
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        *self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// What the runner knows about one Session when it asks whether to close its harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activity {
    pub turn_running: bool,
    pub steers_pending: bool,
    pub background_running: bool,
    /// The last moment the Session was used or its turn ended.
    pub last_active: Instant,
}

/// True when the Session has nothing in flight and has been quiet for at least `threshold`.
pub fn reapable(activity: Activity, now: Instant, threshold: Duration) -> bool {
    !activity.turn_running
        && !activity.steers_pending
        && !activity.background_running
        && now.saturating_duration_since(activity.last_active) >= threshold
}

#[cfg(test)]
mod tests {
    use super::*;

    const THRESHOLD: Duration = Duration::from_secs(600);

    fn quiet_since(last_active: Instant) -> Activity {
        Activity {
            turn_running: false,
            steers_pending: false,
            background_running: false,
            last_active,
        }
    }

    #[test]
    fn a_quiet_session_is_reapable_once_the_threshold_has_passed() {
        let clock = ManualClock::new();
        let activity = quiet_since(clock.now());
        clock.advance(THRESHOLD - Duration::from_secs(1));
        assert!(!reapable(activity, clock.now(), THRESHOLD));
        clock.advance(Duration::from_secs(1));
        assert!(reapable(activity, clock.now(), THRESHOLD));
    }

    #[test]
    fn a_running_turn_is_never_reapable() {
        let clock = ManualClock::new();
        let activity = Activity {
            turn_running: true,
            ..quiet_since(clock.now())
        };
        clock.advance(THRESHOLD * 6);
        assert!(!reapable(activity, clock.now(), THRESHOLD));
    }

    #[test]
    fn a_pending_steer_is_never_reapable() {
        let clock = ManualClock::new();
        let activity = Activity {
            steers_pending: true,
            ..quiet_since(clock.now())
        };
        clock.advance(THRESHOLD * 6);
        assert!(!reapable(activity, clock.now(), THRESHOLD));
    }

    #[test]
    fn a_live_background_job_is_never_reapable() {
        let clock = ManualClock::new();
        let activity = Activity {
            background_running: true,
            ..quiet_since(clock.now())
        };
        clock.advance(THRESHOLD * 6);
        assert!(!reapable(activity, clock.now(), THRESHOLD));
    }
}
