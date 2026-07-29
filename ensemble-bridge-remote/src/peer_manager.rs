//! Peer lifecycle manager for the remote bridge.
//!
//! Owns outbound peer connection attempts, inbound peer registration, active
//! session tracking, and reconnection with exponential backoff. All spawned
//! work is tracked through a shared `TaskTracker` and honours a shared
//! `CancellationToken` so the bridge can shut down cleanly.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// Shared secret peers must present in the handshake (when configured).
    auth_token: Option<String>,
    guard: Arc<LoopGuard>,
    engine: Arc<MappingEngine>,
    sink: Arc<HubSink>,
    outbound_tx: broadcast::Sender<WireMessage>,
    events_tx: mpsc::Sender<PeerEvent>,
    param_cache: ParamCache,
    /// Active sessions by remote bridge_id (at most one keeper per bridge).
    active: Mutex<HashMap<String, ActiveSession>>,
    /// Outbound config keys whose reconnect is suppressed because an inbound
    /// session won the mutual-dial tie-break, by remote bridge_id.
    suppressed: Mutex<HashMap<String, String>>,
    attempts: Mutex<HashMap<String, u32>>,
    /// Monotonic id assigned to each registered session entry.
    next_entry_id: Mutex<u64>,
    /// Count of open inbound connections (enforced by the listener).
    inbound_gauge: Arc<AtomicUsize>,
    /// Shared cancellation token; triggered on bridge shutdown.
    cancel: CancellationToken,
    /// Tracks every spawned task so shutdown can wait for all of them.
    tracker: TaskTracker,
}

/// A session currently registered as the keeper for a peer bridge.
struct ActiveSession {
    /// Unique id so a stale cleanup cannot remove a replacement session.
    entry_id: u64,
    /// Whether this session was inbound.
    is_inbound: bool,
    /// Cancels this session (used when it loses a tie-break replacement).
    cancel: CancellationToken,
    /// Outbound config key, for outbound sessions.
    config_key: Option<String>,
}

/// What to do with a freshly handshaked session to a peer bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionDecision {
    /// No active session to this bridge — run this one.
    Admit,
    /// An equivalent or preferred session is already active — close this one.
    Reject,
    /// This session is preferred over the active one — replace it.
    Replace,
}

/// Deterministic mutual-dial tie-break.
///
/// When both bridges of a pair dial each other, two connections race. Both
/// sides apply the same rule so exactly one survives without reconnect
/// churn: the bridge with the lower bridge_id keeps its *outbound* session;
/// the bridge with the higher bridge_id keeps the corresponding *inbound*
/// session. Same-kind duplicates are always rejected.
fn decide_session(
    local_id: &str,
    remote_id: &str,
    new_is_inbound: bool,
    existing_is_inbound: Option<bool>,
) -> SessionDecision {
    let existing = match existing_is_inbound {
        None => return SessionDecision::Admit,
        Some(existing) => existing,
    };
    if existing == new_is_inbound {
        return SessionDecision::Reject;
    }
    // The lower id prefers outbound (and so rejects inbound duplicates);
    // the higher id prefers inbound.
    let prefer_inbound = local_id > remote_id;
    if new_is_inbound == prefer_inbound {
        SessionDecision::Replace
    } else {
        SessionDecision::Reject
    }
}

/// Releases an inbound connection slot on drop (see `start_listener`).
struct InboundGaugeGuard {
    counted: bool,
    gauge: Arc<AtomicUsize>,
}

impl InboundGaugeGuard {
    fn new(counted: bool, gauge: Arc<AtomicUsize>) -> Self {
        Self { counted, gauge }
    }
}

