# Ensemble Client Implementor Guide (Draft v0.1)

## Status

Draft v0.1

## Audience and Scope

This guide is for anyone implementing an **Ensemble client** — also called a **voice** — in any language. A voice is any process that connects to an Ensemble hub: a sequencer, a GUI, a Python script, a lighting controller, a MIDI/OSC bridge, another hub, and so on.

Ensemble is **local-first**: one user, one machine, one hub, many tools. Clients and the hub typically share a trusted local environment, so there is no authentication or authorisation in v0.1. This guide reflects that assumption.

The per-topic design documents are the authoritative specification. This guide is a **walkthrough**: it ties the topics together in the order a client implementor needs them, summarises the client-relevant parts, and links to the authoritative spec for full detail. Where this guide and a per-topic spec disagree, **the spec wins**.

References throughout use `Ref: <spec>.md`. You will want these open alongside this guide:

* `design-overview.md` — overall vision and principles
* `protocol-spec.md` — wire protocol, envelope, message types, framing
* `lifecycle.md` — voice identity, connection, subscriptions, disconnect
* `routing.md` — address model, pattern matching, captures
* `value-model-specification.md` — the Value model
* `scheduling.md` — hub clock, timestamps, dispatch and ordering
* `manifest.md` — manifest structure and updates
* `local-discovery.md` — finding and connecting to a hub

Ref: `design-overview.md`

## The Mental Model

A client speaks a small MessagePack-based protocol to a hub over a byte stream. The flow is:

```text
open transport
  → send hello
  → receive welcome (assigned voice_id)
  → (optional) set manifest
  → subscribe to patterns
  → receive param snapshots for matching patterns
  → live: send and receive actions, run clock sync
  → disconnect (graceful) or transport close (ungraceful)
```

Everything else is a variation on that loop. The sections below walk through each step.

## Transport and Framing

### Transport

The default transport is **TCP**. The protocol is transport-independent — it can also run over Unix domain sockets, Windows named pipes, or WebSockets — but v0.1 clients connect over TCP. Transport choice does not affect protocol semantics.

Ref: `protocol-spec.md` (Transport Independence)

### Framing

Over the byte stream, every message is a **length-prefixed MessagePack frame**:

```text
[4 bytes: little-endian u32 length][MessagePack payload]
```

* The length is the size in bytes of the MessagePack payload only.
* Each frame contains exactly one message.
* Clients MUST enforce an upper bound on frame size to avoid memory exhaustion from malformed data. The reference codec uses a **1 MiB (1,048,576 bytes)** maximum; a length header exceeding that is rejected as an error rather than allocated.

On read, a clean end-of-stream while reading the 4-byte length header indicates a graceful transport close (treat as connection closed, not an error). A truncated length header (fewer than 4 bytes) is also treated as a clean close.

Ref: `protocol-spec.md` (Framing); the reference codec lives in `ensemble-core/src/codec.rs`.

### Message Envelope

Every message uses the same envelope, serialised as a two-field MessagePack map:

```text
WireMessage {
  type:    String   (the message type identifier, e.g. "action")
  payload: Value    (a Value::Map whose structure depends on type)
}
```

On the wire the field is named `type` (not `msg_type`). Message types are UTF-8 strings; numeric identifiers are deliberately avoided for readability and inspection.

Ref: `protocol-spec.md` (Message Envelope)

## Value Model (Primer)

The `payload` field of every message is a `Value`. Ensemble defines a small, language-neutral value model. A client must be able to encode and decode all of these:

```text
Null        explicit null (a normal value, may appear anywhere)
Bool        true | false
Integer     signed 64-bit  (i64)
Float       IEEE754 double  (f64) — includes NaN, +Inf, -Inf
String      UTF-8 text
Binary      opaque byte string
Tuple       ordered, positional, fixed-semantic-structure collection
List        ordered, variable-length collection
Map         unordered, string-keyed associative collection
TypedBinary { tag: String, data: Binary }
```

Things implementors most often get wrong:

