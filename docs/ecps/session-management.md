# Session management and session-owned policy

Status: agreed design; implementation and cutover pending.

Decision date: 2026-09-06.

Authorization: the project owner agreed to introduce session management after
assessment and an ECP. The latest request authorizes writing this document;
this step changes no implementation or effective contract. The decisions below
record the subsequent scope refinements. Implementation may adapt local types
and module layout within these boundaries; expanding behavior or moving an
owner boundary beyond them requires an amended decision.

## Baseline and evidence

Baseline provenance: `2485eca40dad58c52afba5a76fef86f0b7a36366`.

- `src/session/` owns committed conversation, reasoning effort, and usage through
  `SessionStore`. Its memory implementation retains multiple sessions in a map,
  with process-local incrementing integer IDs, but exposes no listing or deletion.
- `src/kernel/` owns the active session ID. It creates a session at startup and
  can create-and-activate or clear, but cannot select an existing session.
  It awaits an entire run before consuming another command.
- `src/tool/mod.rs` combines registered implementations and definitions with a
  mutable enabled flag. That flag currently governs both model exposure and
  invocation for the whole kernel, so it cannot express independent session
  choices without changing its ownership.
- `src/agent_loop/runtime.rs` binds each run to one session ID, snapshots input
  and definitions, and invokes tools through the registry. It commits user
  input before inference and model output plus usage before executing calls.
- `src/config.rs` reads startup configuration and credentials once. `main`
  constructs one backend, model selection, loop, context strategy, and registry.
- The TUI exposes `/new` and `/clear`, suppresses submission during a pending
  request, and displays progress in terminal scrollback. It does not own a
  replayable session transcript. One-shot `chat` always starts a fresh session.

## Target and non-goals

Provide a normal session lifecycle: create, list, inspect, switch, clear, and
delete, with a memory repository as the only implementation in this change.
Callers use the same domain operations regardless of storage implementation;
they must not branch on memory versus durable storage.

In this proposal, a **turn** is one user submission running an entire agent
loop, corresponding to the current kernel run. A turn may contain multiple
model responses and tool rounds.

The scope excludes durable storage, import/export, cross-process resume or
sharing, session names, automatic titles, branching, undo, a recycle bin,
tombstones, prune/GC commands, and transcript-browser or terminal-replay work.
There is no compatibility requirement for the old session commands or integer
IDs. No dormant persistence implementation or serialization framework is needed.

Model/provider selection remains fixed at startup, as do loop and context
strategy, including the current loop limit. Dynamic model switching, backend
resolution, credential switching, and alternate loop/context implementations
are outside scope. Provider-side conversation state, epochs, and backend-specific
shortcuts are also excluded; committed local history remains authoritative and
provider continuation remains opaque to core code.

## Owners and interfaces

| Component | Authority and responsibility |
| --- | --- |
| Session repository | Owns the session collection, entity lifetime, committed history, usage, and session configuration; performs storage operations and releases owned records. |
| Session entity | Groups one session's data and local invariants inside the repository; it is not a second independently writable copy held by kernel or frontend. |
| Kernel | Serializes commands, owns the active ID, validates activation/deletion policy, and binds a turn to its session and execution inputs. |
| Tool registry | Owns registered implementations and their immutable definitions; has no session enabled state. |
| Turn tool capability | Provides definitions and permitted invocation from the same fixed selection for one turn. |
| Frontends | Parse requests and present replies/snapshots; own neither active-session authority nor executable tool selection. |

Keep the repository interface synchronous for this change. It exposes collection
operations and narrow operations on a specified session, such as snapshot,
append, inference commit, clear, and policy changes. It does not select the
active session. A public owning `Session` object, generic mutation language,
checkout protocol, or extra manager layer is not required merely to separate
the collection and entity concepts. Exact type names may evolve from
`SessionStore` without compatibility wrappers.

Snapshots and summaries are owned read projections. Their authoritative source
is the repository; they may become stale after later commands. The kernel takes
fresh turn inputs, and runtime refreshes its history projection after successful
commits. Presentation snapshots cannot authorize mutation or replace committed
history. Marking the active row is a projection of the kernel-owned active ID,
not an active flag stored in each session.

A list summary may include a short `brief` derived from committed user content,
along with counts and usage. `brief` is display-only diagnostic data: it is not
an identifier, does not need to be unique, and cannot drive switching, deletion,
routing, permissions, or persistence policy. Empty sessions use a fixed empty
label. If a backend caches a brief, history remains authoritative and the
repository refreshes or invalidates the cache on every history mutation; the
frontend never writes a brief back as a second source of truth. Do not call this
field a name or title until editable identity and its uniqueness semantics are
actually introduced.

