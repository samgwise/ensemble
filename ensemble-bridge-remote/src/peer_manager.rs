//! Peer lifecycle manager for the remote bridge.
//!
//! Owns outbound peer connection attempts, inbound peer registration, active
//! session tracking, and reconnection with exponential backoff. All spawned
//! work is tracked through a shared `TaskTracker` and honours a shared
//! `CancellationToken` so the bridge can shut down cleanly.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ensemble_core::protocol::WireMessage;
use tokio::sync::{broadcast, mpsc};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::config::PeerConfig;
use crate::loop_guard::LoopGuard;
use crate::mapping::MappingEngine;
use crate::param_cache::ParamCache;
use crate::remote_peer::{self, HubSink, RemotePeer};

/// Manages the lifecycle of remote peer connections.
///
/// A single `PeerManager` instance is created at startup. It spawns initial
/// outbound connection attempts, registers inbound connections from the QUIC
/// listener, and schedules reconnections when configured peers drop.
pub struct PeerManager {
    inner: Arc<PeerManagerInner>,
    events_rx: Option<mpsc::Receiver<PeerEvent>>,
}

struct PeerManagerInner {
    configs: Vec<PeerConfig>,
    bridge_name: String,
    guard: Arc<LoopGuard>,
    engine: Arc<MappingEngine>,
    sink: Arc<HubSink>,
    outbound_tx: broadcast::Sender<WireMessage>,
    events_tx: mpsc::Sender<PeerEvent>,
    param_cache: ParamCache,
    active: Mutex<HashSet<String>>,
    attempts: Mutex<HashMap<String, u32>>,
    /// Shared cancellation token; triggered on bridge shutdown.
    cancel: CancellationToken,
    /// Tracks every spawned task so shutdown can wait for all of them.
    tracker: TaskTracker,
}

enum PeerEvent {
    /// An outbound peer session completed its handshake.
    Connected { config_key: String },
    /// A peer session has ended and should be cleaned up.
    SessionEnded { config_key: Option<String> },
}

impl PeerManager {
    /// Create a new peer manager.
    ///
    /// `cancel` and `tracker` are shared with the rest of the bridge so that
    /// every spawned session participates in graceful shutdown.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        configs: Vec<PeerConfig>,
        bridge_name: String,
        guard: Arc<LoopGuard>,
        engine: Arc<MappingEngine>,
        sink: Arc<HubSink>,
        outbound_tx: broadcast::Sender<WireMessage>,
        param_cache: ParamCache,
        cancel: CancellationToken,
        tracker: TaskTracker,
    ) -> Self {
        let (events_tx, events_rx) = mpsc::channel(100);
        let inner = Arc::new(PeerManagerInner {
            configs,
            bridge_name,
            guard,
            engine,
            sink,
            outbound_tx,
            events_tx,
            param_cache,
            active: Mutex::new(HashSet::new()),
            attempts: Mutex::new(HashMap::new()),
            cancel,
            tracker,
        });
        Self {
            inner,
            events_rx: Some(events_rx),
        }
    }

    /// Run the peer manager event loop.
    ///
    /// This consumes the manager and loops until the event channel closes or
    /// the shared cancellation token is triggered.
    pub async fn run(mut self) {
        let mut events = self.events_rx.take().expect("event receiver already taken");

        // Initiate outbound connections to every configured peer.
        let configs = self.inner.configs.clone();
        for config in configs {
            self.spawn_outbound_attempt(config, 0);
        }

        loop {
            tokio::select! {
                event = events.recv() => {
                    match event {
                        Some(event) => self.handle_event(event).await,
                        None => break,
                    }
                }
                _ = self.inner.cancel.cancelled() => break,
            }
        }
    }

    /// Return a handle that can register inbound peers.
    ///
    /// The handle shares the same internal state but cannot run the event loop.
    pub fn handle(&self) -> PeerManagerHandle {
        PeerManagerHandle {
            inner: self.inner.clone(),
        }
    }

    fn spawn_outbound_attempt(&self, config: PeerConfig, attempt: u32) {
        let inner = self.inner.clone();
        let tracker = inner.tracker.clone();
        let config_key = format!("{}:{}", config.host, config.port);

        tracker.spawn(async move {
            if attempt > 0 {
                tokio::select! {
                    _ = sleep(backoff_delay(attempt)) => {}
                    _ = inner.cancel.cancelled() => return,
                }
            }

            match remote_peer::connect_to_peer(&config).await {
                Ok(peer) => {
                    inner
                        .run_managed_session(peer, Some(config_key), attempt)
                        .await;
                }
                Err(e) => {
                    eprintln!("[peer {}] Connection failed: {}", config_key, e);
                    let _ = inner
                        .events_tx
                        .send(PeerEvent::SessionEnded {
                            config_key: Some(config_key),
                        })
                        .await;
                }
            }
        });
    }

    async fn handle_event(&mut self, event: PeerEvent) {
        match event {
            PeerEvent::Connected { config_key } => {
                self.inner.attempts.lock().unwrap().insert(config_key, 0);
            }
            PeerEvent::SessionEnded {
                config_key: Some(key),
            } => {
                if let Some(config) = self
                    .inner
                    .configs
                    .iter()
                    .find(|c| format!("{}:{}", c.host, c.port) == key)
                {
                    if config.reconnect {
                        let mut attempts = self.inner.attempts.lock().unwrap();
                        let next_attempt =
                            attempts.get(&key).copied().unwrap_or(0).saturating_add(1);
                        attempts.insert(key.clone(), next_attempt);
                        drop(attempts);
                        self.spawn_outbound_attempt(config.clone(), next_attempt);
                    }
                }
            }
            PeerEvent::SessionEnded { config_key: None } => {
                // Inbound sessions are the remote peer's responsibility to reconnect.
            }
        }
    }
}

