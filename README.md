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
| `ensemble-test-fixtures` | Library | YAML conformance test fixtures |
| `ensemble-conformance` | Test | Conformance test harness |

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

## Configuration

The hub defaults to port `7331`. Override with `ENSEMBLE_HUB_PORT` env var or `--headless` for CI/testing.

The MIDI bridge accepts `--output N`, `--input N`, `--hub PORT`, and `--list`.

## Prior Art and Acknowledgements

Ensemble's design is informed by several existing systems:

- **[O2](https://rbdannenberg.github.io/o2/)** (Roger Dannenberg, CMU) — the clock synchronisation and timestamped message delivery model is directly inspired by O2's approach.
- **[CLASP](https://github.com/lumencanvas/clasp)** — the semantic signal type distinction (Event/Param/Stream) is influenced by CLASP's signal taxonomy.
- **[midi_sender](https://github.com/samgwise/midi_sender)** — the MIDI bridge's play-with-duration and mutex-based cancel pattern is adapted from this project.
- **[OSC](https://opensoundcontrol.stanford.edu/)** — the hierarchical address pattern scheme follows OSC conventions.

## Licence

MIT
