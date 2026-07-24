//! Param state cache for replay to newly connected peers.
//!
//! The local hub automatically replays current param values to a voice when it
//! subscribes to matching patterns. The bridge caches those params (and all
//! subsequent updates) so it can replay them to a new remote peer when it
//! connects.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ensemble_core::protocol::WireMessage;
use tokio::sync::mpsc;

use crate::mapping::{Direction, MappingEngine};
use crate::protocol;

/// Thread-safe cache of the latest param value per address.
#[derive(Clone)]
pub struct ParamCache {
    inner: Arc<Mutex<HashMap<String, WireMessage>>>,
}

impl Default for ParamCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ParamCache {
    /// Create a new empty param cache.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Update the cache with a hub action.
    ///
    /// Only param-type actions are stored, keyed by address. All other actions
    /// are ignored.
    pub fn update(&self, action: &WireMessage) {
        if let Some(signal_type) = protocol::get_signal_type(action) {
            if signal_type == "param" {
                if let Some(address) = protocol::get_address(action) {
                    let mut params = self.inner.lock().unwrap();
                    params.insert(address, action.clone());
                }
            }
        }
    }

    /// Remove a param from the cache (e.g. on unset_param).
    pub fn remove(&self, address: &str) {
        let mut params = self.inner.lock().unwrap();
        params.remove(address);
    }

    /// Send cached param values to a single peer.
    ///
    /// Each cached param is filtered by the outbound mapping rules, translated
    /// to a bridge_action with the local bridge as origin, and sent to the
    /// provided channel.
    pub async fn replay(
        &self,
        engine: &MappingEngine,
        origin: &str,
        sender: mpsc::Sender<WireMessage>,
    ) {
        let params = {
            let params = self.inner.lock().unwrap();
            params.values().cloned().collect::<Vec<_>>()
        };

        for action in params {
            if let Some(bridge_msg) = build_outbound_bridge_action(&action, engine, origin) {
                if sender.send(bridge_msg).await.is_err() {
                    // Peer session ended before replay could complete.
                    break;
                }
            }
        }
    }
}

/// Build a bridge_action message from a hub action, applying outbound mapping.
///
/// Returns `None` if the address does not match any outbound mapping rule.
fn build_outbound_bridge_action(
    action: &WireMessage,
    engine: &MappingEngine,
    origin: &str,
) -> Option<WireMessage> {
    use ensemble_core::protocol::SignalType;

    let address = protocol::get_address(action)?;
    let signal_type_str = protocol::get_signal_type(action).unwrap_or_else(|| "event".to_string());
    let timestamp = protocol::get_timestamp(action);
    let source = protocol::get_source(action);
    let payload = protocol::get_action_payload(action);

    let signal_type = match signal_type_str.as_str() {
        "param" => SignalType::Param,
        "stream" => SignalType::Stream,
        _ => SignalType::Event,
    };

    let mapped_address = engine.map(&address, Direction::Outbound, Some(&signal_type_str))?;
    Some(protocol::bridge_action(
        origin,
        source,
        &mapped_address,
        signal_type,
        timestamp,
        payload,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ensemble_core::protocol::{action_with_source, SignalType, Value};
    use ensemble_values::FloatValue;

    fn make_test_config(from: &str, to: &str) -> crate::config::MappingConfig {
        crate::config::MappingConfig {
            from_pattern: from.to_string(),
            to_template: to.to_string(),
            direction: "outbound".to_string(),
            signal_filter: vec![],
        }
    }

    #[test]
    fn cache_stores_param() {
        let cache = ParamCache::new();
        let action = action_with_source(
            1,
            "/track/1/volume",
            SignalType::Param,
            10.0,
            Value::Float(FloatValue::new(0.5)),
        );
        cache.update(&action);
        assert_eq!(cache.inner.lock().unwrap().len(), 1);
    }

    #[test]
    fn cache_ignores_event() {
        let cache = ParamCache::new();
        let action = action_with_source(1, "/track/1/volume", SignalType::Event, 10.0, Value::Null);
        cache.update(&action);
        assert!(cache.inner.lock().unwrap().is_empty());
    }

    #[test]
    fn remove_clears_param() {
        let cache = ParamCache::new();
        let action = action_with_source(
            1,
            "/track/1/volume",
            SignalType::Param,
            10.0,
            Value::Float(FloatValue::new(0.5)),
        );
        cache.update(&action);
        cache.remove("/track/1/volume");
        assert!(cache.inner.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn replay_sends_mapped_params() {
        let engine =
            MappingEngine::new(&[make_test_config("/track/{id}/volume", "/mixer/{id}/gain")]);
        let cache = ParamCache::new();
        cache.update(&action_with_source(
            1,
            "/track/1/volume",
            SignalType::Param,
            10.0,
            Value::Float(FloatValue::new(0.5)),
        ));

        let (tx, mut rx) = mpsc::channel(10);
        cache.replay(&engine, "bridge-aaa", tx).await;

        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.msg_type, protocol::MSG_BRIDGE_ACTION);
        assert_eq!(protocol::get_origin(&msg), Some("bridge-aaa".to_string()));
        assert_eq!(
            protocol::get_address(&msg),
            Some("/mixer/1/gain".to_string())
        );
        assert_eq!(protocol::get_signal_type(&msg), Some("param".to_string()));
    }

    #[tokio::test]
    async fn replay_skips_unmapped_params() {
        let engine =
            MappingEngine::new(&[make_test_config("/track/{id}/volume", "/mixer/{id}/gain")]);
        let cache = ParamCache::new();
        cache.update(&action_with_source(
            1,
            "/other/value",
            SignalType::Param,
            10.0,
            Value::Float(FloatValue::new(0.5)),
        ));

        let (tx, mut rx) = mpsc::channel(10);
        cache.replay(&engine, "bridge-aaa", tx).await;

        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn replay_uses_latest_value() {
        let engine =
            MappingEngine::new(&[make_test_config("/track/{id}/volume", "/mixer/{id}/gain")]);
        let cache = ParamCache::new();
        cache.update(&action_with_source(
            1,
            "/track/1/volume",
            SignalType::Param,
            10.0,
            Value::Float(FloatValue::new(0.5)),
        ));
        cache.update(&action_with_source(
            2,
            "/track/1/volume",
            SignalType::Param,
            11.0,
            Value::Float(FloatValue::new(0.7)),
        ));

        let (tx, mut rx) = mpsc::channel(10);
        cache.replay(&engine, "bridge-aaa", tx).await;

        let msg = rx.recv().await.unwrap();
        assert_eq!(protocol::get_source(&msg), 2);
        assert_eq!(
            protocol::get_action_payload(&msg),
            Value::Float(FloatValue::new(0.7))
        );
    }
}
