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

/// Start a hub and return the shared state handle as well, so tests can
/// assert hub-side state (voices, params, manifests) directly.
pub async fn start_hub_with_state() -> (ensemble_hub::SharedState, u16) {
    ensemble_hub::start_server(0)
        .await
        .expect("Failed to start hub")
}

// ---------------------------------------------------------------------------
// Poll-based synchronisation
// ---------------------------------------------------------------------------

/// Poll a condition until it holds or the timeout expires. Panics on timeout.
/// Prefer this over fixed sleeps — it is faster and far less flaky.
pub async fn wait_until<F>(what: &str, timeout: std::time::Duration, mut cond: F)
where
    F: FnMut() -> bool,
{
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if cond() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for: {what}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

// Note: `try_lock` is used rather than `blocking_lock`, which panics inside
// an async runtime. A contended attempt simply retries on the next poll.

/// Wait until the hub has exactly `n` connected voices.
pub async fn wait_for_voice_count(state: &ensemble_hub::SharedState, n: usize) {
    wait_until("voice count", std::time::Duration::from_secs(2), || {
        state
            .try_lock()
            .map(|st| st.voices().len() == n)
            .unwrap_or(false)
    })
    .await;
}

/// Wait until a voice has a subscription pattern registered.
pub async fn wait_for_subscription(
    state: &ensemble_hub::SharedState,
    voice_id: ensemble_core::protocol::VoiceId,
    pattern: &str,
) {
    wait_until(
        "subscription registered",
        std::time::Duration::from_secs(2),
        || {
            state
                .try_lock()
                .map(|st| {
                    st.voices().iter().any(|v| {
                        v.id == voice_id && v.subscription_strings.iter().any(|s| s == pattern)
                    })
                })
                .unwrap_or(false)
        },
    )
    .await;
}

/// Wait until the hub's param state contains an address.
pub async fn wait_for_param(state: &ensemble_hub::SharedState, address: &str) {
    wait_until(
        "param state stored",
        std::time::Duration::from_secs(2),
        || {
            state
                .try_lock()
                .map(|st| st.param_state().iter().any(|p| p.address == address))
                .unwrap_or(false)
        },
    )
    .await;
}

/// Wait until the hub's param state no longer contains an address.
pub async fn wait_for_param_absent(state: &ensemble_hub::SharedState, address: &str) {
    wait_until(
        "param state removed",
        std::time::Duration::from_secs(2),
        || {
            state
                .try_lock()
                .map(|st| !st.param_state().iter().any(|p| p.address == address))
                .unwrap_or(false)
        },
    )
    .await;
}

/// Wait until the hub's stored param at an address carries a specific payload.
/// Stronger than [`wait_for_param`]: an overwrite is only observed once the
/// NEW value has been stored.
pub async fn wait_for_param_value(
    state: &ensemble_hub::SharedState,
    address: &str,
    expected: &Value,
) {
    wait_until(
        "param value stored",
        std::time::Duration::from_secs(2),
        || {
            state
                .try_lock()
                .map(|st| {
                    st.param_state().iter().any(|p| {
                        p.address == address
                            && get_value(&payload_map(&p.message), "payload").as_ref()
                                == Some(expected)
                    })
                })
                .unwrap_or(false)
        },
    )
    .await;
}

/// Wait until a manifest exists for a voice.
pub async fn wait_for_manifest(
    state: &ensemble_hub::SharedState,
    voice_id: ensemble_core::protocol::VoiceId,
) {
    wait_until("manifest stored", std::time::Duration::from_secs(2), || {
        state
            .try_lock()
            .map(|st| st.manifest(voice_id).is_some())
            .unwrap_or(false)
    })
    .await;
}

/// Wait until no manifest exists for a voice.
pub async fn wait_for_manifest_absent(
    state: &ensemble_hub::SharedState,
    voice_id: ensemble_core::protocol::VoiceId,
) {
    wait_until(
        "manifest removed",
        std::time::Duration::from_secs(2),
        || {
            state
                .try_lock()
                .map(|st| st.manifest(voice_id).is_none())
                .unwrap_or(false)
        },
    )
    .await;
}

/// Wait until a voice's stored manifest equals `expected`. Stronger than
/// [`wait_for_manifest`]: a second set/patch is only observed once the new
/// content has actually been applied.
pub async fn wait_for_manifest_eq(
    state: &ensemble_hub::SharedState,
    voice_id: ensemble_core::protocol::VoiceId,
    expected: &ensemble_manifest::VoiceManifest,
) {
    wait_until(
        "manifest applied",
        std::time::Duration::from_secs(2),
        || {
            state
                .try_lock()
                .map(|st| st.manifest(voice_id) == Some(expected))
                .unwrap_or(false)
        },
    )
    .await;
}

// ---------------------------------------------------------------------------
// Plain YAML → Value conversion (for fixture payloads without a type field)
// ---------------------------------------------------------------------------

/// Convert arbitrary YAML to an Ensemble Value (mapping→Map, sequence→List,
/// string→String, integer→Integer, float→Float, bool→Bool, null→Null).
pub fn yaml_plain_to_value(v: &serde_yaml::Value) -> Value {
    match v {
        serde_yaml::Value::Null => Value::Null,
        serde_yaml::Value::Bool(b) => Value::Bool(*b),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else {
                Value::Float(FloatValue::new(n.as_f64().unwrap_or(0.0)))
            }
        }
        serde_yaml::Value::String(s) => Value::String(s.clone()),
        serde_yaml::Value::Sequence(items) => {
            Value::List(items.iter().map(yaml_plain_to_value).collect())
        }
        serde_yaml::Value::Mapping(m) => {
            let mut map = BTreeMap::new();
            for (k, val) in m {
                map.insert(
                    k.as_str()
                        .expect("fixture keys must be strings")
                        .to_string(),
                    yaml_plain_to_value(val),
                );
            }
            Value::Map(map)
        }
        other => panic!("unsupported YAML value in fixture: {other:?}"),
    }
}

