# Reference Implementation Roadmap
Ensemble — bringing the implementation up to the v0.1 specification

Version: Draft v0.1
Status: Proposed
Audience: Core Ensemble Contributors

1. Purpose

This roadmap tracks the incremental, test-first work required to bring the current Ensemble implementation up to the v0.1 design specifications. It complements the PRD (`prd.md`): the PRD describes the target product; this document describes the path from here to there, increment by increment.

The specifications are the source of truth. Where the implementation and a specification disagree, the specification wins. Priority order, per `design/conformance-testing.md`: Specification → Conformance Fixtures → Reference Implementation.

2. Methodology

Test-first. For each increment:

Write failing tests against the specification.
Make the tests pass with the minimum correct change.
Refactor within the increment.
Keep all other tests green throughout.

Small, reviewable increments. Each increment is independently mergeable and leaves the workspace building and tests passing. No big-bang rewrites.

Spec-anchored. Every test cites the specification section it encodes, so the conformance corpus grows as a by-product of implementation work.

No backwards-compatibility shims. Old behaviour is replaced, not preserved alongside new behaviour. Per the standing rule, consult before adding any compatibility code path.

Tests must run whenever routing changes, per the standing project rule, and should be run for every increment before merge.

3. Current State vs Target

Current workspace (4 crates):

ensemble-core — values, protocol messages, codec, clock sync, pattern matching (all in one crate)
ensemble-hub — TCP router, scheduler, param store, and a minimal TUI in one binary
ensemble-client — client library
ensemble-bridge-midi — MIDI bridge binary

Target workspace (11 crates per PRD §3):

ensemble-core, ensemble-values, ensemble-routing, ensemble-protocol, ensemble-clock, ensemble-manifest, ensemble-hub, ensemble-hub-tui, ensemble-client, ensemble-test-fixtures, ensemble-conformance

The fine-grained crate split materialises incrementally: each increment extracts or creates the crate it owns, rather than restructuring everything up front. This keeps each diff small and testable.

4. Headline Divergences

Routing — the current matcher (`ensemble-core/src/pattern.rs`) is character-prefix based and only supports trailing `*` and exact match. The spec is segment-based with single-segment `*`, recursive `**`, and `{name}` captures, plus invalid-pattern rejection. Semantics differ even for `*`: the current `/midi/*` matches `/midi/ch/1/note`; per spec it matches only a single segment. Highest divergence, most self-contained, foundational to everything else.

Value model — current `Value` uses `i32`/`f32` and lacks `Null`, `List`, `Map`, and `TypedBinary`. Spec mandates `i64`/`f64` and the full type set, with the Tuple/List distinction preserved through round-trips.

Protocol messages — current messages are a Rust enum with `Hello` carrying subscriptions and `is_bridge`, `Goodbye`, `ClockSyncRequest/Reply` using a voice-send-time triple, and `Subscribe { patterns: Vec }`. Spec uses a self-describing `WireMessage { type, payload }` envelope, string message types, `u64` voice IDs, `hello { protocol_version, name }` (identity only), `disconnect`, `clock_ping`/`clock_pong { sequence }`, single-pattern `subscribe`, plus `set_manifest`, `patch_manifest`, `update_name`, `unset_param`, and `error`.

Lifecycle — subscriptions are currently coupled into `Hello`. Spec separates connection (identity only) from subscriptions, manifests, and name updates, all mutable at runtime without reconnect.

Manifest — not implemented. Spec defines `VoiceManifest`, `RouteInfo`, capabilities (`provides`/`expects`), tags, and set/patch semantics.

Scheduling — the current hub stores future param values into the param store immediately (violating activation-time retention), does not replay params on `subscribe` (only on connect), does not drop streams under congestion, and tracks no observability metadata. The monotonic hub clock and not-before delivery are broadly correct.

Conformance — not implemented. Spec treats the conformance suite as a first-class deliverable with golden fixtures and certification levels.

TUI — currently bundled into `ensemble-hub` and minimal. Spec separates `ensemble-hub` (no UI) from `ensemble-hub-tui`, with a rich set of views (PRD §6–§11).

5. Progress Tracker

Update this section as work proceeds. Mark each increment `[x]` when its acceptance criteria are met and all tests pass. Use `[~]` for in progress and `[ ]` for not started. Record the merge/commit reference on the line when complete.

- [x] 1. Routing — commit: f458a86
- [x] 2. Value model — commit: bfe4be8
- [x] 3. Protocol messages — commit: _
- [ ] 4. Lifecycle — commit: _
- [ ] 5. Manifest — commit: _
- [ ] 6. Scheduling — commit: _
- [ ] 7. Observability and hub events — commit: _
- [ ] 8. Hub / TUI split — commit: _
- [ ] 9. Conformance harness — commit: _

