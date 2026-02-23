use parking_lot::RwLock;
use std::collections::HashMap;

/// Hard cap on tracked threads. At ~100 bytes/entry this is ~1 MB max.
const MAX_ENTRIES: usize = 10_000;

/// Per-thread mode tracking. Maps thread_ts to mode_name.
///
/// **Eviction**: Entries persist until process restart. Each entry is ~100 bytes
/// (thread_ts + mode_name). For typical usage (hundreds of threads/day) this is
/// negligible. TTL-based eviction is planned for W4B (wake/sleep).
pub struct ThreadModeState {
    active_modes: RwLock<HashMap<String, String>>,
}

impl ThreadModeState {
    pub fn new() -> Self {
        Self {
            active_modes: RwLock::new(HashMap::new()),
        }
    }

    pub fn get_mode(&self, thread_ts: &str) -> Option<String> {
        self.active_modes.read().get(thread_ts).cloned()
    }

    pub fn set_mode(&self, thread_ts: &str, mode_name: String) {
        let mut map = self.active_modes.write();
        if map.len() >= MAX_ENTRIES && !map.contains_key(thread_ts) {
            tracing::warn!(
                capacity = MAX_ENTRIES,
                "ThreadModeState: at capacity; dropping mode for new thread"
            );
            return;
        }
        map.insert(thread_ts.to_string(), mode_name);
    }

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
    fn set_and_get_mode() {
        let state = ThreadModeState::new();
        state.set_mode("ts_123", "pm".to_string());
        assert_eq!(state.get_mode("ts_123"), Some("pm".to_string()));
    }

    #[test]
    fn get_mode_unknown_thread() {
        let state = ThreadModeState::new();
        assert_eq!(state.get_mode("ts_unknown"), None);
    }

    #[test]
    fn clear_mode_removes_mapping() {
        let state = ThreadModeState::new();
        state.set_mode("ts_123", "pm".to_string());
        state.clear_mode("ts_123");
        assert_eq!(state.get_mode("ts_123"), None);
    }

    #[test]
    fn active_count_tracks_entries() {
        let state = ThreadModeState::new();
        assert_eq!(state.active_count(), 0);
        state.set_mode("ts_1", "pm".to_string());
        state.set_mode("ts_2", "ops".to_string());
        assert_eq!(state.active_count(), 2);
        state.clear_mode("ts_1");
        assert_eq!(state.active_count(), 1);
    }
}
