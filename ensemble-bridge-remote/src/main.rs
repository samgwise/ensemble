//! Ensemble Remote Bridge — connects two Ensemble hubs over QUIC.
//!
//! This bridge acts as a proxy: it connects to the local hub as an ordinary
//! voice, listens for inbound QUIC connections from remote bridges, and can
//! also initiate outbound connections to configured peers.
//!
//! Actions are forwarded bidirectionally with address mapping and loop
//! prevention via origin tags.

mod config;
mod local_hub;
mod loop_guard;
mod mapping;
mod param_cache;
mod peer_manager;
mod protocol;
mod remote_peer;

use crate::config::Config;
use crate::param_cache::ParamCache;
use crate::peer_manager::PeerManager;
use anyhow::Result;
use ensemble_core::protocol::*;
use loop_guard::LoopGuard;
use mapping::MappingEngine;
use remote_peer::HubSink;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

#[tokio::main]
async fn main() -> Result<()> {
    // Load configuration.
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "bridge-remote.toml".to_string());

    eprintln!("Ensemble Remote Bridge");
    eprintln!("Loading config from: {}", config_path);

    let config = Config::load(&config_path)?;
    eprintln!("Bridge name: {}", config.bridge.name);
    eprintln!("Listen port: {}", config.bridge.listen_port);
    eprintln!("Peers: {}", config.peer.len());
    eprintln!("Mapping rules: {}", config.mapping.len());

    // Initialise core components.
    let guard = Arc::new(LoopGuard::new());
    eprintln!("Bridge ID: {}", guard.bridge_id());

    let engine = Arc::new(MappingEngine::new(&config.mapping));
    let outbound_count = engine.outbound_patterns().len();
    eprintln!("Outbound subscription patterns: {}", outbound_count);

    // Connect to local hub.
    let mut hub = local_hub::connect_to_hub(&config.local, &config.bridge.name).await?;

    // Subscribe to outbound patterns.
    local_hub::subscribe_to_patterns(&hub, &engine).await?;

    // Create broadcast channel for outbound actions (local hub → all remote peers).
    let (outbound_tx, _) = broadcast::channel::<WireMessage>(1000);

    // Create channel for inbound actions (remote peers → local hub).
    let (inbound_tx, mut inbound_rx) = mpsc::channel::<WireMessage>(1000);
    let hub_sink = Arc::new(HubSink { tx: inbound_tx });

    // Cache of current param values for replay to newly connected peers.
    let param_cache = ParamCache::new();

    // Create the peer manager to orchestrate inbound and outbound connections.
    let manager = PeerManager::new(
        config.peer.clone(),
        config.bridge.name.clone(),
        guard.clone(),
        engine.clone(),
        hub_sink.clone(),
        outbound_tx.clone(),
        param_cache.clone(),
    );
    let manager_handle = manager.handle();

    // Start QUIC listener and register accepted peers with the manager.
    let listener_port = config.bridge.listen_port;
    tokio::spawn(async move {
        let (peer_tx, mut peer_rx) = mpsc::channel::<remote_peer::RemotePeer>(100);
        tokio::spawn(async move {
            if let Err(e) = remote_peer::start_listener(listener_port, peer_tx).await {
                eprintln!("QUIC listener error: {}", e);
            }
        });
        while let Some(peer) = peer_rx.recv().await {
            manager_handle.register_inbound(peer);
        }
    });

    // Run the peer manager (initiates outbound connections and handles reconnects).
    tokio::spawn(async move {
        manager.run().await;
    });

    // Get a sender clone for sending actions back to the hub.
    let hub_sender = hub.sender();

    // Spawn task to forward actions from local hub to broadcast (→ all peers),
    // keeping the param cache up to date for future peer replays.
    let origin = guard.bridge_id().to_string();
    let engine_for_hub = engine.clone();
    let outbound_tx_hub = outbound_tx.clone();
    let param_cache_hub = param_cache.clone();
    tokio::spawn(async move {
        while let Some(msg) = hub.recv_action().await {
            match msg.msg_type.as_str() {
                MSG_UNSET_PARAM => {
                    if let Some(address) = protocol::get_address(&msg) {
                        param_cache_hub.remove(&address);
                    }
                }
                MSG_ACTION => {
                    param_cache_hub.update(&msg);
                    if let Err(e) = local_hub::forward_to_remote(
                        &msg,
                        &engine_for_hub,
                        &origin,
                        &outbound_tx_hub,
                    )
                    .await
                    {
                        eprintln!("Error forwarding to remote: {}", e);
                    }
                }
                other => {
                    eprintln!("Unexpected message from local hub: {}", other);
                }
            }
        }
    });

    // Spawn task to forward inbound actions from peers to local hub.
    tokio::spawn(async move {
        while let Some(action) = inbound_rx.recv().await {
            if let Err(e) = hub_sender.send(action).await {
                eprintln!("Error sending to local hub: {}", e);
            }
        }
    });

    eprintln!("Bridge ready. Press Ctrl+C to exit.");

    // Wait for shutdown signal.
    tokio::signal::ctrl_c().await?;
    eprintln!("Shutting down.");

    Ok(())
}
