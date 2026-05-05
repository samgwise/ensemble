//! Chord client library — connect to a Chord hub and exchange actions.
//!
//! # Example
//! ```no_run
//! use chord_client::Hub;
//! use chord_core::protocol::*;
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut hub = Hub::connect(7331, "my-tool", vec!["/other/*".into()]).await.unwrap();
//!     hub.send_action(Action {
//!         address: "/my-tool/ping".into(),
//!         signal_type: SignalType::Event,
//!         timestamp: 0.0,
//!         payload: Payload::None,
//!     }).await.unwrap();
//! }
//! ```

use std::sync::Arc;
use std::time::Instant;

use chord_core::clock::ClockSync;
use chord_core::protocol::*;
use chord_core::{codec, CodecError};
use tokio::io::{BufReader, BufWriter};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};

/// Local clock wrapper that pairs a monotonic origin with a ClockSync tracker.
struct LocalClock {
    /// Monotonic clock baseline.
    origin: Instant,
    /// Shared sync algorithm from chord-core.
    sync: ClockSync,
}

impl LocalClock {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            sync: ClockSync::new(),
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

    /// Process a clock sync reply.
    fn process_reply(&mut self, voice_send_time: f64, hub_receive_time: f64, hub_send_time: f64) {
        let voice_receive_time = self.local_now();
        self.sync.process_reply(voice_send_time, hub_receive_time, hub_send_time, voice_receive_time);
    }

    fn is_synced(&self) -> bool {
        self.sync.is_synced()
    }
}

// ---------------------------------------------------------------------------
// Hub connection
// ---------------------------------------------------------------------------

/// A connection to a Chord hub.
pub struct Hub {
    /// Our assigned voice ID.
    pub voice_id: VoiceId,
    /// Channel to send messages to the writer task.
    tx: mpsc::Sender<Message>,
    /// Channel to receive routed actions from the hub.
    action_rx: mpsc::Receiver<(VoiceId, Action)>,
    /// Shared clock state.
    clock: Arc<Mutex<LocalClock>>,
}

impl Hub {
    /// Connect to a Chord hub on localhost.
    pub async fn connect(
        port: u16,
        name: &str,
        subscriptions: Vec<String>,
    ) -> Result<Self, CodecError> {
        let stream = TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .map_err(CodecError::Io)?;

        let (reader, writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut writer = BufWriter::new(writer);

        // Send Hello.
        let hello = Message::Hello(VoiceCapabilities {
            name: name.to_string(),
            subscriptions,
            is_bridge: false,
        });
        codec::write_message(&mut writer, &hello).await?;

        // Wait for Welcome.
        let voice_id = match codec::read_message(&mut reader).await? {
            Message::Welcome { voice_id, .. } => voice_id,
            other => {
                return Err(CodecError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Expected Welcome, got {other:?}"),
                )));
            }
        };

        let clock = Arc::new(Mutex::new(LocalClock::new()));
        let (write_tx, mut write_rx) = mpsc::channel::<Message>(256);
        let (action_tx, action_rx) = mpsc::channel::<(VoiceId, Action)>(256);

        // Writer task — sends queued messages to the hub.
        tokio::spawn(async move {
            while let Some(msg) = write_rx.recv().await {
                if codec::write_message(&mut writer, &msg).await.is_err() {
                    break;
                }
            }
        });

        // Reader task — receives messages from the hub, dispatches clock sync
        // replies and action messages.
        let reader_clock = clock.clone();
        let reader_tx = write_tx.clone();
        tokio::spawn(async move {
            loop {
                match codec::read_message(&mut reader).await {
                    Ok(Message::ClockSyncReply {
                        voice_send_time,
                        hub_receive_time,
                        hub_send_time,
                    }) => {
                        let mut clk = reader_clock.lock().await;
                        clk.process_reply(voice_send_time, hub_receive_time, hub_send_time);
                    }

                    Ok(Message::ActionMessage { source, action }) => {
                        let _ = action_tx.send((source, action)).await;
                    }

                    Err(CodecError::ConnectionClosed) => break,
                    Err(_) => break,

                    _ => {} // Ignore other messages.
                }
            }
            // Connection lost — reader task exits.
            drop(reader_tx);
        });

        // Clock sync task — sends periodic sync requests.
        let sync_tx = write_tx.clone();
        let sync_clock = clock.clone();
        tokio::spawn(async move {
            loop {
                {
                    let clk = sync_clock.lock().await;
                    let voice_send_time = clk.local_now();
                    let _ = sync_tx
                        .send(Message::ClockSyncRequest { voice_send_time })
                        .await;
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
        })
    }

    /// Send an action to the hub for routing.
    pub async fn send_action(&self, action: Action) -> Result<(), CodecError> {
        let msg = Message::ActionMessage {
            source: self.voice_id,
            action,
        };
        self.tx
            .send(msg)
            .await
            .map_err(|_| CodecError::ConnectionClosed)
    }

    /// Receive the next action routed to this voice.
    /// Returns `None` if the connection is closed.
    pub async fn recv_action(&mut self) -> Option<(VoiceId, Action)> {
        self.action_rx.recv().await
    }

    /// Get the current estimated hub time.
    pub async fn now(&self) -> f64 {
        self.clock.lock().await.hub_now()
    }

    /// Check whether clock sync has been established.
    pub async fn is_synced(&self) -> bool {
        self.clock.lock().await.is_synced()
    }

    /// Send a Goodbye message and close the connection.
    pub async fn disconnect(self) {
        let _ = self.tx.send(Message::Goodbye).await;
    }
}
