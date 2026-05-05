# Chord

A hub-and-spoke protocol for interconnecting small music applications, with centralised timing, scheduling, and protocol bridging.

Chord makes it easy to build bespoke music tools without each tool needing to handle MIDI interfacing, clock synchronisation, or scheduling. A central **hub** provides a reference clock and routes **actions** between **voices** (connected tools). Bridge nodes translate actions to and from external protocols like MIDI.

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
            │  Chord Hub  │
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
- **Chord** — the overall protocol and system
- **Hub** — the central process providing the reference clock and message routing
- **Voice** — a connected client (your tool, a sequencer, a synth controller, etc.)
- **Action** — a message routed through the hub
- **Bridge** — a specialised voice that translates actions to/from external protocols

## Quick Start

### 1. Start the hub

```sh
cargo run --bin chord-hub
```

The hub launches with a TUI showing connected voices and an event log. Press `q` to quit. Use `--headless` for no UI.

### 2. Start the MIDI bridge

```sh
cargo run --bin chord-bridge-midi
```

Lists available MIDI ports and connects to the first output. Use `--output N` to select a specific port, `--list` to just list ports.

### 3. Build a tool

Add `chord-client` and `chord-core` to your `Cargo.toml`:

```toml
[dependencies]
chord-client = { path = "../chord-client" }
chord-core = { path = "../chord-core" }
tokio = { version = "1", features = ["full"] }
```

Connect to the hub and play a note:

```rust
use chord_client::Hub;
use chord_core::protocol::*;

#[tokio::main]
async fn main() {
    let hub = Hub::connect(7331, "my-tool", vec!["/midi/in/*".into()])
        .await
        .unwrap();

    // Play middle C for 500ms at velocity 80 on channel 0.
    hub.send_action(Action {
        address: "/midi/play".into(),
        signal_type: SignalType::Event,
        timestamp: 0.0,
        payload: Payload::Tuple(vec![
            Value::I32(0),    // channel
            Value::I32(60),   // note (middle C)
            Value::I32(80),   // velocity
            Value::F32(0.5),  // duration in seconds
        ]),
    })
    .await
    .unwrap();

    // Schedule a note 1 second from now.
    let future = hub.now().await + 1.0;
    hub.send_action(Action {
        address: "/midi/play".into(),
        signal_type: SignalType::Event,
        timestamp: future,
        payload: Payload::Tuple(vec![
            Value::I32(0),
            Value::I32(64),   // E4
            Value::I32(80),
            Value::F32(0.5),
        ]),
    })
    .await
    .unwrap();

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

Payload: `(channel: i32, note: i32, velocity: i32, duration_secs: f32)`

The bridge sends note-on immediately (or at the action's timestamp if scheduled) and note-off after `duration_secs`. Uses a mutex counter so retriggering the same note cleanly cancels the previous note-off.

**`/midi/cancel`** — Cancel a pending note-off.

Payload: `(channel: i32, note: i32)`

Bumps the mutex counter, invalidating any pending note-off for that channel/note.

**`/midi/cc`** — Send a MIDI Control Change.

Payload: `(channel: i32, cc_number: i32, value: i32)`

**`/midi/in/note-on`** — Received from MIDI input.

Payload: `(channel: i32, note: i32, velocity: i32)`

**`/midi/in/note-off`** — Received from MIDI input.

Payload: `(channel: i32, note: i32)`

**`/midi/in/cc`** — Received from MIDI input.

Payload: `(channel: i32, cc_number: i32, value: i32)`

## Clock Synchronisation

The hub maintains a monotonic reference clock (starts at 0.0 on launch). Voices automatically synchronise using an O2/NTP-style min-RTT filter — `hub.now()` returns the estimated hub time from any voice. Actions with a non-zero `timestamp` are held by the hub and dispatched at the correct moment.

## Wire Protocol

Messages are length-prefixed MessagePack frames over TCP:

```
[4 bytes LE length][MessagePack payload]
```

MessagePack was chosen for cross-language support — native implementations exist for Python, JavaScript, Go, C, and most other languages. This means writing a Chord client in another language requires no code generation or native bindings.

## Crate Structure

| Crate | Type | Description |
|-------|------|-------------|
| `chord-core` | Library | Shared types, wire protocol, clock sync algorithm, pattern matching |
| `chord-hub` | Binary | Central router with TUI |
| `chord-client` | Library | Client library for building tools |
| `chord-bridge-midi` | Binary | MIDI I/O bridge |

## Configuration

The hub defaults to port `7331`. Override with `CHORD_HUB_PORT` env var or `--headless` for CI/testing.

The MIDI bridge accepts `--output N`, `--input N`, `--hub PORT`, and `--list`.

## Prior Art and Acknowledgements

Chord's design is informed by several existing systems:

- **[O2](https://rbdannenberg.github.io/o2/)** (Roger Dannenberg, CMU) — the clock synchronisation and timestamped message delivery model is directly inspired by O2's approach.
- **[CLASP](https://github.com/lumencanvas/clasp)** — the semantic signal type distinction (Event/Param/Stream) is influenced by CLASP's signal taxonomy.
- **[midi_sender](https://github.com/samgwise/midi_sender)** — the MIDI bridge's play-with-duration and mutex-based cancel pattern is adapted from this project.
- **[OSC](https://opensoundcontrol.stanford.edu/)** — the hierarchical address pattern scheme follows OSC conventions.

## Licence

MIT
