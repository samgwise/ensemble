# ensemble-clock

Clock synchronization algorithm for the Ensemble protocol.

Part of the [Ensemble](https://github.com/samgwise/ensemble) project.

## Usage

```rust
use ensemble_clock::Clock;

let mut clock = Clock::new();
clock.tick();
```