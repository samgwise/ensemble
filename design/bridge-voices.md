# Ensemble Bridge Architecture Specification (Draft v0.1)
Status

Draft v0.1

Purpose

This specification defines the role of bridges within the Ensemble ecosystem.

Bridges allow Ensemble systems to interact with external protocols, devices, services, and other Ensemble instances while preserving the core architecture of:

One User
One Machine
One Hub
Many Tools


The goal of this specification is to ensure bridges integrate naturally with Ensemble without introducing special protocol behaviour.

Design Philosophy
Bridges Are Ordinary Voices

A bridge is an Ensemble voice.

The hub does not distinguish between:

Sequencer
Visualiser
Python Script
MIDI Bridge
OSC Bridge
Hub Bridge


All participate in Ensemble using the same:

connection lifecycle
manifests
routing
scheduling
discovery mechanisms

No special protocol operations are defined for bridges.

Bridges Translate Systems

A bridge exists to translate between:

Ensemble


and

External System


Examples:

Ensemble ↔ MIDI
Ensemble ↔ OSC
Ensemble ↔ Serial
Ensemble ↔ MQTT
Ensemble ↔ DMX
Ensemble ↔ Ensemble


The bridge acts as a semantic adapter between two systems.

Bridges Apply Policy

External systems do not necessarily support Ensemble concepts such as:

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
     Ensemble Hub


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

When an external event enters Ensemble through a bridge:

MIDI Event
OSC Message
Serial Packet
Sensor Update


the bridge should timestamp the resulting Ensemble Action using its current estimate of hub time.

Example:

timestamp = hub.now()


at the moment the bridge receives the event.

Rationale

This ensures all Actions entering Ensemble participate in the shared hub timeline.

From the perspective of other voices:

all incoming actions


appear to originate within the same temporal framework.

Outgoing Events

When a bridge receives an Ensemble Action for external transmission:

Ensemble Action
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

Bridges frequently encounter mismatches between Ensemble semantics and external protocol capabilities.

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

Bridges are not required to preserve every Ensemble feature.

Example:

Ensemble Feature


↓

No External Equivalent


The bridge should implement the closest practical mapping.

Perfect semantic equivalence is not required.

Hub Bridges
Overview

A Hub Bridge connects two independent Ensemble hubs.

Example:

Hub A
    ↕
Hub Bridge
    ↕
Hub B


A Hub Bridge is still a bridge and therefore still a normal Ensemble voice.

Local-First Principle

Ensemble's primary deployment model remains:

One Machine
One Hub


Networked operation is achieved through Hub Bridges rather than by extending the Ensemble address namespace.

Address Space

Hub Bridges do not create a global Ensemble namespace.

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


belong in applications and tooling rather than in the Ensemble protocol.

Future Toolbox Applications

A future Ensemble ecosystem may provide utility applications offering:

Filter
Mapper
Router
Scaler
Transformer
Logger


These applications are ordinary Ensemble voices.

They do not require protocol changes.

Bridge Observability

Bridges should participate fully in Ensemble observability mechanisms.

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

preserve every Ensemble semantic
support every Ensemble value type
perform perfect time translation
implement retained state
implement scheduling unavailable in the external protocol
implement every capability advertised within the Ensemble ecosystem

These are implementation decisions.

Summary

The central principle of Ensemble bridging is:

Bridges are ordinary Ensemble voices that translate between Ensemble and external systems.

Key properties:

No bridge-specific protocol layer
Discovery through manifests, tags, and capabilities
Incoming events timestamped using estimated hub time
Outgoing events respect Ensemble scheduling
Semantic adaptation is expected
Hub-to-Hub communication occurs through bridges
Filtering and transformation remain tooling concerns

This approach keeps the Ensemble protocol small while allowing the ecosystem to grow around a rich set of interoperable bridges.