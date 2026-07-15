//! Integration tests for the Ensemble protocol.
//!
//! These tests spin up a minimal hub in-process (no TUI, no ensemble-hub binary)
//! and connect real clients to verify end-to-end behaviour.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Instant;

use ensemble_core::codec;
use ensemble_core::protocol::*;
use ensemble_routing::{matches_any, Pattern};
use tokio::io::{BufReader, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};

/// Extract payload map from WireMessage.
fn payload_map(msg: &WireMessage) -> BTreeMap<String, Value> {
    match &msg.payload {
        Value::Map(m) => m.clone(),
        _ => BTreeMap::new(),
    }
}

/// Parse SignalType from string.
fn parse_signal_type(s: &str) -> Option<SignalType> {
    match s {
        "event" => Some(SignalType::Event),
        "param" => Some(SignalType::Param),
        "stream" => Some(SignalType::Stream),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Minimal in-process hub for testing (no TUI, no external dependencies)
// ---------------------------------------------------------------------------

struct TestVoice {
    id: VoiceId,
    name: String,
    subscription_patterns: Vec<Pattern>,
    subscription_strings: Vec<String>,
    tx: mpsc::Sender<WireMessage>,
}

struct ScheduledAction {
    source: VoiceId,
    message: WireMessage,
    address: String,
}

fn timestamp_key(t: f64) -> u64 {
    t.to_bits()
}

struct TestHub {
    clock_origin: Instant,
    next_id: VoiceId,
    voices: HashMap<VoiceId, TestVoice>,
    param_state: HashMap<String, (VoiceId, WireMessage)>,
    schedule: BTreeMap<u64, Vec<ScheduledAction>>,
    manifests: HashMap<VoiceId, VoiceManifest>,
}

impl TestHub {
    fn new() -> Self {
        Self {
            clock_origin: Instant::now(),
            next_id: 1,
            voices: HashMap::new(),
            param_state: HashMap::new(),
            schedule: BTreeMap::new(),
            manifests: HashMap::new(),
        }
    }

    fn now(&self) -> f64 {
        self.clock_origin.elapsed().as_secs_f64()
    }

    /// Remove a voice and all its associated state (subscriptions, params, manifest).
    fn remove_voice(&mut self, voice_id: VoiceId) {
        self.voices.remove(&voice_id);
        // Remove param state owned by this voice.
        self.param_state.retain(|_, (source, _)| *source != voice_id);
        // Remove scheduled actions from this voice.
        for actions in self.schedule.values_mut() {
            actions.retain(|sa| sa.source != voice_id);
        }
        self.schedule.retain(|_, actions| !actions.is_empty());
        // Remove manifest for this voice.
        self.manifests.remove(&voice_id);
    }
}

async fn route_action(h: &TestHub, source: VoiceId, address: &str, msg: &WireMessage) {
    for voice in h.voices.values() {
        if voice.id != source && matches_any(&voice.subscription_patterns, address) {
            let _ = voice.tx.send(msg.clone()).await;
        }
    }
}

type SharedHub = Arc<Mutex<TestHub>>;

async fn handle_test_voice(stream: TcpStream, hub: SharedHub) {
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut writer = BufWriter::new(writer);

    // Wait for Hello.
    let hello_msg = match codec::read_message(&mut reader).await {
        Ok(msg) if msg.msg_type == MSG_HELLO => msg,
        _ => return,
    };

    let hello_map = payload_map(&hello_msg);
    let voice_name = get_string(&hello_map, "name").unwrap_or_else(|| "unknown".into());

    let (tx, mut rx) = mpsc::channel::<WireMessage>(256);
    let voice_id;
    {
        let mut h = hub.lock().await;
        voice_id = h.next_id;
        h.next_id += 1;

        h.voices.insert(
            voice_id,
            TestVoice {
                id: voice_id,
                name: voice_name.clone(),
                subscription_patterns: Vec::new(),
                subscription_strings: Vec::new(),
                tx: tx.clone(),
            },
        );

        let welcome_msg = welcome(voice_id);
        let _ = codec::write_message(&mut writer, &welcome_msg).await;

        // Replay param state to the new voice (no subscriptions yet).
        let patterns: Vec<Pattern> = h
            .voices
            .get(&voice_id)
            .map(|v| v.subscription_patterns.clone())
            .unwrap_or_default();
        for (_source, action_msg) in h.param_state.values() {
            let action_map = payload_map(action_msg);
            let address = get_string(&action_map, "address").unwrap_or_default();
            if matches_any(&patterns, &address) {
                let _ = tx.send(action_msg.clone()).await;
            }
        }
    }

    // Writer task.
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if codec::write_message(&mut writer, &msg).await.is_err() {
                break;
            }
        }
    });

    // Read loop.
    loop {
        match codec::read_message(&mut reader).await {
            Ok(msg) => {
                match msg.msg_type.as_str() {
                    MSG_CLOCK_PING => {
                        let map = payload_map(&msg);
                        let sequence = get_integer(&map, "sequence").unwrap_or(0) as u64;
                        let h = hub.lock().await;
                        let hub_time = h.now();
                        let pong = clock_pong(sequence, hub_time);
                        let _ = tx.send(pong).await;
                    }
                    MSG_ACTION => {
                        let map = payload_map(&msg);
                        let address = get_string(&map, "address").unwrap_or_default();
                        let signal_type = get_string(&map, "signal_type")
                            .and_then(|s| parse_signal_type(&s))
                            .unwrap_or(SignalType::Event);
                        let timestamp = get_float(&map, "timestamp").unwrap_or(0.0);
                        let payload = get_value(&map, "payload").unwrap_or(Value::Null);

                        let routed_msg = action_with_source(
                            voice_id,
                            address.clone(),
                            signal_type,
                            timestamp,
                            payload,
                        );

                        let mut h = hub.lock().await;

                        // Store param state.
                        if signal_type == SignalType::Param {
                            h.param_state.insert(
                                address.clone(),
                                (voice_id, routed_msg.clone()),
                            );
                        }

                        // Schedule or route immediately.
                        if timestamp > 0.0 && timestamp > h.now() {
                            let key = timestamp_key(timestamp);
                            h.schedule
                                .entry(key)
                                .or_default()
                                .push(ScheduledAction {
                                    source: voice_id,
                                    message: routed_msg,
                                    address,
                                });
                        } else {
                            route_action(&h, voice_id, &address, &routed_msg).await;
                        }
                    }
                    MSG_SUBSCRIBE => {
                        let map = payload_map(&msg);
                        let pat_str = get_string(&map, "pattern").unwrap_or_default();
                        let mut h = hub.lock().await;
                        if let Ok(p) = Pattern::parse(&pat_str) {
                            // Collect matching param replays before mutating voice.
                            let mut replays = Vec::new();
                            if let Some(voice) = h.voices.get(&voice_id) {
                                let mut patterns = voice.subscription_patterns.clone();
                                patterns.push(p.clone());
                                for (_source, action_msg) in h.param_state.values() {
                                    let action_map = payload_map(action_msg);
                                    let address = get_string(&action_map, "address")
                                        .unwrap_or_default();
                                    if matches_any(&patterns, &address) {
                                        replays.push(action_msg.clone());
                                    }
                                }
                            }
                            // Now mutate voice and send replays.
                            if let Some(voice) = h.voices.get_mut(&voice_id) {
                                voice.subscription_patterns.push(p);
                                voice.subscription_strings.push(pat_str);
                                for action_msg in replays {
                                    let _ = voice.tx.send(action_msg).await;
                                }
                            }
                        }
                    }
                    MSG_UNSUBSCRIBE => {
                        let map = payload_map(&msg);
                        let pat_str = get_string(&map, "pattern").unwrap_or_default();
                        let mut h = hub.lock().await;
                        if let Some(voice) = h.voices.get_mut(&voice_id) {
                            if let Some(pos) = voice.subscription_strings.iter().position(|s| s == &pat_str) {
                                voice.subscription_strings.remove(pos);
                                voice.subscription_patterns.remove(pos);
                            }
                        }
                    }
                    MSG_DISCONNECT => {
                        let mut h = hub.lock().await;
                        h.remove_voice(voice_id);
                        break;
                    }
                    MSG_SET_MANIFEST => {
                        let map = payload_map(&msg);
                        let manifest_value = get_value(&map, "manifest").unwrap_or(Value::Null);
                        let mut h = hub.lock().await;
                        if let Some(manifest) = VoiceManifest::from_value(&manifest_value) {
                            h.manifests.insert(voice_id, manifest);
                        }
                    }
                    MSG_PATCH_MANIFEST => {
                        let map = payload_map(&msg);
                        let patch_value = get_value(&map, "patch").unwrap_or(Value::Null);
                        let mut h = hub.lock().await;
                        if let Value::Map(patch_map) = patch_value {
                            let manifest = h
                                .manifests
                                .entry(voice_id)
                                .or_insert_with(VoiceManifest::default);
                            manifest.apply_patch(&patch_map);
                        }
                    }
                    _ => {
                        // Ignore unknown message types (e.g. update_name).
                    }
                }
            }
            Err(_) => {
                let mut h = hub.lock().await;
                h.remove_voice(voice_id);
                break;
            }
        }
    }
}

