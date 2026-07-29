# ensemble-routing

Pattern matching and address routing for the Ensemble protocol.

Part of the [Ensemble](https://github.com/samgwise/ensemble) project.

## Usage

```rust
use ensemble_routing::Pattern;

// Parse and validate a pattern (rejects invalid syntax at parse time).
let pattern = Pattern::parse("/track/{id}/volume").unwrap();

// Match an address, recovering named captures.
let captures = pattern.matches("/track/7/volume").unwrap();
assert_eq!(captures.get("id"), Some("7"));

// Patterns support `*` (one segment), `**` (remainder, final segment only)
// and `{name}` captures. `match_with_suffix` additionally returns the
// segments consumed by a trailing `**`, for template expansion.
let pattern = Pattern::parse("/track/{id}/**").unwrap();
let (captures, suffix) = pattern.match_with_suffix("/track/7/sends/reverb").unwrap();
assert_eq!(captures.get("id"), Some("7"));
assert_eq!(suffix, vec!["sends", "reverb"]);
```
