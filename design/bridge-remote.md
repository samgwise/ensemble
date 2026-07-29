# Ensemble Hub-to-Hub Bridge (`ensemble-bridge-remote`) — Design Specification

## Status

Draft v0.1

Covers the current implementation as of the integration-test milestone.

## Purpose

This document specifies the `ensemble-bridge-remote` crate, which connects two or more Ensemble hubs over an IP network. It is a bridge in the ordinary Ensemble sense: it joins the local hub as a regular voice, speaks the standard Ensemble wire protocol on the local side, and runs a small bridge-specific protocol over QUIC on the remote side.

The goal is to extend the local-first Ensemble model across machines while keeping each hub an independent authority and without introducing special-case behaviour in the hub.

## Scope

In scope:

* Connecting multiple Ensemble hubs through a bridge-per-hub proxy
* Bidirectional forwarding of `action`, `param`, and `stream` messages, including `unset_param` propagation
* Pattern-based address mapping between local and remote address spaces
* Loop prevention across chains and meshes of bridges
* Multi-hop forwarding with origin preservation and duplicate suppression
* Reconnection with exponential backoff (peers and the local hub)
* Param state replay to newly connected peers
* Optional shared-secret authentication and an inbound connection cap
* Self-signed TLS for QUIC

Out of scope for v0.1:

* Clock synchronisation between hubs (bridged actions are forwarded as immediate; see Timestamps)
* User-provided TLS certificates and certificate validation
* Authorisation between bridges (the shared secret is authentication only)
* Encryption beyond QUIC TLS

## Design Principles

### Bridges Are Ordinary Voices

The local bridge instance connects to the hub using `ensemble-client` and participates like any other voice. It subscribes to addresses, sends actions, and receives actions. The hub does not know that the voice is a bridge; it only sees subscriptions and actions.

### Transparent Address Translation

Each bridge maps addresses between the local hub's namespace and the shared remote namespace. A hub can keep its local address space unchanged; the bridge applies the mapping on egress and ingress.

### Loop-Free Forwarding

Every action that crosses a bridge carries an `origin` tag — the bridge ID of the bridge that first sent it onto the remote network — and a unique `msg_id`. A bridge drops any action whose `origin` matches its own ID, and drops any `msg_id` it has already processed. Because each message is handled at most once per bridge, flooding terminates even in cyclic topologies and delivery is exactly-once per bridge.

### Transport over QUIC

Bridge-to-bridge traffic uses QUIC for a single multiplexed connection per peer pair. A single bidirectional QUIC stream carries reliable messages (hello, param, event), and QUIC datagrams carry stream-type actions with best-effort delivery.

## Topology

A bridge is a proxy between one local hub and zero or more remote bridges. Topologies are built from individual peer relationships in bridge configuration.

### One-to-One (Two Hubs)

```text
Hub A          Hub B
  │              │
  ▼              ▼
Bridge A ◄────► Bridge B
```

Bridge A listens on a configured port. Bridge B is configured to connect to A. The connection is bidirectional; once the QUIC connection is established, actions can flow both ways.

### Star / Mesh

```text
        Bridge A (listener)
       /       \
Bridge B       Bridge C
   │              │
 Hub B          Hub C
```

Multiple bridges may connect to the same listener. A bridge can also both listen and initiate outbound connections. The resulting graph is a mesh of pairwise relationships.

### Direction of Initiation

The bridge distinguishes:

* **Inbound peer**: the remote side opened the QUIC connection to this bridge's listener.
* **Outbound peer**: this bridge opened the QUIC connection to a configured remote host and port.

After the connection is established, the direction of initiation only matters for the handshake (to avoid a deadlock), for reconnection logic (outbound peers are retried; inbound peers are not), and for the mutual-dial tie-break (see Duplicate Detection).

## Configuration

Configuration is loaded from a TOML file. The default path is `bridge-remote.toml`, but a custom path may be passed as a command-line argument.

