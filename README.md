# Ensemble

A hub-and-spoke protocol for interconnecting small music applications, with centralised timing, scheduling, and protocol bridging.

Ensemble makes it easy to build bespoke music tools without each tool needing to handle MIDI interfacing, clock synchronisation, or scheduling. A central **hub** provides a reference clock and routes **actions** between **voices** (connected tools). Bridge nodes translate actions to and from external protocols like MIDI.

## Architecture

```
┌──────────┐  ┌──────────┐  ┌──────────┐
│  Tool A  │  │  Tool B  │  │  Tool C  │
│ (voice)  │  │ (voice)  │  │ (voice)  │
└────┬─────┘  └────┬─────┘  └────┬─────┘
     │             │             │
     └─────────────┼─────────────┘
                   │ TCP (localhost)
            ┌──────▼──────┐
            │  Ensemble Hub  │
            │  (router +  │
            │   clock)    │
            └──────┬──────┘
                   │
     ┌─────────────┼─────────────┐
     │             │             │
┌────▼─────┐ ┌────▼─────┐ ┌────▼─────┐
│  MIDI    │ │  OSC     │ │  CLASP   │
│  Bridge  │ │  Bridge  │ │  Bridge  │
└──────────┘ └──────────┘ └──────────┘
```

**Terminology:**
- **Ensemble** — the overall protocol and system
- **Hub** — the central process providing the reference clock and message routing
- **Voice** — a connected client (your tool, a sequencer, a synth controller, etc.)
- **Action** — a message routed through the hub
- **Bridge** — a specialised voice that translates actions to/from external protocols

## Quick Start

### 1. Start the hub

```sh
cargo run --bin ensemble-hub
```

The hub launches with a TUI showing connected voices and an event log. Press `q` to quit. Use `--headless` for no UI.

### 2. Start the MIDI bridge

```sh
cargo run --bin ensemble-bridge-midi
```

Lists available MIDI ports and connects to the first output. Use `--output N` to select a specific port, `--list` to just list ports.

### 3. Build a tool

Add `ensemble-client` and `ensemble-core` to your `Cargo.toml`:

```toml
[dependencies]
ensemble-client = { path = "../ensemble-client" }
ensemble-core = { path = "../ensemble-core" }
tokio = { version = "1", features = ["full"] }
```

Connect to the hub and play a note:

```rust
use ensemble_client::Hub;
use ensemble_core::protocol::*;

#[tokio::main]
async fn main() {
    let mut hub = Hub::connect(7331, "my-tool")
        .await
        .unwrap();

    // Subscribe to MIDI input.
    hub.subscribe("/midi/in/*").await.unwrap();

    // Play middle C for 500ms at velocity 80 on channel 0.
    hub.send_action(action(
        "/midi/play",
        SignalType::Event,
        0.0,
        Value::Tuple(vec![
            Value::Integer(0),    // channel
            Value::Integer(60),   // note (middle C)
            Value::Integer(80),   // velocity
            Value::Float(FloatValue::new(0.5)),  // duration in seconds
        ]),
    ))
    .await
    .unwrap();

    // Schedule a note 1 second from now.
    let future = hub.now().await + 1.0;
    hub.send_action(action(
        "/midi/play",
        SignalType::Event,
        future,
        Value::Tuple(vec![
            Value::Integer(0),
            Value::Integer(64),   // E4
            Value::Integer(80),
            Value::Float(FloatValue::new(0.5)),
        ]),
    ))
    .await
    .unwrap();

    // Receive actions routed to us.
    if let Some(msg) = hub.recv_action().await {
        let map = match &msg.payload {
            Value::Map(m) => m,
            _ => return,
        };
        let address = get_string(map, "address").unwrap_or_default();
        println!("Received: {}", address);
    }

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    hub.disconnect().await;
}
```

## Action Reference

### Signal Types

| Type | Behaviour |
|------|-----------|
| `Event` | Fire-and-forget. Not persisted by the hub. |
| `Param` | Stateful. Hub remembers last value and replays to late-joining voices. |
| `Stream` | High-rate, best-effort. Dropped under congestion rather than queued. |

### MIDI Bridge Actions

**`/midi/play`** — Play a note with automatic note-off scheduling.

Payload: `(channel: Integer, note: Integer, velocity: Integer, duration_secs: Float)`