* **Integer is i64.** Voice IDs are `u64` conceptually but carried as Integer; values up to `i64::MAX` round-trip safely. For larger values, use Binary/TypedBinary.
* **Tuple and List are distinct.** A Tuple `(channel, note, velocity)` means "position defines meaning"; a List `[note1, note2, note3]` means "a collection". Some serializers encode both the same way — preserve the distinction in your API where you can, and treat received arrays based on documented semantics where you cannot.
* **Maps are string-keyed and unordered.** Non-string keys are invalid. Never rely on insertion, iteration, or serialisation order.
* **Null is not "delete".** A Param set to `Null` means "current value is null". Removing retained Param state is a separate operation (`unset_param`).
* **TypedBinary is the extensibility hatch.** Use `{ tag, data }` for values that don't fit the core model (e.g. `ensemble/f32`, `org.example.matrix`). The `ensemble/*` tag namespace is reserved. The hub treats TypedBinary as opaque: it routes and stores it but never interprets it.
* **No protocol-level schemas.** Validation is advisory and expressed through manifest `payload_hint`s, not enforced by the protocol.

Ref: `value-model-specification.md`

## Connection Lifecycle

### Connect

1. Open a transport connection to the hub (see Discovery below for how to find it).
2. Send `hello`:

```text
hello {
  protocol_version: u32   (use 1 for v0.1)
  name: String
}
```

```text
→ { "type": "hello", "payload": { "protocol_version": 1, "name": "Step Sequencer" } }
```

3. Receive `welcome`:

```text
welcome {
  voice_id: u64
}
```

```text
← { "type": "welcome", "payload": { "voice_id": 42 } }
```

After `welcome`, the voice is connected and active. Until `welcome` arrives, no other messages should be sent.

### Voice Identity

Each connected voice has three identifiers:

* **Voice ID** — assigned by the hub, unique within the hub's lifetime, immutable, authoritative. Not user-facing. Do not embed it in addresses (e.g. do not use `/voice/42/...`); routing must be identity-independent.
* **Name** — provided by the client in `hello`, descriptive, not unique, may change at runtime via `update_name`.
* **UI Name** — display-oriented, assigned by the client/user/tooling, never participates in routing.

If the hub cannot support the requested `protocol_version`, it replies with an `error` (code `unsupported_protocol_version`) and closes the connection.

Ref: `lifecycle.md` (Voice Identity, Connection Flow); `protocol-spec.md` (Lifecycle Messages)

### Disconnect

Graceful: send `disconnect` with an empty payload:

```text
disconnect {}
→ { "type": "disconnect", "payload": {} }
```

The hub then removes the voice's subscriptions, manifest, and voice state.

Ungraceful: if the transport closes (socket closed, connection lost), the same cleanup occurs. A client SHOULD treat a clean EOF on the length-header read as a normal close.

Ref: `lifecycle.md` (Disconnection)

## Subscriptions and Routing

A voice subscribes to routing patterns to receive actions addressed under matching paths. Subscriptions are independent of manifests and may change at any time without reconnecting.

### Subscribe / Unsubscribe

```text
subscribe   { pattern: String }
unsubscribe { pattern: String }
```

```text
→ { "type": "subscribe", "payload": { "pattern": "/midi/**" } }
```

### Patterns

Pattern matching operates on **path segments** (split on `/`), not characters. Addresses MUST begin with `/` and contain one or more segments.

| Mechanism | Syntax | Matches |
| --- | --- | --- |
| Exact | `/track/17/volume` | that address only |
| Single segment | `*` | exactly one segment |
| Recursive | `**` | zero or more remaining segments |
| Named capture | `{name}` | one segment, exposed by name as a string |

Rules a client implementor must enforce when building patterns to send:

* `**` MUST appear as the **final** segment. `/**/volume` and `/track/**/volume` are invalid.
* Capture names are UTF-8, non-empty, and MUST NOT contain `/`, `{`, `}`, or `*`.
* Captures are always strings — no `{id:int}` type syntax. Clients convert as needed.
* The hub SHOULD reject invalid patterns with an `error` (code `invalid_pattern`).

