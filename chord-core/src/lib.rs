//! Chord core — shared types, wire protocol, and codec for the Chord protocol.

pub mod clock;
pub mod codec;
pub mod pattern;
pub mod protocol;

pub use clock::ClockSync;
pub use codec::{read_message, write_message, CodecError};
pub use pattern::{matches_any, matches_pattern};
pub use protocol::*;
