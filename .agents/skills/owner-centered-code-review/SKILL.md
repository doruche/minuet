---
name: owner-centered-code-review
description: Review medium-to-large, stateful, concurrent, or layered changes for ownership integrity, protocol boundaries, lifecycle, failure semantics, and justified complexity. Use for requested change reviews or bounded engineering audits where build and style checks are insufficient; do not invoke for trivial isolated edits unless explicitly requested.
---

# Owner-Centered Code Review

Review whether the code expresses a coherent responsibility and state model
under realistic success, failure, cancellation, concurrency, and teardown
paths. Assume syntax and basic compilation are handled elsewhere unless
evidence says otherwise.

## Establish the Review Boundary

Choose the mode explicitly:

- **Change review:** inspect the requested diff and follow a risk across files
  only when the change exposes a concrete boundary dependency.
- **Bounded engineering audit:** inspect a named subsystem, owner surface, or
  completed implementation slice even when no failure is known. Read the live
  implementation and its current callers; do not infer the design from file
  names or proposal documents alone.

Use engineering audits at natural semantic boundaries: after a meaningful
implementation slice, before a contract or ownership cutover, or when repeated
edits add new roles, state carriers, compatibility bridges, or cross-owner
coordination to the same surface. Do not schedule them by line, file, commit, or
elapsed-time thresholds alone.

An audit authorizes findings, not an automatic refactor. Keep repair advice
within the requested scope. If a repair would change the product target, public
interface, compatibility promise, state owner, acceptance criteria, or
validation claim, identify the required decision instead of silently expanding
the work.

Prefer current code, tests, effective contracts, and runtime evidence over
historical plans. Keep proposed behavior distinct from behavior already in
effect.

## Recover the Ownership Model

For each important mutable fact or cross-boundary operation, determine:

- the authoritative source of truth;
- who may mutate it and preserve its invariants;
- what other participants hold: state, a snapshot, a token, a capability, a
  handle, or diagnostic identity;
- the allowed transitions and the operation that owns them;
- the handoff, publication, commit, or linearization point;
- who owns failure, rollback or fail-forward, cancellation, and final cleanup;
- the contract visible outside the owner.

Write this model down in the review only when it materially clarifies a
finding. Do not demand a diagram or inventory for a locally obvious change.

## Review Axes

### Responsibility topology

- Distinguish a module or file boundary from a semantic owner boundary. A
  same-owner split can reduce proof surface without creating a new owner.
- Treat two components that independently decide the same mutable fact as
  duplicated authority, not as evidence that the system needs more
  coordinators.
- Judge semantic span rather than line count. A long cohesive table can be
  harmless; a shorter file that mixes unrelated state machines, policies,
  lifecycles, or backends can be hazardous.
- Check dependency direction. A consumer should not implement the provider's
  policy or reach into its private representation to complete local work.

### Local semantic integrity

- Look for writable derived state, stale mirrors, diagnostic fields that drive
  behavior, loose flags that admit contradictory combinations, and mutation
  paths that bypass the authoritative transition API.
- Require a cache or snapshot to state its source, freshness or staleness
  rules, invalidation owner, and whether it may affect behavior.
- Require every wrapper, state variant, token, phase, or manager-like
  abstraction to encode a real capability, exclude a reachable illegal state,
  represent an owner boundary, or materially reduce the proof surface.
- Do not prescribe methods, free functions, object orientation, functional
  style, or state machines as ends in themselves.

### Cross-owner protocol

- Trace ownership transfer, publication order, commit points, lock order,
  re-entry, retries, cancellation, partial failure, and teardown.
- Keep notification, wake capability, diagnostic identity, and observation
  distinct from ownership transfer unless the protocol explicitly makes them
  authoritative.
- Ask whether caller-visible phases represent real domain or correctness
  boundaries, or merely expose lock and implementation mechanics.
- Ensure every acquired resource, registration, publication, and retained
  reference has an owner on success, failure, cancellation, and teardown paths.

### Protocol tax and directness

For every phase, carrier, registration, acknowledgement, retry path, retirement
state, compatibility bridge, or forwarding layer:

- map it to a concrete race, lifetime transition, external contract, failure
  path, or proven reuse;
- ask whether a later and more local decision point would remove rollback,
  re-drive, or cross-lifecycle state;
- check whether one consumer's special case has been generalized without
  another real consumer or proof benefit;
- compare it with the simplest direct design that preserves every identified
  obligation.

Complexity without a named obligation is evidence to investigate, not an
automatic defect. Conversely, a simpler sketch without a race, failure, and
lifecycle argument is not yet a demonstrated repair.

### Boundaries, failure, and observability

- Validate untrusted input before irreversible effects unless the external
  contract requires another order.
- Keep external representations and compatibility policy at the boundary;
  translate them into internal domain concepts before core logic when that
  reduces coupling.
- Distinguish invalid input, unsupported behavior, permission failure, resource
  exhaustion, and internal invariant failure. Do not convert impossible
  internal states into success or a silent no-op.
- Require temporary bridges and fallback paths to state their purpose, visible
  behavior, observability, and removal condition.
- Check whether rare failures can be connected to the relevant input, object
  identity, state transition, and result without adding noisy hot-path logging.
- Match evidence to the claim: focused tests for local behavior, broader tests
  for shared contracts, and explicit gaps where the available evidence cannot
  prove the stated scope.

## Engineering Compromise

Engineering cost may justify proposing a weaker product target, but never
silent degradation. Keep these categories distinct:

- **Correctness invariant:** ownership, concurrency, lifecycle, cleanup,
  safety, and honesty properties that cannot be traded away merely to finish
  the task.
- **Target guarantee or capability:** promised behavior that may change only
  through an explicit decision by the appropriate authority.
- **Implementation preference:** internal types, helpers, algorithms, and file
  layout that may change inside the protected boundary.

When the original target becomes disproportionately costly, report the
evidence, affected guarantees, completed work and its disposition, viable
alternatives, validation impact, and the decision required. Do not present a
reduced implementation as completion before the target is actually revised.

## Finding Levels

Use the following levels and include the plain-language meaning so
collaborators need not know the names in advance:

- **Apollyon — blocking correctness:** wrong results, corruption, security
  failure, crashes, or severe unrecoverable state. Must fix before completion.
- **Keter — blocking architecture:** current evidence shows confused ownership,
  leaked private representation, unauditable lifecycle or protocol, or a
  structure that blocks a named current or accepted next step. Must fix or
  obtain an explicit boundary decision.
- **Euclid — non-blocking engineering debt:** concrete coupling, awkward
  structure, mediocre naming, or test friction worth fixing, but the current
  main path remains sound.
- **Safe — optional:** preference, speculative future concern, or theoretical
  purity. Record only when requested or when a cheap local fix is already in
  scope.

Do not promote speculation into Keter. A Keter finding must name the current
code path, ownership or truth-source conflict, leaked boundary, unauditable
lifecycle, or blocked named path. Stop searching for more findings once the
remaining observations are Safe unless the user explicitly asks for polish.

## Output

Lead with findings ordered by severity. For each finding include:

1. affected location or code path;
2. concrete observed evidence;
3. the intended owner, state, lifecycle, or protocol model;
4. the model the code actually expresses;
5. current impact or named path at risk;
6. the smallest credible repair boundary;
7. missing evidence when the conclusion is not yet confirmed.

Separate confirmed findings from questions, assumptions, and optional cleanup.
When there is no blocking issue, say so explicitly and identify residual test
or evidence risk.
