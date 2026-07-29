//! Ensemble client library — connect to an Ensemble hub and exchange actions.
//!
//! The [`Hub`] struct is the main entry point. It handles:
//! - TCP connection to the hub with automatic Hello/Welcome handshake
//! - Background clock synchronisation (transparent, no setup needed)
//! - Sending and receiving actions via async channels
//!
//! # Quick Start
//!
//! ```no_run
//! use ensemble_client::Hub;
//! use ensemble_core::protocol::*;
//!
//! #[tokio::main]
//! async fn main() {
//!     // Connect to the hub.
//!     let mut hub = Hub::connect(7331, "my-tool")
//!         .await.unwrap();
//!
//!     // Subscribe to actions under /other/.
//!     hub.subscribe("/other/*").await.unwrap();
//!
//!     // Send an action (immediate delivery).
//!     hub.send_action(action(
//!         "/my-tool/ping",
//!         SignalType::Event,
//!         0.0,
//!         Value::Null,
//!     )).await.unwrap();
//!
//!     // Schedule an action 1 second in the future.
//!     let future_time = hub.now().await + 1.0;
//!     hub.send_action(action(
//!         "/my-tool/delayed",
//!         SignalType::Event,
//!         future_time,
//!         Value::String("g'day".into()),
//!     )).await.unwrap();
//!
//!     // Receive actions routed to us.
//!     if let Some(action_msg) = hub.recv_action().await {
//!         let map = match &action_msg.payload {
//!             Value::Map(m) => m,
//!             _ => panic!("Expected Map payload"),
//!         };
//!         let source = get_integer(map, "source").unwrap_or(0);
//!         let address = get_string(map, "address").unwrap_or_default();
//!         println!("Received {} from voice {}", address, source);
//!     }
//!
//!     // Surface any errors the hub reported (e.g. rejected subscriptions).
//!     while let Some(err) = hub.try_recv_error() {
//!         eprintln!("Hub error: {err}");
//!     }
//!
//!     hub.disconnect().await;
//! }
//! ```
//!
//! # Error surfacing
//!
//! Errors reported by the hub (`error` control messages, e.g. a rejected
//! subscription or a reserved-namespace violation) are queued on a bounded
//! channel rather than printed. Drain them with [`Hub::recv_error`] or
//! [`Hub::try_recv_error`]. If the application never drains the channel and
//! it fills, subsequent errors are dropped so action delivery is never
//! stalled by unconsumed errors.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ensemble_clock::ClockSync;
use ensemble_core::protocol::*;
use ensemble_core::{codec, CodecError};
use ensemble_discovery as discovery;
use tokio::io::{BufReader, BufWriter};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex};

/// Default hub port used when discovery cannot locate a running hub.
const DEFAULT_HUB_PORT: u16 = 7331;

/// Maximum number of clock pings awaiting a pong before the oldest is
/// evicted. Keeps memory bounded if the hub stops replying.
const MAX_PENDING_PINGS: usize = 64;

/// Capacity of the hub-error channel. Errors beyond this are dropped rather
/// than allowed to stall the reader task (and with it, action delivery).
const MAX_PENDING_ERRORS: usize = 64;

/// How long [`Hub::disconnect`] waits for the writer queue to drain before
/// closing the connection anyway.
const DISCONNECT_FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// An error reported by the hub via an `error` control message.
///
/// Errors are non-fatal unless the hub closes the connection afterwards
/// (as it does for `unsupported_protocol_version`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubError {
    /// Machine-readable error code (see the `ERR_*` constants in
    /// `ensemble_core::protocol`).
    pub code: String,
    /// Human-readable description of what went wrong.
    pub message: String,
}

impl std::fmt::Display for HubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for HubError {}

/// Local clock wrapper that pairs a monotonic origin with a ClockSync tracker.
struct LocalClock {
    /// Monotonic clock baseline.
    origin: Instant,
    /// Shared sync algorithm from ensemble-core.
    sync: ClockSync,
    /// Next clock ping sequence number.
    next_sequence: u64,
    /// Track when each ping was sent (sequence -> local time).
    pending_pings: std::collections::HashMap<u64, f64>,
}

