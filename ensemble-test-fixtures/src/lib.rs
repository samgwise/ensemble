//! Ensemble test fixtures — language-neutral YAML fixtures for conformance testing.
//!
//! This crate provides YAML fixture files that describe test cases for each
//! conformance area of the Ensemble protocol. The fixtures are intentionally
//! simple (no anchors, tags, or complex keys) so they can be easily converted
//! to JSON or parsed by any language with YAML support.

/// Re-export the fixture directory path for use by the conformance runner.
pub const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");
