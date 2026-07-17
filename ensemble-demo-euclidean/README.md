# ensemble-demo-euclidean

Euclidean rhythm generator — demonstration voice for Ensemble.

Generates Euclidean rhythms using Björklund's algorithm and publishes trigger events to the hub. The TUI provides real-time visualisation and keyboard control of BPM, steps, hits, rotation, and output address.

## Prerequisites

- A running Ensemble hub (`ensemble-hub` or `ensemble-hub-tui`)

## Usage

```bash
cargo run --bin ensemble-demo-euclidean
```

The generator connects to the hub automatically via port file discovery.

## Params

| Address | Type | Default | Description |
|---|---|---|---|
| `/demo/euclid/bpm` | Float | 120.0 | Tempo in steps per minute |
| `/demo/euclid/steps` | Integer | 16 | Number of steps per bar |
| `/demo/euclid/hits` | Integer | 4 | Number of hits per bar |
| `/demo/euclid/rotation` | Integer | 0 | Cyclic rotation offset |
| `/demo/euclid/output` | String | `/demo/euclid/trigger` | Trigger output address |

## Output

The generator publishes trigger events (SignalType::Event, Null payload) to the output address. The `ensemble-demo-pitch-cycler` subscribes to these triggers by default and cycles through a pitch pattern.

## TUI Controls

| Key | Action |
|---|---|
| `←` / `→` | Decrease / increase steps |
| `↑` / `↓` | Decrease / increase hits |
| `Shift+←` / `Shift+→` | Decrease / increase rotation |
| `B` / `b` | BPM +10 / -1 |
| `Space` | Pause / resume |
| `Q` | Quit |

## Euclidean Rhythms

Euclidean rhythms distribute `hits` as evenly as possible across `steps`. They were formalised by Godfried Toussaint in 2004, based on the Euclidean GCD algorithm. Many traditional rhythms from around the world are Euclidean:

| E(hits, steps) | Name |
|---|---|
| E(2, 5) | Khafif-e-ramal (Persian) |
| E(3, 8) | Tresillo (Cuban) |
| E(5, 8) | Cinquillo (Cuban) |
| E(5, 16) | Bossa nova (Brazilian) |
