//! Ensemble manifest types — runtime-discoverable metadata about a voice.
//!
//! Manifests are advisory: they do not affect routing, enforce type safety,
//! or create subscriptions. They exist for observability, discovery, and
//! documentation.
//!
//! This crate implements the manifest structures defined in `design/manifest.md`.

use std::collections::BTreeMap;

use ensemble_protocol::SignalType;
use ensemble_values::Value;

// ---------------------------------------------------------------------------
// RouteInfo
// ---------------------------------------------------------------------------

/// Describes a single route exposed by a voice.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteInfo {
    /// An Ensemble routing pattern (e.g. `/transport/bpm`, `/midi/{ch}/note`).
    pub address: String,
    /// The semantic signal type of this route.
    pub signal: SignalType,
    /// Free-form descriptive hint about the payload shape (e.g. "float", "(int, int, int)").
    pub payload_hint: Option<String>,
    /// Human-readable explanation of the route's purpose.
    pub description: Option<String>,
    /// Optional example payload for tooling and inspection.
    pub example: Option<Value>,
}

impl RouteInfo {
    /// Serialise this route info to a `Value::Map`.
    pub fn to_value(&self) -> Value {
        let mut m = BTreeMap::new();
        m.insert("address".into(), Value::String(self.address.clone()));
        m.insert(
            "signal".into(),
            Value::String(match self.signal {
                SignalType::Event => "event".into(),
                SignalType::Param => "param".into(),
                SignalType::Stream => "stream".into(),
            }),
        );
        if let Some(ref hint) = self.payload_hint {
            m.insert("payload_hint".into(), Value::String(hint.clone()));
        }
        if let Some(ref desc) = self.description {
            m.insert("description".into(), Value::String(desc.clone()));
        }
        if let Some(ref ex) = self.example {
            m.insert("example".into(), ex.clone());
        }
        Value::Map(m)
    }

    /// Deserialise a route info from a `Value::Map`.
    pub fn from_value(v: &Value) -> Option<Self> {
        let m = match v {
            Value::Map(m) => m,
            _ => return None,
        };
        let address = match m.get("address")? {
            Value::String(s) => s.clone(),
            _ => return None,
        };
        let signal = match m.get("signal")? {
            Value::String(s) => match s.as_str() {
                "event" => SignalType::Event,
                "param" => SignalType::Param,
                "stream" => SignalType::Stream,
                _ => return None,
            },
            _ => return None,
        };
        let payload_hint = m.get("payload_hint").and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        });
        let description = m.get("description").and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        });
        let example = m.get("example").cloned();
        Some(RouteInfo {
            address,
            signal,
            payload_hint,
            description,
            example,
        })
    }
}

// ---------------------------------------------------------------------------
// VoiceManifest
// ---------------------------------------------------------------------------

/// Runtime-discoverable metadata about a voice's capabilities and interfaces.
///
/// Manifests are advisory — they do not affect routing, enforce type safety,
/// or create/remove subscriptions.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceManifest {
    /// Human-readable voice name (need not be unique).
    pub name: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Optional application version string (no format enforced).
    pub version: Option<String>,
    /// Free-form tags for filtering, search, and categorisation.
    pub tags: Vec<String>,
    /// Capabilities offered by this voice.
    pub provides: Vec<String>,
    /// Capabilities likely required by this voice.
    pub expects: Vec<String>,
    /// Routes exposed by this voice.
    pub routes: Vec<RouteInfo>,
}

impl Default for VoiceManifest {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: None,
            version: None,
            tags: Vec::new(),
            provides: Vec::new(),
            expects: Vec::new(),
            routes: Vec::new(),
        }
    }
}

