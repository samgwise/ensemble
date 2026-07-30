//! Conformance test suite — loads YAML fixtures and verifies them against
//! the Ensemble reference implementation.
//!
//! Every suite is driven by its fixture file: cases, steps, and expectations
//! come from ensemble-test-fixtures, so drift between the spec and the
//! implementation fails the suite. Synchronisation uses poll-based state
//! observation rather than fixed sleeps wherever the hub exposes the state.
//!
//! Organised by certification level:
//! - **Core**: routing, values, protocol, lifecycle
//! - **Full**: core + scheduling, params, manifests

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ensemble_conformance::*;
use ensemble_hub::SharedState;
use ensemble_routing::Pattern;
use tokio::io::{BufReader, BufWriter};
use tokio::net::TcpStream;

// ===========================================================================
// Shared scenario runner
// ===========================================================================

/// Drives fixture "steps" against a live hub. Voices are tracked by name;
/// actions and inline expects operate on the most recently connected voice.
struct Scenario {
    state: SharedState,
    port: u16,
    voices: HashMap<String, Hub>,
    current: Option<String>,
    /// Most recently disconnected/dropped voice, for removal assertions.
    last_removed: Option<VoiceId>,
    /// Deliveries that didn't match an address-specific expect. Replays
    /// arrive in unspecified order, so non-matches are buffered for later
    /// expects rather than discarded.
    pending: Vec<WireMessage>,
}

impl Scenario {
    async fn new() -> Self {
        let (state, port) = start_hub_with_state().await;
        Scenario {
            state,
            port,
            voices: HashMap::new(),
            current: None,
            last_removed: None,
            pending: Vec::new(),
        }
    }

    /// The current (most recently connected) voice.
    fn cur(&mut self) -> &mut Hub {
        let key = self.current.as_ref().expect("no current voice");
        self.voices
            .get_mut(key)
            .expect("current voice not connected")
    }

    /// Connect a voice and make it current. Duplicate display names get a
    /// unique map key so both connections stay alive (the hub accepts them).
    async fn connect(&mut self, name: &str) {
        let hub = Hub::connect(self.port, name)
            .await
            .unwrap_or_else(|e| panic!("connect '{name}' failed: {e}"));
        let key = if self.voices.contains_key(name) {
            format!("{name}#{}", self.voices.len() + 1)
        } else {
            name.to_string()
        };
        self.voices.insert(key.clone(), hub);
        self.current = Some(key);
        wait_for_voice_count(&self.state, self.voices.len()).await;
    }

    /// Detach the current voice (graceful or not) and wait for the hub to
    /// notice. Records the voice for later removal assertions.
    async fn remove_current(&mut self, graceful: bool) {
        let name = self.current.take().expect("no current voice");
        let hub = self.voices.remove(&name).expect("current voice not found");
        self.last_removed = Some(hub.voice_id);
        if graceful {
            hub.disconnect().await;
        } else {
            drop(hub);
        }
        wait_for_voice_count(&self.state, self.voices.len()).await;
    }

    /// Execute all steps of a fixture case.
    async fn run_steps(&mut self, case: &serde_yaml::Value) {
        let steps = case
            .get("steps")
            .and_then(|s| s.as_sequence())
            .expect("case has no steps");
        // Some cases act on a single voice without an explicit connect step —
        // connect one implicitly.
        let starts_with_connect =
            steps.first().and_then(|s| yaml_str(s, "action")).as_deref() == Some("connect");
        if self.current.is_none() && !starts_with_connect {
            self.connect("voice").await;
        }
        for step in steps {
            self.run_step(step).await;
        }
    }

