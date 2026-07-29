//! Regression tests for hub robustness fixes.
//!
//! Covers:
//! - A malformed `patch_manifest` must not kill the voice's connection.
//! - Duplicate subscriptions must not cause duplicate delivery.
//! - Unsubscribe removes all occurrences of a pattern.
//! - A slow subscriber must not stall routing for the rest of the hub.

use std::time::Duration;

use ensemble_client::Hub;
use ensemble_core::codec::{read_message, write_message};
use ensemble_core::protocol::*;
use ensemble_hub::start_server;
use tokio::io::{BufReader, BufWriter};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// Receive the next action within a generous timeout, failing the test on timeout.
async fn recv_within(hub: &mut Hub, secs: u64, what: &str) -> WireMessage {
    timeout(Duration::from_secs(secs), hub.recv_action())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
        .unwrap_or_else(|| panic!("connection closed while waiting for {what}"))
}

/// Assert that no further action arrives within a short window.
async fn assert_silence(hub: &mut Hub, millis: u64, what: &str) {
    let result = timeout(Duration::from_millis(millis), hub.recv_action()).await;
    assert!(
        result.is_err(),
        "expected no {what}, but one was delivered: {:?}",
        result.ok().flatten().map(|m| m.msg_type)
    );
}

/// A malformed patch_manifest (patch is not a map) must be rejected with the
/// connection kept alive — the voice stays registered and functional.
#[tokio::test]
async fn malformed_patch_manifest_keeps_voice_functional() {
    let (state, port) = start_server(0).await.expect("start server");

    let mut a = Hub::connect(port, "voice-a").await.expect("connect a");
    let b = Hub::connect(port, "voice-b").await.expect("connect b");

    // Send a malformed patch (not a map) — previously this silently killed
    // the connection handler and leaked the voice.
    a.patch_manifest(Value::Integer(42))
        .await
        .expect("send malformed patch");

    // Give the hub a moment to process the bad patch.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The voice must still be registered.
    {
        let st = state.lock().await;
        assert_eq!(
            st.voices().len(),
            2,
            "both voices should still be registered after malformed patch"
        );
    }

    // ... and still functional: A subscribes, B publishes, A receives.
    a.subscribe("/test/**").await.expect("subscribe a");
    tokio::time::sleep(Duration::from_millis(100)).await;
    b.send_action(action("/test/alive", SignalType::Event, 0.0, Value::Null))
        .await
        .expect("b sends action");
    let msg = recv_within(&mut a, 5, "action after malformed patch").await;
    assert_eq!(msg.msg_type, MSG_ACTION);

    // A valid patch afterwards must still apply.
    let mut patch = std::collections::BTreeMap::new();
    patch.insert("name".into(), Value::String("patched-a".into()));
    a.patch_manifest(Value::Map(patch)).await.expect("valid patch");
    tokio::time::sleep(Duration::from_millis(100)).await;
    {
        let st = state.lock().await;
        let manifest = st.manifest(a.voice_id).expect("manifest should exist");
        assert_eq!(manifest.name, "patched-a");
    }

    a.disconnect().await;
    b.disconnect().await;
}

/// Subscribing the same pattern twice must not deliver actions twice.
#[tokio::test]
async fn duplicate_subscribe_delivers_once() {
    let (_state, port) = start_server(0).await.expect("start server");

    let mut a = Hub::connect(port, "dup-a").await.expect("connect a");
    let b = Hub::connect(port, "dup-b").await.expect("connect b");

    a.subscribe("/dup/**").await.expect("subscribe once");
    a.subscribe("/dup/**").await.expect("subscribe twice (deduped)");
    tokio::time::sleep(Duration::from_millis(100)).await;

    b.send_action(action("/dup/x", SignalType::Event, 0.0, Value::Null))
        .await
        .expect("b sends action");

    let msg = recv_within(&mut a, 5, "first delivery").await;
    assert_eq!(msg.msg_type, MSG_ACTION);
    // Exactly one copy may arrive.
    assert_silence(&mut a, 500, "duplicate delivery").await;

    a.disconnect().await;
    b.disconnect().await;
}

/// Unsubscribe must remove every occurrence of the pattern.
#[tokio::test]
async fn unsubscribe_stops_all_delivery() {
    let (_state, port) = start_server(0).await.expect("start server");

    let mut a = Hub::connect(port, "unsub-a").await.expect("connect a");
    let b = Hub::connect(port, "unsub-b").await.expect("connect b");

    a.subscribe("/unsub/**").await.expect("subscribe");
    a.unsubscribe("/unsub/**").await.expect("unsubscribe");
    tokio::time::sleep(Duration::from_millis(100)).await;

    b.send_action(action("/unsub/x", SignalType::Event, 0.0, Value::Null))
        .await
        .expect("b sends action");

    assert_silence(&mut a, 500, "delivery after unsubscribe").await;

    a.disconnect().await;
    b.disconnect().await;
}

/// A subscriber that stops reading its socket must not stall routing for the
/// rest of the hub. Guaranteed sends to the stalled voice block only the
/// publishing voice's own handler — never the shared state lock.
#[tokio::test]
async fn slow_subscriber_does_not_stall_hub() {
    let (_state, port) = start_server(0).await.expect("start server");

    // Raw voice that subscribes and then never reads again. Its hub-side
    // channel (capacity 256) plus socket buffers fill up, after which
    // guaranteed sends to it block.
    let stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("raw connect");
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut writer = BufWriter::new(writer);
    write_message(&mut writer, &hello("slow-voice"))
        .await
        .expect("raw hello");
    let welcome_msg = read_message(&mut reader).await.expect("raw welcome");
    assert_eq!(welcome_msg.msg_type, MSG_WELCOME);
    write_message(&mut writer, &subscribe("/slow/**"))
        .await
        .expect("raw subscribe");
    // Leak the raw connection so it stays open but is never read.
    std::mem::forget((reader, writer));

    // Publisher of traffic to the stalled voice.
    let p = Hub::connect(port, "publisher").await.expect("connect p");
    // Independent fast lane: subscriber A and publisher Q.
    let mut a = Hub::connect(port, "fast-a").await.expect("connect a");
    let q = Hub::connect(port, "fast-q").await.expect("connect q");
    a.subscribe("/fast/**").await.expect("a subscribe");
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Flood the stalled voice well beyond its channel capacity. These sends
    // only enqueue to the publisher's own writer queue, so they complete.
    for _ in 0..2000 {
        p.send_action(action("/slow/x", SignalType::Event, 0.0, Value::Null))
            .await
            .expect("p flood send");
    }

    // Give the flood time to hit the stalled voice's full channel.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // The fast lane must keep flowing regardless.
    q.send_action(action("/fast/y", SignalType::Event, 0.0, Value::Null))
        .await
        .expect("q sends action");
    let msg = recv_within(&mut a, 5, "fast-lane action during flood").await;
    assert_eq!(msg.msg_type, MSG_ACTION);

    p.disconnect().await;
    q.disconnect().await;
    a.disconnect().await;
}
