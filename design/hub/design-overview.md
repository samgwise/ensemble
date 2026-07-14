# Chord Hub Observability & Diagnostics Specification (Draft v0.1)
Status

Draft v0.1

Purpose

The Chord Hub provides:

routing
scheduling
Param storage
clock authority

In addition, the Hub acts as the primary observability tool for a running Chord system.

This specification defines:

hub-generated events
diagnostic capabilities
inspection interfaces
monitoring behaviour
Design Philosophy
Visibility Is A Feature

Chord systems should be observable by default.

Users should not need:

packet sniffers
debug print statements
custom monitoring tools

to understand message flow.

The Hub should make system behaviour visible.

Diagnostics Should Not Affect Routing

Observability features:

inspect
display
record

but do not alter routing behaviour.

Protocol First

All observable information should be represented in ways that are usable by:

the built-in TUI
future GUIs
external monitoring tools

The built-in hub UI should consume the same underlying information available to other tools.

Hub Event Namespace

The Hub may emit Actions using reserved addresses:

/hub/**


These are ordinary Chord Actions.

They may be subscribed to like any other Action.

Reserved Namespace
/ hub /**


is reserved for Hub-generated events.

Applications should not publish into this namespace.

Voice Events
Voice Joined

Address:

/hub/voice/joined


Signal:

Event


Payload:

(
    voice_id,
    name
)


Example:

(42, "Step Sequencer")

Voice Left

Address:

/hub/voice/left


Signal:

Event


Payload:

(
    voice_id,
    name
)

Voice Renamed

Address:

/hub/voice/renamed


Signal:

Event


Payload:

(
    voice_id,
    old_name,
    new_name
)

Manifest Events
Manifest Set

Address:

/hub/manifest/set


Payload:

(
    voice_id
)

Manifest Updated

Address:

/hub/manifest/updated


Payload:

(
    voice_id
)

Routing Events

Routing events should initially be optional.

Many installations may choose to disable them.

They are primarily useful for diagnostics.

Action Routed

Address:

/hub/action/routed


Payload:

{
    sender,
    address,
    recipient_count
}


Example:

{
  "sender": 42,
  "address": "/transport/bpm",
  "recipient_count": 3
}

Action Dropped

Address:

/hub/action/dropped


Payload:

{
    address,
    reason
}


Reasons may include:

stream_congestion
invalid_route
internal_error

Logging Events

The Hub defines conventional logging addresses.

Applications may publish:

/log/debug
/log/info
/log/warn
/log/error


These are conventions rather than protocol-level messages.

Recommended Payload

Tuple:

(
    source,
    message
)


or

Map:

{
  "source": "...",
  "message": "..."
}

Voice Registry

The Hub maintains a live registry of connected voices.

Each entry includes:

Voice ID
Name
UI Name
Connection Time
Tags
Capabilities
Subscriptions
Manifest


This registry is accessible to:

Hub TUI
Future GUI
Future monitoring tools
Manifest Browser

The Hub should provide a manifest inspection view.

For every voice:

Provides
Expects
Tags
Routes
Descriptions
Examples


Example:

Voice:
    MIDI Bridge

Tags:
    bridge
    midi

Provides:
    midi-output

Routes:
    /midi/play
    /midi/cc

Route Browser

The Hub should support route-centric inspection.

Example:

/transport/bpm


Display:

Signal Type:
    Param

Publishers:
    2

Subscribers:
    4

Current Value:
    120

Param Inspector

The Hub should allow inspection of retained Param state.

Example:

/transport/bpm = 120

Last Writer:
    Voice 7

Updated:
    13:42:10.212

Action Monitor

The Hub should maintain a live stream of recent actions.

Recommended display:

Timestamp
Sender
Address
Signal
Payload


Example:

13:42:10.251

Voice:
    Step Sequencer

Address:
    /midi/play

Payload:
    (0,60,100,0.5)

Routing Trace

A future routing trace view may show:

Action
    ↓
Matched Subscriptions
    ↓
Recipients


Example:

/track/1/volume

Matched:
    /track/{id}/volume
    /track/**


Recipients:

Mixer
Visualiser
Recorder

Route Testing

The Hub should provide a route testing utility.

Input:

Pattern:
/track/{id}/volume

Address:
/track/7/volume


Result:

Match

Captures:
    id = "7"

Unicode Diagnostics

Routing tools may optionally provide warnings for:

Confusable Characters
Mixed Scripts
Normalization Ambiguities


Warnings must not alter matching behaviour.

Example:

Warning:
Capture name contains visually confusable characters.

Timing Diagnostics

The Hub should expose timing information.

Per voice:

Estimated Offset
Estimated RTT
Last Ping


Example:

Voice:
    Python Script

RTT:
    0.6 ms

Clock Offset:
    +0.2 ms

Scheduling Diagnostics

The Hub may expose:

Pending Scheduled Actions


including:

Address
Timestamp
Originating Voice


Example:

Dispatch:
    52.1

Address:
    /midi/play

Voice:
    Sequencer

Performance Metrics

The Hub may expose:

Messages Received
Messages Routed
Stream Drops
Connected Voices
Stored Params


Example:

Voices:
    8

Params:
    124

Pending Actions:
    12

Stream Drops:
    0

Discovery & Suggestions

Capabilities exist primarily to support discovery.

The Hub may suggest compatible voices.

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


Result:

Suggested Connection:
    Arpeggiator ↔ MIDI Bridge


These suggestions have no routing effect.

Future Utility Layer

The Hub may eventually support user-configurable:

Message Filters
Address Mapping
Transformations
Scaling
Splitting
Merging


These features are considered tooling and not core protocol behaviour.

Design Goals Summary

The Hub should function as:

Router
Clock Authority
Param Store

+

Message Monitor
Manifest Browser
Route Explorer
Protocol Inspector
Diagnostic Console
Discovery Tool


The intent is that a user can understand a running Chord system primarily through Hub tooling, rather than through ad hoc logging, packet inspection, or custom debugging utilities. This observability-first approach is a core part of the Chord experience and a major differentiator from many existing creative coding and music-control ecosystems.