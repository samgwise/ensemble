//! Bridge run loop and test helpers.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use anyhow::Result;
use ensemble_client::Hub;
use ensemble_core::protocol::*;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::config::{Config, LocalConfig};
use crate::loop_guard::LoopGuard;
use crate::mapping::MappingEngine;
use crate::param_cache::ParamCache;
use crate::peer_manager::{self, PeerManager};
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
/// provided, it is sent the actual listener port once the QUIC endpoint is
/// bound *and* the local hub connection has been established for the first
/// time.
pub async fn run_bridge(
    config: Config,
    mut shutdown: Option<mpsc::Receiver<()>>,
    ready_tx: Option<oneshot::Sender<u16>>,
) -> Result<()> {
    eprintln!("Ensemble Remote Bridge");
    eprintln!("Bridge name: {}", config.bridge.name);
    eprintln!(
        "Listen address: {}:{}",
        config.bridge.listen_addr, config.bridge.listen_port
    );
    eprintln!("Peers: {}", config.peer.len());
    eprintln!("Mapping rules: {}", config.mapping.len());
    if config.bridge.auth_token.is_some() {
        eprintln!("Authentication: shared secret enabled");
    }

    // Cancellation token and task tracker used to stop and await all spawned
    // tasks on shutdown.
    let cancel = CancellationToken::new();
    let tracker = TaskTracker::new();

    // Initialise core components.
    let guard = Arc::new(LoopGuard::new());
    eprintln!("Bridge ID: {}", guard.bridge_id());

    let engine = Arc::new(MappingEngine::new(&config.mapping));
    let outbound_count = engine.outbound_patterns().len();
    eprintln!("Outbound subscription patterns: {}", outbound_count);

    // Create broadcast channel for outbound actions (local hub → all remote peers).
    let (outbound_tx, _) = broadcast::channel::<WireMessage>(1000);

    // Create channel for inbound actions (remote peers → local hub).
    let (inbound_tx, mut inbound_rx) = mpsc::channel::<WireMessage>(1000);
    let hub_sink = Arc::new(HubSink { tx: inbound_tx });

    // Cache of current param values for replay to newly connected peers.
    let param_cache = ParamCache::new();

    // Tracks open inbound connections so the listener can enforce its cap.
    let inbound_gauge = Arc::new(AtomicUsize::new(0));

    // Create the peer manager to orchestrate inbound and outbound connections.
    let manager = PeerManager::new(
        config.peer.clone(),
        config.bridge.name.clone(),
        guard.clone(),
        engine.clone(),
        hub_sink.clone(),
        outbound_tx.clone(),
        param_cache.clone(),
        cancel.clone(),
        tracker.clone(),
        config.bridge.auth_token.clone(),
        inbound_gauge.clone(),
    );
    let manager_handle = manager.handle();

    // Start QUIC listener and register accepted peers with the manager.
    let (peer_tx, mut peer_rx) = mpsc::channel::<RemotePeer>(100);
    let listener_port = remote_peer::start_listener(
        &config.bridge.listen_addr,
        config.bridge.listen_port,
        peer_tx,
        inbound_gauge,
        config.bridge.max_inbound,
        cancel.clone(),
        tracker.clone(),
    )
    .await?;

    // Register inbound peers with the manager until shutdown.
    let register_handle = manager_handle.clone();
    let register_cancel = cancel.clone();
    tracker.spawn(async move {
        loop {
            tokio::select! {
                peer = peer_rx.recv() => {
                    match peer {
                        Some(peer) => register_handle.register_inbound(peer),
                        None => break,
                    }
                }
                _ = register_cancel.cancelled() => break,
            }
        }
    });

    // Run the peer manager (initiates outbound connections and handles reconnects).
    tracker.spawn(async move {
        manager.run().await;
    });

    // Supervise the local hub connection: reconnect with exponential backoff
    // and resubscribe when the hub drops, matching peer behaviour, rather
    // than leaving the bridge running with no local attachment. The current
    // hub sender is published through a watch channel so the inbound
    // forwarder always delivers to the live connection.
    let (hub_sender_tx, hub_sender_rx) = watch::channel::<Option<mpsc::Sender<WireMessage>>>(None);
    let (hub_ready_tx, hub_ready_rx) = oneshot::channel::<()>();
    {
        let local = config.local.clone();
        let name = config.bridge.name.clone();
        let engine = engine.clone();
        let outbound_tx = outbound_tx.clone();
        let param_cache = param_cache.clone();
        let origin = guard.bridge_id().to_string();
        let hub_cancel = cancel.clone();
        tracker.spawn(async move {
            supervise_local_hub(
                local,
                name,
                engine,
                outbound_tx,
                param_cache,
                origin,
                hub_sender_tx,
                Some(hub_ready_tx),
                hub_cancel,
            )
            .await;
        });
    }

    // Forward inbound actions from peers to the local hub.
    let inbound_forward_cancel = cancel.clone();
    tracker.spawn(async move {
        let hub_sender_rx = hub_sender_rx;
        loop {
            tokio::select! {
                action = inbound_rx.recv() => {
                    match action {
                        Some(action) => {
                            let sender = hub_sender_rx.borrow().clone();
                            match sender {
                                Some(tx) => {
                                    if let Err(e) = tx.send(action).await {
                                        eprintln!("Error sending to local hub: {}", e);
                                    }
                                }
                                None => {
                                    eprintln!(
                                        "Local hub unavailable; dropping inbound action"
                                    );
                                }
                            }
                        }
                        None => break,
                    }
                }
                _ = inbound_forward_cancel.cancelled() => break,
            }
        }
    });

    // Embedded callers (integration tests) wait for the first successful hub
    // connection before the bridge reports ready, so forwarding works as soon
    // as `listen_port` is usable. The standalone binary does not gate
    // startup on the hub being reachable.
    if let Some(ready_tx) = ready_tx {
        let _ = hub_ready_rx.await;
        let _ = ready_tx.send(listener_port);
    }

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

    // Signal every spawned task to stop, then wait for all of them — including
    // the QUIC listener, which drops the endpoint and releases the UDP port —
    // to finish before returning.
    cancel.cancel();
    tracker.close();
    tracker.wait().await;

    Ok(())
}

