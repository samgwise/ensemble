//! Euclidean Rhythm Generator — demonstration voice for Ensemble.
//!
//! Generates Euclidean rhythms using Björklund's algorithm and publishes
//! trigger events to the hub. The TUI provides real-time visualisation
//! and keyboard control of BPM, steps, hits, rotation, and output address.
//!
//! # Params
//!
//! * `/demo/euclid/bpm` — tempo in beats per minute (default 120.0)
//! * `/demo/euclid/steps` — number of steps per bar (default 16)
//! * `/demo/euclid/hits` — number of hits per bar (default 4)
//! * `/demo/euclid/rotation` — rotation offset (default 0)
//! * `/demo/euclid/output` — trigger output address (default `/demo/euclid/trigger`)
//!
//! # Usage
//!
//! ```bash
//! cargo run --bin ensemble-demo-euclidean
//! ```

mod algorithm;
mod scheduler;
mod tui;

use std::sync::Arc;

use anyhow::Result;
use ensemble_client::Hub;
use ensemble_core::protocol::*;
use tokio::sync::Mutex;

use algorithm::euclidean;
use scheduler::SchedulerState;
use tui::run_tui;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Shared state between the TUI, scheduler, and hub connection.
pub struct AppState {
    /// Hub connection.
    pub hub: Hub,
    /// Scheduler state (params + step counter).
    pub scheduler: SchedulerState,
    /// Whether the scheduler is running.
    pub running: bool,
    /// Whether the TUI should quit.
    pub should_quit: bool,
}

impl AppState {
    /// Recompute the pattern from current params.
    pub fn pattern(&self) -> Vec<bool> {
        let s = &self.scheduler;
        euclidean(s.steps, s.hits, s.rotation)
    }

    /// Publish all current params to the hub.
    pub async fn publish_params(&self) {
        let s = &self.scheduler;
        let now = self.hub.now().await;

        let _ = self.hub.send_action(action(
            "/demo/euclid/bpm",
            SignalType::Param,
            now,
            Value::Float(FloatValue::new(s.bpm)),
        )).await;

        let _ = self.hub.send_action(action(
            "/demo/euclid/steps",
            SignalType::Param,
            now,
            Value::Integer(s.steps as i64),
        )).await;

        let _ = self.hub.send_action(action(
            "/demo/euclid/hits",
            SignalType::Param,
            now,
            Value::Integer(s.hits as i64),
        )).await;

        let _ = self.hub.send_action(action(
            "/demo/euclid/rotation",
            SignalType::Param,
            now,
            Value::Integer(s.rotation as i64),
        )).await;

        let _ = self.hub.send_action(action(
            "/demo/euclid/output",
            SignalType::Param,
            now,
            Value::String(s.output_address.clone()),
        )).await;
    }

    /// Send a trigger event to the output address.
    pub async fn send_trigger(&self) {
        let now = self.hub.now().await;
        let _ = self.hub.send_action(action(
            &self.scheduler.output_address,
            SignalType::Event,
            now,
            Value::Null,
        )).await;
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    // Connect to the hub using automatic discovery.
    let hub = Hub::connect_with_discovery("euclidean-generator").await?;
    eprintln!(
        "Connected to hub as voice #{} — Euclidean Generator",
        hub.voice_id
    );

    // Build shared state.
    let state = Arc::new(Mutex::new(AppState {
        hub,
        scheduler: SchedulerState::new(),
        running: true,
        should_quit: false,
    }));

    // Publish initial params.
    {
        let s = state.lock().await;
        s.publish_params().await;
    }

    // Spawn the scheduler task.
    let sched_state = state.clone();
    tokio::spawn(async move {
        scheduler::run_scheduler(sched_state).await;
    });

    // Run the TUI (blocks until quit).
    run_tui(state.clone()).await?;

    // Stop the scheduler.
    {
        let mut s = state.lock().await;
        s.running = false;
        s.should_quit = true;
    }

    eprintln!("Euclidean Generator shutting down.");
    Ok(())
}