    async fn run_step(&mut self, step: &serde_yaml::Value) {
        if let Some(expect) = yaml_str(step, "expect") {
            self.run_expect(step, &expect).await;
            return;
        }
        match yaml_str(step, "action").as_deref() {
            Some("connect") => {
                let name = yaml_str(step, "name").expect("connect needs a name");
                self.connect(&name).await;
            }
            Some("subscribe") => {
                let pattern = yaml_str(step, "pattern").expect("subscribe needs a pattern");
                let voice_id = self.cur().voice_id;
                self.cur().subscribe(&pattern).await.expect("subscribe");
                wait_for_subscription(&self.state, voice_id, &pattern).await;
            }
            Some("send_action") => {
                // A step may name a specific voice to act on (without changing
                // the current voice used by later expects).
                let voice = match yaml_str(step, "voice") {
                    Some(name) => self
                        .voices
                        .get_mut(&name)
                        .unwrap_or_else(|| panic!("step voice '{name}' is not connected")),
                    None => self.cur(),
                };
                let now = voice.now().await;
                let msg = build_action(step, now);
                let address = yaml_str(step, "address").unwrap_or_default();
                let signal = parse_signal_type(
                    &yaml_str(step, "signal_type").unwrap_or_else(|| "event".into()),
                )
                .unwrap_or(SignalType::Event);
                let timestamp = get_float(&payload_map(&msg), "timestamp").unwrap_or(0.0);
                let expected_payload = get_value(&payload_map(&msg), "payload");
                voice.send_action(msg).await.expect("send_action");
                // Immediate params are stored before later subscribers can
                // join — wait for the value so replays are deterministic.
                // Future params activate later, so there is nothing to wait on.
                if signal == SignalType::Param && timestamp <= now {
                    wait_for_param_value(&self.state, &address, &expected_payload.unwrap()).await;
                }
            }
            Some("unset_param") => {
                let address = yaml_str(step, "address").expect("unset_param needs an address");
                self.cur()
                    .sender()
                    .send(unset_param(&address))
                    .await
                    .expect("unset_param send");
                wait_for_param_absent(&self.state, &address).await;
            }
            Some("disconnect") => self.remove_current(true).await,
            Some("drop_connection") => self.remove_current(false).await,
            Some("set_manifest") => {
                let manifest =
                    yaml_to_manifest(step.get("manifest").expect("set_manifest needs a manifest"));
                let voice_id = self.cur().voice_id;
                self.cur()
                    .set_manifest(&manifest)
                    .await
                    .expect("set_manifest");
                // Wait for THIS manifest to be stored — a plain existence
                // wait would pass on a previous set.
                wait_for_manifest_eq(&self.state, voice_id, &manifest).await;
            }
            Some("patch_manifest") => {
                let patch =
                    yaml_plain_to_value(step.get("patch").expect("patch_manifest needs a patch"));
                let voice_id = self.cur().voice_id;
                // Compute the expected result (existing manifest or default
                // plus the patch) and wait for it to be applied.
                let Value::Map(patch_map) = patch.clone() else {
                    panic!("patch_manifest fixture patch must be a map")
                };
                let mut expected = {
                    let st = self.state.lock().await;
                    st.manifest(voice_id).cloned().unwrap_or_default()
                };
                expected.apply_patch(&patch_map);
                self.cur()
                    .patch_manifest(patch)
                    .await
                    .expect("patch_manifest");
                wait_for_manifest_eq(&self.state, voice_id, &expected).await;
            }
            Some("wait_ms") => {
                let ms = yaml_i64(step, "duration").expect("wait_ms needs a duration");
                tokio::time::sleep(Duration::from_millis(ms as u64)).await;
            }
            Some(other) => panic!("unknown step action in fixture: '{other}'"),
            None => panic!("fixture step has neither action nor expect: {step:?}"),
        }
    }

    async fn run_expect(&mut self, step: &serde_yaml::Value, expect: &str) {
        match expect {
            // The handshake is already validated by connect().
            "welcome" => {}
            "param_replay" | "action_received" => {
                let want_address = yaml_str(step, "address");
                let msg = self.recv_until(want_address.as_deref()).await;
                if let Some(expected) = yaml_f64(step, "expected_value") {
                    let map = payload_map(&msg);
                    let payload = get_float(&map, "payload")
                        .unwrap_or_else(|| panic!("param_replay payload must be a float"));
                    assert!(
                        (payload - expected).abs() < 0.001,
                        "param_replay: expected value {expected}, got {payload}"
                    );
                }
            }
            "no_param_replay" | "no_additional_replay" | "no_routing" => {
                assert_silence(self.cur(), 400, expect).await;
            }
            "manifest_removed" => {
                let removed = self
                    .last_removed
                    .expect("manifest_removed expects a prior disconnect");
                wait_for_manifest_absent(&self.state, removed).await;
            }
            other => panic!("unknown expect in fixture: '{other}'"),
        }
    }

    /// Receive actions until one arrives (optionally until one matches
    /// `want_address`). Buffered non-matches are checked first; new
    /// non-matches are buffered for later expects.
    async fn recv_until(&mut self, want_address: Option<&str>) -> WireMessage {
        let matches = |msg: &WireMessage| {
            want_address.is_none_or(|addr| {
                get_string(&payload_map(msg), "address").as_deref() == Some(addr)
            })
        };
        if let Some(pos) = self.pending.iter().position(matches) {
            return self.pending.remove(pos);
        }
        for _ in 0..5 {
            let msg = tokio::time::timeout(Duration::from_secs(2), self.cur().recv_action())
                .await
                .expect("timed out waiting for action/replay")
                .expect("connection closed while waiting for action/replay");
            if matches(&msg) {
                return msg;
            }
            self.pending.push(msg);
        }
        panic!("expected an action at '{want_address:?}' within 5 deliveries");
    }
}

