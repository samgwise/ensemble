# Chord Connection & Lifecycle Specification (Draft v0.1)

## Status

Draft v0.1

## Purpose

Defines:

* voice identity
* connection establishment
* manifest updates
* subscription management
* disconnect behaviour

***

# Design Principles

## Minimal Connection

A voice should be able to connect before it knows:

* its routes
* its subscriptions
* its capabilities

Connection establishes identity only.

All other state may be supplied later.

***

## Runtime Mutation

The following may change without reconnecting:

```text
Manifest
Subscriptions
Capabilities
Name
```

***

# Voice Identity

Each connected voice contains three identifiers.

***

## Voice ID

Assigned by the hub.

Example:

```text
42
```

Properties:

* unique within hub lifetime
* immutable
* authoritative
* not user-facing

***

## Name

Provided by the client.

Example:

```text
Step Sequencer
```

Properties:

* may change
* not unique
* descriptive

***

## UI Name

Display-oriented name.

Examples:

```text
Step Sequencer #2

Bassline

Drums
```

May be assigned by:

* the client
* the user
* hub tooling

Used only for presentation.

Never participates in routing.

***

# Connection Flow

## Connect

Establish transport connection.

Example:

```text
TCP
Unix Socket
Named Pipe
```

Transport details are independent from protocol semantics.

***

## Hello

Client sends:

```rust
Hello {
    protocol_version: u32,
    name: String,
}
```

Example:

```rust
Hello {
    protocol_version: 1,
    name: "MIDI Bridge",
}
```

***

## Welcome

Hub replies:

```rust
Welcome {
    voice_id: VoiceId,
}
```

Example:

```rust
Welcome {
    voice_id: 42,
}
```

The voice is now active.

***

# Subscription Management

Subscriptions are independent from manifests.

A voice may subscribe or unsubscribe at any time.

***

## Subscribe

```rust
Subscribe {
    pattern: String
}
```

Example:

```text
/midi/**
```

***

## Unsubscribe

```rust
Unsubscribe {
    pattern: String
}
```

***

# Subscription Behaviour

When a new subscription is registered:

```text
Register Pattern
→ Compute Param Snapshot
→ Deliver Snapshot
→ Deliver Live Updates
```

Snapshot delivery must complete before live traffic begins.

***

# Name Updates

A voice may update its advertised name.

Example:

```text
Untitled Project
```

becoming:

```text
My Performance Patch
```

without reconnecting.

The hub should update displays accordingly.

***

# Disconnection

## Graceful

Voice sends:

```rust
Disconnect
```

Hub:

* removes subscriptions
* removes manifest
* removes voice state

***

## Ungraceful

If the underlying transport closes:

```text
Socket Closed
Connection Lost
```

the same cleanup occurs.

***

# Routing Independence

Routing must remain independent from voice identity.

Addresses describe:

```text
Capabilities
State
Actions
```

not processes.

Example:

```text
/transport/bpm
```

rather than:

```text
/voice/42/transport/bpm
```

Voice IDs must not be embedded into Chord addressing semantics.

***