## Identity and lifecycle

Use an opaque `SessionId` backed by UUID v4, with a full textual UUID at the
frontend boundary. Generate a fresh ID for each creation and check that insertion
does not overwrite a currently existing session. Do not deliberately recycle
deleted IDs or retain all historical IDs. Random UUID uniqueness is an accepted
practical guarantee, not proof against every historical collision. IDs identify
entities; they do not imply creation order or authorization.

The lifecycle decisions are:

- Startup always creates and activates a new session. Even a future repository
  containing older sessions must not implicitly resume one.
- Creation copies the startup session defaults and activates the new session
  only after repository creation succeeds. There is no inheritance from the
  currently active session.
- Listing includes all logical sessions retained by the repository, including
  those with no interaction records. Inspection reports at least identity,
  session policy, history size, and usage. Neither operation requires persistence
  or a network token-count request.
- Switching validates that the target exists before replacing the active ID.
  It neither copies history nor executes inference. Failure leaves the active
  ID unchanged; selecting the current ID can succeed without a state change.
- Clear removes history and usage together, preserving identity and session
  configuration. It does not delete the session or reset it to startup defaults.
- Delete is allowed only for a non-active session. Success immediately removes
  it from repository lookup and listing and releases repository-owned data.
  Later reads or switches fail as not found. There is no logical-delete state,
  recovery window, or cleanup command. Backend space reclamation is an internal
  responsibility; deletion does not promise secure erasure of storage media.

A logical session may exist without any durable record. Creation and settings
changes alone must not require persistence of a never-used conversation. The
first committed interaction, including an accepted user prompt whose inference
later fails, supplies conversation data eligible for storage. The memory backend
simply retains the logical entity; no materialization flag or parallel state
machine is needed for it. Empty sessions remain selectable within the process
until explicitly deleted or repository teardown. Clear must not silently remove
their identity or visibility.

This defines the boundary for future deferred persistence, not a claim that
durability, file creation, or crash recovery is validated here. A later backend
must define its physical write and failure guarantees without making callers
manage loading, saving, or storage-specific lifecycle states.

## Session configuration and turn tools

`config.toml` is read once at startup. Its strategy values initialize an
immutable set of new-session defaults or construct fixed runtime components.
It is not reread during turns, mutated by session commands, or written back.
No mutable global preferences object, configuration lock, or preferences file
is introduced. Runtime edits affect only the selected session in process memory;
new sessions still use the startup defaults, and restart rereads the file.

Session configuration in this change contains reasoning effort and the enabled
tool selection. Model/provider, loop, and context strategy remain fixed runtime
inputs rather than duplicated mutable session fields. This is the explicit
current capability limit, not a competing session override path. Supporting
dynamic selection later requires real variants and a new ownership decision;
do not add unused selector fields now.

Keep tool implementations centrally registered for the runtime. Move enabled
selection into the repository-owned session configuration. Validate requested
tool identities against registration before accepting a selection change;
invalid changes fail without partially changing session policy.

At turn start, resolve the session selection into a narrow immutable capability
for both definitions and invocation. Runtime cannot expose one selection to the
model and then invoke against unrestricted registry access or another session's
selection. Unknown or unselected calls must produce visible tool errors without
executing. A snapshot may share implementation references; it does not create a
per-session registry or claim to isolate mutable state inside arbitrary tools.

Tool listing combines registered metadata with the chosen session's selection.
Context inspection uses that same selection when preparing model-visible tool
definitions. Neither consumer maintains a writable enabled mirror. Preserve
existing argument validation, result encoding, output observation, and tool
error behavior. Broader tool subsystem redesign is outside this change.

## Sequencing, failure, and cleanup

Retain the kernel's single command sequencer. A switch request may enqueue while
a turn runs, but takes effect only when the preceding turn returns. Do not add
Busy admission checks, a second running-state authority, or concurrent management
execution. TUI submission remains disabled while its request is pending.

A run binds to the active ID when execution starts, not when a caller constructs
or enqueues its request. Its commits always target that ID. Queue order
`Run, Switch(B), Run` therefore executes the first run on the prior session and,
if switching succeeds, the second on B. Separate switch and run commands are not
a transaction across multiple callers. Successful switch acknowledgement means
activation has happened, not merely that the request was accepted.

Repository mutations have one commit point. In-memory failure must not leave
partial history/usage, policy, or collection changes. Kernel acknowledges create
or switch only after the repository operation/validation and active-ID update;
it rejects active-session deletion before calling repository deletion. No
frontend owns rollback. If future storage cannot preserve these observable
outcomes, its failure contract requires a separate decision rather than an
in-memory fallback that reports success.

