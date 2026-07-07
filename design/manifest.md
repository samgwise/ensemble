# Chord Manifest Specification (Draft v0.1)

## Status

Draft v0.1

## Purpose

The Manifest system provides runtime-discoverable metadata about a voice's capabilities, intentions, and interfaces.

Manifests exist primarily to support:

* observability
* debugging
* discovery
* documentation
* UI generation
* compatibility suggestions

Manifests do **not** affect routing behaviour.

Manifests do **not** enforce type safety.

Manifests do **not** create or remove subscriptions.

***

# Design Principles

## Runtime First

Chord manifests are dynamic.

A voice may:

* add routes
* remove routes
* change capabilities
* update descriptions

at runtime without reconnecting.

This supports:

* live coding
* Max/MSP
* dynamic patching
* scripting environments
* device discovery

***

## Advisory, Not Authoritative

Manifest information is descriptive.

Applications may use it for:

* documentation
* compatibility checking
* search
* visualisation

but protocol behaviour must not depend upon manifest contents.

***

## Self-Documenting Systems

A user should be able to inspect a running Chord system and discover:

* what voices exist
* what they provide
* what they expect
* what routes they expose

without consulting external documentation.

***

# Manifest Structure

```rust
VoiceManifest {
    name: String,

    description: Option<String>,

    version: Option<String>,

    tags: Vec<String>,

    provides: Vec<String>,

    expects: Vec<String>,

    routes: Vec<RouteInfo>,
}
```

***

# Manifest Fields

## Name

Human-readable voice name.

Examples:

```text
MIDI Bridge
Step Sequencer
OSC Input
```

Need not be unique.

***

## Description

Optional human-readable description.

Example:

```text
Provides MIDI input and output integration.
```

***

## Version

Optional application version string.

Examples:

```text
1.0.0
0.2.3-alpha
```

No format is enforced.

***

# Tags

Tags are free-form UTF-8 strings.

Example:

```json
[
  "midi",
  "bridge",
  "input",
  "output"
]
```

Used for:

* filtering
* search
* categorisation
* UI grouping

Tags carry no protocol semantics.

***

# Capabilities

Capabilities provide higher-level intent information.

## Provides

Capabilities offered by the voice.

Example:

```json
[
  "midi-input",
  "midi-output"
]
```

***

## Expects

Capabilities likely required by the voice.

Example:

```json
[
  "midi-output"
]
```

***

# Capability Semantics

Capabilities are:

* advisory
* non-binding
* runtime metadata

Capabilities do not:

* affect routing
* establish connections
* enforce compatibility

A hub may use capabilities for:

```text
Suggested Connections
Compatible Devices
Discovery Views
```

***

# Routes

Manifested routes describe known interfaces exposed by a voice.

```rust
RouteInfo {
    address: String,

    signal: SignalType,

    payload_hint: Option<String>,

    description: Option<String>,

    example: Option<Value>,
}
```

***

## Address

A Chord routing pattern.

Examples:

```text
/transport/bpm
/midi/play
/track/{id}/volume
```

***

## Signal

One of:

```text
Event
Param
Stream
```

Used for documentation and discovery.

***

## Payload Hint

Free-form descriptive string.

Examples:

```text
float

(channel:int, note:int, velocity:int)

map<string,float>

float | null
```

Payload hints are advisory only.

***

## Description

Human-readable explanation.

Example:

```text
Current playback tempo in beats per minute.
```

***

## Example

Optional example payload.

Example:

```json
120.0
```

or

```json
{
  "note": 60,
  "velocity": 100
}
```

Used for tooling and inspection.

***

# Manifest Updates

## SetManifest

Replaces the current manifest.

```rust
SetManifest {
    manifest: VoiceManifest
}
```

Useful for:

* initial state
* large changes
* reconstruction

***

## PatchManifest

Applies partial updates.

```rust
PatchManifest { ... }
```

Useful for:

* adding routes
* removing routes
* updating metadata

***

The protocol does not distinguish between:

```text
Initial Manifest
```

and

```text
Updated Manifest
```

Both are ordinary runtime updates.
