//! Loop prevention via origin tag tracking and duplicate suppression.
//!
//! Each bridge instance generates a unique `bridge_id` at startup.
//! Every outbound action is stamped with this ID as its `origin`.
//! Inbound actions whose `origin` matches the local `bridge_id` are
//! dropped — they have looped back.
//!
//! Because actions are re-forwarded to other peers with their origin
//! preserved (multi-hop propagation), a message can also reach a bridge it
//! has already visited by a different path. Every message carries a unique
//! `msg_id`; the guard remembers recently seen ids so each message is
//! processed at most once per bridge, which both prevents duplicates and
//! guarantees flooding terminates in cyclic topologies.

use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;

use uuid::Uuid;

/// Maximum number of message ids remembered for duplicate suppression.
/// Older ids are evicted FIFO; a message still circulating after this many
/// newer messages could theoretically be reprocessed once, which is
/// vanishingly unlikely in practice.
const SEEN_CAP: usize = 4096;

/// Bounded FIFO set of recently seen message ids.
struct SeenSet {
    order: VecDeque<String>,
    set: HashSet<String>,
}

impl SeenSet {
    fn new() -> Self {
        Self {
            order: VecDeque::new(),
            set: HashSet::new(),
        }
    }
}

/// Manages loop prevention for a bridge instance.
pub struct LoopGuard {
    /// This bridge's unique identifier.
    bridge_id: String,
    /// Recently seen message ids for duplicate suppression.
    seen: Mutex<SeenSet>,
}

impl LoopGuard {
    /// Create a new loop guard with a random bridge ID.
    pub fn new() -> Self {
        Self {
            bridge_id: Uuid::new_v4().to_string(),
            seen: Mutex::new(SeenSet::new()),
        }
    }

    /// Create a loop guard with a specific bridge ID (for testing).
    #[allow(dead_code)]
    pub fn with_id(bridge_id: String) -> Self {
        Self {
            bridge_id,
            seen: Mutex::new(SeenSet::new()),
        }
    }

    /// This bridge's unique identifier.
    pub fn bridge_id(&self) -> &str {
        &self.bridge_id
    }

    /// Check whether an inbound action has looped back.
    ///
    /// Returns `true` if the action's origin matches this bridge's ID,
    /// meaning it should be dropped.
    pub fn is_loop(&self, origin: &str) -> bool {
        origin == self.bridge_id
    }

    /// Record a message id, returning `true` if it is new (first sighting)
    /// and `false` if it is a duplicate that should be dropped.
    ///
    /// The check and insertion are atomic under the lock so racing peer
    /// sessions cannot both accept the same message.
    pub fn check_and_record(&self, msg_id: &str) -> bool {
        let mut seen = self.seen.lock().unwrap();
        if seen.set.contains(msg_id) {
            return false;
        }
        while seen.order.len() >= SEEN_CAP {
            if let Some(old) = seen.order.pop_front() {
                seen.set.remove(&old);
            } else {
                break;
            }
        }
        let owned = msg_id.to_string();
        seen.order.push_back(owned.clone());
        seen.set.insert(owned);
        true
    }
}

impl Default for LoopGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_ids() {
        let a = LoopGuard::new();
        let b = LoopGuard::new();
        assert_ne!(a.bridge_id(), b.bridge_id());
    }

    #[test]
    fn detects_own_origin() {
        let guard = LoopGuard::with_id("test-id".to_string());
        assert!(guard.is_loop("test-id"));
        assert!(!guard.is_loop("other-id"));
        assert!(!guard.is_loop(""));
    }

    #[test]
    fn duplicate_detection() {
        let guard = LoopGuard::new();
        assert!(guard.check_and_record("msg-1"));
        assert!(!guard.check_and_record("msg-1"));
        assert!(guard.check_and_record("msg-2"));
        assert!(!guard.check_and_record("msg-2"));
        // A different id is still accepted after duplicates.
        assert!(guard.check_and_record("msg-3"));
    }

    #[test]
    fn seen_set_evicts_oldest_at_capacity() {
        let guard = LoopGuard::new();
        for i in 0..SEEN_CAP {
            assert!(guard.check_and_record(&format!("msg-{i}")));
        }
        // The oldest id is still remembered at capacity.
        assert!(!guard.check_and_record("msg-0"));
        // Recording one more evicts the oldest.
        assert!(guard.check_and_record("msg-new"));
        assert!(guard.check_and_record("msg-0"));
        // But a recent id is still a duplicate.
        assert!(!guard.check_and_record("msg-new"));
    }
}
