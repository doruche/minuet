# Ordered model output and session commit handoff

Status: closed; implementation and validated cutover complete.

Proposal date: 2026-09-07.

Authorization: the project owner authorized implementation, appropriate commits,
and one independent subagent reviewer on 2026-09-07. Local design and file
boundaries may adapt to evidence within the target. A scope expansion or a
distorted target requires a renewed decision and ECP amendment. The cutover
record below establishes the completed scope; unrelated work remains outside
this authorization.

## Baseline and evidence

Baseline provenance: `7f108931c27df20582ab3a2f0a4904404b787e63`.

The closed [transcript ECP](session-transcript.md) introduced session-owned
semantic history and TUI replay. The current ownership topology remains useful:
the adapter owns protocol conversion, session owns both histories, kernel
serializes commands, loop policy chooses execution steps, and TUI owns display.
This proposal corrects the handoffs between those owners without rewriting the
closed ECP or treating proposed behavior as an effective contract.

The live implementation exposes three concrete problems:

- `LoopContext::infer_and_commit` in
  [runtime.rs](../../src/agent_loop/runtime.rs) concatenates every text effect
  and separately collects tool calls. `CommittedModelTurn` and
  `RunEvent::ModelTurnCommitted` cannot represent message/call interleaving.
  Meanwhile, [memory.rs](../../src/session/memory.rs) creates transcript entries
  in output-item order. Live and replay therefore consume different semantic
  shapes from the same response.
- Live parses the concatenated turn text as Markdown; replay parses each
  stored model message separately. Multiple messages can acquire different
  formatting or link interpretation on replay. This is a document-boundary
  mismatch, not evidence that Minuet should repair model-authored Markdown.
- Runtime independently constructs its next-input items and tool activities,
  then derives repository outcomes from those activities. Repository separately
  constructs context outputs from those outcomes. The current copies agree,
  but committed history and the run-local input depend on parallel assembly.

The existing tool commit window is not evidence of ordinary cancellation
failure. Kernel-owned execution continues after caller/observer detachment,
and shutdown is queued behind the run. The memory repository validates the
whole tool round before publishing either history and performs no storage I/O.
Under the valid serialized call sequence, that commit should succeed.

## Target and protected boundaries

Preserve ordered model messages and tool-call requests through model commit,
live publication, and replay. Use repository commit results as the handoff for
committed presentation facts and executable requests. Keep one authoritative
provider context, and enforce pending-round obligations before network work.

Preserve these boundaries:

- Provider continuation remains opaque and retains its original order and
  content. Transcript is not used to reconstruct inference input.
- Session remains the sole writable owner of context, transcript, invocation
  terminal states, usage, and configuration. Kernel alone owns activation and
  command serialization; immutable presentation snapshots may become stale.
- Tools execute sequentially after the entire model response commits. Results
  still commit as one round, and step limits still explicitly skip pending
  calls. Model-turn limits and tool selection retain their existing meaning.
- Accepted-work, observer backpressure/detachment, inference deadlines, and
  shutdown behavior remain unchanged. Ordinary tool errors remain committable
  failed results, not repository or run-protocol failures.
- New/switch/clear/delete, session isolation, and replay without execution
  retain their existing lifecycle guarantees.
- CLI still prints the final turn's text summary, retaining its current
  ordered concatenation and exit behavior. That summary is explicitly a lossy
  projection and cannot define TUI message boundaries or transcript order.

The proposal permits focused changes to the Rust interfaces for model semantic
output, repository commits, loop capabilities, and run observation/results.
Update their in-repository consumers together; do not add an unused legacy
path to preserve the old flattened presentation model. Record actual public
API changes at implementation closeout. Provider wire compatibility and user
command grammar are protected; corrected live message boundaries and ordering
are intentional user-visible changes.

Persistence, crash recovery, automatic tool retries, per-tool result commits,
new cancellation behavior, a user-facing blocked-session state or recovery
through clear, event sourcing, a generic execution registry, transcript
pagination, and new provider protocols are outside scope.

## Message boundaries, order, and causality

One model turn is a validated inference response containing an ordered sequence
of semantic messages and call requests, alongside opaque continuation items.
The backend defines complete message boundaries at its protocol boundary.
For the current Responses adapter, each supported `message` output item yields
one complete model-message projection; its supported text/refusal content
retains the existing content-order projection. Empty/non-displayable items
remain in provider context without inventing visible text. Multiple message
items already belong to the supported response shape; core consumers must not
assume one message per response.