```toml
[bridge]
name = "site-a-bridge"
listen_addr = "0.0.0.0"
listen_port = 7400
auth_token = "change-me"
max_inbound = 32

[local]
port = 7331

[[peer]]
host = "192.168.1.100"
port = 7400
reconnect = true
replay_params = true

[[mapping]]
from_pattern = "/transport/**"
to_template  = "/remote/transport/**"
direction    = "both"

[[mapping]]
from_pattern = "/track/{id}/volume"
to_template  = "/mixer/{id}/gain"
direction    = "outbound"
```

### `[bridge]` Section

|| Field | Type | Description |
|| --- | --- | --- |
|| `name` | string | Human-readable bridge name, sent in the bridge handshake. |
|| `listen_addr` | string | Bind address for the QUIC listener: IPv4/IPv6 literal (brackets optional) or resolvable hostname. Defaults to `0.0.0.0`. |
|| `listen_port` | integer | Port for the QUIC listener. `0` binds an ephemeral port. |
|| `auth_token` | optional string | Shared secret peers must present in the handshake. Compared in constant time. When unset, authentication is disabled. |
|| `max_inbound` | integer | Maximum simultaneously open inbound connections. Defaults to `32`. |

### `[local]` Section

| Field | Type | Description |
| --- | --- | --- |
| `port` | optional integer | TCP port of the local Ensemble hub. |

### `[[peer]]` Entries

| Field | Type | Description |
| --- | --- | --- |
| `host` | string | Remote bridge host or IP address. |
| `port` | integer | Remote bridge listener port. |
| `reconnect` | boolean | Whether to retry the connection after a disconnect. |
| `replay_params` | boolean | Whether to send cached param state to this peer after handshake. Defaults to `true`. |

### `[[mapping]]` Entries

| Field | Type | Description |
| --- | --- | --- |
| `from_pattern` | string | Ensemble routing pattern to match against the local address. |
| `to_template` | string | Template for the translated address. |
| `direction` | string | `outbound`, `inbound`, or `both`. |
| `signal_filter` | optional list of strings | If present, only applies to listed signal types (`param`, `event`, `stream`). |

## Address Mapping Semantics

The mapping engine translates addresses between the local hub namespace and the remote bridge namespace. Rules are applied in configuration order; the first matching rule wins.

### Direction

* `outbound`: local hub → remote peer. The bridge subscribes to `from_pattern` locally and translates matching addresses to `to_template` before forwarding.
* `inbound`: remote peer → local hub. The bridge translates matching remote addresses to `to_template` before publishing locally.
* `both`: applies in both directions.

### Patterns and Captures

Patterns use the standard Ensemble routing syntax:

* `*` matches a single path segment.
* `**` matches the remainder of the path.
* `{name}` captures a segment for substitution into the template.

Template substitution:

* `{name}` is replaced with the captured value.
* `**` in the template is replaced with the unmatched path suffix.

### Example

```toml
[[mapping]]
from_pattern = "/track/{id}/volume"
to_template  = "/mixer/{id}/gain"
direction    = "outbound"
```

A local action at `/track/7/volume` is forwarded to the remote peer as `/mixer/7/gain`.

On the remote hub, the reverse bridge must map `/mixer/{id}/gain` back to `/track/{id}/volume` if it wants the original namespace.

## Loop Prevention and Multi-hop Forwarding

Each bridge generates a random UUID at startup, called its `bridge_id`. When a bridge forwards an action from its local hub to remote peers, it tags the message with its own `bridge_id` in the `origin` field and stamps a unique `msg_id`. A bridge that receives a remote action:

1. Drops it when `origin` equals its own `bridge_id` — the message has looped back.
2. Drops it when the `msg_id` has been processed before (a bounded FIFO seen-set, default capacity 4096, provides duplicate suppression).
3. Otherwise forwards it to the local hub (address-mapped) *and* re-forwards it unchanged to its other peers, preserving `origin` and `msg_id`.

Re-forwarding with duplicate suppression is classic flooding: each bridge processes each unique message at most once, so delivery is exactly-once per bridge and propagation terminates in chains, rings and meshes of any connectivity.

### Example: Chain of Three Bridges

```text
Hub A → Bridge A (id=aaa) → Bridge B (id=bbb) → Bridge C (id=ccc) → Bridge A
```

