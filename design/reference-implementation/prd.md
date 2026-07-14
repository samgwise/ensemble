# Product Requirements Document (PRD)
Chord Reference Implementation & Hub TUI

Version: Draft v0.1
 Status: Proposed
 Target Language: Rust
 Audience: Core Chord Contributors, Client Library Authors, Bridge Developers

1. Overview

The Chord Reference Implementation serves four purposes:

Define canonical protocol behaviour.
Validate protocol specifications.
Provide the primary hub runtime.
Deliver a first-class observability experience.

The reference implementation is not merely a protocol daemon. It is intended to become the primary tool for understanding and debugging Chord systems.

2. Product Vision

Chord should provide a development experience where a user can:

Start Hub
Connect Tools
Observe Traffic
Inspect State
Debug Routing
Discover Capabilities


without packet sniffers, console logging, or custom debugging code.

The Hub TUI should become the equivalent of:

top
htop
tmux
Node-RED inspector
MIDI Monitor
OSC Monitor


for Chord systems.

3. Project Structure

The project should be implemented as a Rust workspace.

chord/
│
├─ chord-core/
├─ chord-values/
├─ chord-routing/
├─ chord-protocol/
├─ chord-clock/
├─ chord-manifest/
├─ chord-hub/
├─ chord-hub-tui/
├─ chord-client/
├─ chord-test-fixtures/
├─ chord-conformance/
│
└─ tools/

4. Workspace Crates
chord-values

Defines:

Value
Null
Bool
Integer
Float
String
Binary
Tuple
List
Map
TypedBinary


Responsibilities:

value representation
serialization adapters
validation helpers

No networking.

chord-routing

Defines:

Pattern
Matcher
CaptureSet


Responsibilities:

route parsing
wildcard matching
captures
routing conformance tests

Should be usable independently via FFI.

chord-clock

Defines:

ClockEstimator
RTTSampler
HubTime


Responsibilities:

clock synchronisation
offset estimation
RTT tracking

Should be separately testable.

chord-manifest

Defines:

VoiceManifest
RouteInfo
Tag
Capability


Responsibilities:

manifest handling
patch application
validation
chord-protocol

Defines:

WireMessage


and all protocol message types.

Responsibilities:

MessagePack codec
frame encoding
protocol validation

No transport logic.

chord-client

Client SDK.

Responsibilities:

connect()
publish()
subscribe()
manifest()


Provides:

hub.now()


and scheduling helpers.

chord-core

Shared structures and traits used throughout the ecosystem.

chord-hub

Actual server implementation.

Responsibilities:

voice registry
subscriptions
scheduling
Param storage
clock authority
hub events

No UI.

chord-hub-tui

Terminal user interface.

Responsible for all visualisation and observability.

chord-conformance

Conformance runner.

Executes:

Routing Tests
Value Tests
Protocol Tests
Timing Tests

5. Hub Responsibilities

The hub owns:

Voice Registry

Tracks:

VoiceId
Name
UI Name
Manifest
Capabilities
Subscriptions
Connection State

Scheduler

Executes:

Events
Params
Streams


according to timestamp semantics.

Param Store

Maintains:

Current Value
Last Writer
Last Update Time


for every Param.

Routing Engine

Consumes:

Addresses
Patterns
Subscriptions


via chord-routing.

Hub Event Producer

Generates:

/hub/voice/joined
/hub/voice/left
/hub/voice/renamed

/hub/manifest/set
/hub/manifest/updated

/hub/action/dropped

6. TUI Goals

The TUI is a first-class component.

It should be possible to run:

chord-hub


and immediately understand system behaviour.

7. TUI Layout

Initial layout:

┌────────────────────────────────────┐
│ Voices                             │
├────────────────────────────────────┤
│ Actions                            │
├────────────────────────────────────┤
│ Details                            │
└────────────────────────────────────┘


Keyboard-driven navigation.

8. TUI Views
Voice Browser

Displays:

Voice ID
Name
Tags
Capabilities
Connection Time


Example:

42  MIDI Bridge
43  Bass Sequencer
44  Lighting Controller

Manifest Browser

Displays:

Routes
Descriptions
Examples
Capabilities


For selected voice.

Action Monitor

Live feed.

Displays:

Time
Source
Address
Signal Type


Example:

12:03:17.512
Bass Sequencer
→ /midi/play

Param Inspector

Displays current retained state.

Example:

/transport/bpm = 120

Last Writer:
Bass Sequencer

Updated:
12:03:17

Route Browser

Displays route usage.

Example:

/track/{id}/volume

Subscribers:
  Mixer
  Recorder

Publishers:
  Automation Tool

Scheduling Monitor

Displays:

Pending Actions
Activation Time
Source Voice


Useful for timing diagnostics.

Capability Browser

Displays:

midi-input
midi-output
audio-clock
lighting-output


and associated voices.

Log Viewer

Displays:

/log/debug
/log/info
/log/warn
/log/error


traffic.

9. Route Tester

Built-in testing utility.

Input:

Pattern:
/track/{id}/volume

Address:
/track/7/volume


Result:

MATCH

id = 7

10. Unicode Diagnostics

Route tester should optionally warn about:

mixed scripts
confusable characters
normalization ambiguity


Warnings only.

Never affect routing.

11. Discovery Features

The hub should suggest compatible voices.

Example:

Voice:
  Arpeggiator

Expects:
  midi-output


Matched against:

Voice:
  MIDI Bridge

Provides:
  midi-output


Output:

Suggested Match:
  MIDI Bridge ↔ Arpeggiator

12. Persistence
v0.1

No persistence required.

Hub restart:

Clears Params
Clears Schedules
Resets Clock

13. Performance Goals

Target:

100+ connected voices


and

100,000+ actions/minute


on a typical modern desktop.

Precise optimisation is secondary to correctness and observability.

14. Developer Experience

Provide:

cargo test
cargo run
cargo bench


Support:

cargo test -p chord-routing
cargo test -p chord-conformance


individually.

15. Testing Requirements

Must include:

Routing
Patterns
Captures
Wildcards
Unicode

Values
Tuple
List
TypedBinary
UTF-8

Protocol
Handshake
Subscriptions
Manifest Updates

Timing
Replay
Scheduling
FIFO
Past Timestamps

16. Non-Goals (v0.1)

Not required:

GUI
Authentication
Persistence
Cluster Support
Global Discovery
Web Dashboard
Plugin System


These may be future projects.

17. Success Criteria

A successful v0.1 implementation allows a user to:

Start a hub.
Connect multiple voices.
Observe all actions in real time.
Inspect manifests and capabilities.
Inspect Param state.
Test routing patterns.
Debug timing and scheduling.
Run conformance tests against client implementations.
Deliverables
Core
chord-hub
chord-client
chord-routing
chord-values
chord-protocol
chord-clock

Tooling
chord-hub-tui
chord-conformance

Documentation
Protocol Specs
Implementation Guide
Conformance Guide
Bridge Guide


The primary goal of the reference implementation is not simply to prove the protocol works, but to establish Chord's identity as a shared timing, state, and observability platform for creative systems.