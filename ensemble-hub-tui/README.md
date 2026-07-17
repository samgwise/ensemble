# ensemble-hub-tui

Interactive terminal UI for the Ensemble hub. Provides real-time monitoring of voices, actions, params, scheduling, and routing.

## Usage

```sh
# Start with default port (7331)
cargo run --bin ensemble-hub-tui

# Start on a specific port
cargo run --bin ensemble-hub-tui -- --port 8000
```

For the headless version (no TUI), see [`ensemble-hub`](../ensemble-hub/).

## TUI Controls

- **1–5**: Select detail pane (Params, Schedule, Log, Manifest, Route Tester)
- **Tab**: Cycle detail panes
- **j/k** or **Up/Down**: Navigate voice browser
- **i**: Enter input mode (Route Tester)
- **q**: Quit

## Port Configuration

The TUI hub writes a **port file** on startup, just like the headless hub. Clients can discover the hub automatically via this file.

### Override Priority

Port selection follows this priority order (highest to lowest):

1. `--port <port>` CLI argument
2. `ENSEMBLE_HUB_PORT` environment variable
3. Default port `7331`

```sh
# Environment variable
ENSEMBLE_HUB_PORT=8000 cargo run --bin ensemble-hub-tui

# CLI argument (overrides env var)
cargo run --bin ensemble-hub-tui -- --port 9000
```

## Local Hub Discovery

On startup, after successfully binding to a port, the TUI writes a **port file** containing the bound port number. Clients read this file to discover the hub without manual configuration.

### Port File Locations

| Platform | Path |
|----------|------|
| Linux | `$XDG_RUNTIME_DIR/ensemble/hub.port` (fallback: `/tmp/ensemble-hub-{uid}.port`) |
| macOS | `$TMPDIR/ensemble-hub.port` |
| Windows | `%LOCALAPPDATA%\Ensemble\hub.port` |

The port file path can be overridden by setting `ENSEMBLE_HUB_PORT_FILE` (intended for testing and sandboxed deployments).

### Port File Format

The file contains a single line with the port number:

```text
7331
```

### Lifecycle

- The port file is **created** after the hub successfully binds to a port.
- The port file is **deleted** when the TUI exits (pressing `q` or normal shutdown).
- On startup, the hub checks for stale port files from previous crashes and cleans them up if the referenced port is no longer bound.
- On Unix, the port file is created with mode `0600` (user-readable only).

See [`design/local-discovery.md`](../design/local-discovery.md) for the full discovery specification.
