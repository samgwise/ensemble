//! Bridge wire protocol messages.
//!
//! These messages are exchanged between bridge instances over QUIC.
//! They reuse the same MessagePack encoding as the hub protocol but
//! define bridge-specific message types.
//!
//! Several constructors and accessors are defined ahead of their first use
//! (e.g. bridge_ping/pong for keepalive work, bridge_subscribe for future
//! remote subscriptions). They are kept here so the protocol surface stays
//! complete and stable across increments.
#![allow(dead_code)]

use std::collections::BTreeMap;

use ensemble_core::protocol::*;
use ensemble_values::FloatValue;

// ---------------------------------------------------------------------------
// Message type constants
// ---------------------------------------------------------------------------

pub const MSG_BRIDGE_HELLO: &str = "bridge_hello";
pub const MSG_BRIDGE_SUBSCRIBE: &str = "bridge_subscribe";
pub const MSG_BRIDGE_ACTION: &str = "bridge_action";
pub const MSG_BRIDGE_UNSET: &str = "bridge_unset";
pub const MSG_BRIDGE_PING: &str = "bridge_ping";
pub const MSG_BRIDGE_PONG: &str = "bridge_pong";

// ---------------------------------------------------------------------------
// Constructors
// ---------------------------------------------------------------------------

/// Create a bridge_hello message.
///
/// When `auth_token` is provided it is included in the hello so the peer can
/// verify the shared secret (see [`auth_token_matches`]).
pub fn bridge_hello(bridge_id: &str, name: &str, auth_token: Option<&str>) -> WireMessage {
    let mut map = BTreeMap::new();
    map.insert("bridge_id".into(), Value::String(bridge_id.into()));
    map.insert("name".into(), Value::String(name.into()));
    if let Some(token) = auth_token {
        map.insert("auth_token".into(), Value::String(token.into()));
    }
    WireMessage::new(MSG_BRIDGE_HELLO, Value::Map(map))
}

/// Create a bridge_subscribe message.
pub fn bridge_subscribe(pattern: &str) -> WireMessage {
    let mut map = BTreeMap::new();
    map.insert("pattern".into(), Value::String(pattern.into()));
    WireMessage::new(MSG_BRIDGE_SUBSCRIBE, Value::Map(map))
}

/// Create a bridge_action message with origin tag for loop prevention.
///
/// Each message is stamped with a fresh unique `msg_id` so receiving bridges
/// can suppress duplicates when an action is re-forwarded around a ring or
/// mesh (see `LoopGuard::check_and_record`).
pub fn bridge_action(
    origin: &str,
    source: VoiceId,
    address: &str,
    signal_type: SignalType,
    timestamp: f64,
    payload: Value,
) -> WireMessage {
    let mut map = BTreeMap::new();
    map.insert("origin".into(), Value::String(origin.into()));
    map.insert("msg_id".into(), Value::String(new_msg_id()));
    map.insert("source".into(), Value::Integer(source as i64));
    map.insert("address".into(), Value::String(address.into()));
    map.insert(
        "signal_type".into(),
        Value::String(signal_type_to_str(signal_type).into()),
    );
    map.insert("timestamp".into(), Value::Float(FloatValue::new(timestamp)));
    map.insert("payload".into(), payload);
    WireMessage::new(MSG_BRIDGE_ACTION, Value::Map(map))
}

/// Create a bridge_unset message — propagate a param unset across the bridge.
///
/// Carries the same origin and duplicate-suppression tags as
/// [`bridge_action`] so unsets traverse rings and meshes identically. The
/// address is in the remote (post-mapping) namespace.
pub fn bridge_unset(origin: &str, source: VoiceId, address: &str) -> WireMessage {
    let mut map = BTreeMap::new();
    map.insert("origin".into(), Value::String(origin.into()));
    map.insert("msg_id".into(), Value::String(new_msg_id()));
    map.insert("source".into(), Value::Integer(source as i64));
    map.insert("address".into(), Value::String(address.into()));
    WireMessage::new(MSG_BRIDGE_UNSET, Value::Map(map))
}

