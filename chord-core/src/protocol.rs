//! Wire protocol message types for the Chord protocol.
//!
//! Every message exchanged between hub and voices is a `Message` enum,
//! serialised as length-prefixed MessagePack frames over TCP.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Signal types
// ---------------------------------------------------------------------------

/// Semantic signal type that determines how the hub handles an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalType {
    /// Fire-and-forget, no state retained by the hub.
    Event,
    /// Stateful key-value. Hub remembers last value and replays to late joiners.
    Param,
    /// High-rate best-effort data. Dropped under congestion rather than queued.
    Stream,
}

// ---------------------------------------------------------------------------
// Payload values
// ---------------------------------------------------------------------------

/// A typed value carried by an action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    F32(f32),
    I32(i32),
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
}

/// The payload of an action — either a single value or a tuple of values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Payload {
    Single(Value),
    Tuple(Vec<Value>),
    /// No payload (used for triggers/bangs).
    None,
}

// ---------------------------------------------------------------------------
// Action
// ---------------------------------------------------------------------------

/// An action is the fundamental message unit in Chord.
/// It represents something happening — a note, a parameter change, a trigger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    /// Hierarchical address path (e.g. `/synth/voice/1/note`).
    pub address: String,
    /// How the hub should treat this action.
    pub signal_type: SignalType,
    /// Hub time for delivery. 0.0 means immediate.
    pub timestamp: f64,
    /// The data carried by this action.
    pub payload: Payload,
}

// ---------------------------------------------------------------------------
// Voice capabilities (declared on connect)
// ---------------------------------------------------------------------------

/// Capabilities a voice declares when connecting to the hub.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceCapabilities {
    /// Human-readable name for this voice (e.g. "my-sequencer").
    pub name: String,
    /// Address patterns this voice subscribes to (e.g. ["/midi/*", "/clock"]).
    pub subscriptions: Vec<String>,
    /// Whether this voice is a bridge node.
    pub is_bridge: bool,
}

// ---------------------------------------------------------------------------
// Wire protocol messages
// ---------------------------------------------------------------------------

/// Unique identifier for a connected voice, assigned by the hub.
pub type VoiceId = u32;

/// All messages exchanged between hub and voices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Message {
    // -- Connection lifecycle --
    /// Sent by a voice on connect to introduce itself.
    Hello(VoiceCapabilities),

    /// Sent by the hub in response to Hello.
    Welcome {
        voice_id: VoiceId,
        /// Current hub clock time (for initial offset estimation).
        hub_time: f64,
    },

    /// Sent by either side to cleanly disconnect.
    Goodbye,

    // -- Clock synchronisation --
    /// Sent by a voice to request clock sync.
    ClockSyncRequest {
        /// Voice-local timestamp when this request was sent.
        voice_send_time: f64,
    },

    /// Sent by the hub in response to a clock sync request.
    ClockSyncReply {
        /// The voice's original send timestamp (echoed back).
        voice_send_time: f64,
        /// Hub-local time when the request was received.
        hub_receive_time: f64,
        /// Hub-local time when this reply was sent.
        hub_send_time: f64,
    },

    // -- Actions --
    /// An action routed through the hub.
    ActionMessage {
        /// Which voice sent this action (filled in by the hub when forwarding).
        source: VoiceId,
        /// The action itself.
        action: Action,
    },

    /// Subscribe to additional address patterns after initial Hello.
    Subscribe {
        patterns: Vec<String>,
    },

    /// Unsubscribe from address patterns.
    Unsubscribe {
        patterns: Vec<String>,
    },
}
