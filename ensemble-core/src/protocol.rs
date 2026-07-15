//! Wire protocol message types for the Ensemble protocol.
//!
//! Every message exchanged between hub and voices is a `WireMessage` envelope,
//! serialised as length-prefixed MessagePack frames over TCP.
//!
//! This module re-exports the protocol types from `ensemble-protocol`.

// Re-export the value model from ensemble-values.
pub use ensemble_values::{FloatValue, Value};

// Re-export the protocol types from ensemble-protocol.
pub use ensemble_protocol::{
    // WireMessage envelope
    WireMessage,
    // Message type constants
    MSG_HELLO, MSG_WELCOME, MSG_DISCONNECT, MSG_SUBSCRIBE, MSG_UNSUBSCRIBE,
    MSG_ACTION, MSG_UNSET_PARAM, MSG_CLOCK_PING, MSG_CLOCK_PONG, MSG_ERROR,
    MSG_SET_MANIFEST, MSG_PATCH_MANIFEST, MSG_UPDATE_NAME,
    // Error codes
    ERR_UNSUPPORTED_PROTOCOL_VERSION, ERR_INVALID_PATTERN, ERR_MALFORMED_MANIFEST,
    ERR_INVALID_MESSAGE, ERR_INTERNAL_ERROR,
    // Payload structures
    SignalType, HelloPayload, WelcomePayload, SubscribePayload, UnsubscribePayload,
    ActionPayload, UnsetParamPayload, ClockPingPayload, ClockPongPayload,
    ErrorPayload, UpdateNamePayload, SetManifestPayload, PatchManifestPayload,
    // Helper functions
    hello, welcome, disconnect, subscribe, unsubscribe, action, action_with_source,
    unset_param, clock_ping, clock_pong, error, update_name,
    set_manifest, patch_manifest,
    get_field, get_string, get_integer, get_float, get_value,
    // Types
    VoiceId, PROTOCOL_VERSION,
};

// Re-export manifest types from ensemble-manifest.
pub use ensemble_manifest::{RouteInfo, VoiceManifest};