impl LocalClock {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            sync: ClockSync::new(),
            next_sequence: 0,
            pending_pings: std::collections::HashMap::new(),
        }
    }

    /// Local time in seconds since connection.
    fn local_now(&self) -> f64 {
        self.origin.elapsed().as_secs_f64()
    }

    /// Estimated hub time.
    fn hub_now(&self) -> f64 {
        self.sync.to_hub_time(self.local_now())
    }

    /// Record when a ping was sent.
    fn record_ping(&mut self, sequence: u64) {
        if self.pending_pings.len() >= MAX_PENDING_PINGS {
            // Evict the oldest outstanding ping (sequence numbers increase
            // monotonically, so the smallest key is the oldest) to keep the
            // map bounded if pongs stop arriving.
            if let Some(&oldest) = self.pending_pings.keys().min() {
                self.pending_pings.remove(&oldest);
            }
        }
        self.pending_pings.insert(sequence, self.local_now());
    }

    /// Process a clock pong reply.
    fn process_pong(&mut self, sequence: u64, hub_time: f64) {
        let voice_receive_time = self.local_now();

        // Look up when we sent this ping.
        if let Some(voice_send_time) = self.pending_pings.remove(&sequence) {
            // The new protocol only gives us hub_time (when the hub sent the pong).
            // We assume the hub received the ping and sent the pong at essentially
            // the same time (zero processing time), so hub_receive_time = hub_send_time = hub_time.
            // This is a reasonable approximation for localhost and low-latency networks.
            self.sync
                .process_reply(voice_send_time, hub_time, hub_time, voice_receive_time);
        }
    }

    /// Get the next sequence number for clock ping and record when it's sent.
    fn next_sequence(&mut self) -> u64 {
        let seq = self.next_sequence;
        self.next_sequence += 1;
        self.record_ping(seq);
        seq
    }

    fn is_synced(&self) -> bool {
        self.sync.is_synced()
    }
}

// ---------------------------------------------------------------------------
// Hub connection
// ---------------------------------------------------------------------------

/// A connection to an Ensemble hub.
pub struct Hub {
    /// Our assigned voice ID.
    pub voice_id: VoiceId,
    /// Channel to send messages to the writer task.
    tx: mpsc::Sender<WireMessage>,
    /// Channel to receive routed actions from the hub.
    action_rx: mpsc::Receiver<WireMessage>,
    /// Channel to receive errors reported by the hub.
    error_rx: mpsc::Receiver<HubError>,
    /// Flush-request channel to the writer task, used by [`Hub::disconnect`]
    /// to wait for the write queue to drain before closing.
    flush_tx: mpsc::Sender<oneshot::Sender<()>>,
    /// Shared clock state.
    clock: Arc<Mutex<LocalClock>>,
    /// Handle to the reader task (aborted on drop to close connection).
    reader_handle: Option<tokio::task::JoinHandle<()>>,
    /// Handle to the writer task.
    writer_handle: Option<tokio::task::JoinHandle<()>>,
    /// Handle to the clock sync task.
    clock_handle: Option<tokio::task::JoinHandle<()>>,
}

