//! Conformance test suite — loads YAML fixtures and verifies against the
//! Ensemble reference implementation.
//!
//! Organised by certification level:
//! - **Core**: routing, values, protocol, lifecycle
//! - **Full**: core + scheduling, params, manifests

use std::collections::BTreeMap;
use std::time::Instant;

use ensemble_conformance::*;
use ensemble_core::protocol::*;
use ensemble_routing::Pattern;

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
                    "Fixture '{}': expected match of '{}' against '{}'",
                    name,
                    address,
                    pattern_str
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
                            "Fixture '{}': capture '{}' expected '{}'",
                            name,
                            key,
                            expected
                        );
                    }
                }
            } else {
                assert!(
                    result.is_none(),
                    "Fixture '{}': expected no match of '{}' against '{}'",
                    name,
                    address,
                    pattern_str
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
                    "Fixture '{}': expected '{}' to be rejected",
                    name,
                    pattern_str
                );
            } else {
                assert!(
                    Pattern::parse(&pattern_str).is_ok(),
                    "Fixture '{}': expected '{}' to parse",
                    name,
                    pattern_str
                );
            }
        }
    }

    #[tokio::test]
    async fn namespace_enforcement_fixtures() {
        let fixture = load_fixture("routing/namespace_enforcement.yaml");
        let cases = fixture.get("cases").unwrap().as_sequence().unwrap();

        for case in cases {
            let name = yaml_str(case, "name").unwrap();
            let address = yaml_str(case, "address").unwrap();
            let should_reject = yaml_bool(case, "should_reject").unwrap_or(false);

            let port = start_hub().await;
            let sender = Hub::connect(port, "sender").await.unwrap();
            let mut receiver = Hub::connect(port, "receiver").await.unwrap();

            receiver.subscribe("/**").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            let signal_type = match yaml_str(case, "signal_type").unwrap().as_str() {
                "param" => SignalType::Param,
                "stream" => SignalType::Stream,
                _ => SignalType::Event,
            };

            sender
                .send_action(action(&address, signal_type, 0.0, Value::Integer(1)))
                .await
                .unwrap();

            if should_reject {
                let result = tokio::time::timeout(
                    std::time::Duration::from_millis(300),
                    receiver.recv_action(),
                )
                .await;
                assert!(
                    result.is_err(),
                    "Fixture '{}': action to '{}' should be rejected",
                    name,
                    address
                );
            } else {
                let msg =
                    tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv_action())
                        .await
                        .unwrap_or_else(|_| {
                            panic!("Fixture '{}': timed out waiting for action", name)
                        })
                        .expect("Fixture '{}': channel closed");
                let map = payload_map(&msg);
                assert_eq!(
                    get_string(&map, "address").unwrap_or_default(),
                    address,
                    "Fixture '{}': expected address '{}'",
                    name,
                    address
                );
            }

            sender.disconnect().await;
            receiver.disconnect().await;
        }
    }
}

// ===========================================================================
// CORE: Values suite
// ===========================================================================

mod values {
    use super::*;
    use ensemble_values::FloatValue;

    /// Convert a YAML fixture value spec to an Ensemble Value.
    fn yaml_to_value(v: &serde_yaml::Value) -> Value {
        let type_str = yaml_str(v, "type").unwrap_or_else(|| "null".into());
        match type_str.as_str() {
            "null" => Value::Null,
            "bool" => Value::Bool(v.get("data").unwrap().as_bool().unwrap()),
            "integer" => Value::Integer(v.get("data").unwrap().as_i64().unwrap()),
            "float" => Value::Float(FloatValue::new(v.get("data").unwrap().as_f64().unwrap())),
            "string" => Value::String(v.get("data").unwrap().as_str().unwrap().to_string()),
            "tuple" => {
                let items = v.get("data").unwrap().as_sequence().unwrap();
                Value::Tuple(items.iter().map(yaml_to_value).collect())
            }
            "list" => {
                let items = v.get("data").unwrap().as_sequence().unwrap();
                Value::List(items.iter().map(yaml_to_value).collect())
            }
            "map" => {
                let mapping = v.get("data").unwrap().as_mapping().unwrap();
                let mut m = BTreeMap::new();
                for (k, val) in mapping {
                    m.insert(k.as_str().unwrap().to_string(), yaml_to_value(val));
                }
                Value::Map(m)
            }
            "typed_binary" => {
                let tag = yaml_str(v, "tag").unwrap();
                let data = v.get("data").unwrap().as_sequence().unwrap();
                let bytes: Vec<u8> = data.iter().map(|d| d.as_i64().unwrap() as u8).collect();
                Value::TypedBinary { tag, data: bytes }
            }
            other => panic!("Unknown value type in fixture: {}", other),
        }
    }

