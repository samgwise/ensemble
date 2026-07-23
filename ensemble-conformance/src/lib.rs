//! Ensemble conformance test harness.
//!
//! This crate loads YAML fixtures from `ensemble-test-fixtures` and verifies
//! them against the reference implementation. Tests are organised into suites
//! matching the conformance areas, with two certification levels:
//!
//! - **Core**: Routing, Values, Protocol, Lifecycle
//! - **Full**: Core + Scheduling, Params, Manifests

use std::collections::BTreeMap;
use std::path::PathBuf;

// Re-export fixture types for use in test modules.
pub use ensemble_client::Hub;
pub use ensemble_core::protocol::*;
pub use ensemble_routing::Pattern;

/// The root fixtures directory, resolved at compile time.
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(ensemble_test_fixtures::FIXTURES_DIR)
}

// ---------------------------------------------------------------------------
// Fixture loading
// ---------------------------------------------------------------------------

/// Load and parse a YAML fixture file into a generic Value.
pub fn load_fixture(relative_path: &str) -> serde_yaml::Value {
    let path = fixtures_dir().join(relative_path);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read fixture {}: {}", path.display(), e));
    serde_yaml::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse fixture {}: {}", path.display(), e))
}

/// Helper to extract a string from a YAML value.
pub fn yaml_str(v: &serde_yaml::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// Helper to extract a bool from a YAML value.
pub fn yaml_bool(v: &serde_yaml::Value, key: &str) -> Option<bool> {
    v.get(key).and_then(|v| v.as_bool())
}

/// Helper to extract an i64 from a YAML value.
pub fn yaml_i64(v: &serde_yaml::Value, key: &str) -> Option<i64> {
    v.get(key).and_then(|v| v.as_i64())
}

/// Helper to extract an f64 from a YAML value.
pub fn yaml_f64(v: &serde_yaml::Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|v| v.as_f64())
}

/// Helper to extract a sequence from a YAML value.
pub fn yaml_seq<'a>(v: &'a serde_yaml::Value, key: &str) -> Option<&'a Vec<serde_yaml::Value>> {
    v.get(key).and_then(|v| v.as_sequence())
}

/// Helper to extract a mapping from a YAML value.
pub fn yaml_map<'a>(v: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Mapping> {
    v.get(key).and_then(|v| v.as_mapping())
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Start a hub for conformance testing. Returns the port.
pub async fn start_hub() -> u16 {
    let (_state, port) = ensemble_hub::start_server(0)
        .await
        .expect("Failed to start hub");
    port
}

/// Extract payload map from a WireMessage.
pub fn payload_map(msg: &WireMessage) -> BTreeMap<String, Value> {
    match &msg.payload {
        Value::Map(m) => m.clone(),
        _ => BTreeMap::new(),
    }
}

/// Parse a SignalType from its string representation.
pub fn parse_signal_type(s: &str) -> Option<SignalType> {
    match s {
        "event" => Some(SignalType::Event),
        "param" => Some(SignalType::Param),
        "stream" => Some(SignalType::Stream),
        _ => None,
    }
}
