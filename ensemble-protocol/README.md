# ensemble-protocol

Wire protocol message types for the Ensemble protocol. Defines the `WireMessage` envelope, message type constants, and the hand-rolled builder functions that are the single source of wire truth.

Part of the [Ensemble](https://github.com/samgwise/ensemble) project.

## Usage

```rust
use ensemble_protocol::{action, SignalType};
use ensemble_values::Value;

let msg = action("/synth/freq", SignalType::Param, 0.0, Value::Float(440.0.into()));
```