The bridge sends note-on immediately (or at the action's timestamp if scheduled) and note-off after `duration_secs`. Uses a mutex counter so retriggering the same note cleanly cancels the previous note-off.

**`/midi/cancel`** — Cancel a pending note-off.

Payload: `(channel: Integer, note: Integer)`

Bumps the mutex counter, invalidating any pending note-off for that channel/note.

**`/midi/cc`** — Send a MIDI Control Change.

Payload: `(channel: Integer, cc_number: Integer, value: Integer)`

**`/midi/in/note-on`** — Received from MIDI input.

Payload: `(channel: Integer, note: Integer, velocity: Integer)`

**`/midi/in/note-off`** — Received from MIDI input.

Payload: `(channel: Integer, note: Integer)`

**`/midi/in/cc`** — Received from MIDI input.

Payload: `(channel: Integer, cc_number: Integer, value: Integer)`

## Clock Synchronisation

The hub maintains a monotonic reference clock (starts at 0.0 on launch). Voices automatically synchronise using an O2/NTP-style min-RTT filter — `hub.now()` returns the estimated hub time from any voice. Actions with a non-zero `timestamp` are held by the hub and dispatched at the correct moment.

## Wire Protocol

Messages are length-prefixed MessagePack frames over TCP:

```
[4 bytes LE length][MessagePack payload]
```

MessagePack was chosen for cross-language support — native implementations exist for Python, JavaScript, Go, C, and most other languages. This means writing an Ensemble client in another language requires no code generation or native bindings.

## Crate Structure

| Crate | Type | Description |
|-------|------|-------------|
| `ensemble-core` | Library | Shared types, wire protocol, codec |
| `ensemble-values` | Library | Value model (10 types: Null, Bool, Integer, Float, String, Binary, Tuple, List, Map, TypedBinary) |
| `ensemble-routing` | Library | Pattern matching and address routing |
| `ensemble-protocol` | Library | WireMessage envelope and message types |
| `ensemble-manifest` | Library | Voice manifest types |
| `ensemble-clock` | Library | Clock synchronization algorithm |
| `ensemble-hub` | Library + Binary | Central router (headless binary) |
| `ensemble-hub-tui` | Binary | Hub TUI with voice browser, action monitor, param inspector |
| `ensemble-client` | Library | Client library for building tools |
| `ensemble-bridge-midi` | Binary | MIDI I/O bridge |
| `ensemble-bridge-osc` | Binary | OSC/UDP I/O bridge |
| `ensemble-test-fixtures` | Library | YAML conformance test fixtures |
| `ensemble-conformance` | Test | Conformance test harness |
| `ensemble-demo-euclidean` | Binary | Euclidean rhythm generator demo |
| `ensemble-demo-pitch-cycler` | Binary | Pitch pattern cycler demo |

## Hub TUI

The hub includes a comprehensive TUI for monitoring and debugging:

- **Voice Browser**: View connected voices, subscriptions, and connection times
- **Manifest Browser**: Inspect voice manifests and capabilities
- **Action Monitor**: Real-time view of routed actions
- **Param Inspector**: View current param state and owners
- **Scheduling Monitor**: See scheduled actions and dispatch times
- **Log Viewer**: Hub event log
- **Route Tester**: Test pattern matching interactively

Navigate with number keys (1-7) or Tab. Press `q` to quit.

## Conformance Testing

Ensemble includes a comprehensive conformance test suite to ensure protocol compliance:

```sh
cargo test -p ensemble-conformance
```

The test suite covers:
- **Routing**: Pattern matching, invalid patterns, namespace enforcement
- **Values**: Type preservation, type discrimination
- **Protocol**: Error codes, action structure
- **Lifecycle**: Voice registration, disconnect cleanup
- **Scheduling**: Dispatch timing, activation time retention
- **Params**: State management, scoping
- **Manifests**: Registration, patching, routing independence

Test fixtures are stored as language-neutral YAML files in `ensemble-test-fixtures/fixtures/`, making it easy to port the conformance suite to other implementations.

## Local Hub Discovery

Clients can discover a running hub automatically without specifying a port. The hub writes a **port file** to a platform-specific location after binding, and clients read it before falling back to the default port.

**Discovery order:** port file → default port (`7331`)

**Port file locations:**

| Platform | Path |
|----------|------|
| Linux | `$XDG_RUNTIME_DIR/ensemble/hub.port` |
| macOS | `$TMPDIR/ensemble-hub.port` |
| Windows | `%LOCALAPPDATA%\Ensemble\hub.port` |

**Override priority** (hub port selection): `--port` CLI arg > `ENSEMBLE_HUB_PORT` env var > default `7331`

```rust
// Client-side: automatic discovery
let hub = Hub::connect_with_discovery("my-tool").await?;

// Client-side: explicit port (bypasses discovery)
let hub = Hub::connect(7331, "my-tool").await?;
```

See [design/local-discovery.md](design/local-discovery.md) for the full specification.

## Demo Applications

Two demonstration voices showcase Ensemble's scheduling, param, and event systems working together:

### Euclidean Rhythm Generator

Generates Euclidean rhythms (Björklund's algorithm) and publishes trigger events. Control BPM, steps, hits, and rotation in real-time via the TUI.

```sh
cargo run --bin ensemble-demo-euclidean
```

### Pitch Pattern Cycler

Subscribes to trigger events and cycles through a pitch pattern, sending MIDI notes to the bridge. Edit the pattern, channel, velocity, and duration via the TUI.

```sh
cargo run --bin ensemble-demo-pitch-cycler
```

### Full Demo Chain

Run all four components to hear a Euclidean rhythm played on MIDI:

```sh
# Terminal 1: Start the hub
cargo run --bin ensemble-hub-tui

# Terminal 2: Start the MIDI bridge
cargo run --bin ensemble-bridge-midi -- --output 0

# Terminal 3: Start the Euclidean generator
cargo run --bin ensemble-demo-euclidean

# Terminal 4: Start the pitch cycler
cargo run --bin ensemble-demo-pitch-cycler
```

### OSC Bridge

Translates between Ensemble actions and OSC/UDP. Connects to the hub as a voice, forwards actions under a configurable Ensemble prefix as OSC messages, and publishes received OSC messages back as Ensemble actions.

```sh
# Default: listen on UDP 9001, send to UDP 9000
cargo run --bin ensemble-bridge-osc

# Custom configuration for SuperCollider
cargo run --bin ensemble-bridge-osc -- --name sc-bridge --osc-send-port 57120 --osc-listen-port 57121
```

**CLI options:**

| Option | Default | Description |
|--------|---------|-------------|
| `--name` | `osc-bridge` | Voice name shown in the hub |
| `--ens-prefix` | `/osc/out` | Ensemble prefix for outbound actions |
| `--osc-prefix` | (empty) | OSC prefix for address mapping |
| `--osc-send-host` | `127.0.0.1` | Host to send OSC messages to |
| `--osc-send-port` | `9000` | Port to send OSC messages to |
| `--osc-listen-port` | `9001` | UDP port to listen for inbound OSC |
| `--hub` | (discovery) | Explicit hub port |

## Configuration

The hub defaults to port `7331`. Override with `--port` CLI argument or `ENSEMBLE_HUB_PORT` environment variable.

The MIDI bridge accepts `--output N`, `--input N`, `--hub PORT`, and `--list`.

## Prior Art and Acknowledgements

Ensemble's design is informed by several existing systems:

- **[O2](https://rbdannenberg.github.io/o2/)** (Roger Dannenberg, CMU) — the clock synchronisation and timestamped message delivery model is directly inspired by O2's approach.
- **[CLASP](https://github.com/lumencanvas/clasp)** — the semantic signal type distinction (Event/Param/Stream) is influenced by CLASP's signal taxonomy.
- **[midi_sender](https://github.com/samgwise/midi_sender)** — the MIDI bridge's play-with-duration and mutex-based cancel pattern is adapted from this project.
- **[OSC](https://opensoundcontrol.stanford.edu/)** — the hierarchical address pattern scheme follows OSC conventions.

## Development & Contributing

### Running CI checks locally

The GitHub Actions CI pipeline runs the following checks on every push and pull request. You can run them locally to catch issues before pushing.

**Run all tests** (must use `--test-threads=1` as discovery tests use process-global environment variables):

```sh
cargo test --workspace -- --test-threads=1
```

**Check formatting:**

```sh
cargo fmt --all -- --check
```

**Run clippy (lint):**

```sh
cargo clippy --workspace -- -D warnings
```

**Publish dry-run** (verifies all crates are ready for crates.io):

```sh
cargo publish --dry-run -p ensemble-values
cargo publish --dry-run -p ensemble-routing
# ... etc, in dependency order
```

### CI pipeline

The CI workflow (`.github/workflows/ci.yml`) runs four jobs on every PR targeting `main`:

- **Test** — builds and tests the workspace on Ubuntu, macOS, and Windows
- **Lint** — runs `cargo fmt --check` and `cargo clippy -- -D warnings`
- **SemVer Checks** — runs `cargo semver-checks check-release` to catch accidental breaking API changes (PRs only)
- **Publish Dry Run** — runs `cargo publish --dry-run` for each crate in dependency order

A release workflow (`.github/workflows/release.yml`) is triggered by pushing a `v*` tag. It runs the full test suite, publishes all 13 crates to crates.io in dependency order, and creates a GitHub release.

### Linux build dependencies

On Linux, the MIDI bridge requires ALSA development libraries:

```sh
sudo apt-get install libasound2-dev
```

### Publishing a release

1. Ensure all CI checks pass locally
2. Run `cargo release 0.1.0 --workspace` (or the next version)
3. This updates versions, creates a git tag, and publishes to crates.io

Note: crates.io has a rate limit of 5 new crates per 10 minutes. The first release may require waiting between batches.

## Licence

MIT
