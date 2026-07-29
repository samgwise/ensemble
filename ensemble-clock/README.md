# ensemble-clock

Clock synchronization algorithm for the Ensemble protocol. Estimates the offset between a voice's local clock and the hub's reference clock using a min-RTT filter (O2/NTP-style).

Part of the [Ensemble](https://github.com/samgwise/ensemble) project.

## Usage

```rust
use ensemble_clock::ClockSync;

let mut sync = ClockSync::new();
// Feed clock_pong round-trip timestamps as they arrive.
sync.process_reply(voice_send, hub_receive, hub_send, voice_receive);
let hub_time = sync.to_hub_time(local_now);
```

The best RTT sample is windowed: if it has not improved for `MIN_RTT_WINDOW` (10s), the filter resets so the offset estimate re-converges after network changes.
