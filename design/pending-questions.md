With the reference implementation complete (all 9 increments), most protocol questions are now resolved through implementation decisions.

## Resolved Questions

### 1. Manifest Patch Semantics — RESOLVED

**Decision**: Field replacement patches.

Implemented in `ensemble-manifest`. `patch_manifest` replaces list fields (`tags`, `provides`, `expects`, `routes`) entirely when present. String fields are updated individually. `null` clears optional fields. This is simple and sufficient for the use case.

### 2. Tuple Encoding — RESOLVED

**Decision**: MessagePack ext type distinction.

Implemented in `ensemble-values`. `Tuple` and `List` are distinct variants in the `Value` enum. They serialize differently through MessagePack (Tuple uses ext type, List uses array). This preserves the semantic distinction through the wire format.

### 3. Hub Event Payloads — RESOLVED

**Decision**: Map-based payloads.

Implemented in `ensemble-hub`. Hub events use `Value::Map` with named fields:
- `/hub/voice/joined`: `{voice_id: Integer, name: String}`
- `/hub/voice/left`: `{voice_id: Integer, name: String}`
- `/hub/voice/renamed`: `{voice_id: Integer, old_name: String, new_name: String}`
- `/hub/manifest/set`: `{voice_id: Integer, manifest: Value}`
- `/hub/manifest/updated`: `{voice_id: Integer, patch: Value}`

Maps are easier to evolve than positional tuples.

### 4. Conformance Testing — RESOLVED

**Decision**: YAML fixtures with language-neutral format.

Implemented in `ensemble-test-fixtures` and `ensemble-conformance`. 14 YAML fixture files covering routing, values, protocol, lifecycle, scheduling, params, and manifests. Fixtures use simple YAML (no anchors, tags, or complex keys) for easy conversion to JSON or parsing by other languages. Every suite is driven by its fixtures, so the spec and the tests cannot drift apart.

### 5. Local Hub Discovery — RESOLVED

**Decision**: Fallback-first discovery strategy with platform-appropriate mechanisms.

Documented in `local-discovery.md`. Default port `7331` provides universal fallback. Desktop platforms use port file at platform-specific locations (`$XDG_RUNTIME_DIR/ensemble/hub.port` on Linux, `$TMPDIR/ensemble-hub.port` on macOS, `%LOCALAPPDATA%\Ensemble\hub.port` on Windows). Mobile/embedded platforms use native discovery (Android Services, iOS Bonjour). Multiple override mechanisms supported: CLI argument, environment variable (`ENSEMBLE_HUB_PORT`), configuration file.

## Remaining Open Questions

### 6. Hub-to-Hub Semantic Policies — RESOLVED

**Decision**: Implemented in `ensemble-bridge-remote` (see `bridge-remote.md`).

- **Param replay forwarding**: retained params ARE replayed to newly connected peers, and the replay is fully drained before live traffic is forwarded, so a stale replayed value can never overtake a newer live update. Param unsets propagate across the bridge via a dedicated `bridge_unset` message.
- **Loop prevention**: every bridged message carries a unique `msg_id`; bridges re-forward inbound messages unchanged (origin preserved) and suppress duplicates via a bounded per-bridge seen-set. This gives exactly-once delivery and guaranteed termination in arbitrary topologies (chains, rings, meshes), verified by integration tests.

This was the largest architectural question. It lived in bridge design, not core protocol, and the core protocol is unchanged by its resolution.

### 7. Capability Taxonomy

The mechanism is settled (`provides` / `expects`) but not conventions. Who defines capability strings like `midi-output` or `midi-input`?

Likely approach: Start with a recommended convention list rather than a formal registry.

### 8. Toolbox Scope

Is the Ensemble Toolbox a collection of separate voice processes, or one application hosting multiple utility modules (Mapper, Filter, Scaler, Router, Logger)?

Preferred approach: One runtime, many utility modules — consistent with the hub philosophy.

## Closed Questions

All of the following are settled through the reference implementation:

✅ Routing syntax
✅ UTF-8 everywhere
✅ Capture semantics
✅ Value types (all 10)
✅ TypedBinary
✅ Tuple vs List distinction
✅ Param ownership
✅ Param replay
✅ Explicit unset
✅ Shared clock
✅ Timestamp semantics
✅ FIFO guarantees
✅ Activation-time retention
✅ Manifest architecture
✅ Manifest patch semantics (field replacement)
✅ Lifecycle (connect, disconnect, cleanup)
✅ Voice identity
✅ Wire protocol structure
✅ Hub event payloads (Map-based)
✅ Reserved /hub/ namespace
✅ Bridge philosophy
✅ Capability hints
✅ Hub observability (TUI + hub events)
✅ Conformance suite (YAML fixtures)
✅ Hub / TUI separation
✅ Local hub discovery (port file + fallback strategy)

## Looking Forward

The design is stable. The next major discoveries will come from building real tools and bridges rather than from more protocol design work. The area most likely to evolve based on real-world experience is manifests — not because the design is wrong, but because once people build MIDI bridges, DAW integrations, live coding tools, and Max patches, they'll discover what metadata users actually find valuable.