/// Build a VoiceManifest from a plain YAML mapping (as used in fixtures).
pub fn yaml_to_manifest(v: &serde_yaml::Value) -> ensemble_manifest::VoiceManifest {
    ensemble_manifest::VoiceManifest::from_value(&yaml_plain_to_value(v))
        .expect("fixture manifest must be a valid manifest")
}

// ---------------------------------------------------------------------------
// Typed YAML → Value conversion (fixture payloads carry a "type" field)
// ---------------------------------------------------------------------------

/// Convert a typed fixture value spec (`type` + `data`/`tag` fields) to an
/// Ensemble Value.
pub fn yaml_typed_to_value(v: &serde_yaml::Value) -> Value {
    let type_str = yaml_str(v, "type").unwrap_or_else(|| "null".into());
    match type_str.as_str() {
        "null" => Value::Null,
        "bool" => Value::Bool(v.get("data").unwrap().as_bool().unwrap()),
        "integer" => Value::Integer(v.get("data").unwrap().as_i64().unwrap()),
        "float" => Value::Float(FloatValue::new(v.get("data").unwrap().as_f64().unwrap())),
        "string" => Value::String(v.get("data").unwrap().as_str().unwrap().to_string()),
        "binary" => {
            let data = v.get("data").unwrap().as_sequence().unwrap();
            let bytes: Vec<u8> = data
                .iter()
                .map(|d| {
                    let i = d.as_i64().unwrap();
                    u8::try_from(i).expect("binary fixture bytes must be 0-255")
                })
                .collect();
            Value::Binary(bytes)
        }
        "tuple" => {
            let items = v.get("data").unwrap().as_sequence().unwrap();
            Value::Tuple(items.iter().map(yaml_typed_to_value).collect())
        }
        "list" => {
            let items = v.get("data").unwrap().as_sequence().unwrap();
            Value::List(items.iter().map(yaml_typed_to_value).collect())
        }
        "map" => {
            let mapping = v.get("data").unwrap().as_mapping().unwrap();
            let mut m = BTreeMap::new();
            for (k, val) in mapping {
                m.insert(k.as_str().unwrap().to_string(), yaml_typed_to_value(val));
            }
            Value::Map(m)
        }
        "typed_binary" => {
            let tag = yaml_str(v, "tag").unwrap();
            let data = v.get("data").unwrap().as_sequence().unwrap();
            let bytes: Vec<u8> = data
                .iter()
                .map(|d| {
                    let i = d.as_i64().unwrap();
                    u8::try_from(i).expect("typed_binary fixture bytes must be 0-255")
                })
                .collect();
            Value::TypedBinary { tag, data: bytes }
        }
        other => panic!("Unknown value type in fixture: {}", other),
    }
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