1. Bridge A forwards an action with `origin=aaa`.
2. Bridge B receives it, records the `msg_id`, and forwards it with `origin=aaa`.
3. Bridge C receives it and forwards it with `origin=aaa`.
4. Bridge A receives it, sees `origin=aaa`, and drops it.

## Bridge Wire Protocol

Bridge-to-bridge communication runs over QUIC. The current implementation uses the `quinn` crate with self-signed TLS certificates. Certificate verification is skipped in development, consistent with the local-first trust model.

### Streams and Datagrams

Each peer connection uses:

* A single **bidirectional QUIC stream** for reliable messages:
  * `bridge_hello` handshake
  * `bridge_action` for `param` and `event` signal types
  * `bridge_unset` for param unsets
  * `bridge_ping` / `bridge_pong` (not yet implemented)
* A **datagram channel** for `stream` signal types, for best-effort low-latency delivery. Datagrams that fail to send for congestion or size reasons are logged and dropped; only connection-level failures end the session.

The reliable stream uses the same length-prefixed MessagePack framing as the hub protocol. Datagrams are one complete MessagePack message per QUIC datagram.

### Handshake

When a connection is established, the two bridges perform a handshake to exchange bridge IDs, names, and the optional auth token. To avoid a deadlock, the outbound peer opens the bidirectional stream and writes first; the inbound peer accepts the incoming stream and reads first. The inbound side verifies the token before replying, so an unauthenticated peer learns nothing about the listener; either side closes the connection on mismatch.

Message: `bridge_hello`

Payload:

```text
{
  "bridge_id": "<uuid>",
  "name": "<bridge name>",
  "auth_token": "<optional shared secret>"
}
```

### Forwarded Action

Message: `bridge_action`

Payload:

```text
{
  "origin": "<bridge id of first remote sender>",
  "msg_id": "<unique message id>",
  "source": "<original voice id>",
  "address": "<remote address>",
  "signal_type": "param" | "event" | "stream",
  "timestamp": "<timestamp>",
  "payload": "<action payload>"
}
```

The `address` is the address after outbound mapping (or before inbound mapping). The `source` is the original `voice_id` from the sending hub; the receiving hub will rewrite the source to the local bridge voice's ID when the action is republished. The `timestamp` is in the sending hub's clock domain; receivers forward the action locally as immediate (timestamp `0.0`) because the hubs are not clock-synchronised.

### Forwarded Unset

Message: `bridge_unset`

Payload:

```text
{
  "origin": "<bridge id of first remote sender>",
  "msg_id": "<unique message id>",
  "source": "<original voice id or 0>",
  "address": "<remote address>"
}
```

Carries a param unset across the bridge. It traverses the same loop-guard, duplicate-suppression, mapping and re-forwarding path as `bridge_action`; on receipt it is republished locally as `unset_param` after inbound mapping.

### Ping / Pong (Reserved)

Message types `bridge_ping` and `bridge_pong` are reserved for future keep-alive and latency measurement. The current implementation recognises the type but does not reply.

## Reconnection and Fault Tolerance

### Outbound Reconnection

For each configured peer with `reconnect = true`, the bridge repeatedly attempts to connect. The retry interval follows exponential backoff:

| Attempt | Delay |
| --- | --- |
| 1 | 2 seconds |
| 2 | 4 seconds |
| 3 | 8 seconds |
| 4 | 16 seconds |
| 5+ | 30 seconds (cap) |

A successful connection resets the backoff. A new outbound attempt is scheduled whenever an outbound session ends.

### Inbound Peers

Inbound connections are accepted by the listener and registered with the peer manager; the listener enforces the `max_inbound` cap at accept time. If an inbound peer disconnects, the peer manager removes it from the active set but does not attempt to reconnect. The remote side is responsible for re-initiating the connection if it is configured to do so.

### Local Hub Supervision

The local hub connection is supervised the same way as peers: if the hub drops, the bridge reconnects with the same exponential backoff schedule and resubscribes to the outbound mapping patterns. Reconnect attempts are unbounded; failures are logged with their attempt number. While the hub is unreachable, inbound remote actions are logged and dropped, and peer forwarding resumes when the hub returns.

### Duplicate Detection and Mutual-dial Tie-break

