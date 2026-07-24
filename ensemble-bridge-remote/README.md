# ensemble-bridge-remote

Hub-to-hub bridge for connecting Ensemble instances over IP using QUIC.

## Overview

`ensemble-bridge-remote` connects two or more Ensemble hubs across a network. It acts as an ordinary voice on the local hub and proxies actions to/from remote bridges over QUIC, with:

- **Pattern-based address mapping** — rewrite addresses as they cross hub boundaries
- **Loop prevention** — origin tags prevent message cycles in mesh topologies
- **QUIC transport** — no head-of-line blocking between signal types; streams use datagrams for minimal latency

## Architecture

```
Hub A (local)                         Hub B (remote)
    │                                     │
    ▼                                     ▼
Bridge A  ◄──── QUIC (bridge protocol) ────►  Bridge B
 (proxy)                                    (proxy)
```

Each bridge:
1. Connects to its local hub as an ordinary voice (via `ensemble-client`)
2. Listens on `0.0.0.0:<port>` for inbound QUIC connections from remote bridges
3. Initiates outbound QUIC connections to configured remote peers
4. Forwards actions bidirectionally with address mapping and loop prevention

## Configuration

Copy `bridge-remote.example.toml` to `bridge-remote.toml` and adjust:

```toml
[bridge]
name = "site-a-bridge"
listen_port = 7400

[local]
# port = 7331  # optional, uses discovery if omitted

[[peer]]
host = "192.168.1.100"
port = 7400
reconnect = true

[[mapping]]
from_pattern = "/transport/**"
to_template = "/remote/transport/**"
direction = "both"

[[mapping]]
from_pattern = "/track/{id}/volume"
to_template = "/mixer/{id}/gain"
direction = "outbound"
```

### Mapping Rules

Each `[[mapping]]` rule specifies:

- **`from_pattern`** — Ensemble routing pattern (`*`, `**`, `{capture}`)
- **`to_template`** — Output address template (captures and `**` passthrough)
- **`direction`** — `"outbound"`, `"inbound"`, or `"both"`
- **`signal_filter`** — optional list of signal types (`"param"`, `"event"`, `"stream"`)

## Usage

```bash
# Run with default config file (bridge-remote.toml)
cargo run --bin ensemble-bridge-remote

# Run with custom config file
cargo run --bin ensemble-bridge-remote -- path/to/config.toml
```

## Loop Prevention

Each bridge generates a unique UUID at startup (its `bridge_id`). Every outbound action is stamped with this ID as its `origin`. When a bridge receives an action whose `origin` matches its own `bridge_id`, it drops the message — it has looped back.

This correctly handles chains of any length and mesh topologies.

## QUIC Stream Layout

Each bridge connection uses dedicated QUIC streams per signal type:

| QUIC Stream | Delivery | Used For |
|-------------|----------|----------|
| Control | Reliable, ordered | `bridge_hello`, `bridge_subscribe`, `bridge_ping/pong` |
| Param | Reliable, ordered | Param actions |
| Event | Reliable, ordered | Event actions |
| Datagram | Unreliable | Stream-type actions (best-effort, lowest latency) |

This maps directly to Ensemble's three signal types: Params and Events get guaranteed delivery on independent streams, while Streams use QUIC datagrams for minimal latency.

## TLS Certificates

The bridge generates self-signed certificates at startup. For production use, you can provide your own certificates (future enhancement). Bridge connections should only be made over trusted networks.

## Status

This crate is under active development. The core components are implemented:

- ✅ Configuration parsing
- ✅ Address mapping engine with pattern-based rules
- ✅ Bridge wire protocol
- ✅ Loop prevention via origin tags
- ✅ QUIC listener and connector
- ✅ Local hub connection and forwarding

Remaining work:
- Bidirectional stream management for peer connections
- Reconnection logic for dropped peers
- Param replay on peer connect
- Integration tests with multiple hubs

## License

MIT