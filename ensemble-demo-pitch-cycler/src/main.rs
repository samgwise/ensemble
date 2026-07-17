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
use tokio::sync::Mutex;

use tui::run_tui;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Shared state between the TUI, event listener, and hub connection.
pub struct AppState {
    /// Hub connection.
    pub hub: Hub,
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
    /// Whether the app should quit.
    pub should_quit: bool,
}

impl AppState {
    /// Create a new app state with default values.
    pub fn new(hub: Hub) -> Self {
        Self {
            hub,
            pattern: vec![60, 64, 67, 72], // C major arpeggio
            current_index: 0,
            last_pitch: None,
            trigger_address: "/demo/euclid/trigger".to_string(),
            output_address: "/midi/play".to_string(),
            channel: 0,
            velocity: 100,
            duration: 0.2,
            should_quit: false,
        }
    }

    /// Publish all current params to the hub.
    pub async fn publish_params(&self) {
        let now = self.hub.now().await;

        let pattern_values: Vec<Value> = self.pattern.iter().map(|&p| Value::Integer(p)).collect();
        let _ = self.hub.send_action(action(
            "/demo/pitch/pattern",
            SignalType::Param,
            now,
            Value::List(pattern_values),
        )).await;

        let _ = self.hub.send_action(action(
            "/demo/pitch/trigger",
            SignalType::Param,
            now,
            Value::String(self.trigger_address.clone()),
        )).await;

        let _ = self.hub.send_action(action(
            "/demo/pitch/output",
            SignalType::Param,
            now,
            Value::String(self.output_address.clone()),
        )).await;

        let _ = self.hub.send_action(action(
            "/demo/pitch/channel",
            SignalType::Param,
            now,
            Value::Integer(self.channel),
        )).await;

        let _ = self.hub.send_action(action(
            "/demo/pitch/velocity",
            SignalType::Param,
            now,
            Value::Integer(self.velocity),
        )).await;

        let _ = self.hub.send_action(action(
            "/demo/pitch/duration",
            SignalType::Param,
            now,
            Value::Float(FloatValue::new(self.duration)),
        )).await;
    }

    /// Advance to the next pitch and send a MIDI note.
    pub async fn on_trigger(&mut self) {
        if self.pattern.is_empty() {
            return;
        }

        // Get the current pitch and advance.
        let pitch = self.pattern[self.current_index];
        self.last_pitch = Some(pitch);
        self.current_index = (self.current_index + 1) % self.pattern.len();

        // Send MIDI note to the output address.
        // Format: (channel, pitch, velocity, duration) as a Tuple.
        let now = self.hub.now().await;
        let _ = self.hub.send_action(action(
            &self.output_address,
            SignalType::Event,
            now,
            Value::Tuple(vec![
                Value::Integer(self.channel),
                Value::Integer(pitch),
                Value::Integer(self.velocity),
                Value::Float(FloatValue::new(self.duration)),
            ]),
        )).await;
    }
}

// ---------------------------------------------------------------------------
// Event listener
// ---------------------------------------------------------------------------

/// Listen for trigger events and advance the pitch pattern.
async fn run_event_listener(state: Arc<Mutex<AppState>>) {
    loop {
        // Receive the next action from the hub.
        let action_msg = {
            let mut s = state.lock().await;
            if s.should_quit {
                break;
            }
            s.hub.recv_action().await
        };

        let Some(msg) = action_msg else {
            break;
        };

        // Check if this is a trigger event at our trigger address.
        let map = match &msg.payload {
            Value::Map(m) => m.clone(),
            _ => continue,
        };
        let address = get_string(&map, "address").unwrap_or_default();
        let signal_type = get_string(&map, "signal_type").unwrap_or_default();

        let trigger_addr = {
            let s = state.lock().await;
            s.trigger_address.clone()
        };

        if address == trigger_addr && signal_type == "event" {
            let mut s = state.lock().await;
            s.on_trigger().await;
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
    eprintln!(
        "Connected to hub as voice #{} — Pitch Cycler",
        hub.voice_id
    );

    // Build shared state.
    let state = Arc::new(Mutex::new(AppState::new(hub)));

    // Subscribe to our trigger address.
    {
        let s = state.lock().await;
        let trigger_pattern = format!("{}/*", s.trigger_address.trim_end_matches('/'));
        s.hub.subscribe(&s.trigger_address).await?;
        // Also subscribe to wildcard in case the trigger address is a parent.
        s.hub.subscribe(&trigger_pattern).await.ok();
    }

    // Publish initial params.
    {
        let s = state.lock().await;
        s.publish_params().await;
    }

    // Spawn the event listener task.
    let listener_state = state.clone();
    tokio::spawn(async move {
        run_event_listener(listener_state).await;
    });

    // Run the TUI (blocks until quit).
    run_tui(state.clone()).await?;

    // Signal the event listener to stop.
    {
        let mut s = state.lock().await;
        s.should_quit = true;
    }

    eprintln!("Pitch Cycler shutting down.");
    Ok(())
}