/// Supervise the local hub connection.
///
/// Connects, subscribes to the outbound mapping patterns, and forwards hub
/// traffic until the connection drops — then reconnects with exponential
/// backoff (matching peer behaviour) and resubscribes. Reconnect attempts
/// are unbounded: a hub that is simply restarted must not permanently
/// detach the bridge, so every failure is logged with its attempt number
/// rather than treated as fatal. `ready_notify` (if provided) is signalled
/// after the first successful connection only.
#[allow(clippy::too_many_arguments)]
async fn supervise_local_hub(
    local: LocalConfig,
    name: String,
    engine: Arc<MappingEngine>,
    outbound_tx: broadcast::Sender<WireMessage>,
    param_cache: ParamCache,
    origin: String,
    hub_sender_tx: watch::Sender<Option<mpsc::Sender<WireMessage>>>,
    mut ready_notify: Option<oneshot::Sender<()>>,
    cancel: CancellationToken,
) {
    let mut attempt: u32 = 0;
    loop {
        if attempt > 0 {
            tokio::select! {
                _ = tokio::time::sleep(peer_manager::backoff_delay(attempt)) => {}
                _ = cancel.cancelled() => return,
            }
        }

        let hub = match crate::local_hub::connect_to_hub(&local, &name).await {
            Ok(hub) => hub,
            Err(e) => {
                attempt = attempt.saturating_add(1);
                eprintln!("Local hub connect failed (attempt {}): {}", attempt, e);
                continue;
            }
        };

        if let Err(e) = crate::local_hub::subscribe_to_patterns(&hub, &engine).await {
            attempt = attempt.saturating_add(1);
            eprintln!("Local hub subscribe failed (attempt {}): {}", attempt, e);
            continue;
        }

        eprintln!("Local hub connection established");
        attempt = 0;
        // Publish the new sender so the inbound forwarder can deliver, and
        // tell any embedded caller the bridge is usable.
        let _ = hub_sender_tx.send(Some(hub.sender()));
        if let Some(notify) = ready_notify.take() {
            let _ = notify.send(());
        }

        let shutdown =
            forward_from_hub(hub, &engine, &outbound_tx, &param_cache, &origin, &cancel).await;

        // The connection ended: withdraw the sender so the inbound forwarder
        // stops delivering to a dead hub.
        let _ = hub_sender_tx.send(None);
        if shutdown {
            return;
        }
        attempt = attempt.saturating_add(1);
        eprintln!(
            "Local hub disconnected; reconnecting with backoff (attempt {})",
            attempt
        );
    }
}

/// Forward actions from the local hub to remote peers until the hub
/// disconnects or shutdown begins. Returns `true` if shutdown was requested.
async fn forward_from_hub(
    mut hub: Hub,
    engine: &Arc<MappingEngine>,
    outbound_tx: &broadcast::Sender<WireMessage>,
    param_cache: &ParamCache,
    origin: &str,
    cancel: &CancellationToken,
) -> bool {
    loop {
        tokio::select! {
            msg = hub.recv_action() => {
                match msg {
                    Some(msg) => match msg.msg_type.as_str() {
                        MSG_UNSET_PARAM => {
                            if let Some(address) = crate::protocol::get_address(&msg) {
                                param_cache.remove(&address);
                                crate::local_hub::forward_unset_to_remote(
                                    &address,
                                    engine,
                                    origin,
                                    outbound_tx,
                                );
                            }
                        }
                        MSG_ACTION => {
                            param_cache.update(&msg);
                            if let Err(e) = crate::local_hub::forward_to_remote(
                                &msg,
                                engine,
                                origin,
                                outbound_tx,
                            )
                            .await
                            {
                                eprintln!("Error forwarding to remote: {}", e);
                            }
                        }
                        other => {
                            eprintln!("Unexpected message from local hub: {}", other);
                        }
                    },
                    None => return false,
                }
            }
            _ = cancel.cancelled() => return true,
        }
    }
}
