//! Pitch Pattern Cycler — demonstration voice for Ensemble.
//!
//! Subscribes to trigger events and cycles through a pitch pattern,
//! sending MIDI note events to the output address. Designed to work
//! with the Euclidean rhythm generator as a trigger source.
//!
//! # Params
//!
//! * `/demo/pitch/pattern` — list of MIDI note numbers (default [60, 64, 67, 72])
//! * `/demo/pitch/trigger` — trigger input address (default `/demo/euclid/trigger`)
//! * `/demo/pitch/output` — MIDI output address (default `/midi/play`)
//! * `/demo/pitch/channel` — MIDI channel (default 0)
//! * `/demo/pitch/velocity` — MIDI velocity (default 100)
//! * `/demo/pitch/duration` — note duration in seconds (default 0.2)
//!
//! # Usage
//!
//! ```bash
//! cargo run --bin ensemble-demo-pitch-cycler
//! ```

mod tui;

use std::sync::Arc;

use anyhow::Result;
use ensemble_client::Hub;
use ensemble_core::protocol::*;
use tokio::sync::{mpsc, watch, Mutex};

use tui::run_tui;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Shared state between the TUI and the event listener.
///
/// The `Hub` itself is deliberately NOT kept here: it lives in the event
/// listener task so that `recv_action().await` never runs while this state
/// mutex is held. Sends go through the cheap channel clone in `tx`.
pub struct AppState {
    /// Our assigned voice ID (for display).
    pub voice_id: VoiceId,
    /// Channel to the hub's writer task, for sending actions.
    pub tx: mpsc::Sender<WireMessage>,
    /// Pitch pattern (MIDI note numbers).
    pub pattern: Vec<i64>,
    /// Current index into the pattern.
    pub current_index: usize,
    /// Last pitch played (for display).
    pub last_pitch: Option<i64>,
    /// Trigger input address.
    pub trigger_address: String,
    /// MIDI output address.
    pub output_address: String,
    /// MIDI channel (0-15).
    pub channel: i64,
    /// MIDI velocity (0-127).
    pub velocity: i64,
    /// Note duration in seconds.
    pub duration: f64,
}

impl AppState {
    /// Create a new app state with default values.
    pub fn new(voice_id: VoiceId, tx: mpsc::Sender<WireMessage>) -> Self {
        Self {
            voice_id,
            tx,
            pattern: vec![60, 64, 67, 72], // C major arpeggio
            current_index: 0,
            last_pitch: None,
            trigger_address: "/demo/euclid/trigger".to_string(),
            output_address: "/midi/play".to_string(),
            channel: 0,
            velocity: 100,
            duration: 0.2,
        }
    }

    /// Publish all current params to the hub.
    ///
    /// Params are sent with timestamp 0.0 (immediate dispatch): they are
    /// interactive edits and must never be scheduled into the future.
    pub async fn publish_params(&self) {
        let now = 0.0;

        let pattern_values: Vec<Value> = self.pattern.iter().map(|&p| Value::Integer(p)).collect();
        let _ = self
            .tx
            .send(action(
                "/demo/pitch/pattern",
                SignalType::Param,
                now,
                Value::List(pattern_values),
            ))
            .await;

        let _ = self
            .tx
            .send(action(
                "/demo/pitch/trigger",
                SignalType::Param,
                now,
                Value::String(self.trigger_address.clone()),
            ))
            .await;

        let _ = self
            .tx
            .send(action(
                "/demo/pitch/output",
                SignalType::Param,
                now,
                Value::String(self.output_address.clone()),
            ))
            .await;

        let _ = self
            .tx
            .send(action(
                "/demo/pitch/channel",
                SignalType::Param,
                now,
                Value::Integer(self.channel),
            ))
            .await;

        let _ = self
            .tx
            .send(action(
                "/demo/pitch/velocity",
                SignalType::Param,
                now,
                Value::Integer(self.velocity),
            ))
            .await;

        let _ = self
            .tx
            .send(action(
                "/demo/pitch/duration",
                SignalType::Param,
                now,
                Value::Float(FloatValue::new(self.duration)),
            ))
            .await;
    }