6. Increments

6.1 Routing
Goal: replace the character-prefix matcher with a segment-based engine matching `design/routing.md`.
Crate: extract `ensemble-routing` from `ensemble-core`; hub and client depend on it.
Spec reference: `design/routing.md`; routing fixtures in `design/conformance-testing.md`.
Test-first plan:
- Encode the routing conformance fixtures (exact, wildcard, recursive wildcard, named capture, unicode capture, invalid patterns) as failing tests first.
- Implement `Pattern` parsing and a `Matcher` returning a `CaptureSet`, rejecting invalid patterns (`/foo/**/bar`, `/{foo,bar}`, typed captures `{id:int}`, regex, character classes, alternation, negative matching, recursive captures `/{path**}`).
- Replace `matches_any`/`matches_pattern` call sites in the hub and client with the new API.
Acceptance criteria:
- `cargo test -p ensemble-routing` passes the full routing conformance corpus.
- Invalid patterns are rejected with a structured error usable by the hub's `error` message.
- Hub routing behaves per spec for `*`, `**`, and `{name}`.
Status: Complete.

6.2 Value model
Goal: bring `Value` to `design/value-model-specification.md`.
Crate: extract `ensemble-values` from `ensemble-core`.
Spec reference: `design/value-model-specification.md`; value and serialization fixtures in `design/conformance-testing.md`.
Test-first plan:
- Write round-trip and shape tests for Null, Bool, Integer(i64), Float(f64) including NaN/±Inf, String (UTF-8), Binary, Tuple, List, Map (string-keyed, unordered), and TypedBinary (`ensemble/*` namespace).
- Assert Tuple and List remain distinct through encode/decode, and that map ordering does not affect conformance.
- Implement the type set and MessagePack adapters; remove `i32`/`f32` and the `Payload` Single/Tuple/None enum in favour of the spec value model.
Acceptance criteria:
- `cargo test -p ensemble-values` passes value and serialization conformance.
- No `i32`/`f32` primitives remain in the value model.
- `ensemble/*` TypedBinary namespace reserved.
Status: Complete.

### 6.3 Protocol messages
Goal: move to the `WireMessage { type, payload }` envelope and the spec message set.
Crate: extract `ensemble-protocol` (envelope, message types, codec) from `ensemble-core`.
Spec reference: `design/protocol-spec.md`; protocol fixtures in `design/conformance-testing.md`.
Test-first plan:
- Write frame round-trip and envelope-validation tests for every message type: hello, welcome, disconnect, set_manifest, patch_manifest, update_name, subscribe, unsubscribe, action, unset_param, clock_ping, clock_pong, error.
- Assert unknown message types and missing `type`/`payload` produce errors.
- Implement the envelope, `u64` voice IDs, `protocol_version` negotiation, and `clock_ping`/`clock_pong { sequence, hub_time }`.
Acceptance criteria:
- `cargo test -p ensemble-protocol` passes protocol conformance.
- Hub returns `error` and closes on unsupported `protocol_version`.
- Clock sync uses `clock_ping`/`clock_pong`; old `ClockSyncRequest/Reply` and `Goodbye` removed.
Status: Complete.

6.4 Lifecycle
Goal: separate connection from subscriptions, manifests, and naming per `design/lifecycle.md`.
Spec reference: `design/lifecycle.md`; lifecycle fixtures in `design/conformance-testing.md`.
Test-first plan:
- Write tests for Hello→Welcome (identity only, `u64` voice ID), duplicate names accepted, runtime subscribe/unsubscribe with snapshot-then-live ordering, runtime name update, and graceful/ungraceful disconnect cleanup (subscriptions, manifest, voice state removed).
- Implement `disconnect`, drop subscriptions from `Hello`, make `subscribe`/`unsubscribe` single-pattern, and add `update_name`.
Acceptance criteria:
- Connection establishes identity only; subscriptions mutate at runtime.
- Snapshot delivery completes before live traffic on subscribe.
- Disconnect (graceful and transport-close) cleans up all voice state.
Status: Not started. Depends on 6.3.

6.5 Manifest
Goal: implement the manifest system per `design/manifest.md`.
Crate: create `ensemble-manifest`.
Spec reference: `design/manifest.md`; manifest fixtures in `design/conformance-testing.md`.
Test-first plan:
- Write tests for `set_manifest` (full replace) and `patch_manifest` (partial update), runtime updates without reconnect/restart/new voice ID, and manifest non-interference with routing.
- Implement `VoiceManifest`, `RouteInfo`, `Tag`, capabilities (`provides`/`expects`), and patch application/validation.
Acceptance criteria:
- `set_manifest` replaces; `patch_manifest` updates only specified fields.
- Manifest updates require no reconnect.
- Routing behaviour is independent of manifest contents.
Status: Not started. Depends on 6.3, 6.4.

