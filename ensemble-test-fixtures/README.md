# ensemble-test-fixtures

Language-neutral YAML fixtures for Ensemble protocol conformance testing.

Part of the [Ensemble](https://github.com/samgwise/ensemble) project.

## Usage

```rust
use ensemble_test_fixtures::load_fixture;

let fixture = load_fixture("values/basic.yaml")?;
```