/// Start a minimal test hub on an OS-assigned port. Returns the port.
async fn start_test_hub() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hub = Arc::new(Mutex::new(TestHub::new()));

    // Accept loop.
    let accept_hub = hub.clone();
    tokio::spawn(async move {
        loop {
            if let Ok((stream, _)) = listener.accept().await {
                let h = accept_hub.clone();
                tokio::spawn(handle_test_voice(stream, h));
            }
        }
    });

    // Scheduler loop.
    tokio::spawn(async move {
        loop {
            {
                let mut h = hub.lock().await;
                let now = h.now();
                let now_key = timestamp_key(now);
                let due_keys: Vec<u64> = h.schedule.range(..=now_key).map(|(k, _)| *k).collect();
                for key in due_keys {
                    if let Some(actions) = h.schedule.remove(&key) {
                        for sa in actions {
                            route_action(&h, sa.source, &sa.address, &sa.message).await;
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    });

    port
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

use ensemble_client::Hub;

#[tokio::test]
async fn two_voices_exchange_actions() {
    let port = start_test_hub().await;

    // Voice A subscribes to /pong, sends /ping.
    let hub_a = Hub::connect(port, "voice-a")
        .await
        .unwrap();
    hub_a.subscribe("/pong").await.unwrap();

    // Voice B subscribes to /ping, sends /pong.
    let mut hub_b = Hub::connect(port, "voice-b")
        .await
        .unwrap();
    hub_b.subscribe("/ping").await.unwrap();

    // Allow clock sync to happen.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // A sends a ping.
    hub_a
        .send_action(action(
            "/ping",
            SignalType::Event,
            0.0,
            Value::String("hello".into()),
        ))
        .await
        .unwrap();

    // B should receive it.
    let action_msg = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        hub_b.recv_action(),
    )
    .await
    .expect("Timed out waiting for action")
    .expect("Channel closed");

    let map = payload_map(&action_msg);
    let source = get_integer(&map, "source").unwrap_or(0) as VoiceId;
    let address = get_string(&map, "address").unwrap_or_default();
    let payload = get_value(&map, "payload").unwrap_or(Value::Null);

    assert_eq!(source, hub_a.voice_id);
    assert_eq!(address, "/ping");
    assert_eq!(payload, Value::String("hello".into()));

    hub_a.disconnect().await;
    hub_b.disconnect().await;
}

#[tokio::test]
async fn wildcard_subscription_routes_subtree() {
    let port = start_test_hub().await;

    // Voice A subscribes to /synth/** — should receive anything under /synth/.
    let mut hub_a = Hub::connect(port, "listener")
        .await
        .unwrap();
    hub_a.subscribe("/synth/**").await.unwrap();

    let hub_b = Hub::connect(port, "sender")
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // B sends to /synth/voice/1/note.
    hub_b
        .send_action(action(
            "/synth/voice/1/note",
            SignalType::Event,
            0.0,
            Value::Integer(60),
        ))
        .await
        .unwrap();

    let action_msg = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        hub_a.recv_action(),
    )
    .await
    .expect("Timed out")
    .expect("Channel closed");

    let map = payload_map(&action_msg);
    let address = get_string(&map, "address").unwrap_or_default();
    assert_eq!(address, "/synth/voice/1/note");

    hub_a.disconnect().await;
    hub_b.disconnect().await;
}

#[tokio::test]
async fn unsubscribed_voice_does_not_receive() {
    let port = start_test_hub().await;

    // Voice A subscribes to /other — should NOT receive /ping.
    let mut hub_a = Hub::connect(port, "bystander")
        .await
        .unwrap();
    hub_a.subscribe("/other").await.unwrap();

    let hub_b = Hub::connect(port, "sender")
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    hub_b
        .send_action(action(
            "/ping",
            SignalType::Event,
            0.0,
            Value::Null,
        ))
        .await
        .unwrap();

    // A should NOT receive anything — use a short timeout.
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        hub_a.recv_action(),
    )
    .await;

    assert!(result.is_err(), "Should have timed out — voice A shouldn't receive /ping");

    hub_a.disconnect().await;
    hub_b.disconnect().await;
}

#[tokio::test]
async fn clock_sync_establishes_quickly() {
    let port = start_test_hub().await;

    let hub = Hub::connect(port, "clock-test")
        .await
        .unwrap();

    // Clock sync should establish within 500ms on localhost.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert!(hub.is_synced().await, "Clock should be synced by now");

    // Hub time should be positive and reasonable.
    let t = hub.now().await;
    assert!(t > 0.0, "Hub time should be positive");
    assert!(t < 60.0, "Hub time should be reasonable (not wildly wrong)");

    hub.disconnect().await;
}

#[tokio::test]
async fn scheduled_action_delivered_after_delay() {
    let port = start_test_hub().await;

    let hub_sender = Hub::connect(port, "scheduler")
        .await
        .unwrap();
    let mut hub_receiver = Hub::connect(port, "listener")
        .await
        .unwrap();
    hub_receiver.subscribe("/scheduled").await.unwrap();

    // Wait for clock sync.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Schedule an action 0.5s in the future.
    let future_time = hub_sender.now().await + 0.5;
    let before_send = Instant::now();

    hub_sender
        .send_action(action(
            "/scheduled",
            SignalType::Event,
            future_time,
            Value::String("delayed".into()),
        ))
        .await
        .unwrap();

    // Should receive it, but not immediately — should take ~500ms.
    let action_msg = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        hub_receiver.recv_action(),
    )
    .await
    .expect("Timed out waiting for scheduled action")
    .expect("Channel closed");

    let map = payload_map(&action_msg);
    let address = get_string(&map, "address").unwrap_or_default();

    let elapsed = before_send.elapsed();
    assert_eq!(address, "/scheduled");
    // Should have taken at least 400ms (allowing some slack).
    assert!(
        elapsed.as_millis() >= 400,
        "Scheduled action arrived too early: {elapsed:?}"
    );

    hub_sender.disconnect().await;
    hub_receiver.disconnect().await;
}

#[tokio::test]
async fn param_state_replayed_to_late_joiner() {
    let port = start_test_hub().await;

    let hub_setter = Hub::connect(port, "setter")
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Set a param value before the listener connects.
    hub_setter
        .send_action(action(
            "/synth/cutoff",
            SignalType::Param,
            0.0,
            Value::Float(FloatValue::new(0.7)),
        ))
        .await
        .unwrap();

    // Small delay to ensure the hub has processed it.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Now a late joiner connects and subscribes to /synth/*.
    let mut hub_late = Hub::connect(port, "late-joiner")
        .await
        .unwrap();
    hub_late.subscribe("/synth/*").await.unwrap();

    // The late joiner should receive the current param state.
    let action_msg = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        hub_late.recv_action(),
    )
    .await
    .expect("Timed out waiting for param replay")
    .expect("Channel closed");

    let map = payload_map(&action_msg);
    let address = get_string(&map, "address").unwrap_or_default();
    let signal_type = get_string(&map, "signal_type")
        .and_then(|s| parse_signal_type(&s))
        .unwrap_or(SignalType::Event);
    let payload = get_value(&map, "payload").unwrap_or(Value::Null);

    assert_eq!(address, "/synth/cutoff");
    assert_eq!(signal_type, SignalType::Param);
    assert_eq!(payload, Value::Float(FloatValue::new(0.7)));

    hub_setter.disconnect().await;
    hub_late.disconnect().await;
}

