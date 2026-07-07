# Chord Design Vision (Draft)

## Status

Draft v0.1

This document captures the emerging architectural vision for Chord and serves as a reference for future protocol, implementation, and ecosystem decisions.

***

# Overview

Chord is a **timed message bus for creative systems**.

It provides a shared clock, timestamped message delivery, stateful parameter distribution, subscription-based routing, and protocol bridging. Chord is designed primarily for local creative workflows, where many small tools cooperate through a common runtime.

Examples include:

* generative music systems
* sequencing tools
* live coding environments
* installation control systems
* sensor processing applications
* visualisation tools
* lighting and media controllers
* research prototypes

A key goal is allowing tools to be written in different languages and environments while remaining easy to connect and reason about.

```text
Python Script
      │
Rust Tool
      │
Max Patch
      │
  Chord Hub
      │
MIDI Bridge
      │
Serial Bridge
      │
OSC Bridge
```

***

# Core Philosophy

Chord is **not a music protocol**.

Chord is **not an RPC framework**.

Chord is a timed coordination layer.

Musical applications are a primary target, but the protocol itself remains domain-neutral.

A Chord application should be able to exchange messages about:

```text
notes
lighting
sensor data
visual state
AI agents
game systems
robotics
```

using the same transport.

***

# Design Principles

## Local-First

Chord optimises for:

```text
One User
One Machine
One Hub
Many Tools
```

rather than:

```text
Many Users
Many Machines
One Central Server
```

This provides:

* low latency
* predictable timing
* simple discovery
* straightforward debugging
* no authentication requirements
* a single authoritative clock

***

## Small Tools, Strong Coordination

Individual tools should remain simple.

Responsibilities such as:

* clock synchronisation
* scheduling
* state replay
* protocol translation

should be handled outside individual applications.

***

## Language Neutrality

The protocol should remain practical to implement in:

* Rust
* Python
* JavaScript
* Max/MSP
* SuperCollider
* Go
* C/C++

No code generation should be required.

***

## Observability First

The hub is not only a router.

The hub is the primary debugging and inspection tool.

Users should be able to observe:

* connected voices
* subscriptions
* routed actions
* timing behaviour
* log events
* bridge activity

in one place.

***

# Architecture

## Hub

The hub provides:

* authoritative clock
* routing engine
* action scheduling
* Param storage
* diagnostics

The hub should remain unaware of application-specific semantics.

***

## Voices

A voice is any Chord client.

Examples:

* sequencer
* GUI
* Python script
* lighting controller
* synth tool

Voices:

* connect to a hub
* advertise subscriptions
* send actions
* receive actions

***

## Bridges

A bridge translates between Chord and an external system.

Examples:

```text
MIDI Bridge
OSC Bridge
Serial Bridge
MQTT Bridge
Hub Bridge
```

External networking should generally be implemented as a bridge rather than becoming a core responsibility of the protocol.

A future Chord-to-Chord bridge could connect multiple hubs.

***

# Protocol Layers

## Core Protocol

Responsible for:

* message transport
* payload encoding
* timestamps
* routing
* signal semantics

The protocol answers:

> How do messages move?

***

## Manifest Layer

Responsible for:

* capability advertisement
* payload descriptions
* documentation
* UI generation
* validation hints

The manifest answers:

> What do messages mean?

The manifest is optional.

***

# Action Model

```rust
Action {
    address: String,
    signal_type: SignalType,
    timestamp: f64,
    payload: Value,
}
```

***

## Signal Types

### Event

Fire-and-forget.

```text
Delivered once.
Not retained.
```

Example:

```text
/button/pressed
```

***

### Param

Stateful value.

```text
Latest value retained by hub.
Replayed to late subscribers.
```

Example:

```text
/transport/bpm
```

***

### Stream

Best-effort realtime delivery.

```text
May be dropped under congestion.
Not retained.
```

Example:

```text
/sensor/imu
```

***

# RPC Philosophy

RPC is not part of the core protocol.

Request/reply behaviour should initially be implemented through ordinary messages.

Example:

```text
/query/device-list
```

with

```text
/reply/123
```

Rather than introducing dedicated RPC semantics.

Future standardisation may emerge from experience.

***

# Time Model

The hub maintains a monotonic reference clock.

Properties:

* begins at 0 on hub startup
* never runs backwards
* independent of wall clock time

Clients synchronise using a clock correction mechanism similar to NTP/O2 style RTT sampling.

All timestamps represent:

```text
seconds
float64
hub-relative
```

***

# Routing Model