Use an explicitly message-shaped semantic type instead of an ambiguous text
fragment effect. Retain the useful association between each immutable opaque
continuation and its backend-produced semantic projection. Session consumes
that projection without decoding provider JSON. Stream deltas remain
provisional previews and do not define committed message boundaries.

Each complete message is rendered as its own Markdown document in both live
and replay. Do not merge distinct messages, including adjacent messages, to
resolve references or close code fences. Do not split a complete message based
on stream chunks. Model-authored Markdown is rendered using the existing
renderer, including its handling of incomplete syntax and terminal escaping;
this proposal adds no content repair or Markdown-validity gate.

For example:

```text
Model response and committed order: Message(A) -> Call(X) -> Message(B) -> Call(Y)
Subsequent execution order:         execute X -> execute Y -> commit result batch
```

Live publishes the ordered committed messages and call requests before their
execution observations. A call request is not evidence of execution at that
position. B was generated before X ran and cannot be presented as an answer
based on X's result. Replay may group a committed result under its invocation,
as today, without moving the original messages or implying that execution
occurred between those messages. Grouped replay is not a chronological
execution log.

## Owners and commit handoff

| Owner | Authority and handoff |
| --- | --- |
| Inference adapter | Validates the response and produces immutable ordered messages/calls paired with opaque continuations. |
| Session repository | Atomically records matching context, transcript, and usage; returns immutable committed presentation facts and confirmed call requests. Owns pending/result association and terminal-state validation. |
| LoopContext | Owns the execution lifetime of the current committed round, holds its immutable execution requests, invokes tools, and submits typed results. Publishes committed facts only after successful repository return. |
| Loop policy | Chooses inference, invocation, or explicit skipping through narrow context operations; does not own or transport a round for later reuse. |
| Kernel | Serializes all commands and owns the run's final completion check, reply, and shutdown sequencing. |
| TUI | Renders committed facts and temporary observations; cannot mutate histories or authorize execution. |

The model commit operation consumes the backend's ordered semantic projections
once to establish transcript entries and the corresponding invocation
association. Its returned presentation facts describe exactly what committed;
its confirmed requests are immutable inputs for execution. Runtime must not
independently rebuild a flattened presentation result or infer a different
call set. Concrete return types are implementation choices, not a reason to
introduce another owner or a coordinator layer.

User commits update context and transcript together. Model commits update
continuations, semantic entries, and usage together. Tool commits update
context outputs and invocation terminal states together. Validation precedes
publication, and a rejected operation changes neither side. Successful return
is the cross-owner commit handoff; observer delivery is not a commit point.
Repository-private slot locators and provider call IDs do not become transcript
fields. Consumers receive snapshots or requests, never repository storage.

Construct typed tool outcomes directly from execution results or explicit skip
decisions. Derive observation summaries from those results; do not convert
presentation status back into authoritative outcomes. Tool-protocol encoding
remains owned by the tool boundary, including the model-visible skipped error.

Remove the independently assembled `LoopContext::input` history. Before each
inference, obtain a repository-owned committed-context snapshot and pass it to
`ContextStrategy`. The snapshot is request-local and cannot be mutated into a
second conversation across steps. This accepts snapshot-copy cost for the
current full-context implementation. A retained incremental cache needs
concrete cost evidence and an amendment defining its authoritative delta,
refresh, and validation rules; independently rebuilding committed items is
not an acceptable optimization.

## Round lifetime, failure, and observation

Repository owns whether invocation results have committed. LoopContext owns
the outstanding execution obligation and its request lifetime; these are
different responsibilities, not two writable copies of invocation status.
The policy-facing API should act on the current round (for example,
`invoke_pending` or `skip_pending`) instead of accepting an externally retained
`PendingToolRound`. Invocation consumes the execution opportunity once; commit
failure does not restore permission to execute the same requests again.

Reject inference while a round remains unresolved before contacting the
backend. Reject invalid round operations before executing tools. At run return,
kernel checks the context's completion invariant through its owner API rather
than inspecting repository storage. An unresolved round must not become a
successful run. On a failing return, preserve the originating error and make
an outstanding-round violation observable without attempting implicit cleanup
by execution, retry, skipping, or clear. A subsequent run must not send an
unresolved context upstream or append new input before detecting that violation.