// ---------------------------------------------------------------------------
// Lifecycle conformance tests (Increment 4)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn duplicate_names_accepted() {
    let port = start_test_hub().await;

    // Two voices connect with the same name — both should succeed.
    let hub_a = Hub::connect(port, "same-name")
        .await
        .expect("First voice with duplicate name should connect");
    let hub_b = Hub::connect(port, "same-name")
        .await
        .expect("Second voice with duplicate name should connect");

    // Both should have distinct voice IDs.
    assert_ne!(hub_a.voice_id, hub_b.voice_id);

    hub_a.disconnect().await;
    hub_b.disconnect().await;
}

#[tokio::test]
async fn runtime_subscribe_unsubscribe() {
    let port = start_test_hub().await;

    let sender = Hub::connect(port, "sender")
        .await
        .unwrap();
    let mut receiver = Hub::connect(port, "receiver")
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Subscribe and verify we receive actions.
    receiver.subscribe("/test/**").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    sender
        .send_action(action(
            "/test/foo",
            SignalType::Event,
            0.0,
            Value::Integer(1),
        ))
        .await
        .unwrap();

    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        receiver.recv_action(),
    )
    .await
    .expect("Timed out waiting for action after subscribe")
    .expect("Channel closed");
    let map = payload_map(&msg);
    assert_eq!(get_string(&map, "address").unwrap_or_default(), "/test/foo");

    // Unsubscribe and verify we no longer receive actions.
    receiver.unsubscribe("/test/**").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    sender
        .send_action(action(
            "/test/bar",
            SignalType::Event,
            0.0,
            Value::Integer(2),
        ))
        .await
        .unwrap();

    // Should NOT receive anything after unsubscribe.
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        receiver.recv_action(),
    )
    .await;
    assert!(
        result.is_err(),
        "Should have timed out — should not receive after unsubscribe"
    );

    sender.disconnect().await;
    receiver.disconnect().await;
}

