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
pub const MSG_BRIDGE_PING: &str = "bridge_ping";
pub const MSG_BRIDGE_PONG: &str = "bridge_pong";

// ---------------------------------------------------------------------------
// Constructors
// ---------------------------------------------------------------------------

/// Create a bridge_hello message.
pub fn bridge_hello(bridge_id: &str, name: &str) -> WireMessage {
    let mut map = BTreeMap::new();
    map.insert("bridge_id".into(), Value::String(bridge_id.into()));
    map.insert("name".into(), Value::String(name.into()));
    WireMessage::new(MSG_BRIDGE_HELLO, Value::Map(map))
}

/// Create a bridge_subscribe message.
pub fn bridge_subscribe(pattern: &str) -> WireMessage {
    let mut map = BTreeMap::new();
    map.insert("pattern".into(), Value::String(pattern.into()));
    WireMessage::new(MSG_BRIDGE_SUBSCRIBE, Value::Map(map))
}

/// Create a bridge_action message with origin tag for loop prevention.
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

/// Extract the origin bridge_id from a bridge_action message.
pub fn get_origin(msg: &WireMessage) -> Option<String> {
    match &msg.payload {
        Value::Map(m) => match m.get("origin") {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Extract the address from a bridge_action message.
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

#[cfg(test)]
mod tests {
    use super::*;
    use ensemble_core::codec;

    #[tokio::test]
    async fn roundtrip_bridge_hello() {
        let msg = bridge_hello("abc-123", "test-bridge");
        let mut buf = Vec::new();
        codec::write_message(&mut buf, &msg).await.unwrap();
        let decoded = codec::read_message(&mut std::io::Cursor::new(buf)).await.unwrap();
        assert_eq!(decoded.msg_type, MSG_BRIDGE_HELLO);
        assert_eq!(get_origin(&decoded), None); // hello has no origin
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
        let decoded = codec::read_message(&mut std::io::Cursor::new(buf)).await.unwrap();
        assert_eq!(decoded.msg_type, MSG_BRIDGE_ACTION);
        assert_eq!(get_origin(&decoded), Some("bridge-aaa".to_string()));
        assert_eq!(get_address(&decoded), Some("/transport/bpm".to_string()));
        assert_eq!(get_source(&decoded), 42);
        assert_eq!(get_timestamp(&decoded), 10.5);
    }

    #[tokio::test]
    async fn roundtrip_bridge_ping() {
        let msg = bridge_ping(7);
        let mut buf = Vec::new();
        codec::write_message(&mut buf, &msg).await.unwrap();
        let decoded = codec::read_message(&mut std::io::Cursor::new(buf)).await.unwrap();
        assert_eq!(decoded.msg_type, MSG_BRIDGE_PING);
    }
}
