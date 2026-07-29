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
    make_config_with_auth(name, local_port, listen_port, peers, mappings, None)
}

/// Build a bridge config with an optional auth token.
fn make_config_with_auth(
    name: &str,
    local_port: u16,
    listen_port: u16,
    peers: Vec<PeerConfig>,
    mappings: Vec<MappingConfig>,
    auth_token: Option<&str>,
) -> Config {
    Config {
        bridge: BridgeConfig {
            name: name.to_string(),
            // Tests bind loopback only.
            listen_addr: "127.0.0.1".to_string(),
            listen_port,
            auth_token: auth_token.map(|t| t.to_string()),
            max_inbound: 32,
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

/// Fully bidirectional mapping between /local/** and /remote/**: outbound
/// local actions are bridged out, and inbound bridged actions land back in
/// the local namespace. (A single "both" rule cannot do this — the same
/// pattern would be applied to inbound addresses in the wrong namespace.)
fn bidi_mappings() -> Vec<MappingConfig> {
    vec![
        MappingConfig {
            from_pattern: "/local/**".to_string(),
            to_template: "/remote/**".to_string(),
            direction: "outbound".to_string(),
            signal_filter: vec![],
        },
        MappingConfig {
            from_pattern: "/remote/**".to_string(),
            to_template: "/local/**".to_string(),
            direction: "inbound".to_string(),
            signal_filter: vec![],
        },
    ]
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

#[tokio::test]
async fn mutual_dial_keeps_single_connection() {
    let port_a = start_hub().await;
    let port_b = start_hub().await;

    // Both bridges dial each other. The deterministic tie-break (lower
    // bridge_id keeps its outbound session) must leave exactly one
    // connection, with no reconnect churn and no duplicate delivery.
    let bridge_a = start_bridge(make_config("bridge-a", port_a, 0, vec![], bidi_mappings()))
        .await
        .expect("start bridge a");

    let bridge_b = start_bridge(make_config(
        "bridge-b",
        port_b,
        0,
        vec![peer("127.0.0.1", bridge_a.listen_port)],
        bidi_mappings(),
    ))
    .await
    .expect("start bridge b");

    // Point bridge A back at B so both sides dial (mutual dial).
    // Bridge B's listener port is only known now, so A is restarted with the
    // full mutual configuration. (A second bridge on hub A is not needed;
    // this restart keeps the test to two bridges.)
    bridge_a.shutdown().await;
    let bridge_a = start_bridge(make_config(
        "bridge-a",
        port_a,
        0,
        vec![peer("127.0.0.1", bridge_b.listen_port)],
        bidi_mappings(),
    ))
    .await
    .expect("restart bridge a with mutual dial");

    let client_a = Hub::connect(port_a, "client-a").await.expect("connect a");
    let mut client_b = Hub::connect(port_b, "client-b").await.expect("connect b");

    client_a.subscribe("/local/**").await.unwrap();
    client_b.subscribe("/local/**").await.unwrap();

    // Allow time for both dials, the tie-break, and any churn to settle.
    wait_for_bridge_ready().await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    // A → B direction.
    client_a
        .send_action(action(
            "/local/mutual",
            SignalType::Event,
            0.0,
            Value::Integer(1),
        ))
        .await
        .unwrap();
    let msg_b = recv_action(&mut client_b, 2000)
        .await
        .expect("client b should receive the action once");
    assert_eq!(
        get_string(&payload_map(&msg_b), "address").unwrap(),
        "/local/mutual"
    );
    // No duplicate delivery (which two surviving connections would cause).
    assert!(
        recv_action(&mut client_b, 500).await.is_none(),
        "mutual dial must not duplicate deliveries"
    );

    // B → A direction (the surviving connection is bidirectional).
    client_b
        .send_action(action(
            "/local/reply",
            SignalType::Event,
            0.0,
            Value::Integer(2),
        ))
        .await
        .unwrap();
    let mut client_a = client_a;
    let msg_a = recv_action(&mut client_a, 2000)
        .await
        .expect("client a should receive the reply once");
    assert_eq!(
        get_string(&payload_map(&msg_a), "address").unwrap(),
        "/local/reply"
    );
    assert!(
        recv_action(&mut client_a, 500).await.is_none(),
        "mutual dial must not duplicate deliveries"
    );

    bridge_a.shutdown().await;
    bridge_b.shutdown().await;
}

#[tokio::test]
async fn chain_topology_propagates_multihop() {
    let port_a = start_hub().await;
    let port_b = start_hub().await;
    let port_c = start_hub().await;

    // A — B — C chain: B connects to A; C connects to B. C is not directly
    // connected to A, so it can only receive A's action if bridges
    // re-forward remote actions to their other peers.
    let bridge_a = start_bridge(make_config("bridge-a", port_a, 0, vec![], bidi_mappings()))
        .await
        .expect("start bridge a");

    let bridge_b = start_bridge(make_config(
        "bridge-b",
        port_b,
        0,
        vec![peer("127.0.0.1", bridge_a.listen_port)],
        bidi_mappings(),
    ))
    .await
    .expect("start bridge b");

    let bridge_c = start_bridge(make_config(
        "bridge-c",
        port_c,
        0,
        vec![peer("127.0.0.1", bridge_b.listen_port)],
        bidi_mappings(),
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
            "/local/hop",
            SignalType::Event,
            0.0,
            Value::Integer(11),
        ))
        .await
        .unwrap();

    // One hop: A → B.
    let msg_b = recv_action(&mut client_b, 2000)
        .await
        .expect("client b should receive the action");
    assert_eq!(
        get_string(&payload_map(&msg_b), "address").unwrap(),
        "/local/hop"
    );

    // Two hops: A → B → C, with the origin preserved through B.
    let msg_c = recv_action(&mut client_c, 2000)
        .await
        .expect("client c should receive the action via multi-hop");
    assert_eq!(
        get_string(&payload_map(&msg_c), "address").unwrap(),
        "/local/hop"
    );

    // Exactly-once at every hop.
    assert!(recv_action(&mut client_b, 500).await.is_none());
    assert!(recv_action(&mut client_c, 500).await.is_none());

    bridge_a.shutdown().await;
    bridge_b.shutdown().await;
    bridge_c.shutdown().await;
}

#[tokio::test]
async fn ring_topology_delivers_exactly_once() {
    let port_a = start_hub().await;
    let port_b = start_hub().await;
    let port_c = start_hub().await;

    // Three-bridge ring (triangle): B dials A; C dials both A and B. The
    // action must loop back to its origin and be dropped there, and every
    // other hub must see it exactly once despite multiple paths.
    let bridge_a = start_bridge(make_config("bridge-a", port_a, 0, vec![], bidi_mappings()))
        .await
        .expect("start bridge a");

    let bridge_b = start_bridge(make_config(
        "bridge-b",
        port_b,
        0,
        vec![peer("127.0.0.1", bridge_a.listen_port)],
        bidi_mappings(),
    ))
    .await
    .expect("start bridge b");

    let bridge_c = start_bridge(make_config(
        "bridge-c",
        port_c,
        0,
        vec![
            peer("127.0.0.1", bridge_a.listen_port),
            peer("127.0.0.1", bridge_b.listen_port),
        ],
        bidi_mappings(),
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
            "/local/ring",
            SignalType::Event,
            0.0,
            Value::Integer(21),
        ))
        .await
        .unwrap();

    // Both other hubs receive the action exactly once: duplicate suppression
    // drops second sightings arriving via the other ring path.
    let msg_b = recv_action(&mut client_b, 2000)
        .await
        .expect("client b should receive the ring action");
    let msg_c = recv_action(&mut client_c, 2000)
        .await
        .expect("client c should receive the ring action");
    assert_eq!(
        get_string(&payload_map(&msg_b), "address").unwrap(),
        "/local/ring"
    );
    assert_eq!(
        get_string(&payload_map(&msg_c), "address").unwrap(),
        "/local/ring"
    );

    // No duplicates at B or C, and nothing loops back to A.
    let mut client_a = client_a;
    assert!(recv_action(&mut client_b, 500).await.is_none());
    assert!(recv_action(&mut client_c, 500).await.is_none());
    assert!(recv_action(&mut client_a, 500).await.is_none());

    bridge_a.shutdown().await;
    bridge_b.shutdown().await;
    bridge_c.shutdown().await;
}

#[tokio::test]
async fn unset_param_propagates_across_bridge() {
    let port_a = start_hub().await;
    let port_b = start_hub().await;

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

    // Set the param; it should propagate to hub B.
    client_a
        .send_action(action(
            "/local/param",
            SignalType::Param,
            0.0,
            Value::Float(FloatValue::new(1.5)),
        ))
        .await
        .unwrap();
    let msg = recv_action(&mut client_b, 2000)
        .await
        .expect("client b should receive the param");
    assert_eq!(msg.msg_type, MSG_ACTION);

    // Unset it on hub A; the unset must propagate and reach hub B's
    // subscribers as an unset_param. (The client has no convenience method
    // for unsets, so the raw message is sent through the sender handle.)
    client_a
        .sender()
        .send(unset_param("/local/param"))
        .await
        .unwrap();
    let unset = recv_action(&mut client_b, 2000)
        .await
        .expect("client b should receive the propagated unset");
    assert_eq!(unset.msg_type, MSG_UNSET_PARAM);
    let map = payload_map(&unset);
    assert_eq!(get_string(&map, "address").unwrap(), "/local/param");

    bridge_a.shutdown().await;
    bridge_b.shutdown().await;
}

#[tokio::test]
async fn shared_secret_auth_accepts_matching_tokens() {
    let port_a = start_hub().await;
    let port_b = start_hub().await;

    let bridge_a = start_bridge(make_config_with_auth(
        "bridge-a",
        port_a,
        0,
        vec![],
        vec![both_mapping("/local/**", "/remote/**")],
        Some("s3cret"),
    ))
    .await
    .expect("start bridge a");

    let bridge_b = start_bridge(make_config_with_auth(
        "bridge-b",
        port_b,
        0,
        vec![peer("127.0.0.1", bridge_a.listen_port)],
        vec![both_mapping("/remote/**", "/local/**")],
        Some("s3cret"),
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
            "/local/authed",
            SignalType::Event,
            0.0,
            Value::Integer(5),
        ))
        .await
        .unwrap();
    let msg = recv_action(&mut client_b, 2000)
        .await
        .expect("client b should receive the action with matching tokens");
    assert_eq!(
        get_string(&payload_map(&msg), "address").unwrap(),
        "/local/authed"
    );

    bridge_a.shutdown().await;
    bridge_b.shutdown().await;
}

#[tokio::test]
async fn shared_secret_auth_rejects_mismatched_tokens() {
    let port_a = start_hub().await;
    let port_b = start_hub().await;

    let bridge_a = start_bridge(make_config_with_auth(
        "bridge-a",
        port_a,
        0,
        vec![],
        vec![both_mapping("/local/**", "/remote/**")],
        Some("s3cret"),
    ))
    .await
    .expect("start bridge a");

    let bridge_b = start_bridge(make_config_with_auth(
        "bridge-b",
        port_b,
        0,
        vec![peer("127.0.0.1", bridge_a.listen_port)],
        vec![both_mapping("/remote/**", "/local/**")],
        Some("wr0ng"),
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
            "/local/rejected",
            SignalType::Event,
            0.0,
            Value::Integer(6),
        ))
        .await
        .unwrap();

    // The handshake never completes, so nothing can be forwarded.
    assert!(
        recv_action(&mut client_b, 1000).await.is_none(),
        "mismatched tokens must prevent any forwarding"
    );

    bridge_a.shutdown().await;
    bridge_b.shutdown().await;
}

#[tokio::test]
async fn reconnection_resumes_after_peer_restart() {
    let port_a = start_hub().await;
    let port_b = start_hub().await;

    // Bridge A listens on an ephemeral port that we will rebind after restart.
    let bridge_a = start_bridge(make_config(
        "bridge-a",
        port_a,
        0,
        vec![],
        vec![both_mapping("/local/**", "/remote/**")],
    ))
    .await
    .expect("start bridge a");
    let listen_port = bridge_a.listen_port;

    // Bridge B connects to A and is configured to reconnect on drop.
    let bridge_b = start_bridge(make_config(
        "bridge-b",
        port_b,
        0,
        vec![peer("127.0.0.1", listen_port)],
        vec![both_mapping("/remote/**", "/local/**")],
    ))
    .await
    .expect("start bridge b");

    let client_a = Hub::connect(port_a, "client-a").await.expect("connect a");
    let mut client_b = Hub::connect(port_b, "client-b").await.expect("connect b");

    client_a.subscribe("/local/**").await.unwrap();
    client_b.subscribe("/local/**").await.unwrap();

    wait_for_bridge_ready().await;

    // Forwarding works before the restart.
    client_a
        .send_action(action(
            "/local/event",
            SignalType::Event,
            0.0,
            Value::Integer(1),
        ))
        .await
        .unwrap();
    let first = recv_action(&mut client_b, 2000)
        .await
        .expect("client b should receive first event");
    assert_eq!(
        get_string(&payload_map(&first), "address").unwrap(),
        "/local/event"
    );

    // Restart bridge A on the same port. Graceful shutdown drops the QUIC
    // endpoint, but the OS releases the UDP socket asynchronously (a quinn
    // driver task closes it shortly after the drop), so retry the rebind until
    // the port is free.
    bridge_a.shutdown().await;

    let bridge_a2 = {
        let mut handle = None;
        for _ in 0..30 {
            match start_bridge(make_config(
                "bridge-a",
                port_a,
                listen_port,
                vec![],
                vec![both_mapping("/local/**", "/remote/**")],
            ))
            .await
            {
                Ok(h) => {
                    handle = Some(h);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
        handle.expect("restart bridge a on same port")
    };

    // Bridge B reconnects with exponential backoff (first retry ~2s). Probe
    // with actions until one is delivered, which means B has reconnected and
    // forwarding has resumed.
    let mut second = None;
    for _ in 0..30 {
        client_a
            .send_action(action(
                "/local/event",
                SignalType::Event,
                0.0,
                Value::Integer(2),
            ))
            .await
            .unwrap();
        if let Some(msg) = recv_action(&mut client_b, 300).await {
            second = Some(msg);
            break;
        }
    }
    let second = second.expect("client b should receive event after reconnect");
    assert_eq!(
        get_string(&payload_map(&second), "address").unwrap(),
        "/local/event"
    );

    bridge_a2.shutdown().await;
    bridge_b.shutdown().await;
}