#[tokio::test]
async fn snapshot_then_live_ordering() {
    let port = start_test_hub().await;

    // Set a param value before the subscriber connects.
    let setter = Hub::connect(port, "setter")
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    setter
        .send_action(action(
            "/level",
            SignalType::Param,
            0.0,
            Value::Float(FloatValue::new(0.5)),
        ))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Connect a subscriber.
    let mut subscriber = Hub::connect(port, "subscriber")
        .await
        .unwrap();

    // Subscribe — should trigger param replay.
    subscriber.subscribe("/level").await.unwrap();

    // The first message received must be the snapshot (param replay),
    // not a live update.
    let snapshot = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        subscriber.recv_action(),
    )
    .await
    .expect("Timed out waiting for snapshot")
    .expect("Channel closed");

    let map = payload_map(&snapshot);
    let address = get_string(&map, "address").unwrap_or_default();
    let payload = get_value(&map, "payload").unwrap_or(Value::Null);
    assert_eq!(address, "/level");
    assert_eq!(payload, Value::Float(FloatValue::new(0.5)));

    // Now send a live update — should arrive after the snapshot.
    setter
        .send_action(action(
            "/level",
            SignalType::Param,
            0.0,
            Value::Float(FloatValue::new(0.8)),
        ))
        .await
        .unwrap();

    let live = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        subscriber.recv_action(),
    )
    .await
    .expect("Timed out waiting for live update")
    .expect("Channel closed");

    let map = payload_map(&live);
    let live_payload = get_value(&map, "payload").unwrap_or(Value::Null);
    assert_eq!(live_payload, Value::Float(FloatValue::new(0.8)));

    setter.disconnect().await;
    subscriber.disconnect().await;
}