/// Build an action WireMessage from a fixture step/action spec.
fn build_action(spec: &serde_yaml::Value, now: f64) -> WireMessage {
    let address = yaml_str(spec, "address").expect("action needs an address");
    let signal =
        parse_signal_type(&yaml_str(spec, "signal_type").unwrap_or_else(|| "event".into()))
            .unwrap_or(SignalType::Event);
    let timestamp = if let Some(ts) = yaml_f64(spec, "timestamp") {
        ts
    } else if let Some(offset) = yaml_f64(spec, "timestamp_offset") {
        now + offset
    } else {
        0.0
    };
    let payload = spec
        .get("payload")
        .map(yaml_typed_to_value)
        .unwrap_or(Value::Null);
    action(&address, signal, timestamp, payload)
}

/// Assert that no action arrives within a short window.
async fn assert_silence(hub: &mut Hub, millis: u64, what: &str) {
    let result = tokio::time::timeout(Duration::from_millis(millis), hub.recv_action()).await;
    assert!(
        result.is_err(),
        "expected silence ({what}), but a message was delivered: {:?}",
        result.ok().flatten().map(|m| m.msg_type)
    );
}

/// The variant name of a Value, for expected_type assertions.
fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Integer(_) => "integer",
        Value::Float(_) => "float",
        Value::String(_) => "string",
        Value::Binary(_) => "binary",
        Value::Tuple(_) => "tuple",
        Value::List(_) => "list",
        Value::Map(_) => "map",
        Value::TypedBinary { .. } => "typed_binary",
    }
}

// ===========================================================================
// CORE: Routing suite
// ===========================================================================

mod routing {
    use super::*;

    #[test]
    fn pattern_matching_fixtures() {
        let fixture = load_fixture("routing/pattern_matching.yaml");
        let cases = fixture.get("cases").unwrap().as_sequence().unwrap();

        for case in cases {
            let name = yaml_str(case, "name").unwrap();
            let pattern_str = yaml_str(case, "pattern").unwrap();
            let address = yaml_str(case, "address").unwrap();
            let should_match = yaml_bool(case, "should_match").unwrap();

            let pat = Pattern::parse(&pattern_str)
                .unwrap_or_else(|e| panic!("Fixture '{}': pattern parse failed: {}", name, e));

            let result = pat.matches(&address);

            if should_match {
                assert!(
                    result.is_some(),
                    "Fixture '{name}': expected match of '{address}' against '{pattern_str}'"
                );

                // Verify captures if specified.
                if let Some(captures_map) = yaml_map(case, "captures") {
                    let caps = result.unwrap();
                    for (k, v) in captures_map {
                        let key = k.as_str().unwrap();
                        let expected = v.as_str().unwrap();
                        assert_eq!(
                            caps.get(key),
                            Some(expected),
                            "Fixture '{name}': capture '{key}' expected '{expected}'"
                        );
                    }
                }
            } else {
                assert!(
                    result.is_none(),
                    "Fixture '{name}': expected no match of '{address}' against '{pattern_str}'"
                );
            }
        }
    }

    #[test]
    fn invalid_patterns_fixtures() {
        let fixture = load_fixture("routing/invalid_patterns.yaml");
        let cases = fixture.get("cases").unwrap().as_sequence().unwrap();

        for case in cases {
            let name = yaml_str(case, "name").unwrap();
            let pattern_str = yaml_str(case, "pattern").unwrap();
            let should_reject = yaml_bool(case, "should_reject").unwrap_or(false);

            if should_reject {
                assert!(
                    Pattern::parse(&pattern_str).is_err(),
                    "Fixture '{name}': expected '{pattern_str}' to be rejected"
                );
            } else {
                assert!(
                    Pattern::parse(&pattern_str).is_ok(),
                    "Fixture '{name}': expected '{pattern_str}' to parse"
                );
            }
        }
    }

