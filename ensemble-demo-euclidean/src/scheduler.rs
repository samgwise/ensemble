//! Scheduler — fires trigger events at the correct times based on BPM.
//!
//! The scheduler runs in a dedicated tokio task, advancing through the pattern
//! at the tempo specified by the BPM param. When the current step is a hit,
//! it sends a trigger event to the output address.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::AppState;

/// Scheduler state — params and step counter.
#[derive(Debug, Clone)]
pub struct SchedulerState {
    /// Tempo in beats per minute.
    pub bpm: f64,
    /// Number of steps per bar.
    pub steps: usize,
    /// Number of hits per bar.
    pub hits: usize,
    /// Rotation offset.
    pub rotation: usize,
    /// Output address for trigger events.
    pub output_address: String,
    /// Current step position (0..steps).
    pub current_step: usize,
}

impl SchedulerState {
    /// Create a new scheduler state with default values.
    pub fn new() -> Self {
        Self {
            bpm: 120.0,
            steps: 16,
            hits: 4,
            rotation: 0,
            output_address: "/demo/euclid/trigger".to_string(),
            current_step: 0,
        }
    }

    /// Calculate the duration of one step in seconds.
    ///
    /// At 120 BPM with 16 steps per bar, each step is 1/16 note = 0.125s.
    pub fn step_duration(&self) -> Duration {
        // BPM is beats per minute. A beat is typically a quarter note.
        // If we have `steps` steps per bar, and a bar is 4 beats:
        // step_duration = 60.0 / bpm * (4.0 / steps as f64)
        // But for simplicity, let's treat BPM as steps per minute:
        let steps_per_second = self.bpm / 60.0;
        Duration::from_secs_f64(1.0 / steps_per_second)
    }

    /// Advance to the next step, wrapping around at the end of the bar.
    pub fn advance(&mut self) {
        self.current_step = (self.current_step + 1) % self.steps;
    }
}

/// Run the scheduler loop.
///
/// This function runs indefinitely, advancing through the pattern and firing
/// trigger events at the appropriate times. It should be spawned as a tokio task.
pub async fn run_scheduler(state: Arc<Mutex<AppState>>) {
    loop {
        // Read current state.
        let (step_duration, is_hit, _running) = {
            let s = state.lock().await;
            if !s.running {
                break;
            }
            let pattern = s.pattern();
            let is_hit = pattern.get(s.scheduler.current_step).copied().unwrap_or(false);
            (s.scheduler.step_duration(), is_hit, s.running)
        };

        // If this step is a hit, send a trigger event.
        if is_hit {
            let s = state.lock().await;
            s.send_trigger().await;
        }

        // Advance to the next step.
        {
            let mut s = state.lock().await;
            s.scheduler.advance();
        }

        // Sleep until the next step.
        tokio::time::sleep(step_duration).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_step_duration() {
        let state = SchedulerState {
            bpm: 120.0,
            steps: 16,
            hits: 4,
            rotation: 0,
            output_address: "/test".to_string(),
            current_step: 0,
        };

        // At 120 BPM, we expect 2 steps per second (120/60 = 2).
        let duration = state.step_duration();
        assert_eq!(duration, Duration::from_secs_f64(0.5));
    }

    #[test]
    fn test_advance() {
        let mut state = SchedulerState {
            bpm: 120.0,
            steps: 4,
            hits: 2,
            rotation: 0,
            output_address: "/test".to_string(),
            current_step: 0,
        };

        assert_eq!(state.current_step, 0);
        state.advance();
        assert_eq!(state.current_step, 1);
        state.advance();
        assert_eq!(state.current_step, 2);
        state.advance();
        assert_eq!(state.current_step, 3);
        state.advance();
        assert_eq!(state.current_step, 0); // Wraps around
    }
}
