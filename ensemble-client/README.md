# ensemble-client

Client library for connecting to an Ensemble hub. Handles TCP connection, clock synchronisation, and action send/receive.

## Quick Start

```rust
use ensemble_client::Hub;
use ensemble_core::protocol::*;

#[tokio::main]
async fn main() {
    // Connect with automatic discovery.
    let mut hub = Hub::connect_with_discovery("my-tool")
        .await
        .unwrap();

    // Subscribe to actions.
    hub.subscribe("/midi/in/*").await.unwrap();

    // Send an action.
    hub.send_action(action(
        "/my-tool/ping",
        SignalType::Event,
        0.0,
        Value::Null,
    ))
    .await
    .unwrap();

    // Receive routed actions.
    if let Some(msg) = hub.recv_action().await {
        println!("Received: {:?}", msg);
    }

    hub.disconnect().await;
}
```

## Connecting to the Hub

### Automatic Discovery (Recommended)

```rust
let hub = Hub::connect_with_discovery("my-tool").await?;
```

`connect_with_discovery` attempts to locate the hub automatically using the following order:

1. **Port file** — reads the hub's port file from the platform-specific location (written by the hub on startup)
2. **Default port** — falls back to port `7331`

If the port file exists but the hub is no longer running (stale file), the connection attempt fails gracefully and the client falls back to the default port.

### Explicit Port

```rust
let hub = Hub::connect(7331, "my-tool").await?;
```

Use `Hub::connect(port, name)` when you know the hub's port or need to connect to a non-standard port. This bypasses the port file discovery entirely.

## Port File Locations

The port file is written by the hub (either `ensemble-hub` or `ensemble-hub-tui`) and read by `connect_with_discovery`:

| Platform | Path |
|----------|------|
| Linux | `$XDG_RUNTIME_DIR/ensemble/hub.port` (fallback: `/tmp/ensemble-hub-{uid}.port`) |
| macOS | `$TMPDIR/ensemble-hub.port` |
| Windows | `%LOCALAPPDATA%\Ensemble\hub.port` |

## Sending and Receiving Actions

```rust
// Send an immediate action.
hub.send_action(action(
    "/my-tool/note",
    SignalType::Event,
    0.0,
    Value::Integer(60),
)).await?;

// Schedule an action 1 second from now.
let future = hub.now().await + 1.0;
hub.send_action(action(
    "/my-tool/note",
    SignalType::Event,
    future,
    Value::Integer(64),
)).await?;

// Subscribe to a pattern and receive routed actions.
hub.subscribe("/other/*").await?;
if let Some(action_msg) = hub.recv_action().await {
    // Handle the action.
}
```

## Error Handling

Errors reported by the hub (e.g. a rejected subscription or a reserved-namespace violation) are queued on a bounded error channel rather than printed. Drain them with `recv_error()` (async) or `try_recv_error()` (non-blocking):

```rust
while let Some(err) = hub.try_recv_error() {
    eprintln!("Hub error {}: {}", err.code, err.message);
}
```

If the channel fills because the application never drains it, subsequent errors are dropped so action delivery is never stalled.

## Disconnecting

`hub.disconnect().await` sends a graceful `disconnect` message and waits (with a bounded timeout) for the writer queue to flush it to the socket before closing, so the hub reliably observes the departure.

## Clock Synchronisation

Clock sync happens automatically in the background. Call `hub.now().await` to get the estimated hub time. Use `hub.is_synced().await` to check whether synchronisation has been established. The estimate re-converges automatically if network conditions change (see `ensemble-clock`).

See [`design/local-discovery.md`](../design/local-discovery.md) for the full discovery specification.