`Pending` continues to mean no committed result; it does not assert whether a
tool started, returned, or produced external effects. In the current memory
implementation, rejected tool commits on the valid serialized path indicate
an internal protocol defect. Report the error and retain earlier commits.
Do not disguise it as successful recovery or instruct users to clear as a
normal protocol step. Durable recovery and storage-failure handling require a
separate decision if a persistent repository is introduced.

Keep immediate tool execution observations. If X returns while Y is still
running, the observer can see X's outcome without waiting for the round's
commit. Names and documentation must distinguish execution completion from
result-batch commitment. Repository-confirmed terminal updates are published
only after successful commit; an observer cannot advance a stored invocation.
Ordinary tool failure yields a typed failed outcome and does not discard the
other results in the round.

Live and replay consume the same committed message semantics and rendering
rules. Tool progress, execution timing, and failed model drafts remain
temporary presentation data. The TUI must not publish committed model messages
again from `RunOutcome.text`, including the step-limit path. Any display state
needed to associate observations or avoid duplicate result publication is
run-local, cannot determine execution or commits, and is released on completion
or interface exit. Grouped replay uses only stored final results, never streamed
fragments or execution observations as replacement history.

Kernel still retains accepted work when callers detach. Closing observers
releases pending sends; queued shutdown waits for execution and resource
cleanup. Neither creates a normal partially committed round. Forced process
termination retains the existing absence of persistence and rollback promises.
Adapter request resources remain request-scoped, tools retain responsibility
for their external resources, and session-owned records live until clear,
delete, or process teardown.

## Acceptance evidence and cutover

Before cutover, establish:

1. Adapter and session tests preserve multiple complete messages, opaque items,
   and `Message(A) -> Call(X) -> Message(B) -> Call(Y)` in order. A terminal
   response and the existing finalized-item reconstruction path obey the same
   message contract without changing source adjudication or wire replay.
2. Live/replay terminal evidence proves identical committed message boundaries
   and message/call order. Cover adjacent messages, references split across
   separate messages, and an unclosed fence in one message that must not capture
   another message or a tool notice. Do not require matching animations or
   chronological placement of grouped results. Each committed message appears
   once in live output, including a text-bearing step-limit response.
3. Repository tests prove atomic matching commits, stable retained read views,
   correct association for multiple tool results, and unchanged histories
   after rejection. Successful, failed, and skipped results remain replayable;
   streamed fragments do not become committed outputs.
4. Gated tools prove invocation order and immediate execution observations:
   X's result is observable while Y is pending, without claiming the round has
   committed. Rejected batch commit publishes no committed terminal update and
   triggers no repeat execution or automatic skip/clear.
5. Inference with an outstanding round and duplicate round consumption fail
   before additional network or tool effects. Returning success with an
   unresolved round becomes an observable protocol failure, retaining earlier
   commits. Use bounded test-only fault injection for commit rejection; do not
   add dormant production recovery paths for tests.
6. Captured next-inference input equals the `ContextStrategy` projection of
   repository-committed history after success, failed tools, and skips, both
   within a run and on the next user turn. Later inference failure retains
   earlier commits and does not fabricate model messages or usage.
7. Existing session lifecycle, replay-without-execution, observer detachment,
   shutdown, CLI final-text/exit behavior, and terminal mode restoration remain
   covered. Run the repository's applicable formatter, check, lint, and test
   suite; record actual results and any unproven scope at cutover.

Inspect the actual diff and directly affected paths for architecture friction
before declaring completion. Update only affected effective contracts after
implementation and validation, including session lifecycle and any changed
inference/message rules. Add a small loop contract only for durable rules that
cross policy/runtime/session owners; do not catalogue local implementation.
Update architecture and limitations where the live publication description
changes. Close this ECP with evidence and actual interface changes; retain the
original transcript ECP as historical provenance.

Stop and amend the proposal if the target requires flattening messages or
reordering call requests, decoding provider JSON outside the adapter, exposing
repository storage, introducing another writable history or invocation owner,
weakening failure visibility, or changing a protected compatibility guarantee.
New persistence/recovery, concurrent execution or session mutation, per-tool
commit, cancellation, or a mandatory full-screen renderer also require a new
scope decision. Passing tests for single-message happy paths cannot substitute
for the ordered-output and failure evidence above.

