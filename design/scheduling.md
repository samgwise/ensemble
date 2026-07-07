# Chord Timing, Scheduling & Ordering Specification (Draft v0.1)

## Status

Draft v0.1

## Purpose

This specification defines the temporal behaviour of Chord, including:

* the hub clock
* timestamp representation
* clock synchronisation
* action scheduling
* ordering guarantees
* Param timing semantics

Timing is a core feature of Chord and one of its primary differentiators from traditional message transport systems.

***

# Design Principles

## Shared Time

All connected voices participate in a shared timeline maintained by the hub.

Applications should reason about:

```text
Hub Time
```

rather than local system clocks.

***

## Local First

Chord is optimised for:

```text
One User
One Machine
One Hub
Many Tools
```

The timing model assumes low-latency local coordination and does not attempt to provide internet-scale timing guarantees.

***

## Schedule Everything

Every action exists at a point on the shared timeline.

There is no distinction between:

```text
Immediate messages
```

and

```text
Scheduled messages
```

Immediate messages are simply messages whose timestamp is at or before the current hub time.

***

# Hub Time

## Authoritative Clock

The hub maintains the authoritative Chord clock.

Properties:

```text
Monotonic
Hub-relative
Independent of wall clock
```

The clock:

* starts at `0.0` when the hub launches
* never moves backwards
* continuously increases until shutdown

***

## Hub Restart

Restarting the hub creates a new timeline.

Example:

```text
Hub Start
Clock = 0.0

...runtime...

Hub Stop

Hub Start
Clock = 0.0
```

Any pending scheduled actions are lost unless future persistence mechanisms are introduced.

***

# Timestamp Representation

All timestamps are represented as:

```rust
f64
```

Units:

```text
Seconds
```

Reference:

```text
Time since hub startup
```

Examples:

```text
0.0
1.5
10.25
3600.0
```

***

# Why Float64

Chord standardises on:

```text
IEEE754 Float64
```

because it:

* is widely available across languages
* avoids tick-rate assumptions
* provides sufficient precision for long-running sessions
* matches common scientific and creative coding tools

***

# Clock Synchronisation

## Client Time Estimation

Voices maintain an estimate of current hub time.

Synchronization is performed using:

```text
Round-trip latency measurement
Minimum RTT filtering
Clock offset estimation
```

inspired by:

```text
NTP
O2
```

***

## Client API

Client libraries should expose:

```rust
hub.now()
```

or equivalent.

Clients should not be required to implement clock estimation directly.

***

# Action Scheduling

Every action contains:

```rust
timestamp: f64
```

The timestamp specifies:

> The earliest hub time at which the action may be dispatched.

***

# Immediate Actions

An action is considered immediate when:

```text
timestamp <= current_hub_time
```

Examples:

```rust
timestamp = hub.now()
```

```rust
timestamp = hub.now() - 1.0
```

Both are dispatched immediately.

***

# Past Timestamps

Actions scheduled in the past are never rejected solely because they are late.

Example:

```rust
timestamp = current_time - 5.0
```

Behaviour:

```text
Immediate dispatch.
```

Reasons include:

* clock estimation error
* network latency
* client startup delays
* transient scheduling jitter

***

# Future Timestamps

Actions with timestamps in the future are retained by the hub until their scheduled time.

Example:

Current hub time:

```text
100.0
```

Action:

```text
timestamp = 105.0
```

Result:

```text
Stored by hub
Dispatched at hub time >= 105.0
```

***

# Dispatch Guarantees

Chord guarantees:

```text
Not Before
```

delivery.

An action scheduled for:

```text
100.0
```

will never be dispatched before:

```text
100.0
```

It may be dispatched later due to:

* operating system scheduling
* runtime scheduling
* execution overhead
* transport latency

***

# Scheduling Resolution

Chord intentionally does not define a minimum scheduling accuracy.

The protocol does not specify:

```text
1ms
100µs
10µs
```

accuracy requirements.

Implementations should dispatch as accurately as practical for their execution environment.

***

# Ordering Semantics

## Per-Sender FIFO

For actions from the same voice:

```text
A
B
C
```

submitted in that order with identical timestamps:

```text
10.0
10.0
10.0
```

delivery order must remain:

```text
A
B
C
```

***

## Immediate Message Ordering

For immediate actions:

```text
A
B
C
```

delivery order must remain:

```text
A
B
C
```

for a single sender.

***

## Cross-Sender Ordering

Chord does not define a global ordering between actions from different voices.

Example:

```text
Voice A
Action A @ 10.0

Voice B
Action B @ 10.0
```

Both actions must satisfy:

```text
dispatch_time >= 10.0
```

Their relative ordering is unspecified.

Applications must not depend upon cross-sender ordering.

***

# Param Timing Semantics

## Scheduled Params

A Param may be scheduled into the future.

Example:

Current state:

```text
/transport/bpm = 120
```

Scheduled:

```text
/transport/bpm = 140
timestamp = now + 5
```

***

## Activation-Time Retention

Future Param values do not become current until activated.

At:

```text
now
```

current state remains:

```text
120
```

At:

```text
now + 5
```

current state becomes:

```text
140
```

***

## Snapshot Consistency

Param snapshots must reflect currently active state only.

Future scheduled Param updates are not included.

Example:

Current state:

```text
/transport/bpm = 120
```

Future state:

```text
/transport/bpm = 140
at t+5
```

New subscriber receives:

```text
120
```

not:

```text
140
```

until activation occurs.

***

# Stream Timing Semantics

Streams may be scheduled using the same timestamp mechanism as Events and Params.

Examples:

```text
Automation
Sensor playback
Animation control
```

Stream scheduling follows normal dispatch rules.

***

## Stream Congestion Behaviour

Streams:

```text
Not retained
Best effort
May be dropped
```

Congestion-induced dropping may occur inside hub stream queues.

The protocol does not guarantee delivery of every Stream message.

***

# Observability Metadata

Implementations should internally track:

```text
Received Time
Scheduled Time
Dispatch Time
Sender Voice
```

where practical.

This information is intended for:

* diagnostics
* profiling
* scheduling analysis
* future hub tooling

It does not affect protocol behaviour.

***

# Timestamp Requirement

Every Action MUST contain a timestamp.

There is no optional timestamp field.

Example:

```rust
Action {
    address,
    signal_type,
    timestamp,
    payload,
}
```

This provides a uniform temporal model:

> Every action exists at a specific point on the Chord timeline.

Even when an action is dispatched immediately, its timestamp remains available for:

* inspection
* diagnostics
* tracing
* scheduling analysis

The hub may use receipt time internally when handling overdue actions, but the original timestamp remains part of the action's history and identity.

***

# Summary

Chord timing is based on a shared hub-relative monotonic clock.

Key guarantees:

* Every action has a timestamp.
* Actions are never dispatched before their timestamp.
* Past timestamps dispatch immediately.
* Per-sender FIFO ordering is preserved.
* Cross-sender ordering is unspecified.
* Param snapshots contain only active state.
* Future Param updates activate at scheduled time.
* Streams may be dropped under congestion.
* The hub clock is authoritative and resets on restart.

This model establishes a simple, language-neutral temporal framework suitable for creative tools while remaining practical to implement across a wide range of environments.