### Replay-on-Subscribe (important)

When a subscription is registered, the hub:

```text
register pattern
  → compute the snapshot of matching Params (currently active state only)
  → deliver that snapshot
  → begin live delivery
```

**The snapshot must finish before live traffic begins.** A client that joins mid-session therefore receives the current value of every matching Param before it sees any live updates. Future-scheduled Param values are NOT part of the snapshot.

### Matching Semantics

There is no route precedence. If multiple subscriptions match an address, **all matching subscribers receive the action** — the hub does not pick a "best" match.

Ref: `routing.md` (Address Rules, Pattern Types, Matching Priority); `lifecycle.md` (Subscription Behaviour)

## Sending Actions

The `action` message is the primary data-plane message.

```text
action {
  source:      u64   (OPTIONAL when sending — the hub assigns it)
  address:     String
  signal_type: "event" | "param" | "stream"
  timestamp:   f64   (seconds, hub-relative)
  payload:     Value
}
```

When **sending** an action, omit `source`. The hub assigns the `source` to the sending voice's ID when routing. Including a `source` on send has no useful meaning for a client.

```text
→ { "type": "action", "payload": {
      "address": "/transport/bpm",
      "signal_type": "param",
      "timestamp": 10.5,
      "payload": 120.0
   } }
```

### Signal Types

The `signal_type` determines how the hub treats the action:

* **`event`** — fire-and-forget. No state is retained. Delivered to current matching subscribers; late joiners do not receive past events.
* **`param`** — stateful key-value. The hub remembers the last value per address and **replays it to late-joining subscribers**. Future-scheduled Params activate at their scheduled time; snapshots contain only currently-active state. A Param whose value is `Null` still occupies state — it is not deleted.
* **`stream`** — high-rate best-effort data. Not retained. **May be dropped under congestion** rather than queued. Suitable for sensor streams, automation, animation control.

### Removing Param State

To delete retained Param state, send `unset_param` — do NOT send a `Param` with a `Null` payload:

```text
unset_param { address: String }
→ { "type": "unset_param", "payload": { "address": "/transport/bpm" } }
```

Ref: `protocol-spec.md` (Data Messages); `scheduling.md` (Param Timing Semantics); `value-model-specification.md` (Null, Param Semantics)

## Receiving Routed Actions

When the hub routes an action to a subscriber, the subscriber receives an `action` message with `source` **set by the hub** to the originating voice's ID:

```text
← { "type": "action", "payload": {
      "source": 3,
      "address": "/transport/bpm",
      "signal_type": "param",
      "timestamp": 10.5,
      "payload": 120.0
   } }
```

Use `source` to identify who originated the action. Do not rely on it for security (local-first, trusted environment).

### Ordering and Dispatch Guarantees

* **Not-before delivery.** An action is never dispatched before its `timestamp`.
* **Past timestamps dispatch immediately.** An action with `timestamp <= now` is dispatched straight away; late actions are never rejected solely for being late.
* **Future timestamps are retained** by the hub until their scheduled time, then dispatched.
* **Per-sender FIFO.** Actions from the same voice with equal timestamps are delivered in send order.
* **Cross-sender ordering is unspecified.** Two actions from different voices at the same timestamp may arrive in any order. Do not depend on a global order.
* **Streams may be dropped** under congestion. The protocol does not guarantee delivery of every `stream` message.

Ref: `scheduling.md` (Dispatch Guarantees, Ordering Semantics, Stream Timing Semantics)

## Timing and Clock Synchronisation

### The Hub Clock

The hub maintains the **authoritative Ensemble clock**: monotonic, hub-relative, independent of wall clock. It starts at `0.0` when the hub launches, never moves backwards, and increases until shutdown. **Restarting the hub creates a new timeline** — any pending scheduled actions are lost.

### Every Action Has a Timestamp

There is no optional timestamp field. Every action MUST carry an `f64` seconds timestamp representing "the earliest hub time at which the action may be dispatched". Even immediately-dispatched actions keep their timestamp for inspection and diagnostics.