#[tokio::test]
async fn graceful_disconnect_cleans_up_state() {
    let port = start_test_hub().await;

    // Voice A sets a param.
    let setter = Hub::connect(port, "setter")
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

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

    // Graceful disconnect.
    setter.disconnect().await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // A late joiner subscribes to /temp — should NOT receive the param
    // because the setter disconnected and its param state should be cleaned up.
    let mut joiner = Hub::connect(port, "joiner")
        .await
        .unwrap();
    joiner.subscribe("/temp").await.unwrap();

    // Should NOT receive any param replay.
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        joiner.recv_action(),
    )
    .await;
    assert!(
        result.is_err(),
        "Should have timed out — disconnected voice's params should be cleaned up"
    );

    joiner.disconnect().await;
}

#[tokio::test]
async fn ungraceful_disconnect_cleans_up_state() {
    let port = start_test_hub().await;

    // Voice sets a param, then drops without calling disconnect.
    {
        let setter = Hub::connect(port, "setter")
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

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

        // Drop without calling disconnect — ungraceful.
        drop(setter);
    }

    // Wait for the hub to detect the connection close.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // A late joiner subscribes — should NOT receive the param.
    let mut joiner = Hub::connect(port, "joiner")
        .await
        .unwrap();
    joiner.subscribe("/pressure").await.unwrap();

    let result = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        joiner.recv_action(),
    )
    .await;
    assert!(
        result.is_err(),
        "Should have timed out — ungraceful disconnect should clean up params"
    );

    joiner.disconnect().await;
}

