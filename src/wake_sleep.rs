//! Wake/sleep engine for per-thread inactivity management.
//!
//! Tracks whether each Slack thread is currently awake (bot will respond) or
//! sleeping (bot stays quiet until @mentioned). Transitions are driven by
//! inbound events; the caller is responsible for spawning inactivity timers
//! and calling [`WakeSleepEngine::mark_sleeping`] on expiry.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

/// Inactivity timeout before a thread transitions to sleeping.
pub const INACTIVITY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3600);

/// Hard cap on tracked threads. New threads beyond this limit are treated as
/// awake but not inserted, preventing unbounded allocations.
const MAX_ENTRIES: usize = 10_000;

/// Decision returned by [`WakeSleepEngine::on_event`].
#[derive(Debug, PartialEq, Eq)]
pub enum EventDecision {
    /// Thread is awake — process this event normally.
    Forward,
    /// Thread was sleeping and this @mention woke it — process the event.
    Wake,
    /// Thread is sleeping and this event is not an @mention — discard it.
    Discard,
}

/// Per-thread wake state.
enum WakeState {
    Awake { last_activity: Instant },
    Sleeping,
}

/// Per-thread wake/sleep state tracker.
///
/// All state is protected by an internal `Mutex` so the engine can be shared
/// via `Arc` and called from `&self` contexts (e.g. `dispatch_envelope`).
pub struct WakeSleepEngine {
    states: Mutex<HashMap<String, WakeState>>,
}

impl WakeSleepEngine {
    pub fn new() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
        }
    }

    /// Process an inbound event for the given thread.
    ///
    /// - Unknown threads start as awake.
    /// - Awake threads refresh their `last_activity` on any event.
    /// - Sleeping threads wake only on `is_mention = true`.
    pub fn on_event(&self, thread_key: &str, is_mention: bool) -> EventDecision {
        let mut states = self.states.lock().expect("wake_sleep mutex poisoned");

        match states.get(thread_key) {
            None => {
                if states.len() >= MAX_ENTRIES {
                    tracing::warn!(
                        capacity = MAX_ENTRIES,
                        "WakeSleepEngine: at capacity; treating new thread as awake without tracking"
                    );
                    return EventDecision::Forward;
                }
                states.insert(
                    thread_key.to_string(),
                    WakeState::Awake {
                        last_activity: Instant::now(),
                    },
                );
                EventDecision::Forward
            }
            Some(WakeState::Awake { .. }) => {
                states.insert(
                    thread_key.to_string(),
                    WakeState::Awake {
                        last_activity: Instant::now(),
                    },
                );
                EventDecision::Forward
            }
            Some(WakeState::Sleeping) => {
                if is_mention {
                    states.insert(
                        thread_key.to_string(),
                        WakeState::Awake {
                            last_activity: Instant::now(),
                        },
                    );
                    EventDecision::Wake
                } else {
                    EventDecision::Discard
                }
            }
        }
    }

    /// Transition a thread to the sleeping state.
    ///
    /// Called by the inactivity timer spawned in `slack.rs` on expiry.
    /// No-op for unknown threads: timers are only spawned for tracked threads,
    /// so an untracked key here means the thread was over-capacity and was
    /// never inserted — silently dropping the transition is correct.
    pub fn mark_sleeping(&self, thread_key: &str) {
        let mut states = self.states.lock().expect("wake_sleep mutex poisoned");
        if states.contains_key(thread_key) {
            states.insert(thread_key.to_string(), WakeState::Sleeping);
        } else {
            tracing::debug!(
                thread_key,
                "WakeSleepEngine: mark_sleeping called for untracked thread (capacity drop path)"
            );
        }
    }

    /// Return whether a thread is currently awake.
    #[cfg(test)]
    pub fn is_awake(&self, thread_key: &str) -> bool {
        let states = self.states.lock().expect("wake_sleep mutex poisoned");
        matches!(states.get(thread_key), Some(WakeState::Awake { .. }))
    }
}

impl Default for WakeSleepEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> WakeSleepEngine {
        WakeSleepEngine::new()
    }

    #[test]
    fn unknown_thread_is_forwarded() {
        assert_eq!(engine().on_event("ch:ts", false), EventDecision::Forward);
    }

    #[test]
    fn awake_thread_non_mention_is_forwarded() {
        let e = engine();
        e.on_event("ch:ts", false);
        assert_eq!(e.on_event("ch:ts", false), EventDecision::Forward);
    }

    #[test]
    fn awake_thread_mention_is_forwarded() {
        let e = engine();
        e.on_event("ch:ts", false);
        assert_eq!(e.on_event("ch:ts", true), EventDecision::Forward);
    }

    #[test]
    fn sleeping_thread_non_mention_is_discarded() {
        let e = engine();
        e.on_event("ch:ts", false); // awake
        e.mark_sleeping("ch:ts");
        assert_eq!(e.on_event("ch:ts", false), EventDecision::Discard);
    }

    #[test]
    fn sleeping_thread_mention_wakes_and_returns_wake() {
        let e = engine();
        e.on_event("ch:ts", false); // awake
        e.mark_sleeping("ch:ts");
        assert_eq!(e.on_event("ch:ts", true), EventDecision::Wake);
    }

    #[test]
    fn woken_thread_is_now_awake() {
        let e = engine();
        e.on_event("ch:ts", false); // track the thread first
        e.mark_sleeping("ch:ts");
        e.on_event("ch:ts", true); // wake
        assert!(e.is_awake("ch:ts"));
    }

    #[test]
    fn mark_sleeping_untracked_thread_is_noop() {
        let e = engine();
        // Never register via on_event — thread is untracked (over-capacity path).
        e.mark_sleeping("ch:never_seen");
        // on_event must return Forward (not Discard) because the thread must not
        // have been silently inserted as Sleeping by mark_sleeping.
        assert_eq!(e.on_event("ch:never_seen", false), EventDecision::Forward);
    }

    #[test]
    fn mark_sleeping_transitions_awake_thread() {
        let e = engine();
        e.on_event("ch:ts", false); // awake
        assert!(e.is_awake("ch:ts"));
        e.mark_sleeping("ch:ts");
        assert!(!e.is_awake("ch:ts"));
    }

    #[test]
    fn multiple_threads_are_independent() {
        let e = engine();
        e.on_event("ch:ts1", false); // awake
        e.on_event("ch:ts2", false); // awake
        e.mark_sleeping("ch:ts1");

        assert!(!e.is_awake("ch:ts1"));
        assert!(e.is_awake("ch:ts2"));
        assert_eq!(e.on_event("ch:ts1", false), EventDecision::Discard);
        assert_eq!(e.on_event("ch:ts2", false), EventDecision::Forward);
    }
}