    #[tokio::test]
    async fn namespace_enforcement_fixtures() {
        let fixture = load_fixture("routing/namespace_enforcement.yaml");
        let cases = fixture.get("cases").unwrap().as_sequence().unwrap();

        // One hub for the whole suite (previously a fresh hub per case leaked).
        let mut sc = Scenario::new().await;
        sc.connect("sender").await;
        sc.connect("receiver").await;
        let receiver_id = sc.cur().voice_id;
        sc.cur().subscribe("/**").await.expect("subscribe");
        wait_for_subscription(&sc.state, receiver_id, "/**").await;
        // Sender becomes current again for sending.
        sc.current = Some("sender".into());

        for case in cases {
            let name = yaml_str(case, "name").unwrap();
            let address = yaml_str(case, "address").unwrap();
            let should_reject = yaml_bool(case, "should_reject").unwrap_or(false);
            let signal = parse_signal_type(&yaml_str(case, "signal_type").unwrap())
                .unwrap_or(SignalType::Event);

            sc.cur()
                .send_action(action(&address, signal, 0.0, Value::Integer(1)))
                .await
                .unwrap();

            if should_reject {
                // The hub must report the fixture's error code to the sender...
                let err = tokio::time::timeout(Duration::from_secs(2), sc.cur().recv_error())
                    .await
                    .unwrap_or_else(|_| panic!("Fixture '{name}': timed out waiting for hub error"))
                    .expect("Fixture '{name}': error channel closed");
                let expected_code = yaml_str(case, "error_code").unwrap();
                assert_eq!(
                    err.code, expected_code,
                    "Fixture '{name}': expected error code '{expected_code}'"
                );
                // ... and the action must not be routed.
                let receiver = sc.voices.get_mut("receiver").unwrap();
                assert_silence(receiver, 300, "namespace rejection delivery").await;
            } else {
                let receiver = sc.voices.get_mut("receiver").unwrap();
                let msg = tokio::time::timeout(Duration::from_secs(2), receiver.recv_action())
                    .await
                    .unwrap_or_else(|_| panic!("Fixture '{name}': timed out waiting for action"))
                    .expect("Fixture '{name}': channel closed");
                let map = payload_map(&msg);
                assert_eq!(
                    get_string(&map, "address").unwrap_or_default(),
                    address,
                    "Fixture '{name}': expected address '{address}'"
                );
            }
        }
    }
}

// ===========================================================================
// CORE: Values suite
// ===========================================================================

mod values {
    use super::*;

    #[test]
    fn type_preservation_fixtures() {
        let fixture = load_fixture("values/type_preservation.yaml");
        let cases = fixture.get("cases").unwrap().as_sequence().unwrap();

        for case in cases {
            let name = yaml_str(case, "name").unwrap();
            let value = yaml_typed_to_value(case.get("value").unwrap());

            // Round-trip through MessagePack (the wire format).
            let encoded = rmp_serde::to_vec(&value)
                .unwrap_or_else(|e| panic!("Fixture '{name}': encode failed: {e}"));
            let decoded: Value = rmp_serde::from_slice(&encoded)
                .unwrap_or_else(|e| panic!("Fixture '{name}': decode failed: {e}"));

            assert_eq!(value, decoded, "Fixture '{name}': value did not round-trip");

            // Assert the fixture's expected_* metadata (previously ignored).
            if let Some(expected_type) = yaml_str(case, "expected_type") {
                assert_eq!(
                    type_name(&decoded),
                    expected_type,
                    "Fixture '{name}': expected type '{expected_type}'"
                );
            }
            if let Some(expected_data) = case.get("expected_data") {
                let expected = yaml_plain_to_value(expected_data);
                match (&decoded, &expected) {
                    (Value::Float(a), Value::Float(b)) => {
                        assert!(
                            (a.value() - b.value()).abs() < 1e-9,
                            "Fixture '{name}': expected data {expected:?}"
                        );
                    }
                    _ => assert_eq!(decoded, expected, "Fixture '{name}': data mismatch"),
                }
            }
            if let Some(expected_len) = yaml_i64(case, "expected_length") {
                let len = match &decoded {
                    Value::Tuple(items) | Value::List(items) => items.len(),
                    Value::Binary(bytes) => bytes.len(),
                    other => panic!("Fixture '{name}': expected_length on {other:?}"),
                };
                assert_eq!(
                    len, expected_len as usize,
                    "Fixture '{name}': length mismatch"
                );
            }
            if let Some(expected_keys) = yaml_seq(case, "expected_keys") {
                let Value::Map(m) = &decoded else {
                    panic!("Fixture '{name}': expected_keys on non-map");
                };
                for key in expected_keys {
                    let key = key.as_str().unwrap();
                    assert!(m.contains_key(key), "Fixture '{name}': missing key '{key}'");
                }
            }
            if let Some(expected_tag) = yaml_str(case, "expected_tag") {
                let Value::TypedBinary { tag, .. } = &decoded else {
                    panic!("Fixture '{name}': expected_tag on non-typed-binary");
                };
                assert_eq!(tag, &expected_tag, "Fixture '{name}': tag mismatch");
            }
        }
    }