For an immediate action, use your current estimate of hub time:

```text
timestamp = hub.now()
```

### Clock Synchronisation

Voices estimate hub time using `clock_ping` / `clock_pong`:

```text
clock_ping { sequence: u64 }       (client → hub)
clock_pong { sequence: u64, hub_time: f64 }   (hub → client)
```

The algorithm is implementation-defined (NTP/O2-inspired round-trip measurement with minimum-RTT filtering and clock-offset estimation). A client library SHOULD expose a `hub.now()` (or equivalent) API so application code does not implement clock estimation directly. A reasonable strategy:

```text
on connect:
    start a background clock-sync task
    send clock_ping(seq=0)
    record send time

on clock_pong(seq, hub_time):
    rtt = now_local - send_time[seq]
    update offset estimate using min-RTT filtering

    pace pings: frequent until synced (e.g. every 200ms), then slow (e.g. every 5s)
```

The `hub_time` in `clock_pong` is the hub's time when it sent the pong. The reference client approximates hub-receive-time ≈ hub-send-time, which is reasonable for localhost and low-latency networks.

Ref: `scheduling.md` (Hub Time, Clock Synchronisation, Timestamp Requirement)

## Manifests (Optional but Recommended)

A manifest is runtime-discoverable metadata about a voice's capabilities. It is **advisory only** — it does not affect routing, does not enforce types, and does not create subscriptions. It exists for observability, debugging, discovery, documentation, UI generation, and compatibility suggestions.

### Structure

```text
VoiceManifest {
  name:        String
  description: Option<String>
  version:     Option<String>
  tags:        Vec<String>
  provides:    Vec<String>      (capabilities offered)
  expects:     Vec<String>      (capabilities likely required)
  routes:      Vec<RouteInfo>
}

RouteInfo {
  address:      String           (an Ensemble routing pattern)
  signal:       "event" | "param" | "stream"
  payload_hint: Option<String>   (advisory, e.g. "float", "(ch:int,note:int,vel:int)")
  description:  Option<String>
  example:      Option<Value>
}
```

### Publishing and Updating

```text
set_manifest    { manifest: Value }   (replace the whole manifest)
patch_manifest  { patch: Value }      (partial update)
update_name     { name: String }      (rename without touching the manifest)
```

Use `set_manifest` for the initial publication and for large changes; use `patch_manifest` for small updates. The protocol does not distinguish "initial" from "updated" — both are ordinary runtime updates.

**Patch semantics** (the patch is a map of only the fields to update):

* Scalar fields (`name`, `description`, `version`): present → replace; `null` → clear to none; absent → unchanged.
* List fields (`tags`, `provides`, `expects`, `routes`): present → **replace the whole list** (not merge); absent → unchanged. To add an item, send the complete new list.
* If no manifest has been set yet, `patch_manifest` creates a default (empty) manifest and applies the patch to it.

Ref: `manifest.md` (Manifest Structure, Manifest Updates, Patch Semantics)

## Errors and Observability

### Error Messages

The hub sends `error` (hub → client) for protocol-level problems:

```text
error {
  code:    String
  message: String
}
← { "type": "error", "payload": { "code": "invalid_pattern", "message": "Recursive wildcard must be final segment." } }
```

Known error codes (not exhaustive):

```text
unsupported_protocol_version
invalid_pattern
malformed_manifest
invalid_message
internal_error
reserved_namespace
```

A client SHOULD surface error messages to the implementor/operator. They are not fatal unless the hub closes the transport afterwards (as it does for `unsupported_protocol_version`).

### Hub Observability

The hub may emit ordinary Ensemble actions on a reserved namespace:

```text
/hub/voice/joined
/hub/voice/left
/hub/manifest/updated
/hub/warning
/hub/error
```

These are normal data-plane actions (subscribe to `/hub/**` to receive them). They describe events that occurred; they do not change protocol state. The `/hub/` namespace is reserved — applications should not publish to it.