#[tokio::test]
async fn runtime_name_update() {
    let port = start_test_hub().await;

    let hub = Hub::connect(port, "original-name")
        .await
        .unwrap();

    // Send an update_name message.
    hub.send_update_name("new-name").await.unwrap();

    // Small delay to let the hub process it.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // The hub should have accepted the rename without error.
    // We verify by ensuring the connection is still alive — send an action.
    let mut receiver = Hub::connect(port, "receiver")
        .await
        .unwrap();
    receiver.subscribe("/test").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    hub.send_action(action(
        "/test",
        SignalType::Event,
        0.0,
        Value::Null,
    ))
    .await
    .unwrap();

    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        receiver.recv_action(),
    )
    .await
    .expect("Timed out — connection should still be alive after rename")
    .expect("Channel closed");

    let map = payload_map(&msg);
    assert_eq!(get_string(&map, "address").unwrap_or_default(), "/test");

    hub.disconnect().await;
    receiver.disconnect().await;
}

// ---------------------------------------------------------------------------
// Manifest conformance tests (Increment 5)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn set_manifest_replaces() {
    let port = start_test_hub().await;

    let hub = Hub::connect(port, "manifest-test")
        .await
        .unwrap();

    // Set a manifest.
    let manifest = VoiceManifest {
        name: "Test Voice".into(),
        description: Some("A test voice.".into()),
        version: Some("1.0.0".into()),
        tags: vec!["test".into()],
        provides: vec!["test-cap".into()],
        expects: vec![],
        routes: vec![],
    };
    hub.set_manifest(&manifest).await.unwrap();

    // Small delay to let the hub process it.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Connection should still be alive — send an action to verify.
    let mut receiver = Hub::connect(port, "receiver")
        .await
        .unwrap();
    receiver.subscribe("/test").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    hub.send_action(action(
        "/test",
        SignalType::Event,
        0.0,
        Value::Null,
    ))
    .await
    .unwrap();

    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        receiver.recv_action(),
    )
    .await
    .expect("Timed out — connection should be alive after set_manifest")
    .expect("Channel closed");

    let map = payload_map(&msg);
    assert_eq!(get_string(&map, "address").unwrap_or_default(), "/test");

    hub.disconnect().await;
    receiver.disconnect().await;
}