The peer manager tracks active sessions by remote `bridge_id`. If both sides of a bridge pair are configured to connect to each other, two connections race. Both bridges apply the same deterministic rule so exactly one survives without reconnect churn: the bridge with the lower `bridge_id` keeps its *outbound* session; the bridge with the higher `bridge_id` keeps the matching *inbound* session and suppresses outbound reconnects while that inbound session is active (dialling resumes if it ends). Same-kind duplicates are always rejected.

## Param Replay

When a new peer connects, the bridge may send the current state of local Params so that the remote hub can synchronise without waiting for live updates. This behaviour is controlled by the `replay_params` peer setting and is enabled by default for inbound peers.

### Cache Population

The param cache is populated from two sources:

1. **Hub subscription replay**: when the bridge subscribes to an outbound pattern, the hub sends the current Param value for matching addresses. These actions are written into the cache.
2. **Live updates**: every subsequent `action` with `signal_type = param` updates the cache for the mapped local address.

When an `unset_param` action is received, the corresponding cache entry is removed.

### Replay on Handshake

After the peer handshake completes, the peer manager sends each cached param through a dedicated replay channel to the new peer. The param is mapped through the outbound mapping rules before being sent as a `bridge_action`. This ensures the receiving bridge sees the param in the remote namespace. The session writer drains the replay fully before sending any live traffic, so the peer sees a consistent state snapshot ahead of updates.

## Graceful Shutdown

A shared `CancellationToken` and `TaskTracker` (from `tokio-util`) propagate through the bridge. On shutdown (`Ctrl+C` or the embedded shutdown signal) `run_bridge` cancels the token and calls `tracker.close()` then `tracker.wait().await`, which blocks until every spawned task has exited.

Cancellation reaches:

* the QUIC listener (stops accepting, calls `endpoint.close()`, and drops the endpoint)
* the inbound-peer register loop
* the peer manager event loop (and any pending outbound reconnect backoff)
* the local-hub forwarder
* the inbound forwarder
* every active peer session, via a child token scoped to that session

Each peer session runs its writer and reader sub-tasks in a `JoinSet`. When the first finishes (peer disconnect or shutdown) the session cancels its child token and joins the rest, so the tracked session task does not return while a sub-task lingers.

### UDP port release

Dropping a quinn `Endpoint` does not synchronously close the UDP socket; an internal driver task releases it shortly afterwards. So `shutdown()` returning means the endpoint has been dropped and the driver is draining, but the OS may take a few more milliseconds to free the port. Code that needs to rebind the same port immediately (e.g. restarting a bridge in a test) should retry the bind briefly. The reconnection integration test demonstrates this.

## Operational Notes

### Running the Bridge

```bash
cargo run -p ensemble-bridge-remote -- bridge-remote.toml
```

The bridge runs until it receives a shutdown signal (`Ctrl+C` or an explicit shutdown signal when embedded).

### Logging

The current implementation logs to stderr using `eprintln!`. All major lifecycle events (listen port, peer connect/disconnect, handshake completion, loop drops, shutdown) are reported. Structured logging is a future enhancement.

### Security Considerations

* The bridge uses self-signed TLS certificates and does not validate the peer certificate. This is suitable for trusted local networks only.
* An optional shared-secret `auth_token` authenticates peers at the handshake (constant-time comparison); mismatched peers are rejected before any session state is created.
* `max_inbound` caps simultaneously open inbound connections.
* Production deployments should provide real certificates and enable certificate validation. This is planned for a future release.

## Future Enhancements

* **Observability**: advertise the bridge via the manifest system with tags `["bridge", "remote"]`, expose bridge ID and peer count, and emit metrics.
* **Clock synchronisation**: align timestamps across hubs and rebase bridged actions into the local clock domain. For v0.1, bridged actions are forwarded as immediate (timestamp `0.0`).
* **User-provided TLS certificates**: support certificate paths in configuration and enable peer verification.

## Open Questions

1. Should the bridge support address mapping at the remote side only, or is bidirectional configuration required in all deployments?
2. Should the bridge advertise its peers and connection status through the local hub manifest for UI visibility?
3. Should the default `listen_port` be fixed or always require explicit configuration?