Preserve existing turn commit ordering: accept the user prompt into history
before network work, commit validated inference output and usage together, and
only then expose executable calls. A commit error stops that run and remains
visible; earlier commits remain. Do not retry tools, discard accepted history,
or promise rollback of external tool effects to accommodate storage errors.
This change does not alter the existing tool-round commit granularity.

Successful enqueue still transfers work to kernel; detached replies do not
cancel mutations or execution. Shutdown remains an ordered barrier followed by
joining the kernel task. Turn-scoped snapshots and capabilities are released
on return/error; the kernel-owned repository and registry are released at task
teardown. No background save operation or independently retained session owner
is introduced. Observer backpressure and interruption retain their current
semantics.

## Frontend boundary

Use one formal TUI command family:

```text
/session new
/session list
/session info
/session switch <ID>
/session clear
/session delete <ID>
```

`info` and `clear` apply to the active session. List identifies the active row,
and creation/switch replies identify the resulting active session. Parse full
UUIDs at the command boundary; malformed IDs and absent targets fail visibly.
Remove `/new` and `/clear` without aliases. No name resolution or ID-prefix
matching is required. Existing model-effort and tool commands operate on the
active session policy through kernel commands.

Switching changes the target of subsequent work; it does not rewrite terminal
scrollback or replay old progress. Creating a session has the same behavior: it
does not clear the terminal or replay any history. The terminal is a continuous
work log rather than a complete view of the active session. Command replies must
identify the resulting active ID, and list output must mark it, so users can
verify the target despite the absence of replay. One-shot `chat` retains its
fresh-session, single-turn behavior without new resume or session-selection
options.

## Acceptance evidence

Before semantic cutover, establish:

1. **Repository:** UUID parse/display behavior and collision-safe insertion;
   create/list/inspect/delete; empty logical sessions remain visible; failed
   operations leave state unchanged; clear preserves configuration and identity
   while removing history and usage together. Do not claim that finite ID tests
   prove global uniqueness or that memory tests prove future durability.
2. **Session isolation:** populate sessions A and B with different effort and
   tool choices, switch repeatedly, and demonstrate distinct model inputs,
   permitted invocation, history, and usage. A new session uses startup defaults
   even after the active session's settings change.
3. **Kernel sequencing:** hold a run in progress and enqueue a switch; demonstrate
   that it cannot complete before the run returns and all original commits stay
   on its bound session. Cover run failure followed by queued switching, invalid
   targets, active deletion rejection, and post-delete lookup failure. Preserve
   accepted-work, observer, and shutdown evidence.
4. **Tools and configuration:** exposure, actual dispatch, tool listing, and
   context inspection agree with the session selection. Unknown/unselected calls
   do not invoke implementations. Verify invalid policy changes do not partially
   apply, the configuration file is untouched, and runtime changes are not
   inherited by a new session or a restarted process.
5. **Frontends:** parser/help and relevant PTY coverage exercise the formal
   command family, full IDs, active-session feedback, and visible failures;
   obsolete aliases are rejected. Preserve one-shot CLI and terminal progress
   behavior without claiming a transcript browser.

Use focused inline owner tests plus kernel and frontend evidence, with scripted
backends or local HTTP fixtures for observable request inputs. No live provider
probe is necessary for this storage/ownership change. Run relevant checks and
the repository's recommended local suite for the eventual implementation;
record commands, results, and any evidence limits at cutover.

## Stop conditions and cutover

Stop and amend the decision if implementation requires a second mutable session
copy, registry-owned enabled state, unrestricted dispatch bypassing the turn's
selection, frontend-owned activation, or private storage/locks leaking across
boundaries. Also stop for concurrent turns or immediate switching during a run,
configuration writeback, provider/model switching, durable-storage claims,
tombstones/GC, or other scope expansion beyond this target.

Before claiming completion, inspect the actual diff and directly affected paths
under the project's architecture-friction rules. Resolve blocking ownership,
failure, lifecycle, or correctness gaps; report concrete residual engineering
debt with its smallest repair boundary. Do not weaken acceptance to obtain a
passing result.

After implementation and validation, update affected architecture, limitations,
and command documentation, and add the small effective session contract under
`docs/contracts/session/` for shared lifecycle and ownership rules. Keep private
type layout and implementation mechanics in code. Record implementation
revisions, acceptance results, scope limits, and cutover outcome here before
closing this ECP. Until then, existing effective contracts remain unchanged.
