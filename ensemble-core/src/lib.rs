//! Ensemble core — shared types, wire protocol, and codec for the Ensemble protocol.
//!
//! This crate contains everything needed to implement an Ensemble voice or hub:
//!
//! - [`protocol`] — the wire protocol: the [`WireMessage`] envelope, message
//!   type constants, [`SignalType`], message builder functions, and payload
//!   field accessors (re-exported from `ensemble-protocol`, plus the value
//!   and manifest models from `ensemble-values` and `ensemble-manifest`)
//! - [`codec`] — length-prefixed MessagePack frame encoding/decoding over async streams
//! - [`ClockSync`] — O2/NTP-style clock synchronisation algorithm (min-RTT
//!   filter), re-exported from `ensemble-clock`
//!
//! The wire format is `[4 bytes LE length][MessagePack payload]` over TCP.
//! MessagePack was chosen for cross-language interoperability — native
//! implementations exist for Python, JavaScript, Go, C, and most other languages.
//!
//! For routing and pattern matching, see the `ensemble-routing` crate. For
//! local hub discovery (port files), see the `ensemble-discovery` crate.

pub mod codec;
pub mod protocol;

pub use codec::{read_message, write_message, CodecError};
pub use ensemble_clock::ClockSync;
pub use protocol::*;
