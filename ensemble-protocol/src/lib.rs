//! Ensemble wire protocol — message envelope and types.
//!
//! This crate implements the Ensemble Wire Protocol Specification (Draft v0.1).
//! It defines the WireMessage envelope, message type constants, and payload structures.

use ensemble_values::Value;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Current protocol version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Unique identifier for a connected voice, assigned by the hub.
pub type VoiceId = u64;

// ---------------------------------------------------------------------------
// Message type constants
// ---------------------------------------------------------------------------

pub const MSG_HELLO: &str = "hello";
pub const MSG_WELCOME: &str = "welcome";
pub const MSG_DISCONNECT: &str = "disconnect";
pub const MSG_SUBSCRIBE: &str = "subscribe";
pub const MSG_UNSUBSCRIBE: &str = "unsubscribe";
pub const MSG_ACTION: &str = "action";
pub const MSG_UNSET_PARAM: &str = "unset_param";
pub const MSG_CLOCK_PING: &str = "clock_ping";
pub const MSG_CLOCK_PONG: &str = "clock_pong";
pub const MSG_ERROR: &str = "error";
pub const MSG_SET_MANIFEST: &str = "set_manifest";
pub const MSG_PATCH_MANIFEST: &str = "patch_manifest";
pub const MSG_UPDATE_NAME: &str = "update_name";

// ---------------------------------------------------------------------------
// WireMessage envelope
// ---------------------------------------------------------------------------

/// The self-describing message envelope used by all Ensemble protocol messages.
///
/// Every message is a WireMessage with a `type` string and a `payload` Value.
/// The type string identifies the message kind; the payload carries the data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireMessage {
    /// Message type identifier (e.g. "hello", "action", "clock_ping").
    #[serde(rename = "type")]
    pub msg_type: String,

    /// Message payload. Structure depends on msg_type.
    pub payload: Value,
}

impl WireMessage {
    /// Create a new WireMessage with the given type and payload.
    pub fn new(msg_type: impl Into<String>, payload: Value) -> Self {
        Self {
            msg_type: msg_type.into(),
            payload,
        }
    }
}

// ---------------------------------------------------------------------------
// Signal types
// ---------------------------------------------------------------------------

/// Semantic signal type that determines how the hub handles an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignalType {
    /// Fire-and-forget, no state retained by the hub.
    Event,
    /// Stateful key-value. Hub remembers last value and replays to late joiners.
    Param,
    /// High-rate best-effort data. Dropped under congestion rather than queued.
    Stream,
}

// ---------------------------------------------------------------------------
// Payload structures
// ---------------------------------------------------------------------------

/// Hello message payload — establishes protocol session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloPayload {
    pub protocol_version: u32,
    pub name: String,
}

/// Welcome message payload — assigns voice ID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WelcomePayload {
    pub voice_id: VoiceId,
}

/// Subscribe message payload — register a subscription pattern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubscribePayload {
    pub pattern: String,
}

/// Unsubscribe message payload — remove a subscription pattern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnsubscribePayload {
    pub pattern: String,
}

/// Action message payload — application traffic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionPayload {
    /// VoiceId of the originating voice. Set by the hub when routing.
    /// Clients may omit when sending (hub assigns based on connection).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<VoiceId>,
    pub address: String,
    pub signal_type: SignalType,
    pub timestamp: f64,
    pub payload: Value,
}

/// Unset param message payload — remove retained param state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnsetParamPayload {
    pub address: String,
}

/// Clock ping message payload — client → hub timing request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockPingPayload {
    pub sequence: u64,
}

/// Clock pong message payload — hub → client timing response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockPongPayload {
    pub sequence: u64,
    pub hub_time: f64,
}

/// Error message payload — hub → client error notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
}

/// Update name message payload — runtime renaming.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateNamePayload {
    pub name: String,
}

/// Set manifest message payload — replace current manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetManifestPayload {
    pub manifest: Value,
}

/// Patch manifest message payload — partial manifest update.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatchManifestPayload {
    pub patch: Value,
}

