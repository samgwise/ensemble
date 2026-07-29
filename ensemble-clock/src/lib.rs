//! Clock synchronisation algorithm for Ensemble voices.
//!
//! Uses a min-RTT filter to estimate the offset between a voice's local
//! clock and the hub's reference clock, similar to O2/NTP.
//!
//! The best sample is windowed: if the minimum RTT has not improved for
//! [`MIN_RTT_WINDOW`], the filter resets so the offset can re-converge after
//! network changes (e.g. a route change that raises the achievable RTT).

use std::time::{Duration, Instant};

/// How long the best RTT sample may go unimproved before the filter resets.
///
/// After this window the minimum RTT is forgotten and the next valid sample
/// becomes the new best, letting the offset estimate track network changes.
pub const MIN_RTT_WINDOW: Duration = Duration::from_secs(10);

/// Tracks clock offset between a voice's local clock and the hub's clock.
///
/// The algorithm works by collecting round-trip time samples and using the
/// sample with the minimum RTT to estimate the offset. This filters out
/// delayed packets that would skew the estimate upward.
#[derive(Debug)]
pub struct ClockSync {
    /// Estimated offset: hub_time = local_time + offset.
    offset: f64,
    /// Minimum RTT seen so far (used for filtering).
    min_rtt: f64,
    /// Whether we've completed at least one sync.
    synced: bool,
    /// When the current best sample was recorded. Used to expire the
    /// min-RTT filter so a stale best sample doesn't pin the offset
    /// forever after network conditions change.
    best_sample_at: Option<Instant>,
}

impl ClockSync {
    /// Create a new clock sync tracker with no offset estimate.
    pub fn new() -> Self {
        Self {
            offset: 0.0,
            min_rtt: f64::MAX,
            synced: false,
            best_sample_at: None,
        }
    }

    /// Whether at least one successful sync has been processed.
    pub fn is_synced(&self) -> bool {
        self.synced
    }

    /// Current estimated offset (hub_time = local_time + offset).
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// Current minimum RTT observed.
    pub fn min_rtt(&self) -> f64 {
        self.min_rtt
    }

    /// Convert a local time to estimated hub time.
    pub fn to_hub_time(&self, local_time: f64) -> f64 {
        local_time + self.offset
    }

    /// Process a clock sync round-trip and update the offset estimate.
    ///
    /// # Arguments
    /// * `voice_send_time` - local time when the sync request was sent
    /// * `hub_receive_time` - hub time when the request was received
    /// * `hub_send_time` - hub time when the reply was sent
    /// * `voice_receive_time` - local time when the reply was received
    ///
    /// Returns `true` if the offset was updated (i.e. this was the best sample so far).
    pub fn process_reply(
        &mut self,
        voice_send_time: f64,
        hub_receive_time: f64,
        hub_send_time: f64,
        voice_receive_time: f64,
    ) -> bool {
        self.process_reply_at(
            voice_send_time,
            hub_receive_time,
            hub_send_time,
            voice_receive_time,
            Instant::now(),
        )
    }

    /// Inner implementation of [`ClockSync::process_reply`] with an explicit
    /// timestamp, kept separate so the window expiry logic can be tested
    /// without waiting out real time.
    fn process_reply_at(
        &mut self,
        voice_send_time: f64,
        hub_receive_time: f64,
        hub_send_time: f64,
        voice_receive_time: f64,
        now: Instant,
    ) -> bool {
        // Expire the best sample if it has gone unimproved for the window.
        // The offset estimate itself is kept — it remains the best guess
        // until a fresh sample replaces it — but the RTT filter forgets the
        // stale best so a new (possibly worse) baseline can be established.
        if let Some(best_at) = self.best_sample_at {
            if now.duration_since(best_at) >= MIN_RTT_WINDOW {
                self.min_rtt = f64::MAX;
                self.best_sample_at = None;
            }
        }

        // RTT excluding hub processing time.
        let rtt = (voice_receive_time - voice_send_time) - (hub_send_time - hub_receive_time);

        // Only update if this is the best (lowest RTT) sample.
        if rtt >= 0.0 && rtt < self.min_rtt {
            self.min_rtt = rtt;
            // Offset estimated at the midpoint of the round trip.
            self.offset = hub_receive_time - voice_send_time - (rtt / 2.0);
            self.synced = true;
            self.best_sample_at = Some(now);
            return true;
        }

        false
    }
}

