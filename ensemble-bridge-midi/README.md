# ensemble-bridge-midi

Translates between Ensemble actions and MIDI I/O. Connects to the hub as a bridge voice, subscribes to `/midi/*`, and converts incoming Ensemble actions into MIDI output messages. Optionally listens on a MIDI input port and publishes incoming MIDI as Ensemble actions.

## Hub connection

By default the bridge uses **automatic port discovery** to locate the hub. The hub writes its bound TCP port to a platform-specific port file at startup, and the bridge reads this file to determine where to connect. This means you can start the bridge without knowing the hub's port.

If the hub is running on a non-default port or the port file is unavailable, you can specify the port explicitly with `--hub <port>`.

## Prerequisites

- A running Ensemble hub (`ensemble-hub` or `ensemble-hub-tui`)
- At least one MIDI output device (required)
- Optionally, a MIDI input device
- The `midir` crate requires platform MIDI backends — on Linux this is ALSA (`libasound2-dev`); on macOS and Windows the system frameworks are used automatically

## Building

```bash
cargo build --bin ensemble-bridge-midi
```

## Usage

```bash
# Run with defaults (output port 0, auto-discover hub)
cargo run --bin ensemble-bridge-midi

# Specify MIDI output and input port indices
cargo run --bin ensemble-bridge-midi -- --output 1 --input 0

# List available MIDI ports and exit
cargo run --bin ensemble-bridge-midi -- --list

# Connect to a hub on an explicit port (bypasses discovery)
cargo run --bin ensemble-bridge-midi -- --hub 8000 --output 2
```

## Command-line arguments

| Argument | Default | Description |
|---|---|---|
| `--output <index>` | `0` | MIDI output port index (from `--list`) |
| `--input <index>` | *(none)* | MIDI input port index; if omitted, no input listener is started |
| `--hub <port>` | *(auto)* | TCP port of the Ensemble hub; if omitted, the hub is discovered automatically via its port file |
| `--list` | — | Print all available MIDI input and output ports, then exit |

## Ensemble action protocol

The bridge subscribes to `/midi/*` and handles three action addresses:

### `/midi/play` — schedule a note

Payload is a tuple of `(channel, note, velocity, duration_secs)`:

```
address: /midi/play
payload: (0, 60, 100, 0.5)    # channel 0, middle C, velocity 100, 500ms duration
```

The bridge sends a MIDI Note-On immediately, then schedules a Note-Off after `duration_secs`. A mutex counter per channel/note pair ensures that retriggering or cancelling a note invalidates any pending Note-Off — only the most recent event for a given key will produce a Note-Off.

### `/midi/cancel` — cancel a pending note-off

Payload is a tuple of `(channel, note)`:

```
address: /midi/cancel
payload: (0, 60)              # channel 0, middle C
```

Invalidates any pending Note-Off for the given channel/note. The key remains sounding until explicitly stopped or superseded by a new `/midi/play`.

### `/midi/cc` — send a control change

Payload is a tuple of `(channel, cc_number, value)`:

```
address: /midi/cc
payload: (0, 7, 100)          # channel 0, CC 7 (volume), value 100
```

Sends the CC message immediately with no scheduling.

## MIDI input

When `--input` is specified, the bridge listens on the given MIDI input port and publishes incoming MIDI as Ensemble actions:

| MIDI message | Action address | Payload |
|---|---|---|
| Note-On (velocity > 0) | `/midi/in/note-on` | `(channel, note, velocity)` |
| Note-Off (or Note-On with velocity 0) | `/midi/in/note-off` | `(channel, note)` |
| Control Change | `/midi/in/cc` | `(channel, cc_number, value)` |

All input actions are published with `SignalType::Event` and timestamp `0.0`.

> **Note:** MIDI input forwarding to the hub is currently limited — incoming messages are logged but not yet forwarded as hub actions. Full bidirectional support requires the client library to expose a send-only handle.

## Key state and cancel safety

The bridge tracks per-key state using a mutex counter pattern. Each `(channel, note)` pair has a monotonically increasing event counter. When a new `/midi/play` or `/midi/cancel` arrives:

1. The counter is bumped, producing a new event ID.
2. The Note-On is sent immediately if the event ID matches.
3. A tokio task sleeps for the duration, then attempts Note-Off — but only if the event ID still matches.

This means retriggering a note (sending a new `/midi/play` for the same key before the previous Note-Off fires) silently drops the old Note-Off, avoiding stuck notes without requiring explicit cancellation.
