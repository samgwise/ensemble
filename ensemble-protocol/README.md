# ensemble-protocol

Wire protocol message types for the Ensemble protocol.

Part of the [Ensemble](https://github.com/samgwise/ensemble) project.

## Usage

```rust
use ensemble_protocol::{WireMessage, Action};

let msg = WireMessage::Action(Action {
    address: "/synth/freq".into(),
    value: 440.0.into(),
});
```