impl Hub {
    /// Connect to an Ensemble hub on localhost.
    pub async fn connect(port: u16, name: &str) -> Result<Self, CodecError> {
        let stream = TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .map_err(CodecError::Io)?;

        let (reader, writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut writer = BufWriter::new(writer);

        // Send Hello.
        let hello_msg = hello(name);
        codec::write_message(&mut writer, &hello_msg).await?;

        // Wait for Welcome.
        let welcome_msg = codec::read_message(&mut reader).await?;
        if welcome_msg.msg_type != MSG_WELCOME {
            return Err(CodecError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Expected welcome, got {:?}", welcome_msg.msg_type),
            )));
        }
        let welcome_map = match &welcome_msg.payload {
            Value::Map(m) => m.clone(),
            _ => {
                return Err(CodecError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Welcome payload must be a Map",
                )));
            }
        };
        let voice_id = get_integer(&welcome_map, "voice_id").ok_or_else(|| {
            CodecError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Welcome missing voice_id",
            ))
        })? as VoiceId;

        let clock = Arc::new(Mutex::new(LocalClock::new()));
        let (write_tx, mut write_rx) = mpsc::channel::<WireMessage>(256);
        let (action_tx, action_rx) = mpsc::channel::<WireMessage>(256);
        let (error_tx, error_rx) = mpsc::channel::<HubError>(MAX_PENDING_ERRORS);
        let (flush_tx, flush_rx) = mpsc::channel::<oneshot::Sender<()>>(1);

        // Writer task — sends queued messages to the hub. Flush requests are
        // only serviced once the message queue is empty (biased select polls
        // the queue first), so an acknowledged flush guarantees everything
        // queued beforehand has been written to the socket.
        let writer_handle = tokio::spawn(async move {
            let mut flush_rx = Some(flush_rx);
            loop {
                tokio::select! {
                    biased;
                    msg = write_rx.recv() => {
                        match msg {
                            Some(msg) => {
                                if codec::write_message(&mut writer, &msg).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    req = async { flush_rx.as_mut().unwrap().recv().await }, if flush_rx.is_some() => {
                        match req {
                            Some(ack) => {
                                let _ = ack.send(());
                            }
                            // Flush requester gone — stop polling a closed
                            // channel (it would otherwise spin ready).
                            None => flush_rx = None,
                        }
                    }
                }
            }
        });

        // Reader task — receives messages from the hub, dispatches clock pong
        // and action messages.
        let reader_clock = clock.clone();
        let reader_handle = tokio::spawn(async move {
            loop {
                match codec::read_message(&mut reader).await {
                    Ok(msg) => {
                        match msg.msg_type.as_str() {
                            MSG_CLOCK_PONG => {
                                let map = match &msg.payload {
                                    Value::Map(m) => m.clone(),
                                    _ => continue,
                                };
                                let sequence = get_integer(&map, "sequence").unwrap_or(0) as u64;
                                let hub_time = get_float(&map, "hub_time").unwrap_or(0.0);
                                let mut clk = reader_clock.lock().await;
                                clk.process_pong(sequence, hub_time);
                            }
                            MSG_ACTION => {
                                let _ = action_tx.send(msg).await;
                            }
                            // Forward unset_param so consumers (e.g. the remote bridge)
                            // can keep their local param cache in sync with the hub.
                            MSG_UNSET_PARAM => {
                                let _ = action_tx.send(msg).await;
                            }
                            MSG_ERROR => {
                                // Route errors to the error channel so
                                // applications can observe rejections.
                                // Best-effort: if the channel is full (the
                                // app isn't draining it), drop rather than
                                // stall the reader task.
                                if let Value::Map(map) = &msg.payload {
                                    let _ = error_tx.try_send(HubError {
                                        code: get_string(map, "code").unwrap_or_default(),
                                        message: get_string(map, "message").unwrap_or_default(),
                                    });
                                }
                            }
                            _ => {} // Ignore other messages
                        }
                    }
                    Err(CodecError::ConnectionClosed) => break,
                    Err(_) => break,
                }
            }
        });

        // Clock sync task — sends periodic clock pings.
        let sync_tx = write_tx.clone();
        let sync_clock = clock.clone();
        let clock_handle = tokio::spawn(async move {
            loop {
                // Take the sequence under the lock, then release it before
                // sending so a congested writer channel can't hold up pong
                // processing (or anyone else needing the clock).
                let sequence = {
                    let mut clk = sync_clock.lock().await;
                    clk.next_sequence()
                };
                let _ = sync_tx.send(clock_ping(sequence)).await;
                // Sync frequently at first, then slow down.
                let interval = {
                    let clk = sync_clock.lock().await;
                    if clk.is_synced() {
                        Duration::from_secs(5)
                    } else {
                        Duration::from_millis(200)
                    }
                };
                tokio::time::sleep(interval).await;
            }
        });

        Ok(Hub {
            voice_id,
            tx: write_tx,
            action_rx,
            error_rx,
            flush_tx,
            clock,
            reader_handle: Some(reader_handle),
            writer_handle: Some(writer_handle),
            clock_handle: Some(clock_handle),
        })
    }

    /// Connect to the hub using automatic port discovery.
    ///
    /// Discovery order: port file (written by the hub at startup) → default
    /// port 7331. If the port file exists but the connection fails (e.g. stale
    /// file), the default port is tried as a fallback.
    pub async fn connect_with_discovery(name: &str) -> Result<Self, CodecError> {
        // Try the port file first.
        if let Some(port) = discovery::read_port_file() {
            if discovery::is_port_bound(port) {
                match Self::connect(port, name).await {
                    Ok(hub) => return Ok(hub),
                    Err(_) => {
                        // Port file was stale — fall through to default.
                    }
                }
            }
        }

        // Fallback to the well-known default port.
        Self::connect(DEFAULT_HUB_PORT, name).await
    }

    /// Send an action to the hub for routing.
    pub async fn send_action(&self, msg: WireMessage) -> Result<(), CodecError> {
        if msg.msg_type != MSG_ACTION {
            return Err(CodecError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "send_action only accepts action messages",
            )));
        }
        self.tx
            .send(msg)
            .await
            .map_err(|_| CodecError::ConnectionClosed)
    }

    /// Get a clone of the internal sender for use in other tasks.
    ///
    /// This allows sending actions from tasks that don't have access to the `Hub`
    /// reference (e.g., when the main task is blocked on `recv_action()`).
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use ensemble_client::Hub;
    /// # async fn example() {
    /// let hub = Hub::connect(7331, "my-tool").await.unwrap();
    /// let sender = hub.sender();
    ///
    /// // Spawn a task that can send actions without holding &Hub
    /// tokio::spawn(async move {
    ///     use ensemble_core::protocol::*;
    ///     let _ = sender.send(action(
    ///         "/my-tool/event",
    ///         SignalType::Event,
    ///         0.0,
    ///         Value::Null,
    ///     )).await;
    /// });
    /// # }
    /// ```
    pub fn sender(&self) -> mpsc::Sender<WireMessage> {
        self.tx.clone()
    }

    /// Receive the next action routed to this voice.
    /// Returns `None` if the connection is closed.
    pub async fn recv_action(&mut self) -> Option<WireMessage> {
        self.action_rx.recv().await
    }

    /// Receive the next error reported by the hub.
    ///
    /// Returns `None` once the connection has closed and all queued errors
    /// have been drained.
    ///
    /// Delivery is best-effort: if the application does not drain the error
    /// channel and it fills (currently 64 entries), further errors are
    /// dropped rather than allowed to stall action delivery.
    pub async fn recv_error(&mut self) -> Option<HubError> {
        self.error_rx.recv().await
    }

    /// Non-blocking variant of [`Hub::recv_error`].
    ///
    /// Returns `None` if no error is currently queued.
    pub fn try_recv_error(&mut self) -> Option<HubError> {
        self.error_rx.try_recv().ok()
    }

    /// Subscribe to actions matching the given pattern.
    pub async fn subscribe(&self, pattern: &str) -> Result<(), CodecError> {
        let msg = subscribe(pattern);
        self.tx
            .send(msg)
            .await
            .map_err(|_| CodecError::ConnectionClosed)
    }

    /// Unsubscribe from a previously registered pattern.
    pub async fn unsubscribe(&self, pattern: &str) -> Result<(), CodecError> {
        let msg = unsubscribe(pattern);
        self.tx
            .send(msg)
            .await
            .map_err(|_| CodecError::ConnectionClosed)
    }

    /// Update the advertised name for this voice.
    pub async fn send_update_name(&self, name: &str) -> Result<(), CodecError> {
        let msg = update_name(name);
        self.tx
            .send(msg)
            .await
            .map_err(|_| CodecError::ConnectionClosed)
    }

    /// Set or replace the manifest for this voice.
    pub async fn set_manifest(&self, manifest: &VoiceManifest) -> Result<(), CodecError> {
        let msg = set_manifest(manifest.to_value());
        self.tx
            .send(msg)
            .await
            .map_err(|_| CodecError::ConnectionClosed)
    }

    /// Apply a partial patch to this voice's manifest.
    /// The patch is a `Value::Map` containing only the fields to update.
    pub async fn patch_manifest(&self, patch: Value) -> Result<(), CodecError> {
        let msg = patch_manifest(patch);
        self.tx
            .send(msg)
            .await
            .map_err(|_| CodecError::ConnectionClosed)
    }

    /// Get the current estimated hub time.
    pub async fn now(&self) -> f64 {
        self.clock.lock().await.hub_now()
    }

    /// Check whether clock sync has been established.
    pub async fn is_synced(&self) -> bool {
        self.clock.lock().await.is_synced()
    }

    /// Send a disconnect message and close the connection.
    ///
    /// The disconnect is flushed before the connection is torn down: after
    /// queueing the message this waits (with a bounded timeout) for the
    /// writer task to confirm everything queued ahead of it has been
    /// written to the socket, so the hub reliably observes the graceful
    /// disconnect. Dropping the `Hub` then aborts the background tasks and
    /// closes the socket.
    pub async fn disconnect(self) {
        if self.tx.send(disconnect()).await.is_ok() {
            let (ack_tx, ack_rx) = oneshot::channel();
            if self.flush_tx.send(ack_tx).await.is_ok() {
                // Bounded wait — if the writer is wedged (e.g. a dead
                // socket), close anyway rather than hang the caller.
                let _ = tokio::time::timeout(DISCONNECT_FLUSH_TIMEOUT, ack_rx).await;
            }
        }
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        // Abort all spawned tasks to close the TCP connection.
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.writer_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.clock_handle.take() {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_pings_are_bounded_with_oldest_evict() {
        let mut clock = LocalClock::new();

        // Record more pings than the cap allows.
        let total = MAX_PENDING_PINGS as u64 + 10;
        for seq in 0..total {
            clock.record_ping(seq);
        }

        // The map stays at the cap, with the oldest sequences evicted and
        // the newest all retained.
        assert_eq!(clock.pending_pings.len(), MAX_PENDING_PINGS);
        assert!(!clock.pending_pings.contains_key(&0));
        assert!(!clock.pending_pings.contains_key(&9));
        assert!(clock.pending_pings.contains_key(&10));
        assert!(clock.pending_pings.contains_key(&(total - 1)));
    }
}