impl Default for ClockSync {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state() {
        let cs = ClockSync::new();
        assert!(!cs.is_synced());
        assert_eq!(cs.offset(), 0.0);
        assert_eq!(cs.to_hub_time(10.0), 10.0); // No offset yet.
    }

    #[test]
    fn basic_offset_estimation() {
        let mut cs = ClockSync::new();

        // Simulate: voice is 5 seconds behind the hub.
        // voice sends at local t=1.0
        // hub receives at hub t=6.001 (offset=5, one-way delay=0.001)
        // hub sends reply at hub t=6.002 (1ms processing)
        // voice receives at local t=1.003 (one-way delay=0.001)
        let updated = cs.process_reply(1.0, 6.001, 6.002, 1.003);

        assert!(updated);
        assert!(cs.is_synced());

        // RTT = (1.003 - 1.0) - (6.002 - 6.001) = 0.003 - 0.001 = 0.002
        // offset = 6.001 - 1.0 - (0.002 / 2) = 5.001 - 0.001 = 5.0
        let expected_offset = 5.0;
        assert!((cs.offset() - expected_offset).abs() < 1e-9);

        // Verify to_hub_time.
        assert!((cs.to_hub_time(2.0) - 7.0).abs() < 1e-9);
    }

    #[test]
    fn min_rtt_filtering() {
        let mut cs = ClockSync::new();

        // First sample: good RTT (0.002s).
        cs.process_reply(1.0, 6.001, 6.002, 1.003);
        let first_rtt = cs.min_rtt();
        let first_offset = cs.offset();

        // Second sample: worse RTT (0.010s) — should be ignored.
        let updated = cs.process_reply(2.0, 7.001, 7.002, 2.011);
        assert!(!updated);
        assert_eq!(cs.offset(), first_offset); // Offset unchanged.
        assert_eq!(cs.min_rtt(), first_rtt); // RTT unchanged.

        // Third sample: even better RTT (0.001s) — should update.
        let updated = cs.process_reply(3.0, 8.0005, 8.001, 3.0015);
        assert!(updated);
        assert!(cs.min_rtt() < first_rtt, "min RTT should have improved");
    }

