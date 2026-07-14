Ensemble Value Model Specification (Draft v0.1)
Status

Draft v0.1

Purpose

This specification defines the Ensemble value system used for:

Action payloads
Param values
Manifest examples
Future protocol extensions

The Ensemble value model is intentionally small, language-neutral, and independent from any particular serialization format.

Design Principles
Language Neutral

The value model should be practical to implement in:

Rust
Python
JavaScript
Go
C/C++
Max/MSP
SuperCollider

No generated schemas or code generation are required.

Serialization Independent

Ensemble values describe semantic meaning.

They are not tied to a specific encoding format.

Example:

Tuple


remains conceptually distinct from:

List


even if both are encoded identically within a particular serializer.

The Ensemble value model is independent from:

MessagePack
JSON
CBOR
Future encodings

Minimal Core

The protocol defines a small number of primitive types that are widely supported across languages.

Specialized or application-specific data should use:

TypedBinary


rather than extending the core value system.

Value Types

Ensemble defines the following value types:

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

Null

Represents an explicit null value.

Examples:

null


Use cases include:

optional values
unset fields
partial state descriptions

Null is a normal value and may appear anywhere a value is permitted.

Param Semantics

Null does not remove Param state.

Example:

/track/1/name = null


means:

Current value is null


not:

Delete Param


Param removal is performed using:

unset_param

Bool

Boolean value.

Allowed values:

true
false


Examples:

/transport/playing = true
/mixer/muted = false

Integer

Signed 64-bit integer.

Definition:

i64


Range:

-9,223,372,036,854,775,808

to

 9,223,372,036,854,775,807


Examples:

60
127
-1

Float

IEEE754 double precision floating-point value.

Definition:

f64


Examples:

0.5
120.0
3.141592653589793

Special Float Values

All valid IEEE754 values are permitted.

Including:

NaN
+Infinity
-Infinity


Interpretation of these values is application-specific.

The hub does not assign semantic meaning to them.

String

UTF-8 encoded text.

Examples:

"hello"
"track 1"
"音量"
"مستوى"


Strings are used throughout Ensemble for:

addresses
capture names
manifest metadata
tags
capabilities
type tags
Binary

Opaque sequence of bytes.

Examples:

Audio frame
Image data
Serialized structures
MIDI SysEx


The hub does not interpret binary payloads.

The hub should preserve binary data unchanged.

Tuple

Ordered collection with fixed positional meaning.

Example:

(channel, note, velocity)


The position of each value carries meaning.

Tuple Characteristics

Properties:

Ordered
Positional
Fixed semantic structure


Examples:

(0, 60, 100)

(x, y)

(r, g, b)

Usage

Tuples are useful when:

field meanings are well-defined
positional access is natural
fixed structure is expected

Example:

(channel, note, velocity, duration)


A Tuple is not intended to be arbitrarily resized.

List

Ordered collection of values.

Properties:

Ordered
Variable length


Examples:

[60, 62, 64, 67]

["kick.wav", "snare.wav"]

[0.1, 0.2, 0.3, 0.4]

Usage

Lists are appropriate when:

collection length varies
values are conceptually members of the same set
iteration is expected
Tuple vs List

Ensemble intentionally distinguishes between Tuple and List.

Tuple:

(channel, note, velocity)


means:

Position defines meaning.


List:

[note1, note2, note3]


means:

Collection of values.

Serialization Note

Some serializers may encode both Tuple and List using a common representation.

Implementations should preserve the semantic distinction where possible.

Map

Associative collection of key-value pairs.

Properties:

Unordered
String-keyed

Key Requirements

Map keys must be:

UTF-8 strings


Only.

Valid:

{
  "note": 60,
  "velocity": 100
}


Invalid:

{
  123: "foo"
}

Ordering

Map ordering is not significant.

Applications must not rely upon:

Insertion order
Iteration order
Serialization order


Different implementations may store Maps differently.

Security Considerations

Ordering guarantees are intentionally omitted because:

implementations vary
optimization strategies vary
some environments intentionally randomize ordering

Applications must treat Maps as unordered collections.

TypedBinary

TypedBinary provides an extensibility mechanism for application-defined value types.

Structure:

TypedBinary {
    tag: String,
    data: Binary
}

Purpose

TypedBinary allows transmission of values which do not belong in the core value model.

Examples:

f32
complex64
rational
opencv-mat
fft-frame

Tag Requirements

Tags must be:

UTF-8
Case-sensitive


Examples:

ensemble/f32
ensemble/complex64
org.example.matrix

Reserved Namespace

The following namespace is reserved:

ensemble/*


Reserved tags may be defined by future Ensemble specifications.

Hub Behaviour

The hub treats TypedBinary values as opaque.

The hub:

Routes them
Stores them
Replays them


but does not interpret their contents.

Future tooling may provide custom visualisation support for specific tags.

Nesting

Values may be nested arbitrarily.

Examples:

{
  "track": {
    "name": "Bass",
    "notes": [60, 62, 64]
  }
}


or:

[
    (0, 60, 100),
    (0, 64, 100),
    (0, 67, 100)
]

Implementation Limits

The protocol places no limits on:

Container depth
Message size
Map size
List size
Tuple size


Implementations may provide configurable operational limits.

Examples:

Maximum message size
Maximum nesting depth
Maximum container entries


These are implementation concerns rather than protocol semantics.

Equality

Ensemble does not define value equality semantics.

Example:

120
120
120


may be transmitted as three independent updates.

The hub does not:

Deduplicate
Coalesce
Suppress


value updates based on equality comparisons.

Type Validation

The protocol itself performs no schema validation.

Example:

/transport/bpm


may be documented as:

float


through manifests, but enforcement remains implementation-specific.

Manifest Hints

Validation and documentation should be expressed through:

payload_hint


metadata.

Examples:

float

(channel:int, note:int, velocity:int)

float | null

list<float>


These hints are advisory.

They do not alter the Ensemble value model.

Summary

Ensemble v0.1 defines the following value types:

Null

Bool

Integer (i64)
Float (f64)

String (UTF-8)
Binary

Tuple
List
Map

TypedBinary


Key properties:

UTF-8 strings throughout the protocol
Explicit distinction between Tuple and List
String-keyed unordered Maps
Opaque Binary and TypedBinary support
No protocol-level schemas
No hub-level value equality semantics
Serialization-independent design
Configurable implementation limits

The value model is designed to remain small, portable, and extensible while providing enough structure to support rich tooling, manifests, validation hints, and future protocol evolution.