    /// Advance to the next pitch in the pattern and return it.
    ///
    /// Pure state mutation — the caller performs the actual send so that no
    /// mutex guard is ever held across a network await. Returns `None` when
    /// the pattern is empty.
    pub fn advance_pattern(&mut self) -> Option<i64> {
        if self.pattern.is_empty() {
            return None;
        }

        // Get the current pitch and advance.
        let pitch = self.pattern[self.current_index];
        self.last_pitch = Some(pitch);
        self.current_index = (self.current_index + 1) % self.pattern.len();
        Some(pitch)
    }
}

// ---------------------------------------------------------------------------
// Event listener
// ---------------------------------------------------------------------------

/// Listen for trigger events and advance the pitch pattern.
///
/// Owns the `Hub` so `recv_action().await` can run without any state lock
/// being held. The state mutex is only ever locked to process an
/// already-received message, and is released again before any send.
/// Shutdown is signalled via the `quit` watch channel, so the loop exits
/// promptly even while blocked in `recv_action()`.
async fn run_event_listener(
    mut hub: Hub,
    state: Arc<Mutex<AppState>>,
    mut quit: watch::Receiver<bool>,
) {
    loop {
        let msg = tokio::select! {
            // Check the quit signal first so shutdown is never delayed by a
            // message that happens to arrive at the same time.
            biased;
            _ = quit.changed() => break,
            msg = hub.recv_action() => msg,
        };

        let Some(msg) = msg else {
            break;
        };

        // Check if this is a trigger event at our trigger address.
        let map = match &msg.payload {
            Value::Map(m) => m.clone(),
            _ => continue,
        };
        let address = get_string(&map, "address").unwrap_or_default();
        let signal_type = get_string(&map, "signal_type").unwrap_or_default();

        // Lock the state only to process this already-received message.
        // The guard is dropped before we send anything to the hub.
        let note = {
            let mut s = state.lock().await;
            if address == s.trigger_address && signal_type == "event" {
                s.advance_pattern().map(|pitch| {
                    (
                        s.output_address.clone(),
                        s.channel,
                        pitch,
                        s.velocity,
                        s.duration,
                    )
                })
            } else {
                None
            }
        };

        // Send the MIDI note outside the lock.
        // Format: (channel, pitch, velocity, duration) as a Tuple.
        if let Some((output, channel, pitch, velocity, duration)) = note {
            let now = hub.now().await;
            let _ = hub
                .send_action(action(
                    &output,
                    SignalType::Event,
                    now,
                    Value::Tuple(vec![
                        Value::Integer(channel),
                        Value::Integer(pitch),
                        Value::Integer(velocity),
                        Value::Float(FloatValue::new(duration)),
                    ]),
                ))
                .await;
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    // Connect to the hub using automatic discovery.
    let hub = Hub::connect_with_discovery("pitch-cycler").await?;
    eprintln!("Connected to hub as voice #{} — Pitch Cycler", hub.voice_id);

    // Build shared state (the hub stays outside the state mutex).
    let state = Arc::new(Mutex::new(AppState::new(hub.voice_id, hub.sender())));

    // Subscribe to our trigger address before handing the hub to the listener.
    let trigger_address = {
        let s = state.lock().await;
        s.trigger_address.clone()
    };
    hub.subscribe(&trigger_address).await?;

    // Publish initial params.
    {
        let s = state.lock().await;
        s.publish_params().await;
    }

    // Shutdown signal for the event listener. A watch channel is used so the
    // listener wakes even while blocked in `recv_action()`, and so main never
    // needs to lock the state to stop it.
    let (quit_tx, quit_rx) = watch::channel(false);

    // Spawn the event listener task (takes ownership of the hub).
    let listener_state = state.clone();
    let listener = tokio::spawn(async move {
        run_event_listener(hub, listener_state, quit_rx).await;
    });

    // Run the TUI (blocks until quit).
    run_tui(state.clone()).await?;

    // Signal the event listener to stop and wait for it, so the hub
    // connection is dropped (and the voice disconnected) before we exit.
    let _ = quit_tx.send(true);
    let _ = listener.await;

    eprintln!("Pitch Cycler shutting down.");
    Ok(())
}