## Cutover record

Cutover date: 2026-09-07.

The implementation retains the existing source-directory owners. No execution
registry, storage facade, full-screen renderer, compatibility bridge, or
incremental input cache was needed. `Screen::publish_entries` is the common
live/replay publication operation. The current whole-round execution protocol
allows tool-result display deduplication without a frontend state registry:
executed results arrive first as observations, while skipped results are shown
only on committed publication. This ordering obligation is now explicit in
[the loop contract](../contracts/agent-loop/execution.md).

Repository `ensure_ready` checks uncommitted results. Runtime's optional request
list represents only the unconsumed execution opportunity, and is taken before
invocation or skipping. It cannot be restored on failure. Kernel checks both
obligations through runtime completion; a failing policy return retains its
original error alongside any outstanding-round violation. The context snapshot
is refreshed from repository history for every inference, including selective
context strategies. No scope amendment or target reduction was necessary.

Actual Rust API changes:

- `OutputEffect::Text` becomes `OutputEffect::Message`, denoting a complete
  message rather than a stream fragment.
- `SessionRepository` adds `ensure_ready`; `commit_inference` returns
  `ModelCommit` with ordered entries and confirmed calls; `commit_tool_round`
  returns committed invocation snapshots and rejects absent rounds.
- `PendingToolRound`, `invoke_and_commit`, and `skip_and_commit` are removed.
  Policy uses `has_pending_tools`, `invoke_pending`, and `skip_pending` on the
  current context. `CommittedModelTurn` exposes ordered entries and an explicit
  lossy `text_summary()` instead of an executable round and flattened text.
- `ModelTurnCommitted` carries ordered entries. `ToolFinished` becomes
  `ToolExecutionFinished`; `ToolRoundCommitted` publishes confirmed results.
  Loop errors explicitly represent invalid operations and unfinished returns.
- `RunOutcome.text`, CLI output/exit behavior, provider wire formats, session
  commands, and kernel handle submission/shutdown semantics remain supported.

Acceptance evidence was checked against implementation and tests:

| Requirement | Evidence |
| --- | --- |
| Ordered adapter output and opaque continuation | `complete_and_reconstructed_turns_preserve_message_boundaries_and_call_order`, including out-of-order finalized-item arrivals. |
| Live/replay message boundaries, order, and one-time publication | `pty_live_and_replay_preserve_interleaving_and_independent_markdown_documents`; `pty_unclosed_model_code_cannot_capture_the_run_limit_notice`. |
| Atomic matching commits and immutable reads | Repository `semantic_order_and_results_share_reads_without_copying_payloads`, `invalid_rounds_cannot_partially_change_either_history`, and clear/skip lifecycle tests. |
| Execution observations versus commitment | Runtime `ordered_commit_and_execution_observations_have_distinct_handoffs`, with X observable while Y is gated; commit rejection tests cover both invocation and skipping. |
| Single-use execution and completion validation | Runtime round-operation, unfinished-run, and dropped-operation tests; kernel `kernel_rejects_policy_completion_with_unresolved_calls`. |
| Fresh provider input across rounds and runs | `every_inference_reads_repository_context_across_rounds_and_runs`, under full and selective context strategies, with success, failure, skip, repeated tool names and reused call IDs. Later inference failure preserves history and usage. |
| Protected frontend and lifecycle behavior | Full kernel, CLI, and PTY suites, including replay without execution, failed tools, new/switch/clear, detachment, shutdown and terminal restoration. |

Validation passed in the Nix development environment:

- `cargo fmt --all -- --check`
- `cargo check --locked --all-targets --all-features`
- `cargo clippy --locked --all-targets --all-features -- -D warnings`
- `cargo test --locked --all-targets --all-features`: 143 passed, 1 ignored.
- `git diff --check`

The ignored live MiniMax integration test was not run; this cutover claims
fixture-backed preservation of existing provider behavior, not new live-provider
compatibility. One independent subagent reviewed the actual code, tests, and
updated contracts using the owner-centered review criteria. Both that review
and the primary closeout found no remaining blocking or Euclid findings.
Session and inference contracts, architecture, and limitations were updated;
the original closed transcript ECP remains unchanged as historical provenance.