    #[test]
    fn type_discrimination_fixtures() {
        let fixture = load_fixture("values/type_discrimination.yaml");
        let cases = fixture.get("cases").unwrap().as_sequence().unwrap();

        for case in cases {
            let name = yaml_str(case, "name").unwrap();
            let value_a = yaml_typed_to_value(case.get("value_a").unwrap());
            let value_b = yaml_typed_to_value(case.get("value_b").unwrap());
            let should_be_equal = yaml_bool(case, "should_be_equal").unwrap_or(false);

            if should_be_equal {
                assert_eq!(
                    value_a, value_b,
                    "Fixture '{name}': expected values to be equal"
                );
            } else {
                assert_ne!(
                    value_a, value_b,
                    "Fixture '{name}': expected values to be distinct"
                );
            }
        }
    }
}

// ===========================================================================
// CORE: Protocol suite
// ===========================================================================

mod protocol {
    use super::*;

    #[test]
    fn error_codes_fixtures() {
        let fixture = load_fixture("protocol/error_codes.yaml");
        let cases = fixture.get("cases").unwrap().as_sequence().unwrap();

        // Verify that all expected error codes exist as constants.
        let known_codes = [
            ERR_UNSUPPORTED_PROTOCOL_VERSION,
            ERR_INVALID_PATTERN,
            ERR_MALFORMED_MANIFEST,
            ERR_INVALID_MESSAGE,
            ERR_INTERNAL_ERROR,
            ERR_RESERVED_NAMESPACE,
        ];

        for case in cases {
            let name = yaml_str(case, "name").unwrap();
            let error_spec = case.get("error").unwrap();
            let expected_code = yaml_str(error_spec, "code").unwrap();

            assert!(
                known_codes.contains(&expected_code.as_str()),
                "Fixture '{name}': error code '{expected_code}' not found in known codes"
            );

            // Verify the error() helper creates a proper error message.
            let message = yaml_str(error_spec, "message").unwrap();
            let err_msg = error(&expected_code, &message);
            assert_eq!(err_msg.msg_type, MSG_ERROR);
            let map = payload_map(&err_msg);
            assert_eq!(get_string(&map, "code").unwrap(), expected_code);
            assert_eq!(get_string(&map, "message").unwrap(), message);
        }
    }

    #[test]
    fn action_structure_fixtures() {
        let fixture = load_fixture("protocol/action_structure.yaml");
        let cases = fixture.get("cases").unwrap().as_sequence().unwrap();

        for case in cases {
            let name = yaml_str(case, "name").unwrap();
            let action_spec = case.get("action").unwrap().get("payload").unwrap();

            // Build the action with the implementation's own constructors...
            let address = yaml_str(action_spec, "address").unwrap();
            let signal = parse_signal_type(&yaml_str(action_spec, "signal_type").unwrap())
                .unwrap_or(SignalType::Event);
            let timestamp = yaml_f64(action_spec, "timestamp").unwrap_or(0.0);
            let payload = action_spec
                .get("payload")
                .map(yaml_typed_to_value)
                .unwrap_or(Value::Null);
            let msg = if let Some(source) = yaml_i64(action_spec, "source") {
                action_with_source(source as u64, &address, signal, timestamp, payload)
            } else {
                action(&address, signal, timestamp, payload)
            };

            // ... round-trip it through the wire format...
            let encoded = rmp_serde::to_vec(&msg)
                .unwrap_or_else(|e| panic!("Fixture '{name}': encode failed: {e}"));
            let decoded: WireMessage = rmp_serde::from_slice(&encoded)
                .unwrap_or_else(|e| panic!("Fixture '{name}': decode failed: {e}"));
            assert_eq!(msg, decoded, "Fixture '{name}': wire round-trip failed");
            assert_eq!(
                decoded.msg_type, MSG_ACTION,
                "Fixture '{name}': wrong msg type"
            );

            // ... and verify the fixture's field expectations on the wire map.
            let map = payload_map(&decoded);
            if let Some(expected_fields) = yaml_seq(case, "expected_fields") {
                for field in expected_fields {
                    let field = field.as_str().unwrap();
                    assert!(
                        map.contains_key(field),
                        "Fixture '{name}': expected field '{field}' on the wire"
                    );
                }
            }
            if let Some(optional_fields) = yaml_seq(case, "optional_fields") {
                for field in optional_fields {
                    let field = field.as_str().unwrap();
                    if action_spec.get(field).is_none() {
                        assert!(
                            !map.contains_key(field),
                            "Fixture '{name}': optional field '{field}' must be omitted when unset"
                        );
                    }
                }
            }
            if let Some(expected_signal) = yaml_str(case, "expected_signal_type") {
                assert_eq!(
                    get_string(&map, "signal_type").unwrap(),
                    expected_signal,
                    "Fixture '{name}': signal type mismatch on the wire"
                );
            }
        }
    }
}

