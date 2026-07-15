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

// ---------------------------------------------------------------------------
// Minimal in-process hub for testing (no TUI, no external dependencies)
// ---------------------------------------------------------------------------

struct TestVoice {
    id: VoiceId,
    subscriptions: Vec<String>,
    subscription_patterns: Vec<Pattern>,
    tx: mpsc::Sender<Message>,
}

struct ScheduledAction {
    source: VoiceId,
    action: Action,
}

fn timestamp_key(t: f64) -> u64 {
    t.to_bits()
}

struct TestHub {
    clock_origin: Instant,
    next_id: VoiceId,
    voices: HashMap<VoiceId, TestVoice>,
    param_state: HashMap<String, (VoiceId, Action)>,
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

async fn route_action(h: &TestHub, source: VoiceId, action: &Action) {
    let msg = Message::ActionMessage {
        source,
        action: action.clone(),
    };
    for voice in h.voices.values() {
        if voice.id != source && matches_any(&voice.subscription_patterns, &action.address) {
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
    let hello = match codec::read_message(&mut reader).await {
        Ok(Message::Hello(caps)) => caps,
        _ => return,
    };

    // Parse subscription patterns.
    let subscription_patterns: Vec<Pattern> = hello
        .subscriptions
        .iter()
        .filter_map(|s| Pattern::parse(s).ok())
        .collect();

    let (tx, mut rx) = mpsc::channel::<Message>(256);
    let voice_id;
    {
        let mut h = hub.lock().await;
        voice_id = h.next_id;
        h.next_id += 1;
        let hub_time = h.now();

        h.voices.insert(
            voice_id,
            TestVoice {
                id: voice_id,
                subscriptions: hello.subscriptions.clone(),
                subscription_patterns,
                tx: tx.clone(),
            },
        );

        let welcome = Message::Welcome { voice_id, hub_time };
        let _ = codec::write_message(&mut writer, &welcome).await;

        // Replay param state to the new voice.
        let patterns: Vec<Pattern> = h
            .voices
            .get(&voice_id)
            .map(|v| v.subscription_patterns.clone())
            .unwrap_or_default();
        for (source, action) in h.param_state.values() {
            if matches_any(&patterns, &action.address) {
                let msg = Message::ActionMessage {
                    source: *source,
                    action: action.clone(),
                };
                let _ = tx.send(msg).await;
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
            Ok(Message::ClockSyncRequest { voice_send_time }) => {
                let h = hub.lock().await;
                let hub_time = h.now();
                let reply = Message::ClockSyncReply {
                    voice_send_time,
                    hub_receive_time: hub_time,
                    hub_send_time: hub_time,
                };
                let _ = tx.send(reply).await;
            }
            Ok(Message::ActionMessage { action, .. }) => {
                let mut h = hub.lock().await;

                // Store param state.
                if action.signal_type == SignalType::Param {
                    h.param_state.insert(
                        action.address.clone(),
                        (voice_id, action.clone()),
                    );
                }

                // Schedule or route immediately.
                if action.timestamp > 0.0 && action.timestamp > h.now() {
                    let key = timestamp_key(action.timestamp);
                    h.schedule
                        .entry(key)
                        .or_default()
                        .push(ScheduledAction {
                            source: voice_id,
                            action,
                        });
                } else {
                    route_action(&h, voice_id, &action).await;
                }
            }
            Ok(Message::Goodbye) | Err(_) => {
                let mut h = hub.lock().await;
                h.voices.remove(&voice_id);
                break;
            }
            _ => {}
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
                            route_action(&h, sa.source, &sa.action).await;
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
    let hub_a = Hub::connect(port, "voice-a", vec!["/pong".into()])
        .await
        .unwrap();

    // Voice B subscribes to /ping, sends /pong.
    let mut hub_b = Hub::connect(port, "voice-b", vec!["/ping".into()])
        .await
        .unwrap();

    // Allow clock sync to happen.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // A sends a ping.
    hub_a
        .send_action(Action {
            address: "/ping".into(),
            signal_type: SignalType::Event,
            timestamp: 0.0,
            payload: Payload::Single(Value::String("hello".into())),
        })
        .await
        .unwrap();

    // B should receive it.
    let (source, action) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        hub_b.recv_action(),
    )
    .await
    .expect("Timed out waiting for action")
    .expect("Channel closed");

    assert_eq!(source, hub_a.voice_id);
    assert_eq!(action.address, "/ping");
    assert_eq!(
        action.payload,
        Payload::Single(Value::String("hello".into()))
    );

    hub_a.disconnect().await;
    hub_b.disconnect().await;
}

#[tokio::test]
async fn wildcard_subscription_routes_subtree() {
    let port = start_test_hub().await;

    // Voice A subscribes to /synth/** — should receive anything under /synth/.
    let mut hub_a = Hub::connect(port, "listener", vec!["/synth/**".into()])
        .await
        .unwrap();

    let hub_b = Hub::connect(port, "sender", vec![])
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // B sends to /synth/voice/1/note.
    hub_b
        .send_action(Action {
            address: "/synth/voice/1/note".into(),
            signal_type: SignalType::Event,
            timestamp: 0.0,
            payload: Payload::Single(Value::I32(60)),
        })
        .await
        .unwrap();

    let (_, action) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        hub_a.recv_action(),
    )
    .await
    .expect("Timed out")
    .expect("Channel closed");

    assert_eq!(action.address, "/synth/voice/1/note");

    hub_a.disconnect().await;
    hub_b.disconnect().await;
}

#[tokio::test]
async fn unsubscribed_voice_does_not_receive() {
    let port = start_test_hub().await;

    // Voice A subscribes to /other — should NOT receive /ping.
    let mut hub_a = Hub::connect(port, "bystander", vec!["/other".into()])
        .await
        .unwrap();

    let hub_b = Hub::connect(port, "sender", vec![])
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    hub_b
        .send_action(Action {
            address: "/ping".into(),
            signal_type: SignalType::Event,
            timestamp: 0.0,
            payload: Payload::None,
        })
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

    let hub = Hub::connect(port, "clock-test", vec![])
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

    let hub_sender = Hub::connect(port, "scheduler", vec![])
        .await
        .unwrap();
    let mut hub_receiver = Hub::connect(port, "listener", vec!["/scheduled".into()])
        .await
        .unwrap();

    // Wait for clock sync.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Schedule an action 0.5s in the future.
    let future_time = hub_sender.now().await + 0.5;
    let before_send = Instant::now();

    hub_sender
        .send_action(Action {
            address: "/scheduled".into(),
            signal_type: SignalType::Event,
            timestamp: future_time,
            payload: Payload::Single(Value::String("delayed".into())),
        })
        .await
        .unwrap();

    // Should receive it, but not immediately — should take ~500ms.
    let (_, action) = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        hub_receiver.recv_action(),
    )
    .await
    .expect("Timed out waiting for scheduled action")
    .expect("Channel closed");

    let elapsed = before_send.elapsed();
    assert_eq!(action.address, "/scheduled");
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

    let hub_setter = Hub::connect(port, "setter", vec![])
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Set a param value before the listener connects.
    hub_setter
        .send_action(Action {
            address: "/synth/cutoff".into(),
            signal_type: SignalType::Param,
            timestamp: 0.0,
            payload: Payload::Single(Value::F32(0.7)),
        })
        .await
        .unwrap();

    // Small delay to ensure the hub has processed it.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Now a late joiner connects and subscribes to /synth/*.
    let mut hub_late = Hub::connect(port, "late-joiner", vec!["/synth/*".into()])
        .await
        .unwrap();

    // The late joiner should receive the current param state.
    let (_, action) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        hub_late.recv_action(),
    )
    .await
    .expect("Timed out waiting for param replay")
    .expect("Channel closed");

    assert_eq!(action.address, "/synth/cutoff");
    assert_eq!(action.signal_type, SignalType::Param);
    assert_eq!(action.payload, Payload::Single(Value::F32(0.7)));

    hub_setter.disconnect().await;
    hub_late.disconnect().await;
}