Chord uses hierarchical addresses inspired by OSC.

Examples:

```text
/transport/bpm
/track/7/volume
/midi/note/on
```

***

## Routing Library

Routing should exist as a standalone implementation.

Potential crate:

```text
chord-routing
```

Goals:

* deterministic behaviour
* shared conformance tests
* reusable from FFI
* independent evolution

***

## Pattern Matching

Chord should define its own pattern syntax rather than inheriting OSC behaviour.

Goals:

* deterministic
* language-neutral
* capture support

Example:

```text
/track/{id}/volume
```

matching:

```text
/track/7/volume
```

produces:

```json
{
  "id": "7"
}
```

Initially captures should be strings.

Type conversion remains the responsibility of clients.

***

## Conformance Testing

Routing behaviour should be defined through a shared test suite.

Example:

```yaml
pattern: "/track/{id}/volume"
path: "/track/7/volume"

captures:
  id: "7"
```

All implementations should pass the same test corpus.

***

# Value Model

Chord should provide a small, language-neutral value model.

Required types:

```rust
Null
Bool
Integer
Float
String
Binary
List
Map
```

Definitions:

```text
Integer = signed 64-bit
Float   = IEEE754 double
String  = UTF-8
Binary  = opaque bytes
```

Lists and Maps may be nested arbitrarily.

***

## Why Int64 and Float64

Chord intentionally avoids:

```text
i32
u32
f32
f64
...
```

as core protocol primitives.

A single integer and float type simplifies interoperability across languages.

Implementations with narrower native representations perform conversion at the boundary.

***

# Typed Binary Extension

Chord should support an optional opaque binary extension:

```rust
TypedBinary {
    tag: String,
    data: Vec<u8>,
}
```

Examples:

```text
f32
complex64
rational
opencv-mat
fft-frame
```

The protocol does not interpret these values.

They exist as an escape hatch for specialised applications.

Future hub tooling may learn to visualise specific tags.

***

# Transport Abstraction

Chord should not be tied to TCP.

Instead:

```text
Protocol
+
Encoding
+
Transport
```

remain separate concerns.

***

## Initial Transport

```text
TCP
Length-prefixed frames
MessagePack
```

***

## Possible Future Transports

```text
Unix Domain Sockets
Windows Named Pipes
WebSockets
QUIC
```

The protocol definition should not require transport-specific behaviour.

***

# Encoding

Initial encoding:

```text
MessagePack
```

Reasons:

* compact
* language neutral
* widely available
* no code generation
* natural support for nested structures

***

# Manifest System

The manifest is optional metadata attached to a voice.

Example:

```json
{
  "name": "MIDI Bridge",
  "provides": [
    {
      "address": "/midi/play",
      "signal": "event",
      "payload": "(channel:int,note:int,velocity:int,duration:float)"
    }
  ]
}
```

Payload descriptions are advisory.

They are not enforced by the protocol.

***

## Manifest Uses

* documentation
* auto-generated UIs
* debugging
* discovery
* validation
* testing

***

# Logging & Observability

The hub should function as a protocol inspector.

Potential features:

* live action monitor
* routing traces
* timing visualisation
* Param inspection
* bridge monitoring

Example trace:

```text
08:31:12.514
Sequencer
 -> /track/1/note

08:31:12.515
Hub
 -> Routed to MIDI Bridge

08:31:12.516
MIDI Bridge
 -> MIDI Note On
```

This capability is considered a major feature rather than a convenience.

***

# Non-Goals (Current)

Chord is not currently intended to provide:

* distributed consensus
* internet-scale operation
* schema-enforced typing
* mandatory RPC
* global service discovery
* peer-to-peer routing
* hard realtime guarantees

These may be explored through future extensions but are not core requirements.

***

# Summary

Chord is envisioned as a:

> Local-first, timed message bus for creative systems.

The protocol remains deliberately small:

* shared clock
* scheduled delivery
* Event / Param / Stream semantics
* hierarchical routing
* language-neutral values
* transport independence

Everything else—music conventions, bridge behaviour, validation, discovery, and tooling—can evolve as layers above that stable core.

***

# Future Hub Features

The manifest and lifecycle systems are intended to support future tooling such as:

* route browsers
* capability discovery
* connection suggestions
* protocol inspection
* message tracing
* routing diagnostics
* mapping utilities
* filter graphs

without requiring changes to core routing semantics.

***

With these specs in place, the next major specification should be **Timing, Scheduling & Ordering Semantics**, since timing is the most distinctive feature of Chord and will influence both hub implementation and client expectations.