// ===========================================================================
// CORE: Lifecycle suite
// ===========================================================================

mod lifecycle {
    use super::*;

    #[tokio::test]
    async fn voice_registration_fixtures() {
        let fixture = load_fixture("lifecycle/voice_registration.yaml");
        let cases = fixture.get("cases").unwrap().as_sequence().unwrap();

        for case in cases {
            let name = yaml_str(case, "name").unwrap();

            // The version-rejection case needs a raw connection (the client
            // always speaks the current protocol version).
            if name == "protocol_version_check" {
                protocol_version_check_case(case).await;
                continue;
            }

            let mut sc = Scenario::new().await;
            sc.run_steps(case).await;

            match yaml_str(case, "expected_result").as_deref() {
                Some("both_voices_connected") => {
                    assert_eq!(sc.voices.len(), 2, "Fixture '{name}': expected 2 voices");
                }
                Some("distinct_voice_ids") => {
                    let ids: Vec<VoiceId> = sc.voices.values().map(|h| h.voice_id).collect();
                    let unique: std::collections::HashSet<_> = ids.iter().collect();
                    assert_eq!(
                        unique.len(),
                        ids.len(),
                        "Fixture '{name}': voice IDs must be distinct"
                    );
                }
                _ => {}
            }
        }
    }

    /// Send a hello with a non-current protocol version over a raw connection
    /// and assert the hub rejects it with the fixture's error code.
    async fn protocol_version_check_case(case: &serde_yaml::Value) {
        let name = yaml_str(case, "name").unwrap();
        let steps = case.get("steps").unwrap().as_sequence().unwrap();
        let connect_step = &steps[0];
        let version = yaml_i64(connect_step, "protocol_version").unwrap();
        let voice_name = yaml_str(connect_step, "name").unwrap();
        let expected_code = yaml_str(&steps[1], "error_code").unwrap();

        let (_state, port) = start_hub_with_state().await;
        let stream = TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .expect("raw connect");
        let (reader, writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut writer = BufWriter::new(writer);

        // Hand-build the hello so the version field can be invalid.
        let mut hello_map = std::collections::BTreeMap::new();
        hello_map.insert("protocol_version".into(), Value::Integer(version));
        hello_map.insert("name".into(), Value::String(voice_name));
        let hello_msg = WireMessage::new(MSG_HELLO, Value::Map(hello_map));
        ensemble_core::codec::write_message(&mut writer, &hello_msg)
            .await
            .expect("write hello");

        let response = ensemble_core::codec::read_message(&mut reader)
            .await
            .expect("read rejection");
        assert_eq!(
            response.msg_type, MSG_ERROR,
            "Fixture '{name}': expected an error response"
        );
        let map = payload_map(&response);
        assert_eq!(
            get_string(&map, "code").unwrap(),
            expected_code,
            "Fixture '{name}': expected error code '{expected_code}'"
        );

        // The hub must close the connection after rejecting.
        let closed = ensemble_core::codec::read_message(&mut reader).await;
        assert!(
            matches!(closed, Err(ensemble_core::CodecError::ConnectionClosed)),
            "Fixture '{name}': hub must close the connection after rejection"
        );
    }

    #[tokio::test]
    async fn disconnect_cleanup_fixtures() {
        let fixture = load_fixture("lifecycle/disconnect_cleanup.yaml");
        let cases = fixture.get("cases").unwrap().as_sequence().unwrap();

        for case in cases {
            let mut sc = Scenario::new().await;
            sc.run_steps(case).await;

            // The subscriber disconnect case additionally asserts the departed
            // voice's subscriptions are gone from the hub.
            if yaml_str(case, "name").as_deref() == Some("disconnect_removes_subscriptions") {
                assert_eq!(
                    sc.state.lock().await.voices().len(),
                    1,
                    "only the sender should remain connected"
                );
            }
        }
    }
}

// ===========================================================================
// FULL: Scheduling suite
// ===========================================================================

mod scheduling {
    use super::*;

