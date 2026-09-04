# Minuet Project Guidance

*If `LOCAL.md` exists, read it first.*

## Ownership and Code Shape

Apply these rules to changes involving persistent mutable state, concurrency,
layered subsystems, compatibility boundaries, or non-trivial resource
lifecycles. For small stateless or mechanical changes, apply only the parts
that address a demonstrated risk.

### One authoritative source for each behavioral fact

A mutable fact used for behavioral decisions must have one authoritative owner,
or an explicitly designed replication and reconciliation protocol. Do not keep
a second writable copy merely because it is convenient for a caller.

A derived field is justified only by a concrete need such as performance, a
stable snapshot, cross-lifetime identity, external serialization, or diagnosis.
State beside it:

- the authoritative source;
- whether it may be stale;
- who refreshes or invalidates it;
- whether behavior may depend on it.

Use cheap consistency checks where the language and runtime make them
appropriate.

### One center for each transition

Each state transition needs one component that owns the decision and preserves
its invariants. Other components should hold a capability, token, handle, weak
reference, immutable snapshot, or request—not a parallel authority over the
same transition.

For cross-component work, make these responsibilities explicit when they are
not obvious from the code:

- state owner;
- protocol owner;
- handoff, publication, commit, or linearization point;
- failure, cancellation, and rollback or fail-forward owner;
- final cleanup owner.

“Shared ownership” is not an explanation when two participants can
independently advance the same state or neither owns cleanup.

### Diagnostic data is not protocol state

Mark owner IDs, correlation IDs, debug labels, cached names, generation
numbers, and similar diagnostic fields when they exist only for logs,
assertions, review, or troubleshooting. Diagnostic-only data must not drive
behavior.

If a field begins to determine transitions, permissions, retries, routing, or
cleanup, promote it into the explicit protocol model and document its
invariants and owner.

### Give consumers narrow capabilities

Pass only the authority and context a callee needs. Prefer a narrow request,
capability, token, snapshot, or owner API over a complete task, session, file,
service container, private lock, or internal collection.

Do not let a consumer reach into another owner's private representation to
finish local work. If repeated callers need the same operation, add the narrow
operation at the owning boundary instead of exporting storage details.

### Keep external representation at the boundary

Translate wire formats, database rows, protocol flags, compatibility
structures, CLI representations, and other external forms into internal domain
concepts before they spread through core logic, unless the object itself is
intentionally a boundary representation.

Keep policy at the layer that owns the decision. A low-level helper should not
infer caller policy from incidental fields or concrete caller types.

### Comments preserve constraints, not narration

Add comments for behavior that a future reader cannot safely reconstruct from
code shape alone, including:

- non-obvious invariants and illegal states;
- ordering, lock, lifetime, publication, cancellation, or cleanup requirements;
- compatibility choices and intentionally unsupported behavior;
- fallback paths, accepted limitations, and failure policy;
- temporary bridges and the condition that removes them.

Explain why the constraint exists and when it may change. Do not add comments
that merely restate the next line.

### Temporary paths require an exit

A compatibility bridge, fallback, dual path, migration field, dormant path, or
temporary wrapper must state:

- why it exists;
- its externally visible boundary;
- what may and may not depend on it;
- how failures remain observable;
- the concrete condition for removal or promotion.

Do not let a path become a permanent abstraction merely because it already
runs.

### Split by responsibility, not size

Do not refactor a cohesive module solely because it is long. Consider a
behavior-preserving split when a unit mixes stable roles such as external
adaptation, core state, operations, lifecycle, compatibility, persistence, and
tests, and that mixture enlarges the proof surface.

A same-owner split is structural maintenance when it preserves behavior, public
interfaces, visibility contracts, and state ownership. Moving an owner
boundary, changing a shared contract, or adding a new abstraction layer
requires an explicit design decision.

Keep re-exports and facades narrow enough that the split enforces the intended
boundary rather than merely moving text.

### Every abstraction pays rent

A wrapper, context, manager, phase, state variant, registry, callback layer, or
generic framework should do at least one of the following:

- make an illegal state unreachable;
- encode a real capability or owner boundary;
- close a concrete race, lifetime, failure, or compatibility obligation;
- serve demonstrated reuse;
- materially reduce the reasoning and proof surface.

Naming procedural steps or speculating about future reuse is not sufficient.
Prefer the most direct shape that preserves all identified obligations.

### Cleanup remains owned on every path

For each acquisition, registration, publication, retained reference,
background operation, and partial initialization, identify cleanup on success,
error, cancellation, timeout, and teardown paths.

Withdraw publication, unregister callbacks, or release external visibility
before performing diagnostic checks that could abort cleanup. Do not turn an
impossible internal state into success merely to make teardown appear robust.
