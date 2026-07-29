# ensemble-core

Shared types, wire protocol codec, and re-exports for the Ensemble protocol.

Part of the [Ensemble](https://github.com/samgwise/ensemble) project.

## Usage

```rust
use ensemble_core::protocol::*;
use ensemble_core::codec::{read_message, write_message};
```

Re-exports the protocol builders and constants (`ensemble-protocol`), the value model (`ensemble-values`), manifest types (`ensemble-manifest`), and `ClockSync` (`ensemble-clock`). Local hub discovery lives in the separate `ensemble-discovery` crate.