    #[tokio::test]
    async fn dispatch_timing_fixtures() {
        let fixture = load_fixture("scheduling/dispatch_timing.yaml");
        let cases = fixture.get("cases").unwrap().as_sequence().unwrap();

        for case in cases {
            let name = yaml_str(case, "name").unwrap();
            let mut sc = Scenario::new().await;
            sc.connect("sender").await;
            sc.connect("receiver").await;
            let receiver_id = sc.cur().voice_id;
            sc.cur().subscribe("/test").await.expect("subscribe");
            wait_for_subscription(&sc.state, receiver_id, "/test").await;
            sc.current = Some("sender".into());

            if let Some(action_spec) = case.get("action") {
                let before_send = Instant::now();
                let msg = build_action(action_spec, sc.cur().now().await);
                let expected_payload = get_value(&payload_map(&msg), "payload");
                sc.cur().send_action(msg).await.expect("send action");

                let received = tokio::time::timeout(Duration::from_secs(3), async {
                    sc.voices.get_mut("receiver").unwrap().recv_action().await
                })
                .await
                .unwrap_or_else(|_| panic!("Fixture '{name}': timed out waiting for action"))
                .expect("Fixture '{name}': channel closed");
                let elapsed = before_send.elapsed();

                if yaml_str(case, "expected_delivery").as_deref() == Some("scheduled") {
                    let delay_ms = yaml_i64(case, "expected_delay_ms").unwrap_or(0) as u128;
                    assert!(
                        elapsed.as_millis() >= delay_ms.saturating_sub(100),
                        "Fixture '{name}': scheduled action arrived too early: {elapsed:?}"
                    );
                }
                // "immediate" — arrival itself is the assertion.
                let map = payload_map(&received);
                assert_eq!(
                    get_value(&map, "payload"),
                    expected_payload,
                    "Fixture '{name}': payload mismatch"
                );
            } else if let Some(actions_spec) = yaml_seq(case, "actions") {
                // Multiple actions: assert delivery order matches expected_order.
                for action_spec in actions_spec {
                    let msg = build_action(action_spec, sc.cur().now().await);
                    sc.cur().send_action(msg).await.expect("send action");
                }
                let expected_order = yaml_seq(case, "expected_order").unwrap();
                for expected in expected_order {
                    let received = tokio::time::timeout(Duration::from_secs(2), async {
                        sc.voices.get_mut("receiver").unwrap().recv_action().await
                    })
                    .await
                    .unwrap_or_else(|_| panic!("Fixture '{name}': timed out"))
                    .expect("Fixture '{name}': channel closed");
                    let map = payload_map(&received);
                    assert_eq!(
                        get_integer(&map, "payload"),
                        expected.as_i64(),
                        "Fixture '{name}': FIFO ordering violated"
                    );
                }
            } else {
                panic!("Fixture '{name}': case has neither action nor actions");
            }
        }
    }

    #[tokio::test]
    async fn activation_time_fixtures() {
        let fixture = load_fixture("scheduling/activation_time.yaml");
        let cases = fixture.get("cases").unwrap().as_sequence().unwrap();

        for case in cases {
            let mut sc = Scenario::new().await;
            sc.run_steps(case).await;
        }
    }
}

// ===========================================================================
// FULL: Params suite
// ===========================================================================

mod params {
    use super::*;

    #[tokio::test]
    async fn state_management_fixtures() {
        let fixture = load_fixture("params/state_management.yaml");
        let cases = fixture.get("cases").unwrap().as_sequence().unwrap();

        for case in cases {
            let mut sc = Scenario::new().await;
            sc.run_steps(case).await;

            // The unset case additionally asserts hub-side removal (the
            // runner's no_param_replay expect covers the joiner's view).
            if yaml_str(case, "name").as_deref() == Some("unset_param_removes_state") {
                let address = yaml_str(
                    &case.get("steps").unwrap().as_sequence().unwrap()[2],
                    "address",
                )
                .unwrap();
                wait_for_param_absent(&sc.state, &address).await;
            }
        }
    }
}

// ===========================================================================
// FULL: Manifests suite
// ===========================================================================

mod manifests {
    use super::*;