// ---------------------------------------------------------------------------
// Error codes
// ---------------------------------------------------------------------------

pub const ERR_UNSUPPORTED_PROTOCOL_VERSION: &str = "unsupported_protocol_version";
pub const ERR_INVALID_PATTERN: &str = "invalid_pattern";
pub const ERR_MALFORMED_MANIFEST: &str = "malformed_manifest";
pub const ERR_INVALID_MESSAGE: &str = "invalid_message";
pub const ERR_INTERNAL_ERROR: &str = "internal_error";

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Create a hello message.
pub fn hello(name: impl Into<String>) -> WireMessage {
    WireMessage::new(
        MSG_HELLO,
        Value::Map({
            let mut m = BTreeMap::new();
            m.insert("protocol_version".into(), Value::Integer(PROTOCOL_VERSION as i64));
            m.insert("name".into(), Value::String(name.into()));
            m
        }),
    )
}

/// Create a welcome message.
pub fn welcome(voice_id: VoiceId) -> WireMessage {
    WireMessage::new(
        MSG_WELCOME,
        Value::Map({
            let mut m = BTreeMap::new();
            m.insert("voice_id".into(), Value::Integer(voice_id as i64));
            m
        }),
    )
}

/// Create a disconnect message.
pub fn disconnect() -> WireMessage {
    WireMessage::new(MSG_DISCONNECT, Value::Map(BTreeMap::new()))
}

/// Create a subscribe message.
pub fn subscribe(pattern: impl Into<String>) -> WireMessage {
    WireMessage::new(
        MSG_SUBSCRIBE,
        Value::Map({
            let mut m = BTreeMap::new();
            m.insert("pattern".into(), Value::String(pattern.into()));
            m
        }),
    )
}

/// Create an unsubscribe message.
pub fn unsubscribe(pattern: impl Into<String>) -> WireMessage {
    WireMessage::new(
        MSG_UNSUBSCRIBE,
        Value::Map({
            let mut m = BTreeMap::new();
            m.insert("pattern".into(), Value::String(pattern.into()));
            m
        }),
    )
}

/// Create an action message (without source — hub assigns when routing).
pub fn action(
    address: impl Into<String>,
    signal_type: SignalType,
    timestamp: f64,
    payload: Value,
) -> WireMessage {
    WireMessage::new(
        MSG_ACTION,
        Value::Map({
            let mut m = BTreeMap::new();
            m.insert("address".into(), Value::String(address.into()));
            m.insert(
                "signal_type".into(),
                Value::String(match signal_type {
                    SignalType::Event => "event".into(),
                    SignalType::Param => "param".into(),
                    SignalType::Stream => "stream".into(),
                }),
            );
            m.insert(
                "timestamp".into(),
                Value::Float(ensemble_values::FloatValue::new(timestamp)),
            );
            m.insert("payload".into(), payload);
            m
        }),
    )
}

/// Create an action message with an explicit source voice ID.
/// Used by the hub when routing actions to subscribers.
pub fn action_with_source(
    source: VoiceId,
    address: impl Into<String>,
    signal_type: SignalType,
    timestamp: f64,
    payload: Value,
) -> WireMessage {
    WireMessage::new(
        MSG_ACTION,
        Value::Map({
            let mut m = BTreeMap::new();
            m.insert("source".into(), Value::Integer(source as i64));
            m.insert("address".into(), Value::String(address.into()));
            m.insert(
                "signal_type".into(),
                Value::String(match signal_type {
                    SignalType::Event => "event".into(),
                    SignalType::Param => "param".into(),
                    SignalType::Stream => "stream".into(),
                }),
            );
            m.insert(
                "timestamp".into(),
                Value::Float(ensemble_values::FloatValue::new(timestamp)),
            );
            m.insert("payload".into(), payload);
            m
        }),
    )
}

/// Create an unset_param message.
pub fn unset_param(address: impl Into<String>) -> WireMessage {
    WireMessage::new(
        MSG_UNSET_PARAM,
        Value::Map({
            let mut m = BTreeMap::new();
            m.insert("address".into(), Value::String(address.into()));
            m
        }),
    )
}

