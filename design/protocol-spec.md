Chord Wire Protocol & Message Types Specification (Draft v0.1)
Status

Draft v0.1

Purpose

This specification defines the Chord wire protocol, including:

transport framing
message encoding
message envelopes
protocol message types
control plane vs data plane separation
version negotiation

This document intentionally describes protocol communication and not application semantics.

Design Principles
Language Neutral

The wire protocol must remain easy to implement in:

Rust
Python
JavaScript
Go
C/C++
Max/MSP
SuperCollider

No code generation is required.

Self-Describing

Messages should be understandable when inspected directly.

Protocol inspection tools should not require a generated schema to identify message meaning.

Extensible

New message types may be introduced in future protocol versions without requiring redesign of the envelope structure.

Separate Control and Data

Chord distinguishes between:

Control Plane

Protocol operations:

hello
subscribe
set_manifest
clock_ping

Data Plane

User/application actions:

/transport/bpm
/midi/play
/track/1/volume

Observability Plane

Hub-generated actions:

/hub/voice/joined
/hub/error

Protocol Stack
Chord Messages
        ↓
MessagePack
        ↓
Length-Prefixed Frames
        ↓
Transport

Transport Independence

Chord is transport-independent.

The protocol may be carried over:

TCP
Unix Domain Sockets
Windows Named Pipes
WebSockets
Future transports


The transport layer is outside the scope of this specification.

Framing

Messages are transmitted as length-prefixed MessagePack frames.

Format:

[4 byte little-endian length]
[MessagePack payload]


The length specifies the size of the MessagePack payload in bytes.

Each frame contains exactly one Chord message.

Encoding

All messages are encoded using:

MessagePack


Reasons:

compact
language neutral
mature implementations
supports nested structures
supports binary payloads
Message Envelope

Every protocol message uses a common envelope.

Conceptual structure:

WireMessage {
    type: String,
    payload: Value
}

Example
{
  "type": "hello",
  "payload": {
    "protocol_version": 1,
    "name": "MIDI Bridge"
  }
}

Message Type Naming

Message types are UTF-8 strings.

Examples:

hello
welcome
subscribe
action
clock_ping


Numeric message identifiers are intentionally avoided.

Reasons:

readability
easier inspection
easier debugging
simpler forward compatibility

Future protocol revisions may define alternative optimized encodings.

Protocol Versioning

Version negotiation occurs during connection establishment.

Clients specify:

protocol_version


during the Hello message.

Version Mismatch

If the protocol version is unsupported:

The hub should return:

error


and close the connection.

Version negotiation beyond this is out of scope for v0.1.

Message Categories

Chord messages fall into five categories:

Lifecycle
Discovery
Routing
Data
Timing
Errors

Lifecycle Messages
hello

Client → Hub

Establishes protocol session.

Payload:

{
    protocol_version: u32,
    name: String
}


Example:

{
  "type": "hello",
  "payload": {
    "protocol_version": 1,
    "name": "Step Sequencer"
  }
}

welcome

Hub → Client

Assigns a Voice ID.

Payload:

{
    voice_id: u64
}


Example:

{
  "type": "welcome",
  "payload": {
    "voice_id": 42
  }
}


After Welcome is received the voice is considered connected.

disconnect

Client → Hub

Requests graceful disconnection.

Payload:

{}


The hub removes:

subscriptions
manifest state
voice registration
Discovery Messages

Discovery messages manage runtime metadata.

set_manifest

Client → Hub

Replaces the current manifest.

Payload:

{
    manifest: VoiceManifest
}


Used for:

initial manifest publication
major manifest changes
patch_manifest

Client → Hub

Updates portions of an existing manifest.

Payload:

{
    patch: ...
}


Patch semantics are intentionally unspecified in v0.1 and may evolve independently.

update_name

Client → Hub

Updates the client-advertised name.

Payload:

{
    name: String
}


This allows runtime renaming without requiring manifest replacement.

Routing Messages

Subscriptions are managed independently from manifests.

subscribe

Client → Hub

Payload:

{
    pattern: String
}


Example:

{
  "type": "subscribe",
  "payload": {
    "pattern": "/midi/**"
  }
}


Upon registration:

Register Subscription
→ Replay Matching Params
→ Begin Live Delivery

unsubscribe

Client → Hub

Payload:

{
    pattern: String
}


Removes a previously registered subscription.

Data Messages

Data messages carry Chord application traffic.

action

Client ↔ Hub

Payload:

{
    address: String,
    signal_type: SignalType,
    timestamp: f64,
    payload: Value
}


Example:

{
  "type": "action",
  "payload": {
    "address": "/transport/bpm",
    "signal_type": "param",
    "timestamp": 10.5,
    "payload": 120.0
  }
}


Actions are the primary message type used by Chord applications.

unset_param

Client ↔ Hub

Removes retained Param state.

Payload:

{
    address: String
}


Example:

{
  "type": "unset_param",
  "payload": {
    "address": "/transport/bpm"
  }
}


This operation is separate from Null values.

Timing Messages

Timing messages support clock synchronization.

clock_ping

Client → Hub

Payload:

{
    sequence: u64
}


Used to estimate latency and clock offset.

clock_pong

Hub → Client

Payload:

{
    sequence: u64,
    hub_time: f64
}


Allows clients to estimate:

round-trip time
hub clock offset

The synchronization algorithm itself is implementation-defined.

Error Messages
error

Hub → Client

Payload:

{
    code: String,
    message: String
}


Example:

{
  "type": "error",
  "payload": {
    "code": "invalid_pattern",
    "message": "Recursive wildcard must be final segment."
  }
}

Example Error Codes

Possible examples include:

unsupported_protocol_version
invalid_pattern
malformed_manifest
invalid_message
internal_error


This list is not exhaustive.

Observability Events

Protocol operations are not represented as Actions.

For example:

hello
subscribe
set_manifest


remain dedicated protocol messages.

Hub Events

The hub may generate ordinary Chord Actions using reserved addresses.

Examples:

/hub/voice/joined
/hub/voice/left
/hub/manifest/updated
/hub/warning
/hub/error


These are part of the observability plane.

They do not alter protocol state.

They simply describe events which occurred.

Reserved Namespace

Addresses beginning with:

/hub/


are reserved for hub-generated actions.

Applications should not rely on publishing to this namespace.

Future specifications may extend the namespace.

Control Plane Summary

Protocol messages:

hello
welcome
disconnect

set_manifest
patch_manifest
update_name

subscribe
unsubscribe

clock_ping
clock_pong

error


These messages change hub state.

They are not routable.

Data Plane Summary

Application traffic:

action
unset_param


These messages participate in:

routing
scheduling
replay
bridging
observability
Summary

Chord v0.1 defines a simple MessagePack-based protocol built around:

String Message Types
Shared Message Envelope
Length-Prefixed Frames
Transport Independence


Core message types:

Lifecycle:
    hello
    welcome
    disconnect

Discovery:
    set_manifest
    patch_manifest
    update_name

Routing:
    subscribe
    unsubscribe

Data:
    action
    unset_param

Timing:
    clock_ping
    clock_pong

Errors:
    error


This structure keeps protocol mechanics separate from application actions while still allowing the hub to expose runtime events through reserved /hub/** action addresses for observability and tooling.