# ensemble-bridge-remote

Hub-to-hub bridge for connecting Ensemble instances over IP using QUIC.

## Overview

`ensemble-bridge-remote` connects two or more Ensemble hubs across a network. It acts as an ordinary voice on the local hub and proxies actions to/from remote bridges over QUIC, with:

- **Pattern-based address mapping** — rewrite addresses as they cross hub boundaries
- **Loop prevention** — origin tags plus per-message duplicate suppression keep rings and meshes cycle-free
- **Multi-hop forwarding** — actions propagate across chains of bridges with their origin preserved
- **QUIC transport** — no head-of-line blocking between signal types; streams use datagrams for minimal latency
- **Optional shared-secret authentication** and an inbound connection cap

## Architecture

```
Hub A (local)                         Hub B (remote)
    │                                     │
    ▼                                     ▼
Bridge A  ◄──── QUIC (bridge protocol) ────►  Bridge B
 (proxy)                                    (proxy)
```

Each bridge:
1. Connects to its local hub as an ordinary voice (via `ensemble-client`), reconnecting with exponential backoff if the hub drops
2. Listens on a configurable address (default `0.0.0.0:<port>`) for inbound QUIC connections from remote bridges
3. Initiates outbound QUIC connections to configured remote peers, retrying with exponential backoff
4. Forwards actions bidirectionally with address mapping and loop prevention

## Configuration

Copy `bridge-remote.example.toml` to `bridge-remote.toml` and adjust:

```toml
[bridge]
name = "site-a-bridge"
# listen_addr = "0.0.0.0"  # optional bind address (IPv4, IPv6 or hostname)
listen_port = 7400
# auth_token = "change-me"  # optional shared secret; peers must match
# max_inbound = 32         # optional cap on open inbound connections

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

A single `"both"` rule applies the *same* pattern in both directions, so for
fully bidirectional bridging between two namespaces you normally want a pair
of rules (one `outbound`, one `inbound`) — see the example file.

## Usage

```bash
# Run with default config file (bridge-remote.toml)
cargo run --bin ensemble-bridge-remote

# Run with custom config file
cargo run --bin ensemble-bridge-remote -- path/to/config.toml
```

## Loop Prevention and Multi-hop Forwarding

Each bridge generates a unique UUID at startup (its `bridge_id`). Every action it injects onto the bridge network is stamped with this ID as its `origin` and with a unique `msg_id`. A bridge that receives a remote action:

1. Drops it if `origin` matches its own `bridge_id` — the action has looped back.
2. Drops it if the `msg_id` has been seen before — duplicate suppression guarantees exactly-once delivery per bridge and that flooding terminates in cyclic topologies.
3. Forwards it to the local hub (address-mapped) *and* re-forwards it unchanged to its other peers, so actions propagate across chains, rings and meshes with their original origin preserved.

## QUIC Stream Layout

Each peer connection uses:

- **One bidirectional QUIC stream** for all reliable, ordered messages: the `bridge_hello` handshake, `bridge_action` for param and event signal types, `bridge_unset` for param unsets, and `bridge_ping`/`bridge_pong` (reserved).
- **QUIC datagrams** for stream-signal actions: best-effort, lowest latency. Datagrams that fail for congestion or size reasons are logged and dropped without harming the session.

## Security

- **TLS**: the bridge generates self-signed certificates at startup and does not validate peer certificates, which is suitable for trusted networks. User-provided certificates are a future enhancement.
- **Shared-secret authentication**: set `auth_token` on both ends of a peering; the token is exchanged in `bridge_hello` and compared in constant time. Mismatched peers are rejected at the handshake.
- **Inbound connection cap**: `max_inbound` (default 32) limits simultaneously open inbound connections; connections beyond the cap are closed at accept time.

## Status

This crate is under active development. Implemented:

- ✅ Configuration parsing (listener address, auth token, inbound cap)
- ✅ Address mapping engine with pattern-based rules
- ✅ Bridge wire protocol (hello, action, unset; ping/pong reserved)
- ✅ Loop prevention (origin tags) and duplicate suppression (msg_id)
- ✅ Multi-hop forwarding across chains, rings and meshes
- ✅ Deterministic mutual-dial tie-break
- ✅ QUIC listener and connector, with hostname/IPv6 resolution
- ✅ Local hub connection with supervised reconnect and resubscribe
- ✅ Param replay on peer connect (drained ahead of live traffic)
- ✅ Integration tests with multiple hubs

Remaining work:
- Keepalive pings and latency measurement (`bridge_ping`/`bridge_pong`)
- Clock-domain rebasing of bridged timestamps (currently forwarded as immediate)
- User-provided TLS certificates and peer verification

## License

MIT