/// Create a clock_ping message.
pub fn clock_ping(sequence: u64) -> WireMessage {
    WireMessage::new(
        MSG_CLOCK_PING,
        Value::Map({
            let mut m = BTreeMap::new();
            m.insert("sequence".into(), Value::Integer(sequence as i64));
            m
        }),
    )
}

/// Create a clock_pong message.
pub fn clock_pong(sequence: u64, hub_time: f64) -> WireMessage {
    WireMessage::new(
        MSG_CLOCK_PONG,
        Value::Map({
            let mut m = BTreeMap::new();
            m.insert("sequence".into(), Value::Integer(sequence as i64));
            m.insert(
                "hub_time".into(),
                Value::Float(ensemble_values::FloatValue::new(hub_time)),
            );
            m
        }),
    )
}

/// Create an error message.
pub fn error(code: impl Into<String>, message: impl Into<String>) -> WireMessage {
    WireMessage::new(
        MSG_ERROR,
        Value::Map({
            let mut m = BTreeMap::new();
            m.insert("code".into(), Value::String(code.into()));
            m.insert("message".into(), Value::String(message.into()));
            m
        }),
    )
}

/// Create an update_name message.
pub fn update_name(name: impl Into<String>) -> WireMessage {
    WireMessage::new(
        MSG_UPDATE_NAME,
        Value::Map({
            let mut m = BTreeMap::new();
            m.insert("name".into(), Value::String(name.into()));
            m
        }),
    )
}

// ---------------------------------------------------------------------------
// Payload extraction helpers
// ---------------------------------------------------------------------------

/// Extract a field from a Value::Map.
pub fn get_field<'a>(map: &'a BTreeMap<String, Value>, key: &str) -> Option<&'a Value> {
    map.get(key)
}