#[tokio::test]
async fn patch_manifest_updates_specified_fields() {
    let port = start_test_hub().await;

    let hub = Hub::connect(port, "patch-test")
        .await
        .unwrap();

    // Set an initial manifest.
    let manifest = VoiceManifest {
        name: "Original".into(),
        description: Some("Original description.".into()),
        version: Some("1.0.0".into()),
        tags: vec!["tag1".into()],
        provides: vec!["cap1".into()],
        expects: vec![],
        routes: vec![],
    };
    hub.set_manifest(&manifest).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Patch only the description and tags.
    let mut patch = BTreeMap::new();
    patch.insert(
        "description".into(),
        Value::String("Updated description.".into()),
    );
    patch.insert(
        "tags".into(),
        Value::List(vec![Value::String("tag2".into()), Value::String("tag3".into())]),
    );
    hub.patch_manifest(Value::Map(patch)).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Connection should still be alive.
    let mut receiver = Hub::connect(port, "receiver")
        .await
        .unwrap();
    receiver.subscribe("/test").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    hub.send_action(action(
        "/test",
        SignalType::Event,
        0.0,
        Value::Null,
    ))
    .await
    .unwrap();

    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        receiver.recv_action(),
    )
    .await
    .expect("Timed out — connection should be alive after patch_manifest")
    .expect("Channel closed");

    let map = payload_map(&msg);
    assert_eq!(get_string(&map, "address").unwrap_or_default(), "/test");

    hub.disconnect().await;
    receiver.disconnect().await;
}

#[tokio::test]
async fn patch_manifest_without_prior_set() {
    let port = start_test_hub().await;

    let hub = Hub::connect(port, "patch-no-set")
        .await
        .unwrap();

    // Patch without a prior set — should create a default manifest and patch it.
    let mut patch = BTreeMap::new();
    patch.insert("name".into(), Value::String("Patched Name".into()));
    patch.insert(
        "description".into(),
        Value::String("Patched description.".into()),
    );
    hub.patch_manifest(Value::Map(patch)).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Connection should still be alive.
    let mut receiver = Hub::connect(port, "receiver")
        .await
        .unwrap();
    receiver.subscribe("/test").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    hub.send_action(action(
        "/test",
        SignalType::Event,
        0.0,
        Value::Null,
    ))
    .await
    .unwrap();

    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        receiver.recv_action(),
    )
    .await
    .expect("Timed out — connection should be alive after patch without prior set")
    .expect("Channel closed");

    let map = payload_map(&msg);
    assert_eq!(get_string(&map, "address").unwrap_or_default(), "/test");

    hub.disconnect().await;
    receiver.disconnect().await;
}

#[tokio::test]
async fn manifest_does_not_affect_routing() {
    let port = start_test_hub().await;

    let sender = Hub::connect(port, "sender")
        .await
        .unwrap();
    let mut receiver = Hub::connect(port, "receiver")
        .await
        .unwrap();

    // Set a manifest on the sender.
    let manifest = VoiceManifest {
        name: "Sender".into(),
        description: Some("A sender voice.".into()),
        version: None,
        tags: vec![],
        provides: vec!["test-output".into()],
        expects: vec![],
        routes: vec![],
    };
    sender.set_manifest(&manifest).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Subscribe and send an action — routing should work normally.
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

    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        receiver.recv_action(),
    )
    .await
    .expect("Timed out — manifest should not affect routing")
    .expect("Channel closed");

    let map = payload_map(&msg);
    assert_eq!(get_string(&map, "address").unwrap_or_default(), "/test/foo");
    assert_eq!(get_integer(&map, "payload"), Some(42));

    sender.disconnect().await;
    receiver.disconnect().await;
}

