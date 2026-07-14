# Chord Bridge Architecture Specification (Draft v0.1)
Status

Draft v0.1

Purpose

This specification defines the role of bridges within the Chord ecosystem.

Bridges allow Chord systems to interact with external protocols, devices, services, and other Chord instances while preserving the core architecture of:

One User
One Machine
One Hub
Many Tools


The goal of this specification is to ensure bridges integrate naturally with Chord without introducing special protocol behaviour.

Design Philosophy
Bridges Are Ordinary Voices

A bridge is a Chord voice.

The hub does not distinguish between:

Sequencer
Visualiser
Python Script
MIDI Bridge
OSC Bridge
Hub Bridge


All participate in Chord using the same:

connection lifecycle
manifests
routing
scheduling
discovery mechanisms

No special protocol operations are defined for bridges.

Bridges Translate Systems

A bridge exists to translate between:

Chord


and

External System


Examples:

Chord ↔ MIDI
Chord ↔ OSC
Chord ↔ Serial
Chord ↔ MQTT
Chord ↔ DMX
Chord ↔ Chord


The bridge acts as a semantic adapter between two systems.

Bridges Apply Policy

External systems do not necessarily support Chord concepts such as:

Params
Scheduling
Retained State
Capabilities
Structured Values


Bridges therefore make policy decisions regarding how concepts are translated.

This is expected behaviour.

Bridge Model

Conceptually:

External System
        ↕
      Bridge
        ↕
     Chord Hub


The bridge owns all translation logic.

The hub remains unaware of the external protocol.

Discovery

Bridges participate in discovery using the standard manifest system.

No bridge-specific discovery protocol exists.

Recommended Tags

Bridges should advertise:

[
  "bridge"
]


and protocol-specific tags.

Examples:

[
  "bridge",
  "midi"
]

[
  "bridge",
  "osc"
]

[
  "bridge",
  "serial"
]

[
  "bridge",
  "hub"
]


These tags support:

filtering
categorisation
search
UI grouping
Recommended Capabilities

Bridges should advertise useful capabilities where appropriate.

Example:

{
  "provides": [
    "midi-input",
    "midi-output"
  ]
}


Example:

{
  "provides": [
    "serial-device"
  ]
}


Capabilities remain advisory.

They do not alter routing behaviour.

Timestamp Semantics

Timestamp handling is one of the primary responsibilities of a bridge.

Incoming Events

When an external event enters Chord through a bridge:

MIDI Event
OSC Message
Serial Packet
Sensor Update


the bridge should timestamp the resulting Chord Action using its current estimate of hub time.

Example:

timestamp = hub.now()


at the moment the bridge receives the event.

Rationale

This ensures all Actions entering Chord participate in the shared hub timeline.

From the perspective of other voices:

all incoming actions


appear to originate within the same temporal framework.

Outgoing Events

When a bridge receives a Chord Action for external transmission:

Chord Action
      ↓
 Bridge
      ↓
External System


the bridge should honour the Action timestamp where practical.

The hub remains the authoritative scheduler.

Scheduling Strategies

Different external systems may require different strategies.

Immediate Forwarding

Example:

HTTP
MQTT
Serial


The bridge may transmit immediately upon receiving a dispatched Action.

Local Scheduling

Example:

MIDI
DMX
Lighting Control


A bridge may implement its own local scheduler to improve timing accuracy when delivering messages to external hardware.

Example:

Hub
    ↓
Bridge
    ↓
Local Scheduling
    ↓
Device


This is considered valid bridge behaviour.

Temporal Preservation

Where possible, bridges should preserve temporal intent.

The original Action timestamp should remain meaningful even if the external system does not natively support scheduling.

Semantic Translation

Bridges frequently encounter mismatches between Chord semantics and external protocol capabilities.

Examples include:

Param
Stream
Capabilities
Scheduling
Retention


Bridges are expected to apply reasonable translation policies.

Param Translation

Example:

Param:
/transport/bpm


may become:

OSC Message
MIDI CC
Serial Command


depending upon bridge design.

State Retention

External systems may not support retained state.

Examples:

MIDI
Serial
Simple UDP Protocols


In such cases the bridge may:

Translate retained values
Maintain local state
Ignore replay behaviour


depending on implementation goals.

Semantic Fidelity

Bridges are not required to preserve every Chord feature.

Example:

Chord Feature


↓

No External Equivalent


The bridge should implement the closest practical mapping.

Perfect semantic equivalence is not required.

Hub Bridges
Overview

A Hub Bridge connects two independent Chord hubs.

Example:

Hub A
    ↕
Hub Bridge
    ↕
Hub B


A Hub Bridge is still a bridge and therefore still a normal Chord voice.

Local-First Principle

Chord's primary deployment model remains:

One Machine
One Hub


Networked operation is achieved through Hub Bridges rather than by extending the Chord address namespace.

Address Space

Hub Bridges do not create a global Chord namespace.

A Hub Bridge decides:

what to subscribe to
what to forward
what to filter
what to transform

through bridge configuration.

Timestamp Preservation

Hub Bridges should preserve timestamps whenever practical.

Example:

Hub A
Action @ 120.0


↓

Hub Bridge


↓

Hub B
Action @ 120.0


The goal is to preserve temporal intent across systems.

The exact synchronization strategy is implementation-dependent.

Filtering & Transformation

Filtering and transformation are not bridge requirements.

However bridges may implement translation policies such as:

Address Rewriting
Payload Transformation
Value Scaling
Filtering
Aggregation


when necessary to support the external system.

Relationship to Tooling

General-purpose transformation should not be part of the bridge specification.

Examples:

Scale Value
Rename Address
Merge Sources
Split Streams


belong in applications and tooling rather than in the Chord protocol.

Future Toolbox Applications

A future Chord ecosystem may provide utility applications offering:

Filter
Mapper
Router
Scaler
Transformer
Logger


These applications are ordinary Chord voices.

They do not require protocol changes.

Bridge Observability

Bridges should participate fully in Chord observability mechanisms.

Recommended behaviour:

publish manifests
advertise tags
advertise capabilities
expose meaningful route descriptions
emit diagnostics through normal logging conventions

Example:

/log/info
/log/warn
/log/error

Bridge Requirements

A conforming bridge should:

behave as a normal voice
connect using standard lifecycle procedures
publish a manifest
advertise tags and capabilities where practical
timestamp incoming events using estimated hub time
honour outgoing Action timestamps where practical
participate in standard observability tooling
Bridge Non-Requirements

A bridge is not required to:

preserve every Chord semantic
support every Chord value type
perform perfect time translation
implement retained state
implement scheduling unavailable in the external protocol
implement every capability advertised within the Chord ecosystem

These are implementation decisions.

Summary

The central principle of Chord bridging is:

Bridges are ordinary Chord voices that translate between Chord and external systems.

Key properties:

No bridge-specific protocol layer
Discovery through manifests, tags, and capabilities
Incoming events timestamped using estimated hub time
Outgoing events respect Chord scheduling
Semantic adaptation is expected
Hub-to-Hub communication occurs through bridges
Filtering and transformation remain tooling concerns

This approach keeps the Chord protocol small while allowing the ecosystem to grow around a rich set of interoperable bridges.