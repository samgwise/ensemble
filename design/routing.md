# Chord Routing & Pattern Matching Specification (Draft)

**Status:** Draft v0.1  
**Applies to:** Chord Core  
**Audience:** Client Implementers, Hub Implementers, Tool Authors

***

# Purpose

This specification defines how Chord addresses are represented and how routing patterns are matched.

Goals:

* Simple to implement in multiple languages
* Deterministic and portable
* Suitable for subscription matching
* Supports parameter extraction
* Avoids regular expressions
* Provides a shared conformance target for all implementations

***

# Address Model

A Chord address is a UTF-8 string representing a hierarchical path.

Examples:

```text
/midi/note/on
/transport/bpm
/track/7/volume
/sensor/imu/orientation
```

Addresses use:

```text
/
```

as the segment separator.

***

# Address Rules

Addresses MUST:

* Begin with `/`
* Be encoded as UTF-8
* Contain one or more segments

Examples:

Valid:

```text
/foo
/foo/bar
/track/17/volume
```

Invalid:

```text
foo/bar
foo
```

***

# Path Segments

The address:

```text
/track/17/volume
```

contains three segments:

```text
track
17
volume
```

Pattern matching operates on segments, not characters.

***

# Pattern Types

Chord Routing v1 defines four pattern mechanisms:

1. Exact Match
2. Single Segment Wildcard
3. Recursive Wildcard
4. Named Capture

No additional pattern syntax is defined in v1.

***

# Exact Match

An exact segment must match exactly.

Pattern:

```text
/track/17/volume
```

Matches:

```text
/track/17/volume
```

Does not match:

```text
/track/18/volume
/track/17
```

***

# Single Segment Wildcard

Syntax:

```text
*
```

Matches exactly one segment.

Pattern:

```text
/track/*/volume
```

Matches:

```text
/track/1/volume
/track/17/volume
/track/foo/volume
```

Does not match:

```text
/track/1
/track/1/mixer/volume
```

Equivalent segment count is required.

***

# Recursive Wildcard

Syntax:

```text
**
```

Matches zero or more remaining segments.

Pattern:

```text
/track/**
```

Matches:

```text
/track
/track/1
/track/1/volume
/track/1/sends/reverb
```

Does not match:

```text
/foo/track
```

***

## Recursive Wildcard Placement

For Routing v1:

```text
**
```

MUST appear as the final segment of a pattern.

Valid:

```text
/track/**
/midi/**
```

Invalid:

```text
/**/volume
/track/**/volume
```

This restriction keeps implementations simple and predictable.

Future versions may relax this rule.

***

# Named Capture

Syntax:

```text
{name}
```

Captures a single segment and exposes it by name.

Pattern:

```text
/track/{id}/volume
```

Address:

```text
/track/17/volume
```

Produces:

```json
{
  "id": "17"
}
```

***

## Multiple Captures

Pattern:

```text
/device/{device}/control/{control}
```

Address:

```text
/device/mixer/control/gain
```

Produces:

```json
{
  "device": "mixer",
  "control": "gain"
}
```

***

## Capture Type

All captures are strings.

Pattern matching performs no type conversion.

Example:

```text
/track/{id}/volume
```

Address:

```text
/track/17/volume
```

Produces:

```json
{
  "id": "17"
}
```

not:

```json
{
  "id": 17
}
```

Clients are responsible for interpretation and conversion.

***

# Capture Naming Rules

Capture names:

Capture names are UTF-8 strings.

They MUST be non-empty.

They MUST NOT contain:
    /
    {
    }
    \*

Capture names are compared using exact codepoint sequence matching.

No Unicode normalisation is performed.

Valid:

```text
{id}
{track_id}
{device42}
{番号}
{轨道编号}
{идентификатор}
{معرف}
```

Invalid:

```text
{}
{foo/bar}
{foo{bar}}
{foo*bar}
```

| Be aware that it is possible to encounter unicode characters which look identical, such as `{é}` and `{é}`. These will fail to match when compared.

***

# Matching Examples

## Exact

Pattern:

```text
/foo/bar
```

Address:

```text
/foo/bar
```

Result:

```text
Match
```

***

## Wildcard

Pattern:

```text
/foo/*/baz
```

Address:

```text
/foo/bar/baz
```

Result:

```text
Match
```

***

## Wildcard Failure

Pattern:

```text
/foo/*/baz
```

Address:

```text
/foo/bar/qux
```

Result:

```text
No Match
```

***

## Capture

Pattern:

```text
/foo/{name}/baz
```

Address:

```text
/foo/alice/baz
```

Result:

```json
{
  "name": "alice"
}
```

***

## Recursive Wildcard

Pattern:

```text
/foo/**
```

Address:

```text
/foo/bar/baz
```

Result:

```text
Match
```

***

# Matching Priority

A route either matches or does not match.

Routing v1 does not define route precedence.

If multiple subscriptions match an address:

```text
/track/{id}/volume
/track/*/volume
/track/**
```

all matching subscribers receive the action.

The hub does not select a "best" match.

***

# Escaping

Routing v1 does not define escaping rules.

The following characters are reserved within patterns:

```text
*
**
{
}
```

Implementations SHOULD reject invalid pattern syntax.

Future versions may introduce escaping.

***

# Unsupported Features

The following are explicitly out of scope for Routing v1.

## Typed Captures

Not supported:

```text
{id:int}
{int:id}
```

Reason:

* introduces routing-time typing
* complicates interoperability
* can be handled by clients

***

## Regular Expressions

Not supported:

```text
/track/[0-9]+
```

Reason:

* language-dependent behaviour
* difficult conformance testing
* implementation complexity

***

## Character Classes

Not supported:

```text
/foo/[abc]
```

***

## Alternation

Not supported:

```text
/{foo,bar}
```

***

## Negative Matching

Not supported:

```text
!foo
```

***

## Recursive Captures

Not supported:

```text
/{path**}
```

May be considered in future versions.

***

# Conformance Requirements

All routing implementations MUST:

* Produce identical match results
* Produce identical capture sets
* Reject invalid patterns consistently

A shared routing conformance suite should be maintained and executed against every implementation.

Example fixture:

```yaml
pattern: "/track/{id}/volume"
address: "/track/42/volume"

match: true

captures:
  id: "42"
```

***

# Design Rationale

Chord Routing deliberately combines ideas from:

* OSC hierarchical addressing
* Web framework path parameters
* Topic systems such as MQTT

while avoiding:

* regex-heavy syntax
* routing-time type systems
* implementation-specific behaviour

The resulting feature set:

```text
Exact segments
*
**
{name}
```

provides the majority of practical routing requirements while remaining simple enough to implement consistently across Rust, Python, JavaScript, Max/MSP, SuperCollider, and future FFI-based implementations.

***

# Summary

Chord Routing v1 defines:

```text
Exact Match        /foo/bar
Single Wildcard    *
Recursive          **
Capture            {name}
```

with captures returned as strings and no routing-time type system.

The design prioritises portability, predictability, observability, and ease of implementation over maximum expressiveness.
