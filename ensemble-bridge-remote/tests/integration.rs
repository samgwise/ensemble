//! Integration tests for the remote hub-to-hub bridge.
//!
//! These tests spin up real Ensemble hubs and run bridge instances in-process
//! to verify end-to-end forwarding, loop prevention, mesh topologies, and
//! param replay.

use std::collections::BTreeMap;
use std::time::Duration;

use ensemble_bridge_remote::config::{
    BridgeConfig, Config, LocalConfig, MappingConfig, PeerConfig,
};
use ensemble_bridge_remote::start_bridge;
use ensemble_client::Hub;
use ensemble_core::protocol::*;
use ensemble_values::FloatValue;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Start a real Ensemble hub on a random port and return the port.
async fn start_hub() -> u16 {
    let (_state, port) = ensemble_hub::start_server(0).await.expect("start hub");
    port
}

/// Build a bridge config for a test.
fn make_config(
    name: &str,
    local_port: u16,
    listen_port: u16,
    peers: Vec<PeerConfig>,
    mappings: Vec<MappingConfig>,
) -> Config {
    Config {
        bridge: BridgeConfig {
            name: name.to_string(),
            listen_port,
        },
        local: LocalConfig {
            port: Some(local_port),
        },
        peer: peers,
        mapping: mappings,
    }
}

fn both_mapping(from: &str, to: &str) -> MappingConfig {
    MappingConfig {
        from_pattern: from.to_string(),
        to_template: to.to_string(),
        direction: "both".to_string(),
        signal_filter: vec![],
    }
}

fn peer(host: &str, port: u16) -> PeerConfig {
    PeerConfig {
        host: host.to_string(),
        port,
        reconnect: true,
        replay_params: true,
    }
}

fn payload_map(msg: &WireMessage) -> BTreeMap<String, Value> {
    match &msg.payload {
        Value::Map(m) => m.clone(),
        _ => BTreeMap::new(),
    }
}

async fn recv_action(client: &mut Hub, timeout_ms: u64) -> Option<WireMessage> {
    tokio::time::timeout(Duration::from_millis(timeout_ms), client.recv_action())
        .await
        .ok()?
}

