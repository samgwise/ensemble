# ensemble-manifest

Voice manifest types for the Ensemble protocol.

Part of the [Ensemble](https://github.com/samgwise/ensemble) project.

## Usage

```rust
use ensemble_manifest::{Voice, Param};

let voice = Voice {
    name: "synth".into(),
    params: vec![Param::new("freq", 440.0)],
};
```