    /// Assert the stored manifest's fields match the fixture's
    /// expected_fields / patch content (absent keys are not checked).
    fn assert_manifest_fields(
        stored: &ensemble_manifest::VoiceManifest,
        expected: &serde_yaml::Mapping,
        case_name: &str,
    ) {
        for (k, v) in expected {
            let key = k.as_str().unwrap();
            match key {
                "name" => assert_eq!(
                    stored.name,
                    v.as_str().unwrap(),
                    "Fixture '{case_name}': name"
                ),
                "description" => {
                    let expected = if v.is_null() {
                        None
                    } else {
                        Some(v.as_str().unwrap().to_string())
                    };
                    assert_eq!(
                        stored.description, expected,
                        "Fixture '{case_name}': description"
                    );
                }
                "version" => {
                    let expected = if v.is_null() {
                        None
                    } else {
                        Some(v.as_str().unwrap().to_string())
                    };
                    assert_eq!(stored.version, expected, "Fixture '{case_name}': version");
                }
                "tags" => {
                    let expected: Vec<String> = v
                        .as_sequence()
                        .unwrap()
                        .iter()
                        .map(|t| t.as_str().unwrap().to_string())
                        .collect();
                    assert_eq!(stored.tags, expected, "Fixture '{case_name}': tags");
                }
                other => panic!("unsupported expected_field '{other}' in manifests fixture"),
            }
        }
    }

    #[tokio::test]
    async fn registration_fixtures() {
        let fixture = load_fixture("manifests/registration.yaml");
        let cases = fixture.get("cases").unwrap().as_sequence().unwrap();

        for case in cases {
            let name = yaml_str(case, "name").unwrap();
            let mut sc = Scenario::new().await;

            if case.get("steps").is_some() {
                sc.run_steps(case).await;
            } else if let Some(manifest_spec) = case.get("manifest") {
                // Case-level manifest: connect a voice and set it.
                sc.connect("voice").await;
                let voice_id = sc.cur().voice_id;
                sc.cur()
                    .set_manifest(&yaml_to_manifest(manifest_spec))
                    .await
                    .expect("set_manifest");
                wait_for_manifest(&sc.state, voice_id).await;
            }

            // Voice whose manifest we assert on (current, or the one just set).
            let voice_id = sc.cur().voice_id;
            let stored = {
                let st = sc.state.lock().await;
                st.manifest(voice_id).cloned()
            };

            match yaml_str(case, "expected_result").as_deref() {
                Some("manifest_stored") => {
                    let stored = stored.expect("Fixture '{name}': manifest not stored");
                    let expected = yaml_to_manifest(case.get("manifest").unwrap());
                    assert_eq!(
                        stored, expected,
                        "Fixture '{name}': stored manifest differs"
                    );
                    if let Some(defaults) = yaml_map(case, "expected_defaults") {
                        for key in defaults.keys() {
                            match key.as_str().unwrap() {
                                "description" => assert!(stored.description.is_none()),
                                "version" => assert!(stored.version.is_none()),
                                "tags" => assert!(stored.tags.is_empty()),
                                "provides" => assert!(stored.provides.is_empty()),
                                "expects" => assert!(stored.expects.is_empty()),
                                "routes" => assert!(stored.routes.is_empty()),
                                other => panic!("unsupported expected_default '{other}'"),
                            }
                        }
                    }
                }
                Some("second_manifest_stored") => {
                    // The last set_manifest step's manifest must be the stored one.
                    let steps = case.get("steps").unwrap().as_sequence().unwrap();
                    let last_manifest = steps
                        .iter()
                        .rfind(|s| yaml_str(s, "action").as_deref() == Some("set_manifest"))
                        .and_then(|s| s.get("manifest"))
                        .expect("no set_manifest step");
                    let expected = yaml_to_manifest(last_manifest);
                    assert_eq!(
                        stored.expect("Fixture '{name}': no manifest stored"),
                        expected,
                        "Fixture '{name}': second manifest should replace the first"
                    );
                }
                Some("fields_updated") | Some("fields_cleared") => {
                    let stored = stored.expect("Fixture '{name}': no manifest stored");
                    let expected = yaml_map(case, "expected_fields").unwrap();
                    assert_manifest_fields(&stored, expected, &name);
                }
                Some("manifest_created_and_patched") => {
                    // Patch without a prior set must create and patch a manifest.
                    let stored = stored.expect("Fixture '{name}': manifest not created by patch");
                    let steps = case.get("steps").unwrap().as_sequence().unwrap();
                    let patch = steps
                        .iter()
                        .find(|s| yaml_str(s, "action").as_deref() == Some("patch_manifest"))
                        .and_then(|s| s.get("patch"))
                        .and_then(|p| p.as_mapping())
                        .expect("no patch step");
                    assert_manifest_fields(&stored, patch, &name);
                }
                _ => {} // manifest_does_not_affect_routing asserts via step expects.
            }
        }
    }
}