impl Drop for InboundGaugeGuard {
    fn drop(&mut self) {
        if self.counted {
            self.gauge.fetch_sub(1, Ordering::SeqCst);
        }
    }
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
        auth_token: Option<String>,
        inbound_gauge: Arc<AtomicUsize>,
    ) -> Self {
        let (events_tx, events_rx) = mpsc::channel(100);
        let inner = Arc::new(PeerManagerInner {
            configs,
            bridge_name,
            auth_token,
            guard,
            engine,
            sink,
            outbound_tx,
            events_tx,
            param_cache,
            active: Mutex::new(HashMap::new()),
            suppressed: Mutex::new(HashMap::new()),
            attempts: Mutex::new(HashMap::new()),
            next_entry_id: Mutex::new(0),
            inbound_gauge,
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
            self.inner.spawn_outbound_attempt(config, 0);
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

    async fn handle_event(&mut self, event: PeerEvent) {
        match event {
            PeerEvent::Connected { config_key } => {
                self.inner.attempts.lock().unwrap().insert(config_key, 0);
            }
            PeerEvent::SessionEnded {
                config_key: Some(key),
            } => {
                // Skip reconnects suppressed by the mutual-dial tie-break;
                // dialling resumes when the winning inbound session ends.
                let is_suppressed = self
                    .inner
                    .suppressed
                    .lock()
                    .unwrap()
                    .values()
                    .any(|k| k == &key);
                if is_suppressed {
                    return;
                }
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
                        self.inner
                            .spawn_outbound_attempt(config.clone(), next_attempt);
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
    fn spawn_outbound_attempt(self: &Arc<Self>, config: PeerConfig, attempt: u32) {
        let inner = self.clone();
        let tracker = self.tracker.clone();
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

    async fn run_managed_session(
        self: &Arc<Self>,
        peer: RemotePeer,
        config_key: Option<String>,
        attempt: u32,
    ) {
        let addr = peer.addr;
        let is_inbound = peer.is_inbound;
        // Inbound sessions count against the listener's connection cap; the
        // guard releases the slot on every exit path.
        let _gauge_guard = InboundGaugeGuard::new(is_inbound, self.inbound_gauge.clone());

        // Perform handshake (including shared-secret authentication) and
        // identify the peer.
        let (info, send_stream, recv_stream) = match remote_peer::handshake(
            &peer,
            self.guard.bridge_id(),
            &self.bridge_name,
            self.auth_token.as_deref(),
        )
        .await
        {
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

        // Resolve duplicates and mutual dials deterministically by bridge_id.
        // All decisions happen under the lock; the guard is released before
        // any await so the session future stays `Send`.
        let session_cancel = self.cancel.child_token();
        let entry_id = {
            let mut next = self.next_entry_id.lock().unwrap();
            let id = *next;
            *next += 1;
            id
        };
        enum SessionOutcome {
            Admit,
            Replace(ActiveSession),
            Reject,
        }
        let outcome = {
            let mut active = self.active.lock().unwrap();
            let existing = active.get(&info.bridge_id).map(|s| s.is_inbound);
            match decide_session(
                self.guard.bridge_id(),
                &info.bridge_id,
                is_inbound,
                existing,
            ) {
                SessionDecision::Admit => {
                    active.insert(
                        info.bridge_id.clone(),
                        ActiveSession {
                            entry_id,
                            is_inbound,
                            cancel: session_cancel.clone(),
                            config_key: config_key.clone(),
                        },
                    );
                    SessionOutcome::Admit
                }
                SessionDecision::Replace => {
                    let old = active.remove(&info.bridge_id);
                    active.insert(
                        info.bridge_id.clone(),
                        ActiveSession {
                            entry_id,
                            is_inbound,
                            cancel: session_cancel.clone(),
                            config_key: config_key.clone(),
                        },
                    );
                    match old {
                        Some(old) => SessionOutcome::Replace(old),
                        // Unreachable: Replace is only decided against an
                        // existing session.
                        None => SessionOutcome::Admit,
                    }
                }
                SessionDecision::Reject => SessionOutcome::Reject,
            }
        };

        match outcome {
            SessionOutcome::Admit => {}
            SessionOutcome::Replace(old) => {
                // Close the session that lost the tie-break. Its cleanup finds
                // a different entry registered and leaves ours alone, and its
                // outbound reconnect is suppressed here.
                eprintln!(
                    "[peer {}] Tie-break: replacing {} session to {}",
                    addr,
                    if old.is_inbound {
                        "inbound"
                    } else {
                        "outbound"
                    },
                    info.bridge_id
                );
                if let Some(old_key) = old.config_key.clone() {
                    self.suppressed
                        .lock()
                        .unwrap()
                        .insert(info.bridge_id.clone(), old_key);
                }
                old.cancel.cancel();
            }
            SessionOutcome::Reject => {
                eprintln!(
                    "[peer {}] Duplicate connection to {} ({}); closing",
                    addr,
                    info.bridge_id,
                    if is_inbound { "inbound" } else { "outbound" }
                );
                // Suppress reconnect churn from the rejected outbound
                // attempt; dialling resumes if the winning session ends.
                if let Some(key) = config_key.clone() {
                    self.suppressed
                        .lock()
                        .unwrap()
                        .insert(info.bridge_id.clone(), key);
                }
                peer.connection.close(0u32.into(), b"duplicate connection");
                let _ = self
                    .events_tx
                    .send(PeerEvent::SessionEnded { config_key })
                    .await;
                return;
            }
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
            self.outbound_tx.clone(),
            replay_rx,
            session_cancel,
        )
        .await;

        // Remove our entry only if it is still ours — a tie-break replacement
        // may have registered a newer session for this bridge meanwhile.
        let removed = {
            let mut active = self.active.lock().unwrap();
            match active.get(&info.bridge_id) {
                Some(entry) if entry.entry_id == entry_id => {
                    active.remove(&info.bridge_id);
                    true
                }
                _ => false,
            }
        };

        // If our session was the inbound keeper, resume the suppressed
        // outbound dialling for this bridge so connectivity is re-established.
        if removed && is_inbound {
            let suppressed_key = self.suppressed.lock().unwrap().remove(&info.bridge_id);
            if let Some(key) = suppressed_key {
                if let Some(config) = self
                    .configs
                    .iter()
                    .find(|c| format!("{}:{}", c.host, c.port) == key)
                {
                    if config.reconnect {
                        eprintln!(
                            "[peer {}] Inbound session to {} ended; resuming outbound dialling",
                            addr, info.bridge_id
                        );
                        self.spawn_outbound_attempt(config.clone(), 0);
                    }
                }
            }
        }

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
///
/// Shared with the local-hub supervisor so peers and the hub reconnect on
/// the same schedule.
pub(crate) fn backoff_delay(attempt: u32) -> Duration {
    let exp = attempt.min(5); // 2^5 = 32, capped below at 30
    let seconds = (2u32.pow(exp)).min(30);
    Duration::from_secs(u64::from(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Mutual-dial tie-break --

    #[test]
    fn decide_admits_when_no_existing_session() {
        assert_eq!(
            decide_session("aaa", "bbb", true, None),
            SessionDecision::Admit
        );
        assert_eq!(
            decide_session("aaa", "bbb", false, None),
            SessionDecision::Admit
        );
    }

    #[test]
    fn decide_rejects_same_kind_duplicates() {
        // Two inbound or two outbound sessions to the same bridge: the
        // second is always rejected, regardless of id order.
        assert_eq!(
            decide_session("aaa", "bbb", true, Some(true)),
            SessionDecision::Reject
        );
        assert_eq!(
            decide_session("aaa", "bbb", false, Some(false)),
            SessionDecision::Reject
        );
        assert_eq!(
            decide_session("bbb", "aaa", true, Some(true)),
            SessionDecision::Reject
        );
    }

    #[test]
    fn decide_lower_id_keeps_outbound() {
        // Local id "aaa" < remote "bbb": outbound is preferred.
        // An inbound duplicate of the running outbound session is rejected.
        assert_eq!(
            decide_session("aaa", "bbb", true, Some(false)),
            SessionDecision::Reject
        );
        // A late outbound session replaces a tentatively-accepted inbound.
        assert_eq!(
            decide_session("aaa", "bbb", false, Some(true)),
            SessionDecision::Replace
        );
    }

    #[test]
    fn decide_higher_id_keeps_inbound() {
        // Local id "bbb" > remote "aaa": inbound is preferred.
        // An outbound duplicate of the running inbound session is rejected.
        assert_eq!(
            decide_session("bbb", "aaa", false, Some(true)),
            SessionDecision::Reject
        );
        // A late inbound session replaces a tentatively-accepted outbound.
        assert_eq!(
            decide_session("bbb", "aaa", true, Some(false)),
            SessionDecision::Replace
        );
    }

    #[test]
    fn decide_is_symmetric_between_peers() {
        // Both sides must converge on the same surviving connection: for a
        // given pair, the kind one side rejects is the kind the other keeps.
        for (local, remote) in [("aaa", "bbb"), ("bbb", "aaa")] {
            // Inbound arriving when outbound is active.
            let inbound_decision = decide_session(local, remote, true, Some(false));
            // Outbound arriving when inbound is active.
            let outbound_decision = decide_session(local, remote, false, Some(true));
            // Exactly one of the two mixed-kind collisions is a replace.
            let replaces = [inbound_decision, outbound_decision]
                .iter()
                .filter(|d| **d == SessionDecision::Replace)
                .count();
            assert_eq!(
                replaces, 1,
                "pair ({local}, {remote}) should replace exactly once"
            );
        }
    }

    // -- Backoff --

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
