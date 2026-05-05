//! Demo: Two voices exchanging actions through a Chord hub.
//!
//! Run the hub first:  cargo run --bin chord-hub
//! Then this example:  cargo run --example ping_pong

use chord_client::Hub;
use chord_core::protocol::*;

#[tokio::main]
async fn main() {
    println!("Connecting two voices to the hub on port 7331...\n");

    // Voice A: subscribes to /pong, sends /ping.
    let hub_a = Hub::connect(7331, "ping-voice", vec!["/pong".into()])
        .await
        .expect("Failed to connect voice A — is the hub running?");

    // Voice B: subscribes to /ping, sends /pong.
    let mut hub_b = Hub::connect(7331, "pong-voice", vec!["/ping".into()])
        .await
        .expect("Failed to connect voice B");

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
        .send_action(Action {
            address: "/ping".into(),
            signal_type: SignalType::Event,
            timestamp: 0.0,
            payload: Payload::Single(Value::String("hello from A!".into())),
        })
        .await
        .unwrap();

    // Voice B receives the ping.
    if let Some((source, action)) = hub_b.recv_action().await {
        println!(
            "Voice B received from voice {source}: {} => {:?}",
            action.address, action.payload
        );

        // Voice B sends a pong back.
        println!("Voice B sending /pong...");
        hub_b
            .send_action(Action {
                address: "/pong".into(),
                signal_type: SignalType::Event,
                timestamp: 0.0,
                payload: Payload::Single(Value::String("hello back from B!".into())),
            })
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