/// Generate a unique message identifier for duplicate suppression.
fn new_msg_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Create a bridge_ping message.
pub fn bridge_ping(sequence: u64) -> WireMessage {
    let mut map = BTreeMap::new();
    map.insert("sequence".into(), Value::Integer(sequence as i64));
    WireMessage::new(MSG_BRIDGE_PING, Value::Map(map))
}

/// Create a bridge_pong message.
pub fn bridge_pong(sequence: u64, timestamp: f64) -> WireMessage {
    let mut map = BTreeMap::new();
    map.insert("sequence".into(), Value::Integer(sequence as i64));
    map.insert("timestamp".into(), Value::Float(FloatValue::new(timestamp)));
    WireMessage::new(MSG_BRIDGE_PONG, Value::Map(map))
}

/// Convert a SignalType to its string representation.
fn signal_type_to_str(st: SignalType) -> &'static str {
    match st {
        SignalType::Event => "event",
        SignalType::Param => "param",
        SignalType::Stream => "stream",
    }
}

// ---------------------------------------------------------------------------
// Payload field accessors
// ---------------------------------------------------------------------------

/// Extract the origin bridge_id from a bridge message.
pub fn get_origin(msg: &WireMessage) -> Option<String> {
    match &msg.payload {
        Value::Map(m) => match m.get("origin") {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Extract the unique message id from a bridge message, if present.
pub fn get_msg_id(msg: &WireMessage) -> Option<String> {
    match &msg.payload {
        Value::Map(m) => match m.get("msg_id") {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Extract the address from a bridge message.
pub fn get_address(msg: &WireMessage) -> Option<String> {
    match &msg.payload {
        Value::Map(m) => match m.get("address") {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Extract the signal_type string from a bridge_action message.
pub fn get_signal_type(msg: &WireMessage) -> Option<String> {
    match &msg.payload {
        Value::Map(m) => match m.get("signal_type") {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Extract the payload value from a bridge_action message.
pub fn get_action_payload(msg: &WireMessage) -> Value {
    match &msg.payload {
        Value::Map(m) => m.get("payload").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// Extract the timestamp from a bridge_action message.
pub fn get_timestamp(msg: &WireMessage) -> f64 {
    match &msg.payload {
        Value::Map(m) => match m.get("timestamp") {
            Some(Value::Float(f)) => f.value(),
            _ => 0.0,
        },
        _ => 0.0,
    }
}

/// Extract the source voice_id from a bridge_action message.
pub fn get_source(msg: &WireMessage) -> VoiceId {
    match &msg.payload {
        Value::Map(m) => match m.get("source") {
            Some(Value::Integer(i)) => *i as VoiceId,
            _ => 0,
        },
        _ => 0,
    }
}

/// Extract the auth token from a bridge_hello message, if present.
pub fn get_auth_token(msg: &WireMessage) -> Option<String> {
    match &msg.payload {
        Value::Map(m) => match m.get("auth_token") {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

/// Check a presented auth token against the configured expectation.
///
/// When no token is configured (`expected` is `None`) authentication is
/// disabled and any hello is accepted. When a token is configured, the peer
/// must present the identical token; the comparison is constant-time so the
/// handshake does not leak matching prefixes via timing.
pub fn auth_token_matches(expected: Option<&str>, presented: Option<&str>) -> bool {
    match expected {
        None => true,
        Some(expected) => match presented {
            Some(presented) => constant_time_eq(expected.as_bytes(), presented.as_bytes()),
            None => false,
        },
    }
}

/// Constant-time byte-string equality.
///
/// The comparison always scans the full length of the longer input so the
/// running time does not reveal the position of the first difference. (The
/// length itself is not considered secret for a shared-token scheme.)
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff: u8 = if a.len() == b.len() { 0 } else { 1 };
    let max = a.len().max(b.len());
    for i in 0..max {
        let x = if i < a.len() { a[i] } else { 0 };
        let y = if i < b.len() { b[i] } else { 0 };
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use ensemble_core::codec;

    #[tokio::test]
    async fn roundtrip_bridge_hello() {
        let msg = bridge_hello("abc-123", "test-bridge", None);
        let mut buf = Vec::new();
        codec::write_message(&mut buf, &msg).await.unwrap();
        let decoded = codec::read_message(&mut std::io::Cursor::new(buf))
            .await
            .unwrap();
        assert_eq!(decoded.msg_type, MSG_BRIDGE_HELLO);
        assert_eq!(get_origin(&decoded), None); // hello has no origin
        assert_eq!(get_auth_token(&decoded), None);
    }

    #[tokio::test]
    async fn roundtrip_bridge_hello_with_token() {
        let msg = bridge_hello("abc-123", "test-bridge", Some("s3cret"));
        let mut buf = Vec::new();
        codec::write_message(&mut buf, &msg).await.unwrap();
        let decoded = codec::read_message(&mut std::io::Cursor::new(buf))
            .await
            .unwrap();
        assert_eq!(get_auth_token(&decoded), Some("s3cret".to_string()));
    }

    #[tokio::test]
    async fn roundtrip_bridge_action() {
        let msg = bridge_action(
            "bridge-aaa",
            42,
            "/transport/bpm",
            SignalType::Param,
            10.5,
            Value::Float(FloatValue::new(120.0)),
        );
        let mut buf = Vec::new();
        codec::write_message(&mut buf, &msg).await.unwrap();
        let decoded = codec::read_message(&mut std::io::Cursor::new(buf))
            .await
            .unwrap();
        assert_eq!(decoded.msg_type, MSG_BRIDGE_ACTION);
        assert_eq!(get_origin(&decoded), Some("bridge-aaa".to_string()));
        assert!(get_msg_id(&decoded).is_some());
        assert_eq!(get_address(&decoded), Some("/transport/bpm".to_string()));
        assert_eq!(get_source(&decoded), 42);
        assert_eq!(get_timestamp(&decoded), 10.5);
    }

    #[tokio::test]
    async fn roundtrip_bridge_unset() {
        let msg = bridge_unset("bridge-aaa", 9, "/transport/bpm");
        let mut buf = Vec::new();
        codec::write_message(&mut buf, &msg).await.unwrap();
        let decoded = codec::read_message(&mut std::io::Cursor::new(buf))
            .await
            .unwrap();
        assert_eq!(decoded.msg_type, MSG_BRIDGE_UNSET);
        assert_eq!(get_origin(&decoded), Some("bridge-aaa".to_string()));
        assert!(get_msg_id(&decoded).is_some());
        assert_eq!(get_address(&decoded), Some("/transport/bpm".to_string()));
        assert_eq!(get_source(&decoded), 9);
    }

    #[test]
    fn msg_ids_are_unique() {
        let a = bridge_action("o", 1, "/x", SignalType::Event, 0.0, Value::Null);
        let b = bridge_action("o", 1, "/x", SignalType::Event, 0.0, Value::Null);
        assert_ne!(get_msg_id(&a), get_msg_id(&b));
    }

    #[test]
    fn auth_token_matching() {
        // Auth disabled: anything is accepted.
        assert!(auth_token_matches(None, None));
        assert!(auth_token_matches(None, Some("anything")));
        // Configured token must be presented exactly.
        assert!(auth_token_matches(Some("s3cret"), Some("s3cret")));
        assert!(!auth_token_matches(Some("s3cret"), Some("s3creu")));
        assert!(!auth_token_matches(Some("s3cret"), None));
        assert!(!auth_token_matches(Some("s3cret"), Some("s3cret-longer")));
        assert!(!auth_token_matches(Some("s3cret-longer"), Some("s3cret")));
        assert!(!auth_token_matches(Some(""), Some("x")));
        assert!(auth_token_matches(Some(""), Some("")));
    }

    #[tokio::test]
    async fn roundtrip_bridge_ping() {
        let msg = bridge_ping(7);
        let mut buf = Vec::new();
        codec::write_message(&mut buf, &msg).await.unwrap();
        let decoded = codec::read_message(&mut std::io::Cursor::new(buf))
            .await
            .unwrap();
        assert_eq!(decoded.msg_type, MSG_BRIDGE_PING);
    }
}
