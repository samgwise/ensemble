//! Demo: Two voices exchanging actions through an Ensemble hub.
//!
//! Run the hub first:  cargo run --bin ensemble-hub
//! Then this example:  cargo run --example ping_pong

use std::collections::BTreeMap;
use ensemble_client::Hub;
use ensemble_core::protocol::*;

/// Extract payload map from WireMessage.
fn payload_map(msg: &WireMessage) -> BTreeMap<String, Value> {
    match &msg.payload {
        Value::Map(m) => m.clone(),
        _ => BTreeMap::new(),
    }
}

#[tokio::main]
async fn main() {
    println!("Connecting two voices to the hub on port 7331...\n");

    // Voice A: subscribes to /pong, sends /ping.
    let hub_a = Hub::connect(7331, "ping-voice")
        .await
        .expect("Failed to connect voice A — is the hub running?");
    hub_a.subscribe("/pong").await.unwrap();

    // Voice B: subscribes to /ping, sends /pong.
    let mut hub_b = Hub::connect(7331, "pong-voice")
        .await
        .expect("Failed to connect voice B");
    hub_b.subscribe("/ping").await.unwrap();

    println!(
        "Voice A (id={}) connected, Voice B (id={}) connected",
        hub_a.voice_id, hub_b.voice_id
    );

    // Wait a moment for clock sync to establish.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let hub_time = hub_a.now().await;
    println!("Hub time (from voice A): {hub_time:.4}s\n");

    // Voice A sends a ping.
    println!("Voice A sending /ping...");
    hub_a
        .send_action(action(
            "/ping",
            SignalType::Event,
            0.0,
            Value::String("hello from A!".into()),
        ))
        .await
        .unwrap();

    // Voice B receives the ping.
    if let Some(action_msg) = hub_b.recv_action().await {
        let map = payload_map(&action_msg);
        let source = get_integer(&map, "source").unwrap_or(0);
        let address = get_string(&map, "address").unwrap_or_default();
        let payload = get_value(&map, "payload").unwrap_or(Value::Null);

        println!(
            "Voice B received from voice {source}: {address} => {payload:?}"
        );

        // Voice B sends a pong back.
        println!("Voice B sending /pong...");
        hub_b
            .send_action(action(
                "/pong",
                SignalType::Event,
                0.0,
                Value::String("hello back from B!".into()),
            ))
            .await
            .unwrap();
    }

    // Small delay to let the pong route through.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Note: To receive on hub_a we'd need a mutable reference, but this demo
    // is intentionally simple. The TUI on the hub will show both messages routing.

    println!("\nDone! Check the hub TUI to see the event log.");

    // Clean disconnect.
    hub_a.disconnect().await;
    hub_b.disconnect().await;
}