/// Extract a string field from a Value::Map.
pub fn get_string(map: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    match map.get(key)? {
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// Extract an integer field from a Value::Map.
pub fn get_integer(map: &BTreeMap<String, Value>, key: &str) -> Option<i64> {
    match map.get(key)? {
        Value::Integer(i) => Some(*i),
        _ => None,
    }
}

/// Extract a float field from a Value::Map.
pub fn get_float(map: &BTreeMap<String, Value>, key: &str) -> Option<f64> {
    match map.get(key)? {
        Value::Float(f) => Some(f.value()),
        _ => None,
    }
}

/// Extract a Value field from a Value::Map.
pub fn get_value(map: &BTreeMap<String, Value>, key: &str) -> Option<Value> {
    map.get(key).cloned()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_message_roundtrip() {
        let msg = hello("test-voice");
        let encoded = rmp_serde::to_vec(&msg).unwrap();
        let decoded: WireMessage = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn hello_message_structure() {
        let msg = hello("MIDI Bridge");
        assert_eq!(msg.msg_type, MSG_HELLO);
        if let Value::Map(map) = &msg.payload {
            assert_eq!(get_integer(map, "protocol_version"), Some(1));
            assert_eq!(get_string(map, "name"), Some("MIDI Bridge".into()));
        } else {
            panic!("Expected Map payload");
        }
    }

    #[test]
    fn welcome_message_structure() {
        let msg = welcome(42);
        assert_eq!(msg.msg_type, MSG_WELCOME);
        if let Value::Map(map) = &msg.payload {
            assert_eq!(get_integer(map, "voice_id"), Some(42));
        } else {
            panic!("Expected Map payload");
        }
    }

    #[test]
    fn subscribe_message_structure() {
        let msg = subscribe("/midi/**");
        assert_eq!(msg.msg_type, MSG_SUBSCRIBE);
        if let Value::Map(map) = &msg.payload {
            assert_eq!(get_string(map, "pattern"), Some("/midi/**".into()));
        } else {
            panic!("Expected Map payload");
        }
    }

    #[test]
    fn action_message_structure() {
        let msg = action(
            "/transport/bpm",
            SignalType::Param,
            10.5,
            Value::Float(ensemble_values::FloatValue::new(120.0)),
        );
        assert_eq!(msg.msg_type, MSG_ACTION);
        if let Value::Map(map) = &msg.payload {
            assert_eq!(get_string(map, "address"), Some("/transport/bpm".into()));
            assert_eq!(get_string(map, "signal_type"), Some("param".into()));
            assert_eq!(get_float(map, "timestamp"), Some(10.5));
        } else {
            panic!("Expected Map payload");
        }
    }

    #[test]
    fn clock_ping_pong_roundtrip() {
        let ping = clock_ping(123);
        let encoded = rmp_serde::to_vec(&ping).unwrap();
        let decoded: WireMessage = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(ping, decoded);

        let pong = clock_pong(123, 45.67);
        let encoded = rmp_serde::to_vec(&pong).unwrap();
        let decoded: WireMessage = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(pong, decoded);
    }

    #[test]
    fn error_message_structure() {
        let msg = error(ERR_INVALID_PATTERN, "Recursive wildcard must be final segment.");
        assert_eq!(msg.msg_type, MSG_ERROR);
        if let Value::Map(map) = &msg.payload {
            assert_eq!(get_string(map, "code"), Some(ERR_INVALID_PATTERN.into()));
            assert_eq!(
                get_string(map, "message"),
                Some("Recursive wildcard must be final segment.".into())
            );
        } else {
            panic!("Expected Map payload");
        }
    }

    #[test]
    fn disconnect_message_structure() {
        let msg = disconnect();
        assert_eq!(msg.msg_type, MSG_DISCONNECT);
        assert!(matches!(msg.payload, Value::Map(_)));
    }

    #[test]
    fn u64_voice_id_survives_roundtrip() {
        let large_id: VoiceId = 9_223_372_036_854_775_807; // i64::MAX
        let msg = welcome(large_id);
        let encoded = rmp_serde::to_vec(&msg).unwrap();
        let decoded: WireMessage = rmp_serde::from_slice(&encoded).unwrap();
        if let Value::Map(map) = &decoded.payload {
            assert_eq!(get_integer(map, "voice_id"), Some(large_id as i64));
        } else {
            panic!("Expected Map payload");
        }
    }

    #[test]
    fn all_message_types_roundtrip() {
        let messages = vec![
            hello("test"),
            welcome(1),
            disconnect(),
            subscribe("/test/**"),
            unsubscribe("/test/**"),
            action("/test", SignalType::Event, 0.0, Value::Null),
            action_with_source(42, "/test", SignalType::Event, 0.0, Value::Null),
            unset_param("/test"),
            clock_ping(1),
            clock_pong(1, 0.0),
            error(ERR_INTERNAL_ERROR, "test error"),
            update_name("new-name"),
        ];

        for msg in messages {
            let encoded = rmp_serde::to_vec(&msg).unwrap();
            let decoded: WireMessage = rmp_serde::from_slice(&encoded).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn action_with_source_includes_source_field() {
        let msg = action_with_source(
            42,
            "/transport/bpm",
            SignalType::Param,
            10.5,
            Value::Float(ensemble_values::FloatValue::new(120.0)),
        );
        assert_eq!(msg.msg_type, MSG_ACTION);
        if let Value::Map(map) = &msg.payload {
            assert_eq!(get_integer(map, "source"), Some(42));
            assert_eq!(get_string(map, "address"), Some("/transport/bpm".into()));
            assert_eq!(get_string(map, "signal_type"), Some("param".into()));
            assert_eq!(get_float(map, "timestamp"), Some(10.5));
        } else {
            panic!("Expected Map payload");
        }
    }

    #[test]
    fn action_without_source_omits_source_field() {
        let msg = action(
            "/test",
            SignalType::Event,
            0.0,
            Value::Null,
        );
        if let Value::Map(map) = &msg.payload {
            assert!(map.get("source").is_none());
            assert_eq!(get_string(map, "address"), Some("/test".into()));
        } else {
            panic!("Expected Map payload");
        }
    }
}
