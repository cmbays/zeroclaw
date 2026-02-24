use parking_lot::RwLock;
use std::collections::HashMap;

/// Hard cap on tracked threads. At ~150 bytes/entry this is ~1.5 MB max.
const MAX_ENTRIES: usize = 10_000;

/// Per-thread mode tracking. Maps thread_ts to the set of active mode names.
///
/// Modes are additive: activating `[devops]` in a thread that already has `[pm]`
/// active results in both modes responding to subsequent messages.
///
/// **Eviction**: Entries persist until process restart. Each entry is ~150 bytes.
/// For typical usage (hundreds of threads/day) this is negligible.
pub struct ThreadModeState {
    active_modes: RwLock<HashMap<String, Vec<String>>>,
}

impl ThreadModeState {
    pub fn new() -> Self {
        Self {
            active_modes: RwLock::new(HashMap::new()),
        }
    }

    /// Return all modes currently active on the given thread.
    pub fn get_modes(&self, thread_ts: &str) -> Vec<String> {
        self.active_modes
            .read()
            .get(thread_ts)
            .cloned()
            .unwrap_or_default()
    }

    /// Add a mode to the active set for a thread. No-ops if already present.
    /// Capacity check is per-thread (new threads beyond MAX_ENTRIES are dropped;
    /// adding a new mode to an existing thread always succeeds).
    pub fn add_mode(&self, thread_ts: &str, mode_name: String) {
        let mut map = self.active_modes.write();
        if let Some(modes) = map.get_mut(thread_ts) {
            if !modes.contains(&mode_name) {
                modes.push(mode_name);
            }
            return;
        }
        // New thread — enforce capacity ceiling.
        if map.len() >= MAX_ENTRIES {
            tracing::warn!(
                capacity = MAX_ENTRIES,
                "ThreadModeState: at capacity; dropping mode for new thread"
            );
            return;
        }
        map.insert(thread_ts.to_string(), vec![mode_name]);
    }

    #[cfg(test)]
    pub fn clear_mode(&self, thread_ts: &str) {
        self.active_modes.write().remove(thread_ts);
    }

    #[cfg(test)]
    pub fn active_count(&self) -> usize {
        self.active_modes.read().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_get_single_mode() {
        let state = ThreadModeState::new();
        state.add_mode("ts_123", "pm".to_string());
        assert_eq!(state.get_modes("ts_123"), vec!["pm"]);
    }

    #[test]
    fn add_multiple_modes_are_cumulative() {
        let state = ThreadModeState::new();
        state.add_mode("ts_1", "pm".to_string());
        state.add_mode("ts_1", "devops".to_string());
        let modes = state.get_modes("ts_1");
        assert!(modes.contains(&"pm".to_string()));
        assert!(modes.contains(&"devops".to_string()));
        assert_eq!(modes.len(), 2);
    }

    #[test]
    fn add_duplicate_mode_is_idempotent() {
        let state = ThreadModeState::new();
        state.add_mode("ts_1", "pm".to_string());
        state.add_mode("ts_1", "pm".to_string());
        assert_eq!(state.get_modes("ts_1"), vec!["pm"]);
    }

    #[test]
    fn get_modes_unknown_thread_returns_empty() {
        let state = ThreadModeState::new();
        assert!(state.get_modes("ts_unknown").is_empty());
    }

    #[test]
    fn clear_mode_removes_all_modes_for_thread() {
        let state = ThreadModeState::new();
        state.add_mode("ts_123", "pm".to_string());
        state.add_mode("ts_123", "devops".to_string());
        state.clear_mode("ts_123");
        assert!(state.get_modes("ts_123").is_empty());
    }

    #[test]
    fn active_count_tracks_unique_threads() {
        let state = ThreadModeState::new();
        assert_eq!(state.active_count(), 0);
        state.add_mode("ts_1", "pm".to_string());
        state.add_mode("ts_1", "devops".to_string()); // same thread
        state.add_mode("ts_2", "ops".to_string());
        assert_eq!(state.active_count(), 2); // 2 threads
        state.clear_mode("ts_1");
        assert_eq!(state.active_count(), 1);
    }

    #[test]
    fn add_mode_at_capacity_drops_new_thread() {
        let state = ThreadModeState::new();
        for i in 0..MAX_ENTRIES {
            state.add_mode(&format!("ts_{i}"), "pm".to_string());
        }
        assert_eq!(state.active_count(), MAX_ENTRIES);
        state.add_mode("ts_overflow", "pm".to_string());
        assert!(
            state.get_modes("ts_overflow").is_empty(),
            "new thread beyond capacity must be dropped"
        );
        assert_eq!(state.active_count(), MAX_ENTRIES, "count must not increase");
    }

    #[test]
    fn add_mode_at_capacity_allows_new_mode_on_existing_thread() {
        let state = ThreadModeState::new();
        state.add_mode("ts_existing", "pm".to_string());
        for i in 0..MAX_ENTRIES - 1 {
            state.add_mode(&format!("ts_{i}"), "ops".to_string());
        }
        assert_eq!(state.active_count(), MAX_ENTRIES);
        // Adding a second mode to an already-tracked thread must succeed even at capacity.
        state.add_mode("ts_existing", "devops".to_string());
        let modes = state.get_modes("ts_existing");
        assert!(
            modes.contains(&"devops".to_string()),
            "adding a mode to an existing thread must succeed at capacity"
        );
    }
}
