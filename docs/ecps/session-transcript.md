# Session-owned semantic transcript and TUI replay

Status: closed; cutover complete.

Decision date: 2026-09-06.

Authorization: the project owner agreed to implement this ECP after assessing
session switching and TUI replay. This document records the decision and
cutover evidence; it does not authorize unrelated scope.

## Baseline and motivation

Baseline provenance: `aff373907ae575e976d769d6d94a4323a2232392`.

The session repository owns provider-oriented conversation items, including
opaque continuation items and function-call outputs. The agent loop exposes
live run events, but it does not retain a Minuet-owned replay history. The TUI
writes completed output directly to terminal scrollback, so it cannot clear
and reconstruct a selected session. Tool names, arguments, and model text are
available during a run but are not represented together as a stable display
history after the run completes.

Consequently `/session new`, `/session switch`, and `/clear` cannot redraw a
session view, and a switch cannot replay prior user, model, and tool activity.
Provider wire data is not a suitable UI history: it is opaque, provider
specific, and may not contain the display semantics Minuet needs.

## Target and non-goals

Introduce a session-owned semantic transcript as the authoritative history for
display and replay. Its entries are Minuet domain objects, separate from the
provider context history used to construct the next inference request. The
first consumer is the TUI.

The initial transcript contains user messages, model messages, and tool
invocations. A tool invocation records its name, arguments, and a typed
execution state: pending, completed with output, failed with an error, or
skipped with a reason. Provider call IDs do not cross the protocol boundary. A
tool invocation is replayed as one grouped semantic entry; replay does not
claim to reproduce live delta timing or interleaving.

`/session new` creates and activates an empty session and clears the TUI view.
`/session switch` activates the validated target, clears the current view, and
replays the target transcript. `/clear` and `/session clear` preserve session
identity and configuration while clearing provider context, transcript, and
usage.
Replay never re-runs inference, tools, or shell commands. It restores semantic
order and content, not terminal control codes, animation, cursor positions, or
timing.

This change does not add persistence, migration, pagination, cross-process
resume, a full-screen transcript browser, user shell commands, new provider
protocols, or a generic event-sourcing framework. Existing provider context
history remains opaque and is not reconstructed from transcript entries.

## Ownership and handoff

| Owner | Responsibility |
| --- | --- |
| Session repository | Owns transcript and provider context, their lifecycle, read views, and clear/append/update operations |
| Agent loop | Produces semantic transcript mutations at model and tool commit points; owns execution policy |
| Kernel | Serializes session mutations, validates activation, and publishes owned read results to callers |
| TUI | Owns only terminal clearing, layout, replay rendering, and temporary preview state |

The repository remains the only writable owner of transcript state. The public
repository contract does not expose memory-specific slot locators or provider
call IDs; each backend owns its pending-round bookkeeping. A TUI read view is
immutable and owned by the caller; it is not a capability to mutate a session.
Provider context and transcript are distinct facts with distinct contracts,
even though both are session-owned.

Accepted user input is recorded before network work. A validated model turn
records its displayable text and tool invocations before tool execution is
exposed. A tool invocation is first pending and then advances to completed,
failed, or skipped through a repository-owned operation. Provider context keeps
its existing commit ordering and opaque continuation data. If a later commit
fails, the error remains visible and earlier committed records remain; no
frontend performs rollback.

## Acceptance evidence and stop conditions

Before cutover, establish:

1. Transcript types prevent invalid tool-result/status combinations and do not
   expose provider call IDs.
2. Session isolation tests show distinct transcript and context history for
   multiple sessions; failed and skipped tool paths remain replayable.
3. Repository and kernel tests prove ordered mutation, immutable reads,
   unchanged state after rejected operations, and clear semantics.
4. TUI tests or terminal evidence prove new, switch, and clear clear the
   previous viewport, replay the selected transcript in order, and never
   execute replayed operations.
5. Existing inference context preparation and provider wire behavior remain
   unchanged except for the explicitly defined transcript handoff.

Stop and amend this ECP if implementation requires provider-specific decoding
in the TUI, a second writable transcript owner, persistence or migration
behavior, concurrent session mutation, or a new shell-command surface.

At cutover, the affected lifecycle, architecture, and limitations documents
were updated. The implementation is now the effective behavior; this closed
ECP remains the decision record.

## Cutover record

Validation on 2026-09-06 passed:

- `nix develop -c cargo fmt --all`
- `nix develop -c cargo check`
- `nix develop -c cargo clippy --locked --all-targets --all-features -- -D warnings`
- `nix develop -c cargo test --locked --all-targets --all-features`

The live MiniMax integration test remains ignored because it requires a
credential. The terminal suite includes PTY evidence for successful and failed
tool calls, new/switch/clear redraws, Markdown replay, invalid switch
preserving the current view, native scrollback purge requests, and no tool
execution during replay.
