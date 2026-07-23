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
    action,
    action_with_source,
    clock_ping,
    clock_pong,
    disconnect,
    error,
    get_field,
    get_float,
    get_integer,
    get_string,
    get_value,
    // Helper functions
    hello,
    patch_manifest,
    set_manifest,
    subscribe,
    unset_param,
    unsubscribe,
    update_name,
    welcome,
    ActionPayload,
    ClockPingPayload,
    ClockPongPayload,
    ErrorPayload,
    HelloPayload,
    PatchManifestPayload,
    SetManifestPayload,
    // Payload structures
    SignalType,
    SubscribePayload,
    UnsetParamPayload,
    UnsubscribePayload,
    UpdateNamePayload,
    // Types
    VoiceId,
    WelcomePayload,
    // WireMessage envelope
    WireMessage,
    ERR_INTERNAL_ERROR,
    ERR_INVALID_MESSAGE,
    ERR_INVALID_PATTERN,
    ERR_MALFORMED_MANIFEST,
    ERR_RESERVED_NAMESPACE,
    // Error codes
    ERR_UNSUPPORTED_PROTOCOL_VERSION,
    MSG_ACTION,
    MSG_CLOCK_PING,
    MSG_CLOCK_PONG,
    MSG_DISCONNECT,
    MSG_ERROR,
    // Message type constants
    MSG_HELLO,
    MSG_PATCH_MANIFEST,
    MSG_SET_MANIFEST,
    MSG_SUBSCRIBE,
    MSG_UNSET_PARAM,
    MSG_UNSUBSCRIBE,
    MSG_UPDATE_NAME,
    MSG_WELCOME,
    PROTOCOL_VERSION,
};

// Re-export manifest types from ensemble-manifest.
pub use ensemble_manifest::{RouteInfo, VoiceManifest};