Protocol operations (hello, subscribe, set_manifest, etc.) are **not** represented as actions; they are dedicated control-plane messages.

Ref: `protocol-spec.md` (Error Messages, Observability Events, Reserved Namespace)

## Discovery and Connecting

Clients find the hub in this order:

1. **Explicit configuration** — user-specified port via CLI argument, environment variable, or config file.
2. **Platform-appropriate discovery** — the port file on desktop platforms (see below).
3. **Default port fallback** — `7331`.

If all attempts fail, the client should report an error and terminate.

### Default Port

```text
7331
```

### Port File (Desktop)

The hub writes a port file containing a single line with the bound port number. Locations:

```text
Linux:    $XDG_RUNTIME_DIR/ensemble/hub.port   (fallback: /tmp/ensemble-hub-{uid}.port)
macOS:    $TMPDIR/ensemble-hub.port
Windows:  %LOCALAPPDATA%\Ensemble\hub.port
```

Read the file if present, verify the port is reachable before connecting, and fall back to the default port if the file is missing, unreadable, or stale (a crashed hub can leave a stale file — if the connect fails, delete the stale file and fall back).

### Override Priority

When multiple override mechanisms are present:

```text
1. Command-line argument        (highest)
2. Environment variable          (ENSEMBLE_HUB_PORT)
3. Configuration file
4. Discovery mechanism (port file)
5. Default port 7331             (lowest)
```

Ref: `local-discovery.md` (Default Port, Port File Discovery, Override Mechanisms)

## Message Reference

Compact index. `protocol-spec.md` is authoritative; this is a convenience.

| Message | Direction | Payload |
| --- | --- | --- |
| `hello` | client → hub | `{ protocol_version: u32, name: String }` |
| `welcome` | hub → client | `{ voice_id: u64 }` |
| `disconnect` | client → hub | `{}` |
| `subscribe` | client → hub | `{ pattern: String }` |
| `unsubscribe` | client → hub | `{ pattern: String }` |
| `action` | client ↔ hub | `{ source?: u64, address: String, signal_type: "event"\|"param"\|"stream", timestamp: f64, payload: Value }` |
| `unset_param` | client ↔ hub | `{ address: String }` |
| `clock_ping` | client → hub | `{ sequence: u64 }` |
| `clock_pong` | hub → client | `{ sequence: u64, hub_time: f64 }` |
| `set_manifest` | client → hub | `{ manifest: Value }` |
| `patch_manifest` | client → hub | `{ patch: Value }` |
| `update_name` | client → hub | `{ name: String }` |
| `error` | hub → client | `{ code: String, message: String }` |

Notes:

* `source` in `action` is optional and omitted by clients when sending (the hub assigns it). It is present on routed actions received from the hub.
* `unset_param` is shown as bidirectional because the hub may forward it (e.g. so bridges can keep a param cache in sync); client implementors typically only send it.
* On the wire, integers are MessagePack integers (carried as i64), floats are f64, and all maps are string-keyed.

Ref: `protocol-spec.md` (Control Plane Summary, Data Plane Summary)

## Conformance Checklist

A client implementation MUST:

* Frame every message as `[4-byte LE length][MessagePack payload]` and enforce a frame-size upper bound (the reference uses 1 MiB).
* Encode every message as a two-field map `{ "type": String, "payload": Value }`.
* Send `hello` with `protocol_version = 1` as the first message and wait for `welcome` before doing anything else.
* Send a `timestamp` on every `action` (no optional timestamp).
* Omit `source` when sending an `action`; read `source` on received routed actions.
* Use `unset_param` to remove retained Param state (not a `Null` payload).
* Treat a clean EOF on the length-header read as a connection closed, not an error.

A client implementation SHOULD:

* Reject invalid subscription patterns locally before sending (notably the `**`-must-be-final rule).
* Expect and handle the param snapshot on subscribe before live traffic.
* Run background clock synchronisation and expose a `hub.now()`-style API.
* Surface `error` messages to the operator.
* Support the discovery order (explicit config → port file → default 7331) and the override priority (CLI > env `ENSEMBLE_HUB_PORT` > config file > discovery > default).
* Send `disconnect` for a graceful close.
* Publish a manifest so the running system is self-documenting.

