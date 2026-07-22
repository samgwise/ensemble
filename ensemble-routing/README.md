# ensemble-routing

Pattern matching and address routing for the Ensemble protocol.

Part of the [Ensemble](https://github.com/samgwise/ensemble) project.

## Usage

```rust
use ensemble_routing::{Route, Router};

let mut router = Router::new();
router.add(Route::parse("/synth/*/freq").unwrap(), 1);
```