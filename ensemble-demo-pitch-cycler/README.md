# ensemble-demo-pitch-cycler

Pitch pattern cycler — demonstration voice for Ensemble.

Subscribes to trigger events and cycles through a pitch pattern, sending MIDI note events to the output address. Designed to work with the Euclidean rhythm generator as a trigger source.

## Prerequisites

- A running Ensemble hub (`ensemble-hub` or `ensemble-hub-tui`)
- For MIDI output: `ensemble-bridge-midi` connected to a MIDI device

## Usage

```bash
cargo run --bin ensemble-demo-pitch-cycler
```

The cycler connects to the hub automatically via port file discovery.

## Params

| Address | Type | Default | Description |
|---|---|---|---|
| `/demo/pitch/pattern` | List | [60, 64, 67, 72] | MIDI note numbers (C major arpeggio) |
| `/demo/pitch/trigger` | String | `/demo/euclid/trigger` | Trigger input address |
| `/demo/pitch/output` | String | `/midi/play` | MIDI output address |
| `/demo/pitch/channel` | Integer | 0 | MIDI channel (0-15) |
| `/demo/pitch/velocity` | Integer | 100 | MIDI velocity (0-127) |
| `/demo/pitch/duration` | Float | 0.2 | Note duration in seconds |

## How It Works

1. The cycler subscribes to the trigger address (default: `/demo/euclid/trigger`)
2. On each trigger event, it advances to the next pitch in the pattern
3. It sends a MIDI note event to the output address as a Tuple: `(channel, pitch, velocity, duration)`
4. The `ensemble-bridge-midi` bridge receives these events and plays them on a MIDI device

## TUI Controls

| Key | Action |
|---|---|
| `←` / `→` | Remove / add pitch at current position |
| `↑` / `↓` | Adjust current pitch ±1 semitone |
| `C` / `c` | MIDI channel +1 / -1 |
| `V` / `v` | MIDI velocity +10 / -10 |
| `D` / `d` | Note duration +0.05s / -0.05s |
| `Q` | Quit |

## Full Demo Chain

To hear the full demo, run all four components:

```bash
# Terminal 1: Start the hub
cargo run --bin ensemble-hub-tui

# Terminal 2: Start the MIDI bridge (select your MIDI output device)
cargo run --bin ensemble-bridge-midi -- --output 0

# Terminal 3: Start the Euclidean rhythm generator
cargo run --bin ensemble-demo-euclidean

# Terminal 4: Start the pitch pattern cycler
cargo run --bin ensemble-demo-pitch-cycler
```

The Euclidean generator fires triggers at the configured BPM. The pitch cycler receives each trigger, advances through its pattern, and sends MIDI notes to the bridge. The bridge plays the notes on your MIDI device.
