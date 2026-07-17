# ensemble-hub

Headless Ensemble hub binary. Routes actions between connected voices, provides a reference clock, and handles scheduling.

## Usage

```sh
# Start with default port (7331)
cargo run --bin ensemble-hub

# Start on a specific port
cargo run --bin ensemble-hub -- --port 8000

# Start headless (no TUI) — this binary is always headless
cargo run --bin ensemble-hub
```

For the interactive TUI version, see [`ensemble-hub-tui`](../ensemble-hub-tui/).

## Port Configuration

The hub listens on the first available port, starting from the requested port. The actual bound port is written to a **port file** so that clients can discover it automatically.

### Override Priority

Port selection follows this priority order (highest to lowest):

1. `--port <port>` CLI argument
2. `ENSEMBLE_HUB_PORT` environment variable
3. Default port `7331`

```sh
# Environment variable
ENSEMBLE_HUB_PORT=8000 cargo run --bin ensemble-hub

# CLI argument (overrides env var)
cargo run --bin ensemble-hub -- --port 9000
```

## Local Hub Discovery

On startup, after successfully binding to a port, the hub writes a **port file** containing the bound port number. Clients read this file to discover the hub without manual configuration.

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
- The port file is **deleted** on graceful shutdown (SIGTERM, SIGINT, normal exit).
- On startup, the hub checks for stale port files from previous crashes and cleans them up if the referenced port is no longer bound.
- On Unix, the port file is created with mode `0600` (user-readable only).

See [`design/local-discovery.md`](../design/local-discovery.md) for the full discovery specification.