Ref: `protocol-spec.md`, `routing.md` (Conformance Requirements), `scheduling.md` (Timestamp Requirement), `local-discovery.md` (Discovery Strategy)

## Worked Walkthrough (Language-Neutral Pseudocode)

This ties the sections together. It is illustrative, not prescriptive about APIs.

```text
# --- Connect and handshake ---
transport = open_tcp(discover_port())          # see Discovery section
write_frame(transport, hello(name="My Tool"))  # protocol_version = 1
msg = read_frame(transport)
assert msg.type == "welcome"
voice_id = msg.payload["voice_id"]

# --- Background clock sync (run for the life of the connection) ---
spawn task clock_loop:
    seq = 0
    loop:
        send_time = local_now()
        write_frame(transport, clock_ping(sequence=seq))
        remember pending[seq] = send_time
        seq += 1
        sleep(if synced: 5s else 200ms)

    # (pong handling lives in the reader loop below)

# --- Optional: tell the hub what we are ---
write_frame(transport, set_manifest(manifest={
    "name": "My Tool",
    "tags": ["tool"],
    "routes": [
        {"address": "/my-tool/ping", "signal": "event", "payload_hint": "null"}
    ]
}))

# --- Subscribe: snapshot arrives before live traffic ---
write_frame(transport, subscribe(pattern="/other/**"))

# --- Main loop: read everything the hub sends us ---
spawn task reader:
    loop:
        msg = read_frame(transport)
        if closed: break

        match msg.type:
            "action":
                # A routed action (events, params, streams) —
                # source is the originating voice, set by the hub.
                handle_action(msg.payload)

            "unset_param":
                # A param was removed; drop any local cached state for it.
                forget_param(msg.payload["address"])

            "clock_pong":
                # Clock sync reply — feed the estimate.
                seq   = msg.payload["sequence"]
                htime = msg.payload["hub_time"]
                if seq in pending:
                    rtt = local_now() - pending[seq]
                    update_clock_estimate(rtt, htime)
                    forget pending[seq]

            "error":
                log("hub error", msg.payload["code"], msg.payload["message"])

            otherwise:
                ignore   # future message types

# --- Sending an immediate action ---
write_frame(transport, action(
    address="/my-tool/ping",
    signal_type="event",
    timestamp=hub_now(),      # our estimate of hub time
    payload=Null,
))

# --- Scheduling a future action ---
write_frame(transport, action(
    address="/my-tool/delayed",
    signal_type="event",
    timestamp=hub_now() + 1.0,   # one second in the future
    payload=String("g'day"),
))

# --- Removing retained param state (not a Null payload) ---
write_frame(transport, unset_param(address="/my-tool/setting"))

# --- Graceful shutdown ---
write_frame(transport, disconnect())
close(transport)
```

Two subtleties this walkthrough illustrates:

* **Snapshot before live.** After `subscribe`, the hub delivers the matching Param snapshot *before* any live actions. Do not assume the first thing you read after subscribing is live traffic.
* **Timestamps are hub-relative.** Use your synchronised estimate of hub time (`hub_now()`) for both immediate and scheduled actions; never local wall-clock time.

Ref: `lifecycle.md`, `scheduling.md` (Snapshot Consistency, Timestamp Requirement)

## Summary

To implement an Ensemble client: open a TCP connection, perform the `hello`/`welcome` handshake, optionally publish a manifest, subscribe to the patterns you care about (and accept the param snapshot that precedes live traffic), then send and receive `action` messages with hub-relative timestamps while running background `clock_ping`/`clock_pong` synchronisation. Keep framing tight, reject oversized frames, treat a clean EOF as a close, and send `disconnect` when you're done.

The per-topic specs remain the source of truth for any detail this guide compresses. When in doubt, follow the spec.
