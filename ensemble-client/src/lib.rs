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
//!     hub.disconnect().await;
//! }
//! ```

use std::sync::Arc;
use std::time::Instant;

use ensemble_core::clock::ClockSync;
use ensemble_core::protocol::*;
use ensemble_core::{codec, CodecError};
use tokio::io::{BufReader, BufWriter};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};

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
            self.sync.process_reply(
                voice_send_time,
                hub_time,
                hub_time,
                voice_receive_time,
            );
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
    pub async fn connect(
        port: u16,
        name: &str,
    ) -> Result<Self, CodecError> {
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
        let voice_id = get_integer(&welcome_map, "voice_id")
            .ok_or_else(|| {
                CodecError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Welcome missing voice_id",
                ))
            })? as VoiceId;

        let clock = Arc::new(Mutex::new(LocalClock::new()));
        let (write_tx, mut write_rx) = mpsc::channel::<WireMessage>(256);
        let (action_tx, action_rx) = mpsc::channel::<WireMessage>(256);

        // Writer task — sends queued messages to the hub.
        let writer_handle = tokio::spawn(async move {
            while let Some(msg) = write_rx.recv().await {
                if codec::write_message(&mut writer, &msg).await.is_err() {
                    break;
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
                            MSG_ERROR => {
                                // Log errors but continue (could be non-fatal)
                                eprintln!("Hub error: {:?}", msg.payload);
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
                {
                    let mut clk = sync_clock.lock().await;
                    let sequence = clk.next_sequence();
                    let _ = sync_tx.send(clock_ping(sequence)).await;
                }
                // Sync frequently at first, then slow down.
                let clk = sync_clock.lock().await;
                let interval = if clk.is_synced() {
                    std::time::Duration::from_secs(5)
                } else {
                    std::time::Duration::from_millis(200)
                };
                drop(clk);
                tokio::time::sleep(interval).await;
            }
        });

        Ok(Hub {
            voice_id,
            tx: write_tx,
            action_rx,
            clock,
            reader_handle: Some(reader_handle),
            writer_handle: Some(writer_handle),
            clock_handle: Some(clock_handle),
        })
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

    /// Receive the next action routed to this voice.
    /// Returns `None` if the connection is closed.
    pub async fn recv_action(&mut self) -> Option<WireMessage> {
        self.action_rx.recv().await
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

    /// Get the current estimated hub time.
    pub async fn now(&self) -> f64 {
        self.clock.lock().await.hub_now()
    }

    /// Check whether clock sync has been established.
    pub async fn is_synced(&self) -> bool {
        self.clock.lock().await.is_synced()
    }

    /// Send a disconnect message and close the connection.
    pub async fn disconnect(self) {
        let _ = self.tx.send(disconnect()).await;
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