/// A lightweight cloneable handle for registering inbound peers.
#[derive(Clone)]
pub struct PeerManagerHandle {
    inner: Arc<PeerManagerInner>,
}

impl PeerManagerHandle {
    /// Register an inbound peer connection and start its managed session.
    pub fn register_inbound(&self, peer: RemotePeer) {
        let inner = self.inner.clone();
        let tracker = inner.tracker.clone();
        tracker.spawn(async move {
            inner.run_managed_session(peer, None, 0).await;
        });
    }
}

impl PeerManagerInner {
    async fn run_managed_session(
        &self,
        peer: RemotePeer,
        config_key: Option<String>,
        attempt: u32,
    ) {
        let addr = peer.addr;

        // Perform handshake and identify the peer.
        let (info, send_stream, recv_stream) =
            match remote_peer::handshake(&peer, self.guard.bridge_id(), &self.bridge_name).await {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[peer {}] Handshake failed: {}", addr, e);
                    let _ = self
                        .events_tx
                        .send(PeerEvent::SessionEnded { config_key })
                        .await;
                    return;
                }
            };

        // Reject duplicate connections to the same bridge.
        let is_duplicate = {
            let mut active = self.active.lock().unwrap();
            if active.contains(&info.bridge_id) {
                true
            } else {
                active.insert(info.bridge_id.clone());
                false
            }
        };

        if is_duplicate {
            eprintln!(
                "[peer {}] Duplicate connection to {} ({}); closing",
                addr,
                info.bridge_id,
                if peer.is_inbound {
                    "inbound"
                } else {
                    "outbound"
                }
            );
            peer.connection.close(0u32.into(), b"duplicate connection");
            let _ = self
                .events_tx
                .send(PeerEvent::SessionEnded { config_key })
                .await;
            return;
        }

        // Notify the manager of a successful outbound handshake.
        if let Some(key) = config_key.clone() {
            let _ = self
                .events_tx
                .send(PeerEvent::Connected { config_key: key })
                .await;
        }

        eprintln!(
            "[peer {}] Session active (bridge_id: {}, attempt {})",
            addr, info.bridge_id, attempt
        );

        // Create a direct channel for param replay to this peer.
        let (replay_tx, replay_rx) = mpsc::channel(100);
        if self.should_replay_params(&config_key) {
            let cache = self.param_cache.clone();
            let engine = self.engine.clone();
            let origin = self.guard.bridge_id().to_string();
            self.tracker.spawn(async move {
                cache.replay(&engine, &origin, replay_tx).await;
            });
        }
        // If replay is disabled, the sender is dropped and the peer receives no replay.

        // Run the peer session until the connection drops or the bridge shuts down.
        let outbound_rx = self.outbound_tx.subscribe();
        remote_peer::run_peer_session(
            peer,
            info.clone(),
            send_stream,
            recv_stream,
            self.guard.clone(),
            self.engine.clone(),
            self.sink.clone(),
            outbound_rx,
            replay_rx,
            self.cancel.clone(),
        )
        .await;

        // Remove from active sessions and notify the manager.
        self.active.lock().unwrap().remove(&info.bridge_id);
        let _ = self
            .events_tx
            .send(PeerEvent::SessionEnded { config_key })
            .await;
    }

    fn should_replay_params(&self, config_key: &Option<String>) -> bool {
        match config_key {
            None => true, // inbound peers default to replay enabled
            Some(key) => self
                .configs
                .iter()
                .any(|c| format!("{}:{}", c.host, c.port) == *key && c.replay_params),
        }
    }
}

/// Compute the exponential backoff delay for a reconnection attempt.
///
/// The first retry waits 2 seconds, doubling each subsequent attempt, capped
/// at 30 seconds. An attempt count of zero yields a one-second delay for tests
/// that exercise the backoff function directly.
fn backoff_delay(attempt: u32) -> Duration {
    let exp = attempt.min(5); // 2^5 = 32, capped below at 30
    let seconds = (2u32.pow(exp)).min(30);
    Duration::from_secs(u64::from(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_increases_and_caps() {
        assert_eq!(backoff_delay(1).as_secs(), 2);
        assert_eq!(backoff_delay(2).as_secs(), 4);
        assert_eq!(backoff_delay(3).as_secs(), 8);
        assert_eq!(backoff_delay(4).as_secs(), 16);
        assert_eq!(backoff_delay(5).as_secs(), 30);
        assert_eq!(backoff_delay(10).as_secs(), 30);
    }

    #[test]
    fn backoff_attempt_zero_is_immediate() {
        assert_eq!(backoff_delay(0).as_secs(), 1);
    }

    #[test]
    fn backoff_bounds() {
        for attempt in 0..20 {
            let delay = backoff_delay(attempt);
            assert!(
                delay.as_secs() >= 1,
                "attempt {} yielded delay < 1s",
                attempt
            );
            assert!(
                delay.as_secs() <= 30,
                "attempt {} yielded delay > 30s",
                attempt
            );
        }
    }
}