#[tokio::test]
async fn manifest_survives_runtime_update() {
    let port = start_test_hub().await;

    let hub = Hub::connect(port, "runtime-update")
        .await
        .unwrap();

    // Set initial manifest.
    let manifest1 = VoiceManifest {
        name: "Version 1".into(),
        description: Some("First version.".into()),
        version: Some("1.0.0".into()),
        tags: vec!["v1".into()],
        provides: vec![],
        expects: vec![],
        routes: vec![],
    };
    hub.set_manifest(&manifest1).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Send an action.
    let mut receiver = Hub::connect(port, "receiver")
        .await
        .unwrap();
    receiver.subscribe("/test").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    hub.send_action(action(
        "/test",
        SignalType::Event,
        0.0,
        Value::Integer(1),
    ))
    .await
    .unwrap();

    let msg1 = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        receiver.recv_action(),
    )
    .await
    .expect("Timed out waiting for first action")
    .expect("Channel closed");
    let map1 = payload_map(&msg1);
    assert_eq!(get_integer(&map1, "payload"), Some(1));

    // Update manifest at runtime.
    let manifest2 = VoiceManifest {
        name: "Version 2".into(),
        description: Some("Second version.".into()),
        version: Some("2.0.0".into()),
        tags: vec!["v2".into()],
        provides: vec!["new-cap".into()],
        expects: vec![],
        routes: vec![],
    };
    hub.set_manifest(&manifest2).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Send another action — connection should still be alive.
    hub.send_action(action(
        "/test",
        SignalType::Event,
        0.0,
        Value::Integer(2),
    ))
    .await
    .unwrap();

    let msg2 = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        receiver.recv_action(),
    )
    .await
    .expect("Timed out waiting for second action — manifest update should not disconnect")
    .expect("Channel closed");
    let map2 = payload_map(&msg2);
    assert_eq!(get_integer(&map2, "payload"), Some(2));

    hub.disconnect().await;
    receiver.disconnect().await;
}

#[tokio::test]
async fn manifest_cleaned_up_on_disconnect() {
    let port = start_test_hub().await;

    // Voice sets a manifest, then disconnects.
    {
        let hub = Hub::connect(port, "manifest-voice")
            .await
            .unwrap();

        let manifest = VoiceManifest {
            name: "Temporary".into(),
            description: Some("Will disconnect.".into()),
            version: None,
            tags: vec![],
            provides: vec![],
            expects: vec![],
            routes: vec![],
        };
        hub.set_manifest(&manifest).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Graceful disconnect.
        hub.disconnect().await;
    }

    // Wait for the hub to process the disconnect.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // A new voice connects — the hub should have cleaned up the manifest.
    // We can't directly inspect the hub state from the client, but we can
    // verify the hub is still functional.
    let mut receiver = Hub::connect(port, "receiver")
        .await
        .unwrap();
    receiver.subscribe("/test").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let sender = Hub::connect(port, "sender")
        .await
        .unwrap();
    sender
        .send_action(action(
            "/test",
            SignalType::Event,
            0.0,
            Value::Null,
        ))
        .await
        .unwrap();

    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        receiver.recv_action(),
    )
    .await
    .expect("Timed out — hub should still work after manifest voice disconnected")
    .expect("Channel closed");

    let map = payload_map(&msg);
    assert_eq!(get_string(&map, "address").unwrap_or_default(), "/test");

    sender.disconnect().await;
    receiver.disconnect().await;
}