6.6 Scheduling
Goal: correct param timing and scheduling semantics per `design/scheduling.md`.
Crate: extract `ensemble-clock` from `ensemble-core` (hub clock authority, `ClockEstimator`, `RTTSampler`, `HubTime`).
Spec reference: `design/scheduling.md`; scheduling and param fixtures in `design/conformance-testing.md`.
Test-first plan:
- Write tests for immediate and past-timestamp dispatch, not-before delivery for future timestamps, per-sender FIFO for equal timestamps, cross-sender ordering unspecified, param replay on subscribe, activation-time retention (future params not current until activated), snapshot consistency (future updates excluded), and stream best-effort/droppable behaviour.
- Fix the hub: defer future param writes until activation, replay on `subscribe`, add stream congestion dropping, and track observability metadata (received/scheduled/dispatch time, sender).
Acceptance criteria:
- `cargo test -p ensemble-clock` and the scheduling/param conformance tests pass.
- Subscribe triggers snapshot before live updates.
- Future param values do not become current until their timestamp.
- Streams may be dropped under congestion; events and params are not.
Status: Not started. Depends on 6.1, 6.2, 6.3.

6.7 Observability and hub events
Goal: emit reserved `/hub/**` actions and track diagnostics per the observability plane.
Spec reference: `design/protocol-spec.md` (Hub Events, Reserved Namespace), `design/scheduling.md` (Observability Metadata), PRD §5.
Test-first plan:
- Write tests asserting hub events publish on voice joined/left/renamed, manifest set/updated, and action dropped, under `/hub/**`, and that protocol behaviour does not depend on them.
- Implement the hub event producer and reserved-namespace enforcement.
Acceptance criteria:
- Reserved `/hub/**` namespace is enforced (applications should not publish to it).
- Hub events are ordinary actions in the observability plane and do not alter protocol state.
Status: Not started. Depends on 6.3, 6.5.

6.8 Hub / TUI split
Goal: separate the hub runtime from the TUI per the PRD crate structure, and build out the TUI views.
Crate: split `ensemble-hub` (no UI) and create `ensemble-hub-tui`.
Spec reference: PRD §3, §6–§11.
Test-first plan:
- Write tests for the headless hub API (start, accept, route, schedule) independent of any UI.
- Implement the PRD TUI layout and views: Voice Browser, Manifest Browser, Action Monitor, Param Inspector, Route Browser, Scheduling Monitor, Capability Browser, Log Viewer, and the Route Tester with optional Unicode diagnostics.
Acceptance criteria:
- `ensemble-hub` builds and runs headless with no TUI dependency.
- `ensemble-hub-tui` renders the PRD views and route tester.
Status: Not started. Depends on 6.4, 6.5, 6.6, 6.7.

6.9 Conformance harness
Goal: stand up the conformance runner and fixture corpus as a first-class deliverable.
Crate: create `ensemble-test-fixtures` and `ensemble-conformance`.
Spec reference: `design/conformance-testing.md`; PRD §15.
Test-first plan:
- Write the runner against the golden-fixture philosophy (input → expected output), organised into routing, values, lifecycle, scheduling, params, and protocol suites.
- Promote the inline conformance tests written in earlier increments into version-controlled, language-neutral fixtures (YAML/TOML).
- Define Core and Full certification levels.
Acceptance criteria:
- `cargo test -p ensemble-conformance` runs every suite against the reference implementation.
- Fixtures are human-readable, language-neutral, and version-controlled.
- An individual suite runs via `cargo test -p ensemble-conformance <suite>`.
Status: Not started. Depends on all prior increments.

7. Ordering Rationale

Routing first: it is the most self-contained area, has the largest semantic divergence, and every other area (subscriptions, param replay, hub events, manifests) depends on correct matching. Its crate (`ensemble-routing`) is also independently useful via FFI, so it benefits from early isolation.

Value model next: protocol messages and manifests both depend on the value types, so fixing `i64`/`f64` and the full type set early prevents rework.

Protocol messages build on values; lifecycle builds on protocol; manifest builds on lifecycle; scheduling depends on routing, values, and protocol; observability depends on protocol and manifest; the hub/TUI split depends on the runtime areas being correct; and the conformance harness consolidates the test corpus produced throughout.

8. Notes

- Each increment should be its own branch and merge; record the commit reference in the Progress Tracker.
- `design/schema.dbml` is out of scope for v0.1 (PRD §12 defers persistence). When persistence is introduced, revisit and keep `schema.dbml` current with the database schema.
- Comments and documentation use Australian English per the standing rule.