    #[test]
    fn symmetric_delay_gives_exact_offset() {
        let mut cs = ClockSync::new();

        // Perfectly symmetric 0.5ms each way, hub is 10s ahead.
        cs.process_reply(0.0, 10.0005, 10.0005, 0.001);

        // RTT = 0.001 - 0 = 0.001, offset = 10.0005 - 0.0 - 0.0005 = 10.0
        assert!((cs.offset() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn zero_rtt_localhost() {
        let mut cs = ClockSync::new();

        // On localhost, RTT can be essentially zero.
        cs.process_reply(5.0, 5.0, 5.0, 5.0);

        // RTT = 0, offset = 5.0 - 5.0 - 0 = 0.0
        assert!(cs.is_synced());
        assert!((cs.offset() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn negative_rtt_is_rejected() {
        let mut cs = ClockSync::new();

        // Negative RTT can happen with clock skew — should be rejected.
        let updated = cs.process_reply(1.0, 6.0, 6.5, 1.1);
        // RTT = (1.1 - 1.0) - (6.5 - 6.0) = 0.1 - 0.5 = -0.4
        assert!(!updated);
        assert!(!cs.is_synced());
    }

    #[test]
    fn stale_best_sample_resets_min_rtt() {
        let mut cs = ClockSync::new();
        let t0 = Instant::now();

        // Good sample at t0: RTT 0.002.
        assert!(cs.process_reply_at(1.0, 6.001, 6.002, 1.003, t0));
        let good_rtt = cs.min_rtt();

        // Within the window a worse sample is ignored as jitter.
        let t1 = t0 + Duration::from_secs(5);
        assert!(!cs.process_reply_at(2.0, 7.010, 7.010, 2.020, t1)); // RTT 0.020
        assert_eq!(cs.min_rtt(), good_rtt);

        // After the window lapses without improvement the filter resets, so
        // the next sample becomes the new best even though its RTT is worse.
        let t2 = t0 + MIN_RTT_WINDOW + Duration::from_secs(1);
        assert!(cs.process_reply_at(3.0, 8.010, 8.010, 3.020, t2)); // RTT 0.020
        assert!(cs.min_rtt() > good_rtt);
    }

    #[test]
    fn reconverges_after_network_change() {
        let mut cs = ClockSync::new();
        let t0 = Instant::now();

        // Initial network: symmetric 1ms each way, true offset 5.0.
        assert!(cs.process_reply_at(0.0, 5.001, 5.001, 0.002, t0));
        assert!((cs.offset() - 5.0).abs() < 1e-9);

        // Network change: path becomes slow and asymmetric (45ms out, 5ms
        // back, RTT 0.050). Within the window these samples look like jitter
        // and are ignored; the stale offset persists.
        let t1 = t0 + Duration::from_secs(2);
        assert!(!cs.process_reply_at(10.0, 15.045, 15.045, 10.050, t1));
        assert!((cs.offset() - 5.0).abs() < 1e-9);

        // Once the window expires the filter resets and the same sample
        // profile is accepted, re-converging the offset onto the new
        // (asymmetric) network conditions.
        let t2 = t0 + MIN_RTT_WINDOW + Duration::from_secs(1);
        assert!(cs.process_reply_at(20.0, 25.045, 25.045, 20.050, t2));
        // RTT = 0.050; offset = 25.045 - 20.0 - 0.025 = 5.020.
        assert!((cs.min_rtt() - 0.050).abs() < 1e-9);
        assert!((cs.offset() - 5.020).abs() < 1e-9);
    }

    #[test]
    fn improvements_extend_the_window() {
        let mut cs = ClockSync::new();
        let t0 = Instant::now();

        // Best sample at t0: RTT 0.020.
        assert!(cs.process_reply_at(0.0, 5.010, 5.010, 0.020, t0));

        // An improvement 9s later pushes the window out: it is the time since
        // the last improvement that matters, not the age of the first sample.
        let t1 = t0 + Duration::from_secs(9);
        assert!(cs.process_reply_at(1.0, 6.005, 6.005, 1.010, t1)); // RTT 0.010

        // 18s after the first sample but only 9s after the last improvement,
        // the filter has not reset: a middling sample is still ignored.
        let t2 = t1 + Duration::from_secs(9);
        assert!(!cs.process_reply_at(2.0, 7.0075, 7.0075, 2.015, t2)); // RTT 0.015
        assert!((cs.min_rtt() - 0.010).abs() < 1e-9);
    }

    #[test]
    fn synced_state_survives_window_expiry() {
        let mut cs = ClockSync::new();
        let t0 = Instant::now();

        assert!(cs.process_reply_at(1.0, 6.001, 6.002, 1.003, t0));
        assert!(cs.is_synced());

        // Expiring the filter resets min_rtt but keeps the voice synced: the
        // last offset estimate remains the best available guess.
        let t1 = t0 + MIN_RTT_WINDOW + Duration::from_secs(1);
        assert!(cs.process_reply_at(2.0, 7.050, 7.050, 2.100, t1)); // RTT 0.100
        assert!(cs.is_synced());
    }

    #[test]
    fn many_samples_converge() {
        let mut cs = ClockSync::new();
        let true_offset = 3.0;

        // Simulate 20 samples with varying jitter.
        // The best sample should give us close to the true offset.
        for i in 0..20 {
            let jitter = (i as f64) * 0.001; // Increasing jitter.
            let voice_send = i as f64;
            let one_way = 0.0005 + jitter;
            let hub_recv = voice_send + true_offset + one_way;
            let hub_send = hub_recv + 0.0001; // Tiny processing time.
            let voice_recv = hub_send - true_offset + one_way;
            cs.process_reply(voice_send, hub_recv, hub_send, voice_recv);
        }

        assert!(cs.is_synced());
        // The first sample (i=0) had the lowest jitter, so offset should be
        // close to true_offset.
        assert!((cs.offset() - true_offset).abs() < 0.002);
    }
}
