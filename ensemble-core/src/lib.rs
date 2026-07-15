//! Ensemble core — shared types, wire protocol, and codec for the Ensemble protocol.
//!
//! This crate contains everything needed to implement an Ensemble voice or hub:
//!
//! - [`protocol`] — message types (`Action`, `Message`, `SignalType`, `Payload`, etc.)
//! - [`codec`] — length-prefixed MessagePack frame encoding/decoding over async streams
//! - [`clock`] — O2/NTP-style clock synchronisation algorithm (min-RTT filter)
//!
//! The wire format is `[4 bytes LE length][MessagePack payload]` over TCP.
//! MessagePack was chosen for cross-language interoperability — native
//! implementations exist for Python, JavaScript, Go, C, and most other languages.
//!
//! For routing and pattern matching, see the `ensemble-routing` crate.

pub mod clock;
pub mod codec;
pub mod protocol;

pub use clock::ClockSync;
pub use codec::{read_message, write_message, CodecError};
pub use protocol::*;
