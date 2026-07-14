I think conformance testing is one of the least exciting documents, but also one of the most important.

A lot of protocols die not because the specification is bad, but because after a few years there are:

Rust interpretation
Python interpretation
JavaScript interpretation
Max interpretation


and they all differ subtly.

Given one of Ensemble's goals is:

Many languages
Many environments
One protocol


I think the conformance suite should be treated as a first-class deliverable from very early on.

Ensemble Conformance & Interoperability Specification (Draft v0.1)
Status

Draft v0.1

Purpose

This specification defines how Ensemble implementations demonstrate compatibility with the protocol.

The goals are:

Consistent behaviour across implementations
Early detection of specification ambiguities
Confidence in interoperability
Support for FFI and native implementations
Long-term protocol stability
Philosophy
Specification By Behaviour

The written specifications define:

What the protocol means.


The conformance suite defines:

How behaviour is verified.


Both are considered part of the protocol.

An implementation should pass the conformance suite in addition to reading the specifications.

Conformance Areas

The suite should be divided into independent areas.

Routing
Values
Lifecycle
Scheduling
Protocol
Manifests


Implementations may run individual suites during development.

Golden Test Philosophy

All conformance tests should use:

Input
Expected Output


fixtures.

Avoid implementation-specific tests.

Example:

pattern: "/track/{id}/volume"

address: "/track/42/volume"

match: true

captures:
  id: "42"


Any implementation producing a different result is non-conformant.

Routing Conformance

Routing should likely be the largest test suite.

Exact Match
pattern: "/foo/bar"
address: "/foo/bar"

match: true

Exact Match Failure
pattern: "/foo/bar"
address: "/foo/baz"

match: false

Wildcard
pattern: "/foo/*/bar"
address: "/foo/baz/bar"

match: true

Recursive Wildcard
pattern: "/foo/**"
address: "/foo/a/b/c"

match: true

Named Capture
pattern: "/track/{id}/volume"

address: "/track/17/volume"

match: true

captures:
  id: "17"

Unicode Capture
pattern: "/track/{番号}/volume"

address: "/track/17/volume"

match: true

captures:
  番号: "17"

Invalid Patterns

Example:

pattern: "/foo/**/bar"

valid: false


All implementations should reject the pattern.

Value Conformance

Tests should verify all supported value types.

Integer
value: 42
type: Integer

Float
value: 3.14159
type: Float

NaN
value: NaN
type: Float


Round-trip behaviour must be preserved.

UTF-8 String
value: "音量"
type: String

Tuple
value:
  - 0
  - 60
  - 100

semantic_type: Tuple


The Tuple/List distinction should survive encoding and decoding.

Map
value:
  note: 60
  velocity: 100


Map order must not affect conformance.

Typed Binary
tag: "ensemble/f32"

data: "..."


Tag preservation must be verified.

Serialization Conformance

The protocol intentionally separates:

Value Model


from:

Serialization Format


Tests should verify that implementations preserve Ensemble semantics through encoding and decoding.

Examples:

Tuple remains Tuple
List remains List
UTF-8 preserved
TypedBinary preserved

Lifecycle Conformance
Hello → Welcome

Input:

hello


Expected:

welcome


with valid Voice ID.

Duplicate Names

Input:

Voice A
name = "Sequencer"

Voice B
name = "Sequencer"


Expected:

Both accepted.


Names are not unique identifiers.

Disconnect

Expected behaviour:

Subscriptions removed
Manifest removed
Voice removed

Manifest Conformance
Set Manifest

The new manifest completely replaces the previous manifest.

Patch Manifest

Only specified fields change.

All other fields remain intact.

Runtime Update

Manifest updates must not require:

Reconnect
Restart
New Voice ID

Scheduling Conformance
Immediate Action
timestamp: now


Expected:

Dispatch immediately.

Past Timestamp
timestamp: now - 5


Expected:

Dispatch immediately.

Future Timestamp
timestamp: now + 5


Expected:

Dispatch no earlier than scheduled time.

Per-Voice FIFO

Input:

A @ 10
B @ 10
C @ 10


Expected:

A
B
C

Cross-Voice Ordering

The suite should explicitly verify:

No ordering guarantee.


Applications must not depend on ordering between voices.

Param Conformance
Replay

Current state:

/transport/bpm = 120


New subscription:

/transport/**


Expected:

Receive 120.

Snapshot Before Live Traffic

Expected sequence:

Subscribe
↓
Snapshot
↓
Live Updates


Never:

Subscribe
↓
Live Update
↓
Snapshot

Unset

Given:

unset_param("/transport/bpm")


Expected:

Value removed.


Future subscribers do not receive replay.

Protocol Conformance
Message Types

Unknown message types:

Must generate error.

Version Handling

Unsupported versions:

Return error.
Close connection.

Envelope Validation

Required fields:

type
payload


Must be present.

Hub Event Conformance

Reserved namespace:

/hub/**


should remain reserved.

Implementations should not allow protocol behaviour to depend on hub event publication.

Reference Fixtures

The Ensemble project should maintain:

routing/
values/
lifecycle/
scheduling/
params/
protocol/


fixture directories.

Fixtures should be:

Human readable
Language neutral
Version controlled


Prefer:

or

over implementation-specific formats.

Reference Implementation

A reference implementation is useful but should not define the protocol.

Priority order:

Specification
↓
Conformance Fixtures
↓
Reference Implementation


If the implementation differs from the specification:

Specification wins.

Certification Levels

I would eventually define:

Core

Must pass:

Routing
Values
Protocol
Lifecycle

Full

Must additionally pass:

Scheduling
Params
Manifests

Hub

Must additionally support:

Observability
Hub Events
Retention


This gives lightweight bridges and embedded implementations a clear target.

Summary

A conformant Ensemble implementation should behave identically when presented with the same:

routing patterns
values
manifests
timestamps
protocol messages

The conformance suite becomes the executable definition of the protocol, ensuring that Rust, Python, JavaScript, Max/MSP, SuperCollider and future implementations remain interoperable as the ecosystem grows.