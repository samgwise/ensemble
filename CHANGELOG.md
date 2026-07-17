# Changelog

All notable changes to the Ensemble reference implementation.

## [Unreleased] — OSC Bridge, Demo Applications & Discovery Crate

- Created `ensemble-bridge-osc` crate — bidirectional OSC/UDP bridge with configurable address prefix mapping
- Extracted `ensemble-discovery` into a standalone crate with zero external dependencies
- Added `set_port_file_path()` runtime override for port file location
- Updated all crates to depend on `ensemble-discovery` directly
- Created `ensemble-demo-euclidean` crate with TUI for Euclidean rhythm generation
- Created `ensemble-demo-pitch-cycler` crate with TUI for pitch pattern cycling
- Updated README.md with demo section and crate listing
- Added README.md files for both demo crates

## Increment 9: Conformance Harness — `146ab03`

- Created `ensemble-test-fixtures` crate with 14 YAML fixture files
- Created `ensemble-conformance` crate with test runner
- Implemented 13 conformance tests across 7 suites:
  - Routing: pattern matching, invalid patterns, namespace enforcement
  - Values: type preservation, type discrimination
  - Protocol: error codes, action structure
  - Lifecycle: voice registration, disconnect cleanup
  - Scheduling: dispatch timing, activation time retention
  - Params: state management
  - Manifests: registration, patching, routing independence
- Fixtures use simple YAML (no anchors/tags) for JSON compatibility
- All 13 conformance tests pass

## Increment 8: Hub / TUI Split — `4530379`

- Separated hub runtime from TUI into distinct crates
- `ensemble-hub` converted to library + thin headless binary
  - Moved all server logic to `lib.rs`
  - Added public accessors (`voices()`, `param_state()`, `scheduled_actions()`, `event_log()`, `action_log()`)
  - Added `ActionLogEntry` ring buffer (1000 entries)
  - Removed `ratatui` and `crossterm` dependencies
- Created `ensemble-hub-tui` crate with comprehensive TUI views:
  - Voice Browser, Manifest Browser, Action Monitor
  - Param Inspector, Scheduling Monitor, Log Viewer, Route Tester
  - Tab-based navigation (1-7 keys or Tab)
- All 31 integration tests pass

## Increment 7: Observability and Hub Events — `4aa6720`

- Implemented reserved `/hub/` namespace enforcement
- Added `ERR_RESERVED_NAMESPACE` error code
- Added `emit_hub_event` helper for hub-generated events
- Hub events as ordinary actions with `source: 0`:
  - `/hub/voice/joined` — voice connected
  - `/hub/voice/left` — voice disconnected
  - `/hub/voice/renamed` — voice renamed
  - `/hub/manifest/set` — manifest set
  - `/hub/manifest/updated` — manifest patched
- Hub events routed to all subscribers of `/hub/**` addresses
- 7 new integration tests added
- All 31 tests pass

## Increment 6: Scheduling — `12c7b7f`

- Implemented timestamp-based action scheduling
- Actions with future timestamps held until dispatch time
- Past timestamps dispatch immediately
- FIFO ordering preserved for same-timestamp actions
- Activation-time retention for params:
  - Future params not stored in `param_state` until activation
  - Late joiners receive current value, not future value
  - After activation, future value becomes current
- Scheduler runs as background task polling every 1ms
- 8 new integration tests added
- All 24 tests pass

## Increment 5: Manifest System — `4ced5b3`

- Implemented `VoiceManifest` type with fields:
  - `name`, `description`, `version`, `tags`, `provides`, `expects`, `routes`
- Implemented `RouteInfo` for route descriptions
- Added `set_manifest` and `patch_manifest` protocol messages
- Manifest patch semantics: field replacement (list fields replaced entirely)
- `null` clears optional fields in patches
- Manifests are advisory — do not affect routing
- 6 new integration tests added
- All 16 tests pass

## Increment 4: Lifecycle — `8c6dce0`

- Implemented voice connection and registration
- Hello/Welcome handshake with protocol version check
- Voice ID assignment (unique, immutable, hub-assigned)
- Duplicate names accepted (distinct voice IDs)
- Runtime name updates via `update_name` message
- Graceful disconnect cleanup:
  - Remove subscriptions
  - Remove param state owned by voice
  - Remove scheduled actions from voice
  - Remove manifest
- Ungraceful disconnect (connection drop) also cleans up
- 7 new integration tests added
- All 10 tests pass

## Increment 3: Protocol Messages — `291c6ec`

- Created `ensemble-protocol` crate
- Implemented `WireMessage` envelope: `{type: String, payload: Value}`
- Implemented all protocol message types:
  - Lifecycle: `hello`, `welcome`, `disconnect`
  - Discovery: `set_manifest`, `patch_manifest`, `update_name`
  - Routing: `subscribe`, `unsubscribe`
  - Data: `action`, `unset_param`
  - Timing: `clock_ping`, `clock_pong`
  - Errors: `error`
- Implemented error codes:
  - `unsupported_protocol_version`
  - `invalid_pattern`
  - `malformed_manifest`
  - `invalid_message`
  - `internal_error`
  - `reserved_namespace`
- Helper functions for message construction
- 12 unit tests added

## Increment 2: Value Model — `bfe4be8`

- Created `ensemble-values` crate
- Implemented all 10 value types:
  - `Null`, `Bool`, `Integer(i64)`, `Float(f64)`, `String`
  - `Binary`, `Tuple`, `List`, `Map`, `TypedBinary`
- `FloatValue` wrapper for f64 with bit-pattern equality (NaN support)
- Tuple vs List distinction preserved through MessagePack serialization
- TypedBinary with tag and opaque data
- 37 unit tests covering all types and round-trip serialization
- Conformance fixtures for value model testing

## Increment 1: Routing — `f458a86`

- Created `ensemble-routing` crate
- Implemented segment-based address pattern matching:
  - Exact match: `/foo/bar`
  - Single wildcard: `/track/*/volume`
  - Recursive wildcard: `/track/**`
  - Named capture: `/track/{id}/volume`
- Pattern validation at parse time
- `PatternError` enum for invalid patterns:
  - `RecursiveWildcardNotFinal`
  - `EmptyCaptureName`
  - `InvalidCaptureName`
  - `CharacterClassNotSupported`
  - `AlternationNotSupported`
  - `TypedCaptureNotSupported`
  - `RecursiveCaptureNotSupported`
  - `RegexNotSupported`
  - `NegativeMatchingNotSupported`
- `CaptureSet` for extracted captures
- `matches_any` convenience function
- 45 unit tests covering all pattern types and edge cases

## Pre-Increment History

- `554941e` — Added roadmap for reference implementation
- `8b74cd2` — Project name change to Ensemble
- `dfcefef` — Reference impl PRD added
- `5b2c2d5` — Further work on design specification
- `d6810b4` — v0.2: Scheduled delivery and param state
- `5bfd743` — v0.1: Foundation — hub, client, core protocol
