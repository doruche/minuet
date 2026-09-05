# Minuet Project Guidance

*If `LOCAL.md` exists, read it first.*

Before changing code, read and follow [`CODING_STYLE.md`](CODING_STYLE.md).
Follow [`CONTRIBUTING.md`](CONTRIBUTING.md) for contribution workflow, commits,
pull requests, and verification.

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

## Architecture Friction Closeout

### Applicability

Apply this rule to meaningful changes that can alter state ownership, subsystem
boundaries, public interfaces, compatibility behavior, concurrency, or resource
lifecycle. Do not impose a formal architecture report on trivial or purely
mechanical edits.

### Closeout requirement

Before declaring a meaningful change complete, inspect the actual diff and the
directly affected paths for architecture friction. The purpose is to catch
evidence that implementation pressure has distorted the intended model, not to
perform an unbounded repository audit.

Check for:

- a second source of truth, writable derived state, or an unexplained stale
  mirror;
- state or protocol decisions made outside their owner;
- private representation, locks, storage, or concrete implementation types
  leaking across a boundary;
- public API or visibility growth introduced only to satisfy a local caller;
- caller-, platform-, environment-, architecture-, or test-specific branches
  substituting for a real domain rule;
- a temporary bridge, fallback, dual path, or dormant production path without
  an exit condition;
- failure, cancellation, rollback, publication, or cleanup correctness that
  depends on an implicit order;
- a new wrapper, phase, token, manager, registry, or framework without a
  concrete obligation;
- a workaround that obtains a passing result by weakening the oracle,
  validation scope, compatibility honesty, or visible error behavior;
- a module whose new responsibility no longer fits its existing owner or
  lifecycle.

For cross-boundary changes, ask four closing questions:

1. Who owns every changed behavioral fact?
2. Where does ownership or authority hand off?
3. Who owns failure and final cleanup after each possible partial result?
4. What evidence proves the externally claimed scope rather than only the
   happy path?

### Evidence threshold

Architecture friction requires concrete evidence from at least one of:

- a live code path or actual diff;
- a conflicting state or truth-source model;
- an owner, handoff, lifecycle, or cleanup gap;
- a leaked interface or representation;
- a current or explicitly accepted next step that the present shape blocks;
- validation that no longer proves the stated behavior.

The following do not constitute friction by themselves:

- file length;
- ordinary imports, re-exports, or module registration;
- compiler, formatter, toolchain, or environment failures;
- general dislike of the style;
- speculative future extensibility concerns;
- adjacent technical debt that this change neither worsens nor exposes as a
  dependency.

Do not invent findings to fill a closeout section.

### Disposition

Use these levels when friction is found:

- **Apollyon — blocking correctness:** wrong results, corruption, security
  failure, crashes, or severe unrecoverable state. Stop; do not declare
  completion or perform a semantic cutover.
- **Keter — blocking architecture:** concrete owner confusion, duplicated
  authority, leaked private representation, unauditable lifecycle or protocol,
  or obstruction of a named current or accepted next step. Stop; repair it or
  obtain the required owner, target, or contract decision.
- **Euclid — non-blocking engineering debt:** concrete but locally survivable
  model mismatch, coupling, awkward structure, or test friction. The change may
  close, but report the evidence, impact, and smallest repair direction if it
  remains.
- **Safe — optional:** preference, theoretical purity, or speculative concern.
  Do not report it by default.

Do not promote a concern to Keter merely because a cleaner design is
imaginable. A blocking architecture finding needs evidence that the current
shape already violates or obstructs a real obligation.

### Reporting

If no concrete friction remains, or only Safe observations remain, emit no
placeholder “no friction” report.

For residual Euclid, report briefly:

- the affected code path;
- the intended owner, state, protocol, or lifecycle model;
- the model the implementation currently expresses;
- current impact;
- the smallest credible repair boundary.

For Keter or Apollyon, stop before the completion claim or cutover and report:

- the blocking evidence;
- the current state of the diff or partial implementation;
- what can safely remain;
- the explicit owner, target, interface, contract, or acceptance decision
  needed to proceed.

The scan does not authorize unrelated cleanup. If the repair lies outside the
current task boundary, report it instead of silently expanding scope.