    #[test]
    fn type_preservation_fixtures() {
        let fixture = load_fixture("values/type_preservation.yaml");
        let cases = fixture.get("cases").unwrap().as_sequence().unwrap();

        for case in cases {
            let name = yaml_str(case, "name").unwrap();
            let value_spec = case.get("value").unwrap();
            let value = yaml_to_value(value_spec);

            // Round-trip through MessagePack (the wire format).
            let encoded = rmp_serde::to_vec(&value)
                .unwrap_or_else(|e| panic!("Fixture '{}': encode failed: {}", name, e));
            let decoded: Value = rmp_serde::from_slice(&encoded)
                .unwrap_or_else(|e| panic!("Fixture '{}': decode failed: {}", name, e));

            assert_eq!(
                value, decoded,
                "Fixture '{}': value did not round-trip correctly",
                name
            );
        }
    }

    #[test]
    fn type_discrimination_fixtures() {
        let fixture = load_fixture("values/type_discrimination.yaml");
        let cases = fixture.get("cases").unwrap().as_sequence().unwrap();

        for case in cases {
            let name = yaml_str(case, "name").unwrap();
            let value_a = yaml_to_value(case.get("value_a").unwrap());
            let value_b = yaml_to_value(case.get("value_b").unwrap());
            let should_be_equal = yaml_bool(case, "should_be_equal").unwrap_or(false);

            if should_be_equal {
                assert_eq!(
                    value_a, value_b,
                    "Fixture '{}': expected values to be equal",
                    name
                );
            } else {
                assert_ne!(
                    value_a, value_b,
                    "Fixture '{}': expected values to be distinct",
                    name
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
        let known_codes = vec![
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
                "Fixture '{}': error code '{}' not found in known codes",
                name,
                expected_code
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

            // Verify signal type fixtures.
            if let Some(expected_signal) = yaml_str(case, "expected_signal_type") {
                let action_payload = case.get("action").unwrap().get("payload").unwrap();
                let signal_str = yaml_str(action_payload, "signal_type").unwrap();
                assert_eq!(
                    signal_str, expected_signal,
                    "Fixture '{}': signal type mismatch",
                    name
                );
            }

            // Verify expected fields.
            if let Some(expected_fields) = yaml_seq(case, "expected_fields") {
                let action_payload = case.get("action").unwrap().get("payload").unwrap();
                for field in expected_fields {
                    let field_name = field.as_str().unwrap();
                    assert!(
                        action_payload.get(field_name).is_some(),
                        "Fixture '{}': expected field '{}' in action payload",
                        name,
                        field_name
                    );
                }
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
        let port = start_hub().await;

        // hello_welcome_handshake
        let hub = Hub::connect(port, "test-voice").await.unwrap();
        assert!(hub.voice_id > 0, "Should have received a voice_id");
        hub.disconnect().await;

        // duplicate_names_accepted
        let hub_a = Hub::connect(port, "same-name").await.unwrap();
        let hub_b = Hub::connect(port, "same-name").await.unwrap();
        assert_ne!(
            hub_a.voice_id, hub_b.voice_id,
            "Duplicate names should get distinct IDs"
        );
        hub_a.disconnect().await;
        hub_b.disconnect().await;

        // voice_id_assignment
        let hub_1 = Hub::connect(port, "voice-1").await.unwrap();
        let hub_2 = Hub::connect(port, "voice-2").await.unwrap();
        assert_ne!(hub_1.voice_id, hub_2.voice_id, "Voice IDs must be distinct");
        hub_1.disconnect().await;
        hub_2.disconnect().await;
    }

    #[tokio::test]
    async fn disconnect_cleanup_fixtures() {
        // graceful_disconnect
        {
            let port = start_hub().await;
            let setter = Hub::connect(port, "setter").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            setter
                .send_action(action(
                    "/temp",
                    SignalType::Param,
                    0.0,
                    Value::Float(FloatValue::new(22.5)),
                ))
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            setter.disconnect().await;
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            let mut joiner = Hub::connect(port, "joiner").await.unwrap();
            joiner.subscribe("/temp").await.unwrap();

            let result =
                tokio::time::timeout(std::time::Duration::from_millis(300), joiner.recv_action())
                    .await;
            assert!(
                result.is_err(),
                "Disconnected voice's params should be cleaned up"
            );
            joiner.disconnect().await;
        }

        // ungraceful_disconnect
        {
            let port = start_hub().await;
            {
                let setter = Hub::connect(port, "setter").await.unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                setter
                    .send_action(action(
                        "/pressure",
                        SignalType::Param,
                        0.0,
                        Value::Float(FloatValue::new(1.0)),
                    ))
                    .await
                    .unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                drop(setter); // ungraceful
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;

            let mut joiner = Hub::connect(port, "joiner").await.unwrap();
            joiner.subscribe("/pressure").await.unwrap();

            let result =
                tokio::time::timeout(std::time::Duration::from_millis(300), joiner.recv_action())
                    .await;
            assert!(
                result.is_err(),
                "Ungraceful disconnect should clean up params"
            );
            joiner.disconnect().await;
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
        // immediate_dispatch
        {
            let port = start_hub().await;
            let sender = Hub::connect(port, "sender").await.unwrap();
            let mut receiver = Hub::connect(port, "receiver").await.unwrap();
            receiver.subscribe("/test").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            sender
                .send_action(action("/test", SignalType::Event, 0.0, Value::Integer(1)))
                .await
                .unwrap();

            let msg =
                tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv_action())
                    .await
                    .expect("Timed out")
                    .expect("Channel closed");
            let map = payload_map(&msg);
            assert_eq!(get_integer(&map, "payload"), Some(1));

            sender.disconnect().await;
            receiver.disconnect().await;
        }

        // past_timestamp_immediate
        {
            let port = start_hub().await;
            let sender = Hub::connect(port, "sender").await.unwrap();
            let mut receiver = Hub::connect(port, "receiver").await.unwrap();
            receiver.subscribe("/test").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            sender
                .send_action(action("/test", SignalType::Event, -5.0, Value::Integer(2)))
                .await
                .unwrap();

            let msg =
                tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv_action())
                    .await
                    .expect("Timed out")
                    .expect("Channel closed");
            let map = payload_map(&msg);
            assert_eq!(get_integer(&map, "payload"), Some(2));

            sender.disconnect().await;
            receiver.disconnect().await;
        }

        // future_timestamp_scheduled
        {
            let port = start_hub().await;
            let sender = Hub::connect(port, "sender").await.unwrap();
            let mut receiver = Hub::connect(port, "receiver").await.unwrap();
            receiver.subscribe("/test").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;

            let future_time = sender.now().await + 0.5;
            let before_send = Instant::now();

            sender
                .send_action(action(
                    "/test",
                    SignalType::Event,
                    future_time,
                    Value::Integer(3),
                ))
                .await
                .unwrap();

            let msg =
                tokio::time::timeout(std::time::Duration::from_secs(3), receiver.recv_action())
                    .await
                    .expect("Timed out")
                    .expect("Channel closed");

            let elapsed = before_send.elapsed();
            assert!(
                elapsed.as_millis() >= 400,
                "Scheduled action arrived too early: {:?}",
                elapsed
            );
            let map = payload_map(&msg);
            assert_eq!(get_integer(&map, "payload"), Some(3));

            sender.disconnect().await;
            receiver.disconnect().await;
        }

        // fifo_ordering_same_timestamp
        {
            let port = start_hub().await;
            let sender = Hub::connect(port, "sender").await.unwrap();
            let mut receiver = Hub::connect(port, "receiver").await.unwrap();
            receiver.subscribe("/test").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            let now = sender.now().await;
            for i in 0..3 {
                sender
                    .send_action(action("/test", SignalType::Event, now, Value::Integer(i)))
                    .await
                    .unwrap();
            }

            for expected in 0..3 {
                let msg =
                    tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv_action())
                        .await
                        .expect("Timed out")
                        .expect("Channel closed");
                let map = payload_map(&msg);
                assert_eq!(
                    get_integer(&map, "payload"),
                    Some(expected),
                    "FIFO ordering violated"
                );
            }

            sender.disconnect().await;
            receiver.disconnect().await;
        }
    }

    #[tokio::test]
    async fn activation_time_fixtures() {
        // future_param_not_in_snapshot
        {
            let port = start_hub().await;
            let setter = Hub::connect(port, "setter").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            setter
                .send_action(action(
                    "/level",
                    SignalType::Param,
                    0.0,
                    Value::Float(FloatValue::new(0.5)),
                ))
                .await
                .unwrap();

            let future_time = setter.now().await + 0.5;
            setter
                .send_action(action(
                    "/level",
                    SignalType::Param,
                    future_time,
                    Value::Float(FloatValue::new(0.8)),
                ))
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            let mut joiner = Hub::connect(port, "joiner").await.unwrap();
            joiner.subscribe("/level").await.unwrap();

            let msg = tokio::time::timeout(std::time::Duration::from_secs(2), joiner.recv_action())
                .await
                .expect("Timed out")
                .expect("Channel closed");
            let map = payload_map(&msg);
            let payload = get_float(&map, "payload").unwrap_or(0.0);
            assert!(
                (payload - 0.5).abs() < 0.01,
                "Expected current value 0.5, got {}",
                payload
            );

            setter.disconnect().await;
            joiner.disconnect().await;
        }

        // future_param_activates
        {
            let port = start_hub().await;
            let setter = Hub::connect(port, "setter").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            setter
                .send_action(action(
                    "/level",
                    SignalType::Param,
                    0.0,
                    Value::Float(FloatValue::new(0.5)),
                ))
                .await
                .unwrap();

            let future_time = setter.now().await + 0.5;
            setter
                .send_action(action(
                    "/level",
                    SignalType::Param,
                    future_time,
                    Value::Float(FloatValue::new(0.8)),
                ))
                .await
                .unwrap();

            tokio::time::sleep(std::time::Duration::from_millis(600)).await;

            let mut joiner2 = Hub::connect(port, "joiner2").await.unwrap();
            joiner2.subscribe("/level").await.unwrap();

            let msg2 =
                tokio::time::timeout(std::time::Duration::from_secs(2), joiner2.recv_action())
                    .await
                    .expect("Timed out")
                    .expect("Channel closed");
            let map2 = payload_map(&msg2);
            let payload2 = get_float(&map2, "payload").unwrap_or(0.0);
            assert!(
                (payload2 - 0.8).abs() < 0.01,
                "Expected activated value 0.8, got {}",
                payload2
            );

            setter.disconnect().await;
            joiner2.disconnect().await;
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
        // param_state_stored
        {
            let port = start_hub().await;
            let setter = Hub::connect(port, "setter").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            setter
                .send_action(action(
                    "/synth/cutoff",
                    SignalType::Param,
                    0.0,
                    Value::Float(FloatValue::new(0.7)),
                ))
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            let mut joiner = Hub::connect(port, "joiner").await.unwrap();
            joiner.subscribe("/synth/*").await.unwrap();

            let msg = tokio::time::timeout(std::time::Duration::from_secs(2), joiner.recv_action())
                .await
                .expect("Timed out")
                .expect("Channel closed");
            let map = payload_map(&msg);
            assert_eq!(get_string(&map, "address").unwrap(), "/synth/cutoff");
            let payload = get_float(&map, "payload").unwrap_or(0.0);
            assert!((payload - 0.7).abs() < 0.01);

            setter.disconnect().await;
            joiner.disconnect().await;
        }

        // param_state_overwritten
        {
            let port = start_hub().await;
            let setter = Hub::connect(port, "setter").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            setter
                .send_action(action(
                    "/level",
                    SignalType::Param,
                    0.0,
                    Value::Float(FloatValue::new(0.5)),
                ))
                .await
                .unwrap();
            setter
                .send_action(action(
                    "/level",
                    SignalType::Param,
                    0.0,
                    Value::Float(FloatValue::new(0.8)),
                ))
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            let mut joiner = Hub::connect(port, "joiner").await.unwrap();
            joiner.subscribe("/level").await.unwrap();

            let msg = tokio::time::timeout(std::time::Duration::from_secs(2), joiner.recv_action())
                .await
                .expect("Timed out")
                .expect("Channel closed");
            let map = payload_map(&msg);
            let payload = get_float(&map, "payload").unwrap_or(0.0);
            assert!(
                (payload - 0.8).abs() < 0.01,
                "Expected 0.8, got {}",
                payload
            );

            setter.disconnect().await;
            joiner.disconnect().await;
        }

        // param_scoped_by_address
        {
            let port = start_hub().await;
            let setter = Hub::connect(port, "setter").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            setter
                .send_action(action(
                    "/track/1/volume",
                    SignalType::Param,
                    0.0,
                    Value::Float(FloatValue::new(0.5)),
                ))
                .await
                .unwrap();
            setter
                .send_action(action(
                    "/track/2/volume",
                    SignalType::Param,
                    0.0,
                    Value::Float(FloatValue::new(0.8)),
                ))
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            let mut joiner = Hub::connect(port, "joiner").await.unwrap();
            joiner.subscribe("/track/*/volume").await.unwrap();

            // Should receive two param replays.
            let mut received = std::collections::HashMap::new();
            for _ in 0..2 {
                let msg =
                    tokio::time::timeout(std::time::Duration::from_secs(2), joiner.recv_action())
                        .await
                        .expect("Timed out")
                        .expect("Channel closed");
                let map = payload_map(&msg);
                let addr = get_string(&map, "address").unwrap();
                let val = get_float(&map, "payload").unwrap_or(0.0);
                received.insert(addr, val);
            }

            assert!((received["/track/1/volume"] - 0.5).abs() < 0.01);
            assert!((received["/track/2/volume"] - 0.8).abs() < 0.01);

            setter.disconnect().await;
            joiner.disconnect().await;
        }
    }
}

// ===========================================================================
// FULL: Manifests suite
// ===========================================================================

mod manifests {
    use super::*;
    use ensemble_manifest::VoiceManifest;

    #[tokio::test]
    async fn registration_fixtures() {
        // set_manifest
        {
            let port = start_hub().await;
            let hub = Hub::connect(port, "manifest-test").await.unwrap();

            let manifest = VoiceManifest {
                name: "MIDI Bridge".into(),
                description: Some("Provides MIDI input and output".into()),
                version: Some("1.0.0".into()),
                tags: vec!["midi".into(), "bridge".into()],
                provides: vec!["midi-input".into(), "midi-output".into()],
                expects: vec![],
                routes: vec![],
            };
            hub.set_manifest(&manifest).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Verify connection is still alive.
            let mut receiver = Hub::connect(port, "receiver").await.unwrap();
            receiver.subscribe("/test").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            hub.send_action(action("/test", SignalType::Event, 0.0, Value::Null))
                .await
                .unwrap();

            let msg =
                tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv_action())
                    .await
                    .expect("Timed out")
                    .expect("Channel closed");
            let map = payload_map(&msg);
            assert_eq!(get_string(&map, "address").unwrap(), "/test");

            hub.disconnect().await;
            receiver.disconnect().await;
        }

        // minimal_manifest
        {
            let port = start_hub().await;
            let hub = Hub::connect(port, "minimal").await.unwrap();

            let manifest = VoiceManifest::new("Simple Voice");
            hub.set_manifest(&manifest).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Connection should still be alive.
            let mut receiver = Hub::connect(port, "receiver").await.unwrap();
            receiver.subscribe("/test").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            hub.send_action(action("/test", SignalType::Event, 0.0, Value::Null))
                .await
                .unwrap();

            let msg =
                tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv_action())
                    .await
                    .expect("Timed out")
                    .expect("Channel closed");

            hub.disconnect().await;
            receiver.disconnect().await;
        }

        // patch_manifest
        {
            let port = start_hub().await;
            let hub = Hub::connect(port, "patch-test").await.unwrap();

            let manifest = VoiceManifest {
                name: "Original".into(),
                description: Some("Original description".into()),
                version: Some("1.0.0".into()),
                tags: vec!["tag1".into()],
                provides: vec![],
                expects: vec![],
                routes: vec![],
            };
            hub.set_manifest(&manifest).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            let mut patch = BTreeMap::new();
            patch.insert(
                "description".into(),
                Value::String("Updated description".into()),
            );
            patch.insert(
                "tags".into(),
                Value::List(vec![
                    Value::String("tag2".into()),
                    Value::String("tag3".into()),
                ]),
            );
            hub.patch_manifest(Value::Map(patch)).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Connection should still be alive.
            let mut receiver = Hub::connect(port, "receiver").await.unwrap();
            receiver.subscribe("/test").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            hub.send_action(action("/test", SignalType::Event, 0.0, Value::Null))
                .await
                .unwrap();

            let msg =
                tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv_action())
                    .await
                    .expect("Timed out")
                    .expect("Channel closed");

            hub.disconnect().await;
            receiver.disconnect().await;
        }

        // manifest_does_not_affect_routing
        {
            let port = start_hub().await;
            let sender = Hub::connect(port, "sender").await.unwrap();

            let manifest = VoiceManifest {
                name: "Sender".into(),
                description: None,
                version: None,
                tags: vec![],
                provides: vec!["test-output".into()],
                expects: vec![],
                routes: vec![],
            };
            sender.set_manifest(&manifest).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            let mut receiver = Hub::connect(port, "receiver").await.unwrap();
            receiver.subscribe("/test/**").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            sender
                .send_action(action(
                    "/test/foo",
                    SignalType::Event,
                    0.0,
                    Value::Integer(42),
                ))
                .await
                .unwrap();

            let msg =
                tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv_action())
                    .await
                    .expect("Timed out — manifest should not affect routing")
                    .expect("Channel closed");
            let map = payload_map(&msg);
            assert_eq!(get_string(&map, "address").unwrap(), "/test/foo");
            assert_eq!(get_integer(&map, "payload"), Some(42));

            sender.disconnect().await;
            receiver.disconnect().await;
        }
    }
}