/// Wait for a bridge peer connection to stabilise.
async fn wait_for_bridge_ready() {
    tokio::time::sleep(Duration::from_millis(600)).await;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_hubs_forward_event() {
    let port_a = start_hub().await;
    let port_b = start_hub().await;

    // Bridge A listens, no outbound peers.
    let bridge_a = start_bridge(make_config(
        "bridge-a",
        port_a,
        0,
        vec![],
        vec![both_mapping("/local/**", "/remote/**")],
    ))
    .await
    .expect("start bridge a");

    // Bridge B connects to A.
    let bridge_b = start_bridge(make_config(
        "bridge-b",
        port_b,
        0,
        vec![peer("127.0.0.1", bridge_a.listen_port)],
        vec![both_mapping("/remote/**", "/local/**")],
    ))
    .await
    .expect("start bridge b");

    let client_a = Hub::connect(port_a, "client-a").await.expect("connect a");
    let mut client_b = Hub::connect(port_b, "client-b").await.expect("connect b");

    client_a.subscribe("/local/**").await.unwrap();
    client_b.subscribe("/local/**").await.unwrap();

    wait_for_bridge_ready().await;

    client_a
        .send_action(action(
            "/local/event",
            SignalType::Event,
            0.0,
            Value::Integer(42),
        ))
        .await
        .unwrap();

    let msg = recv_action(&mut client_b, 2000)
        .await
        .expect("client b should receive forwarded event");
    let map = payload_map(&msg);
    assert_eq!(get_string(&map, "address").unwrap(), "/local/event");
    assert_eq!(get_value(&map, "payload").unwrap(), Value::Integer(42));
    // The source is rewritten by the receiving hub to the bridge's local voice_id,
    // so we only check that it is a valid voice id.
    assert!(get_integer(&map, "source").unwrap() > 0);

    bridge_a.shutdown().await;
    bridge_b.shutdown().await;
}

#[tokio::test]
async fn param_replay_on_peer_connect() {
    let port_a = start_hub().await;
    let port_b = start_hub().await;

    let setter = Hub::connect(port_a, "setter")
        .await
        .expect("connect setter");
    setter
        .send_action(action(
            "/local/param",
            SignalType::Param,
            0.0,
            Value::Float(FloatValue::new(3.5)),
        ))
        .await
        .unwrap();

    // Bridge A joins hub A after the param is set. Hub A will replay the param
    // on subscription, so bridge A's cache is populated.
    let bridge_a = start_bridge(make_config(
        "bridge-a",
        port_a,
        0,
        vec![],
        vec![both_mapping("/local/**", "/remote/**")],
    ))
    .await
    .expect("start bridge a");

    // Give bridge A time to subscribe and receive the replay.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Bridge B connects to A and should receive the cached param replay.
    let bridge_b = start_bridge(make_config(
        "bridge-b",
        port_b,
        0,
        vec![peer("127.0.0.1", bridge_a.listen_port)],
        vec![both_mapping("/remote/**", "/local/**")],
    ))
    .await
    .expect("start bridge b");

    let mut client_b = Hub::connect(port_b, "client-b").await.expect("connect b");
    client_b.subscribe("/local/**").await.unwrap();

    // Wait for the replay to arrive from bridge A via bridge B.
    let msg = recv_action(&mut client_b, 2000)
        .await
        .expect("client b should receive param replay");
    let map = payload_map(&msg);
    assert_eq!(get_string(&map, "address").unwrap(), "/local/param");
    assert_eq!(get_string(&map, "signal_type").unwrap(), "param");
    assert_eq!(
        get_value(&map, "payload").unwrap(),
        Value::Float(FloatValue::new(3.5))
    );

    bridge_a.shutdown().await;
    bridge_b.shutdown().await;
}

#[tokio::test]
async fn loop_prevention_drops_looped_action() {
    let port_a = start_hub().await;
    let port_b = start_hub().await;

    // Bridge A listens, bridge B connects to A. They share a single bidirectional
    // QUIC connection, so actions can flow both ways. When A forwards an action
    // to B, B forwards it back to A, and A must drop it because the origin is
    // its own bridge_id.
    let bridge_a = start_bridge(make_config(
        "bridge-a",
        port_a,
        0,
        vec![],
        vec![both_mapping("/local/**", "/remote/**")],
    ))
    .await
    .expect("start bridge a");

    let bridge_b = start_bridge(make_config(
        "bridge-b",
        port_b,
        0,
        vec![peer("127.0.0.1", bridge_a.listen_port)],
        vec![both_mapping("/remote/**", "/local/**")],
    ))
    .await
    .expect("start bridge b");

    let client_a = Hub::connect(port_a, "client-a").await.expect("connect a");
    let mut client_b = Hub::connect(port_b, "client-b").await.expect("connect b");

    client_a.subscribe("/local/**").await.unwrap();
    client_b.subscribe("/local/**").await.unwrap();

    wait_for_bridge_ready().await;

    client_a
        .send_action(action(
            "/local/event",
            SignalType::Event,
            0.0,
            Value::Integer(7),
        ))
        .await
        .unwrap();

    // Client B should receive the action exactly once.
    let msg = recv_action(&mut client_b, 2000)
        .await
        .expect("client b should receive the action once");
    let map = payload_map(&msg);
    assert_eq!(get_string(&map, "address").unwrap(), "/local/event");

    // Client B should not receive a second copy (loop).
    let second = recv_action(&mut client_b, 500).await;
    assert!(
        second.is_none(),
        "looped action should be dropped, not forwarded again"
    );

    bridge_a.shutdown().await;
    bridge_b.shutdown().await;
}

#[tokio::test]
async fn mesh_topology_forwards_to_multiple_peers() {
    let port_a = start_hub().await;
    let port_b = start_hub().await;
    let port_c = start_hub().await;

    // Bridge A is the centre; B and C connect to A.
    let bridge_a = start_bridge(make_config(
        "bridge-a",
        port_a,
        0,
        vec![],
        vec![both_mapping("/local/**", "/remote/**")],
    ))
    .await
    .expect("start bridge a");

    let bridge_b = start_bridge(make_config(
        "bridge-b",
        port_b,
        0,
        vec![peer("127.0.0.1", bridge_a.listen_port)],
        vec![both_mapping("/remote/**", "/local/**")],
    ))
    .await
    .expect("start bridge b");

    let bridge_c = start_bridge(make_config(
        "bridge-c",
        port_c,
        0,
        vec![peer("127.0.0.1", bridge_a.listen_port)],
        vec![both_mapping("/remote/**", "/local/**")],
    ))
    .await
    .expect("start bridge c");

    let client_a = Hub::connect(port_a, "client-a").await.expect("connect a");
    let mut client_b = Hub::connect(port_b, "client-b").await.expect("connect b");
    let mut client_c = Hub::connect(port_c, "client-c").await.expect("connect c");

    client_a.subscribe("/local/**").await.unwrap();
    client_b.subscribe("/local/**").await.unwrap();
    client_c.subscribe("/local/**").await.unwrap();

    wait_for_bridge_ready().await;

    client_a
        .send_action(action(
            "/local/mesh",
            SignalType::Event,
            0.0,
            Value::Integer(99),
        ))
        .await
        .unwrap();

    let msg_b = recv_action(&mut client_b, 2000)
        .await
        .expect("client b should receive mesh action");
    let msg_c = recv_action(&mut client_c, 2000)
        .await
        .expect("client c should receive mesh action");

    assert_eq!(
        get_string(&payload_map(&msg_b), "address").unwrap(),
        "/local/mesh"
    );
    assert_eq!(
        get_string(&payload_map(&msg_c), "address").unwrap(),
        "/local/mesh"
    );

    bridge_a.shutdown().await;
    bridge_b.shutdown().await;
    bridge_c.shutdown().await;
}
