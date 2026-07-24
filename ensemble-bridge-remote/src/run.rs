//! Bridge run loop and test helpers.

use std::sync::Arc;

use anyhow::Result;
use ensemble_core::protocol::*;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::loop_guard::LoopGuard;
use crate::mapping::MappingEngine;
use crate::param_cache::ParamCache;
use crate::peer_manager::PeerManager;
use crate::remote_peer::{self, HubSink, RemotePeer};

/// Handle to a running bridge instance (used by integration tests).
pub struct BridgeHandle {
    /// The actual port the QUIC listener bound to.
    pub listen_port: u16,
    shutdown_tx: mpsc::Sender<()>,
    task: JoinHandle<Result<()>>,
}

impl BridgeHandle {
    /// Signal the bridge to shut down and wait for it to finish.
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(()).await;
        let _ = self.task.await;
    }
}

/// Start a bridge in the background and return a handle to it.
///
/// The bridge is fully initialised and its listener is bound before this
/// returns, so callers can safely connect peers to `listen_port`.
pub async fn start_bridge(config: Config) -> Result<BridgeHandle> {
    let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
    let (ready_tx, ready_rx) = oneshot::channel();

    let task = tokio::spawn(run_bridge(config, Some(shutdown_rx), Some(ready_tx)));
    let listen_port = ready_rx.await?;

    Ok(BridgeHandle {
        listen_port,
        shutdown_tx,
        task,
    })
}

/// Run the bridge until a shutdown signal is received.
///
/// If `shutdown` is `None`, the function waits for `Ctrl+C`. If `ready_tx` is
/// provided, it is sent the actual listener port as soon as the QUIC endpoint
/// is bound.
pub async fn run_bridge(
    config: Config,
    mut shutdown: Option<mpsc::Receiver<()>>,
    ready_tx: Option<oneshot::Sender<u16>>,
) -> Result<()> {
    eprintln!("Ensemble Remote Bridge");
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
    let mut hub = crate::local_hub::connect_to_hub(&config.local, &config.bridge.name).await?;

    // Subscribe to outbound patterns.
    crate::local_hub::subscribe_to_patterns(&hub, &engine).await?;

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
    let (peer_tx, mut peer_rx) = mpsc::channel::<RemotePeer>(100);
    let listener_port = remote_peer::start_listener(config.bridge.listen_port, peer_tx).await?;
    if let Some(tx) = ready_tx {
        let _ = tx.send(listener_port);
    }

    let register_handle = manager_handle.clone();
    tokio::spawn(async move {
        while let Some(peer) = peer_rx.recv().await {
            register_handle.register_inbound(peer);
        }
    });

    // Run the peer manager (initiates outbound connections and handles reconnects).
    tokio::spawn(async move {
        manager.run().await;
    });

    // Get a sender clone for sending actions back to the hub.
    let hub_sender = hub.sender();

    // Forward actions from local hub to broadcast (→ all peers),
    // keeping the param cache up to date for future peer replays.
    let origin = guard.bridge_id().to_string();
    let engine_for_hub = engine.clone();
    let outbound_tx_hub = outbound_tx.clone();
    let param_cache_hub = param_cache.clone();
    tokio::spawn(async move {
        while let Some(msg) = hub.recv_action().await {
            match msg.msg_type.as_str() {
                MSG_UNSET_PARAM => {
                    if let Some(address) = crate::protocol::get_address(&msg) {
                        param_cache_hub.remove(&address);
                    }
                }
                MSG_ACTION => {
                    param_cache_hub.update(&msg);
                    if let Err(e) = crate::local_hub::forward_to_remote(
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

    // Forward inbound actions from peers to local hub.
    tokio::spawn(async move {
        while let Some(action) = inbound_rx.recv().await {
            if let Err(e) = hub_sender.send(action).await {
                eprintln!("Error sending to local hub: {}", e);
            }
        }
    });

    eprintln!(
        "Bridge ready on port {}. Waiting for shutdown.",
        listener_port
    );

    // Wait for shutdown signal.
    match shutdown {
        None => {
            tokio::signal::ctrl_c().await?;
        }
        Some(ref mut rx) => {
            let _ = rx.recv().await;
        }
    }
    eprintln!("Shutting down.");

    Ok(())
}
