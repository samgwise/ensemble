//! Loop prevention via origin tag tracking.
//!
//! Each bridge instance generates a unique `bridge_id` at startup.
//! Every outbound action is stamped with this ID as its `origin`.
//! Inbound actions whose `origin` matches the local `bridge_id` are
//! dropped — they have looped back.

use uuid::Uuid;

/// Manages loop prevention for a bridge instance.
pub struct LoopGuard {
    /// This bridge's unique identifier.
    bridge_id: String,
}

impl LoopGuard {
    /// Create a new loop guard with a random bridge ID.
    pub fn new() -> Self {
        Self {
            bridge_id: Uuid::new_v4().to_string(),
        }
    }

    /// Create a loop guard with a specific bridge ID (for testing).
    #[allow(dead_code)]
    pub fn with_id(bridge_id: String) -> Self {
        Self { bridge_id }
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
}