impl VoiceManifest {
    /// Create a new manifest with only a name (all other fields empty/none).
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Serialise this manifest to a `Value::Map` suitable for the wire protocol.
    pub fn to_value(&self) -> Value {
        let mut m = BTreeMap::new();
        m.insert("name".into(), Value::String(self.name.clone()));
        if let Some(ref desc) = self.description {
            m.insert("description".into(), Value::String(desc.clone()));
        }
        if let Some(ref ver) = self.version {
            m.insert("version".into(), Value::String(ver.clone()));
        }
        m.insert(
            "tags".into(),
            Value::List(self.tags.iter().map(|t| Value::String(t.clone())).collect()),
        );
        m.insert(
            "provides".into(),
            Value::List(self.provides.iter().map(|c| Value::String(c.clone())).collect()),
        );
        m.insert(
            "expects".into(),
            Value::List(self.expects.iter().map(|c| Value::String(c.clone())).collect()),
        );
        m.insert(
            "routes".into(),
            Value::List(self.routes.iter().map(|r| r.to_value()).collect()),
        );
        Value::Map(m)
    }

    /// Deserialise a manifest from a `Value::Map` (as received on the wire).
    ///
    /// Returns `None` if the value is not a map or the required `name` field
    /// is missing or not a string.
    pub fn from_value(v: &Value) -> Option<Self> {
        let m = match v {
            Value::Map(map) => map,
            _ => return None,
        };
        let name = match m.get("name")? {
            Value::String(s) => s.clone(),
            _ => return None,
        };
        let description = m.get("description").and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        });
        let version = m.get("version").and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        });
        let tags = parse_string_list(m.get("tags"));
        let provides = parse_string_list(m.get("provides"));
        let expects = parse_string_list(m.get("expects"));
        let routes = parse_route_list(m.get("routes"));
        Some(VoiceManifest {
            name,
            description,
            version,
            tags,
            provides,
            expects,
            routes,
        })
    }

    /// Apply a partial patch to this manifest.
    ///
    /// Only fields present in the patch map are updated. List fields (`tags`,
    /// `provides`, `expects`, `routes`) are replaced entirely when present in
    /// the patch. Absent fields are left unchanged.
    pub fn apply_patch(&mut self, patch: &BTreeMap<String, Value>) {
        if let Some(Value::String(s)) = patch.get("name") {
            self.name = s.clone();
        }
        if let Some(val) = patch.get("description") {
            self.description = match val {
                Value::String(s) => Some(s.clone()),
                Value::Null => None,
                _ => self.description.take(),
            };
        }
        if let Some(val) = patch.get("version") {
            self.version = match val {
                Value::String(s) => Some(s.clone()),
                Value::Null => None,
                _ => self.version.take(),
            };
        }
        if let Some(val) = patch.get("tags") {
            self.tags = parse_string_list(Some(val));
        }
        if let Some(val) = patch.get("provides") {
            self.provides = parse_string_list(Some(val));
        }
        if let Some(val) = patch.get("expects") {
            self.expects = parse_string_list(Some(val));
        }
        if let Some(val) = patch.get("routes") {
            self.routes = parse_route_list(Some(val));
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a `Value::List` of strings. Returns an empty vec if the value is
/// absent or not a list.
fn parse_string_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Parse a `Value::List` of route info maps. Returns an empty vec if the value
/// is absent or not a list. Invalid entries are silently skipped.
fn parse_route_list(v: Option<&Value>) -> Vec<RouteInfo> {
    match v {
        Some(Value::List(items)) => items.iter().filter_map(RouteInfo::from_value).collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ensemble_values::FloatValue;

    fn sample_manifest() -> VoiceManifest {
        VoiceManifest {
            name: "MIDI Bridge".into(),
            description: Some("Provides MIDI input and output integration.".into()),
            version: Some("1.0.0".into()),
            tags: vec!["midi".into(), "bridge".into()],
            provides: vec!["midi-input".into(), "midi-output".into()],
            expects: vec!["midi-output".into()],
            routes: vec![
                RouteInfo {
                    address: "/midi/play".into(),
                    signal: SignalType::Event,
                    payload_hint: Some("(channel:int, note:int, velocity:int)".into()),
                    description: Some("MIDI note-on event.".into()),
                    example: Some(Value::Map({
                        let mut m = BTreeMap::new();
                        m.insert("note".into(), Value::Integer(60));
                        m.insert("velocity".into(), Value::Integer(100));
                        m
                    })),
                },
                RouteInfo {
                    address: "/transport/bpm".into(),
                    signal: SignalType::Param,
                    payload_hint: Some("float".into()),
                    description: Some("Current playback tempo.".into()),
                    example: Some(Value::Float(FloatValue::new(120.0))),
                },
            ],
        }
    }

    #[test]
    fn manifest_roundtrip() {
        let original = sample_manifest();
        let value = original.to_value();
        let restored = VoiceManifest::from_value(&value).expect("Should parse");
        assert_eq!(original, restored);
    }

    #[test]
    fn minimal_manifest_roundtrip() {
        let original = VoiceManifest::new("Simple");
        let value = original.to_value();
        let restored = VoiceManifest::from_value(&value).expect("Should parse");
        assert_eq!(original, restored);
        assert_eq!(restored.description, None);
        assert_eq!(restored.version, None);
        assert!(restored.tags.is_empty());
        assert!(restored.provides.is_empty());
        assert!(restored.expects.is_empty());
        assert!(restored.routes.is_empty());
    }

    #[test]
    fn from_value_rejects_non_map() {
        assert!(VoiceManifest::from_value(&Value::Null).is_none());
        assert!(VoiceManifest::from_value(&Value::Integer(42)).is_none());
        assert!(VoiceManifest::from_value(&Value::String("nope".into())).is_none());
    }

    #[test]
    fn from_value_rejects_missing_name() {
        let m = Value::Map(BTreeMap::new());
        assert!(VoiceManifest::from_value(&m).is_none());
    }

    #[test]
    fn from_value_rejects_non_string_name() {
        let mut m = BTreeMap::new();
        m.insert("name".into(), Value::Integer(42));
        assert!(VoiceManifest::from_value(&Value::Map(m)).is_none());
    }

    #[test]
    fn route_info_roundtrip() {
        let route = RouteInfo {
            address: "/track/{id}/volume".into(),
            signal: SignalType::Param,
            payload_hint: Some("float".into()),
            description: Some("Track volume fader.".into()),
            example: Some(Value::Float(FloatValue::new(0.75))),
        };
        let value = route.to_value();
        let restored = RouteInfo::from_value(&value).expect("Should parse");
        assert_eq!(route, restored);
    }

    #[test]
    fn route_info_minimal() {
        let route = RouteInfo {
            address: "/test".into(),
            signal: SignalType::Event,
            payload_hint: None,
            description: None,
            example: None,
        };
        let value = route.to_value();
        let restored = RouteInfo::from_value(&value).expect("Should parse");
        assert_eq!(route, restored);
    }

    #[test]
    fn patch_updates_description_only() {
        let mut manifest = sample_manifest();
        let original_tags = manifest.tags.clone();
        let original_provides = manifest.provides.clone();

        let mut patch = BTreeMap::new();
        patch.insert(
            "description".into(),
            Value::String("Updated description.".into()),
        );
        manifest.apply_patch(&patch);

        assert_eq!(manifest.description, Some("Updated description.".into()));
        // Other fields unchanged.
        assert_eq!(manifest.name, "MIDI Bridge");
        assert_eq!(manifest.tags, original_tags);
        assert_eq!(manifest.provides, original_provides);
        assert_eq!(manifest.version, Some("1.0.0".into()));
    }

    #[test]
    fn patch_replaces_tags_entirely() {
        let mut manifest = sample_manifest();
        let mut patch = BTreeMap::new();
        patch.insert(
            "tags".into(),
            Value::List(vec![Value::String("new-tag".into())]),
        );
        manifest.apply_patch(&patch);
        assert_eq!(manifest.tags, vec!["new-tag"]);
        // Name and other fields unchanged.
        assert_eq!(manifest.name, "MIDI Bridge");
        assert_eq!(manifest.description, Some("Provides MIDI input and output integration.".into()));
    }

    #[test]
    fn patch_replaces_routes_entirely() {
        let mut manifest = sample_manifest();
        assert_eq!(manifest.routes.len(), 2);

        let mut patch = BTreeMap::new();
        let new_route = RouteInfo {
            address: "/new/route".into(),
            signal: SignalType::Stream,
            payload_hint: None,
            description: None,
            example: None,
        };
        patch.insert("routes".into(), Value::List(vec![new_route.to_value()]));
        manifest.apply_patch(&patch);

        assert_eq!(manifest.routes.len(), 1);
        assert_eq!(manifest.routes[0].address, "/new/route");
        assert_eq!(manifest.routes[0].signal, SignalType::Stream);
    }

    #[test]
    fn patch_can_clear_optional_fields_with_null() {
        let mut manifest = sample_manifest();
        assert!(manifest.description.is_some());
        assert!(manifest.version.is_some());

        let mut patch = BTreeMap::new();
        patch.insert("description".into(), Value::Null);
        patch.insert("version".into(), Value::Null);
        manifest.apply_patch(&patch);

        assert_eq!(manifest.description, None);
        assert_eq!(manifest.version, None);
        // Name still intact.
        assert_eq!(manifest.name, "MIDI Bridge");
    }

    #[test]
    fn patch_updates_name() {
        let mut manifest = sample_manifest();
        let mut patch = BTreeMap::new();
        patch.insert("name".into(), Value::String("Renamed Bridge".into()));
        manifest.apply_patch(&patch);
        assert_eq!(manifest.name, "Renamed Bridge");
    }

    #[test]
    fn empty_patch_leaves_manifest_unchanged() {
        let original = sample_manifest();
        let mut manifest = original.clone();
        let patch = BTreeMap::new();
        manifest.apply_patch(&patch);
        assert_eq!(manifest, original);
    }

    #[test]
    fn patch_multiple_fields_at_once() {
        let mut manifest = VoiceManifest::new("Original");
        let mut patch = BTreeMap::new();
        patch.insert("name".into(), Value::String("Updated".into()));
        patch.insert(
            "description".into(),
            Value::String("A new description.".into()),
        );
        patch.insert(
            "provides".into(),
            Value::List(vec![Value::String("audio-output".into())]),
        );
        patch.insert(
            "expects".into(),
            Value::List(vec![Value::String("audio-input".into())]),
        );
        manifest.apply_patch(&patch);

        assert_eq!(manifest.name, "Updated");
        assert_eq!(manifest.description, Some("A new description.".into()));
        assert_eq!(manifest.provides, vec!["audio-output"]);
        assert_eq!(manifest.expects, vec!["audio-input"]);
    }

    #[test]
    fn invalid_signal_type_in_route_returns_none() {
        let mut m = BTreeMap::new();
        m.insert("address".into(), Value::String("/test".into()));
        m.insert("signal".into(), Value::String("invalid".into()));
        assert!(RouteInfo::from_value(&Value::Map(m)).is_none());
    }

    #[test]
    fn invalid_route_entries_are_skipped() {
        let mut manifest = VoiceManifest::new("Test");
        let mut patch = BTreeMap::new();
        // Mix of valid and invalid route entries.
        let valid_route = RouteInfo {
            address: "/valid".into(),
            signal: SignalType::Event,
            payload_hint: None,
            description: None,
            example: None,
        };
        patch.insert(
            "routes".into(),
            Value::List(vec![
                valid_route.to_value(),
                Value::String("not a route".into()), // Invalid — should be skipped.
                Value::Integer(42),                   // Invalid — should be skipped.
            ]),
        );
        manifest.apply_patch(&patch);
        assert_eq!(manifest.routes.len(), 1);
        assert_eq!(manifest.routes[0].address, "/valid");
    }
}
