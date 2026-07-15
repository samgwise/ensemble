Honestly, very few fundamental ones.

We've reached the stage where most remaining questions feel like implementation policy rather than protocol definition. That's a good sign.

Open Questions I Would Still Track
1. Manifest Patch Semantics

We've agreed on:

SetManifest
PatchManifest


but not the exact patch model.

Options:

Field replacement
{
  "routes": [...]
}


replaces routes.

Simple.

Operation-based patching
{
  "op": "add_route",
  ...
}


More efficient, more complex.

JSON-Patch style

Probably overkill.

My current preference would be:

Start with field replacement patches. Keep it simple.

This is probably the largest unresolved protocol detail.

2. Tuple Encoding

Conceptually we've settled:

Tuple != List


but not how that survives serialization.

Since MessagePack only has arrays, eventually we need an encoding convention.

Possible approaches:

Tagged representation
Tuple


becomes an internal Ensemble encoding.

MessagePack Extension

Probably not desirable.

Ensemble Value Envelope

Potentially:

{
  "kind": "tuple",
  "value": [...]
}


internally.

I wouldn't solve this yet, but it will matter during implementation.

3. Hub-to-Hub Semantic Policies

We know:

Hub Bridge


exists.

We haven't decided:

Param replay forwarding

Should retained Params be:

replayed?
ignored?
configurable?

Loop prevention
Hub A
 ↔
Hub B


can easily produce cycles.

This is probably the largest architectural question remaining.

Fortunately it lives in bridge design, not core protocol.

4. Capability Taxonomy

The mechanism is settled:

provides
expects


but not conventions.

Example:

midi-output
midi-input


Who defines these?

Options:

Completely organic

Community evolves terminology.

Recommended convention list

Probably my preference.

Formal registry

Too early.

I'd be inclined to start a:

Recommended Capabilities


document later rather than putting it into the protocol.

5. Hub Event Payloads

We created:

/hub/voice/joined
/hub/voice/left


but not formal payload schemas.

For example:

(voice_id, name)


vs

{
  "voice_id": 42,
  "name": "Sequencer"
}


I suspect maps will ultimately win because they're easier to evolve.

6. Toolbox Scope

This is more of a product question.

The idea has emerged naturally:

Ensemble Toolbox


containing:

Mapper
Filter
Scaler
Router
Logger


Open question:

Is Toolbox a collection of voices, or one application hosting multiple utilities?

I actually like your proposal:

One runtime, many utility modules.

It feels consistent with the hub philosophy.

Questions I'd Explicitly Declare Closed

These all feel settled:

✅ Routing syntax

✅ UTF-8 everywhere

✅ Capture semantics

✅ Value types

✅ TypedBinary

✅ Param ownership

✅ Param replay

✅ Explicit unset

✅ Shared clock

✅ Timestamp semantics

✅ FIFO guarantees

✅ Manifest architecture

✅ Lifecycle

✅ Voice identity

✅ Wire protocol structure

✅ Bridge philosophy

✅ Capability hints

✅ Hub observability direction

✅ Conformance suite philosophy

The One Thing I'd Watch Closely

If I were starting implementation tomorrow, the single area I'd expect to evolve based on real-world experience is:

Manifests


Not because the design is wrong, but because once people start building:

MIDI bridges
DAW integrations
live coding tools
Max patches
generative systems

you'll discover what metadata users actually find valuable.

Everything else feels surprisingly stable.

In fact, I think the current state of the design is strong enough that I'd be comfortable moving from specification into implementation planning. The next major discoveries are likely to come from building the first hub, first Rust client, and first bridge rather than from more protocol design work.