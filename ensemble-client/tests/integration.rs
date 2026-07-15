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
}

impl TestHub {
    fn new() -> Self {
        Self {
            clock_origin: Instant::now(),
            next_id: 1,
            voices: HashMap::new(),
            param_state: HashMap::new(),
            schedule: BTreeMap::new(),
        }
    }

    fn now(&self) -> f64 {
        self.clock_origin.elapsed().as_secs_f64()
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
                    MSG_DISCONNECT | _ => {
                        let mut h = hub.lock().await;
                        h.voices.remove(&voice_id);
                        break;
                    }
                }
            }
            Err(_) => {
                let mut h = hub.lock().await;
                h.voices.remove(&voice_id);
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
