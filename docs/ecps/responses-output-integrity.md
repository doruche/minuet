# Responses stream output integrity and finalized-item compatibility

Status: accepted; implementation authorized, cutover pending.

Decision date: 2026-09-06.

Authorization: the project owner agreed to the assessment and authorized this
ECP, then authorized implementation, appropriate commits, and one independent
subagent reviewer. Local implementation boundaries and design may adapt to
evidence within the target. Scope expansion or a distorted target requires a
renewed decision and ECP amendment. Cutover remains subject to the acceptance
below; proposed behavior is not yet an effective contract.

## Baseline and evidence

Baseline provenance: `6fde8e37fbf06f0482181efe10e2a69318986205`.

The Responses adapter sends streaming generation requests, previews text
deltas, and derives the final inference result exclusively from
`response.completed.response.output`. It ignores `response.output_item.done`.
The runtime commits the returned output and usage before exposing tool calls.
The TUI's provisional text cannot replace the committed result.

A synthetic text probe on 2026-09-06 against the locally configured `cch`
provider with `gpt-5.6-luna` returned HTTP 200 and this event structure:

```text
response.output_text.delta: delta = "OK"
response.output_item.done: output_index = 0, completed message with text "OK"
response.completed: status = "completed", output = [], nonzero usage
```

The Minuet one-shot command also exited successfully with empty stdout. A local
HTTP comparison reproduced that result with the sequence above; changing only
the terminal output to contain the message produced `OK`. This establishes a
lost-output path, not a terminal rendering or credential failure.

The successful raw request did not impersonate Codex. An earlier Python probe
received HTTP 403 for an undetermined reason, and another probe timed out;
neither establishes the cause of the successful stream's empty terminal output.
These observations do not establish every gateway access policy. Live tool
calls, reasoning continuation, and token counting on this relay remain unproven.
No credentials or private conversation content belong in retained fixtures.

The [official Responses event reference](https://developers.openai.com/api/reference/resources/responses/streaming-events)
defines item completion separately from response completion and describes using
finalized reasoning items for subsequent requests. The empty-terminal-output
form observed here is an explicit Minuet compatibility target, not a claim that
all Responses providers produce it or that it proves full API compatibility.

The closed [streaming ECP](responses-streaming.md) deliberately required a new
source and reconciliation decision before reconstruction from finalized items.
This proposal supplies that decision without rewriting the historical record.

## Target and protected boundaries

Accept both a complete terminal output array and a successful terminal response
with an empty output array backed by a complete set of finalized output items.
One adapter-owned adjudication produces one final `InferenceResponse` from the
validated stream. Preserve item order, opaque continuation, tool-call identity,
and reported usage; never silently drop observed output to obtain success.

The compatibility rule is based on protocol evidence, not provider names,
domains, model names, client impersonation, or frontend choice. It is intended
to become a supported input form at validated cutover, not a temporary fallback
with an indefinite removal date. Retiring or broadening it requires an explicit
compatibility decision and corresponding contract change.

Keep these existing boundaries:

- Generation remains stream-only, with no automatic retry, non-streaming
  fallback, reconnect, or speculative tool execution.
- The inference capability and backend construction interface remain stable.
  Provider wire data stays in the adapter; continuation remains opaque to core
  consumers.
- Kernel/runtime execution owns the commit handoff; session owns committed
  history and usage; loop policy owns subsequent inference and tool sequencing.
- Observers and TUI previews are presentation projections, never input to result
  adjudication or a source of executable tool calls.
- Input-token counting remains a separate operation. Text-generation success
  does not imply that the provider implements token counting.
- Existing shutdown, observer detachment, and accepted-work semantics remain.

Additional provider protocols, a generic streaming framework, a provider
registry, configuration switches for guessed dialects, persistent protocol
traces, reasoning display, and new cancellation semantics are outside scope.

## Result source and reconciliation

The adapter owns a request-local collection of finalized items and the evidence
needed to associate observed item activity with them. These are provisional
protocol materials, not a second conversation or independently publishable
result. Text deltas remain previews; the adapter does not build an alternative
answer or executable arguments by concatenating them.

The final source is selected only after validating response success:

| Observed form | Result rule |
| --- | --- |
| Terminal output is complete and consistent with observed item evidence | Use the terminal output, retaining its full continuation data. Item events are not required when the terminal output itself supplies the complete result. |
| Terminal output is explicitly `[]`, and a nonempty complete set of finalized items is available | Reconstruct in `output_index` order from those complete items. |
| Terminal output is empty but observed text or item activity lacks complete finalized items | Fail with an output-integrity error. Do not promote previews. |
| Terminal output contains only part of the observed items, or sources substantively conflict | Fail with a protocol-consistency error. Do not concatenate arrays or silently choose a conflicting source. |
| Terminal `output` is missing, null, or not an array | Fail as malformed; the empty-array compatibility rule does not cover these forms. |
| No output activity was observed and terminal output is `[]` | Preserve legitimate empty-result behavior; missing visible text alone is not evidence of truncation. |

Reconstruction requires valid, nonnegative output indexes forming a contiguous
sequence from zero. Every observed started item must have a finalized item;
identities and indexes must agree where supplied. Reject duplicate item
finalization, conflicting identity reuse, and indexes that exceed the declared
resource limits. A complete `done` item may establish an item without an earlier
`added` event. Do not require optional diagnostic metadata merely to recognize
an otherwise supported terminal-only result. Evidence insufficient to associate
partial activity with a complete item cannot authorize reconstruction.

Reconciliation must distinguish completion enrichment from substantive change.
Item type, identity, message content order and text, refusals, tool name,
`call_id`, arguments, and opaque replay data cannot silently contradict or
disappear between finalized and terminal representations. A final response may
add continuation data, including `reasoning.encrypted_content` absent or null
in the finalized item; retain that enriched terminal item. Conflicting nonempty
continuation values are errors. Extra diagnostic metadata alone need not cause
failure, but fields must not be classified as diagnostic if replay or behavior
depends on them.

Implement field-specific reconciliation for the supported output forms rather
than generic recursive JSON merging or blanket raw-JSON equality. Preserve
unknown continuation fields without interpreting them in core code; a
provider deviation requiring their reconciliation to be guessed is a stop
condition, not permission to drop them. The exact local helpers may vary, but
the comparison rules and accepted enrichments must be reviewable and tested.

Refusal content in a completed message must be visible through the existing
text projection, in content order, while the original item remains intact for
replay. A model refusal is not an HTTP authorization failure. Tool-only or
reasoning-only output does not become malformed solely because it has no
assistant body text; this proposal does not introduce a new loop stop policy.

## Success, failure, ownership, and cleanup

An individual text, argument, or output-item `done` event is never whole-response
success. Require a valid `response.completed` with completed status and no
contradictory failure data. Failed, incomplete, malformed, interrupted, and
timed-out streams remain failures even if all observed items were finalized.
`[DONE]` alone cannot authorize success.

Keep the existing requirement for clean stream exhaustion after complete SSE
framing. Reject duplicate response terminal events and output events after a
terminal event; trailing SSE comments and the transport's `[DONE]` marker do
not constitute additional output. A late transport or framing failure before
exhaustion prevents commit. Early return at a terminal event is not part of
this cutover. Unknown extension events may remain ignorable where they do not
replace required completion evidence.

The adapter is the sole owner of event validation and final source selection.
Output indexes and supplied identities used for association are protocol state,
not diagnostic labels. One successful return transfers the immutable result
to runtime. Runtime commits output and usage before publishing the committed
turn or exposing tool calls; finalized items cannot bypass that handoff.

Usage comes from the validated terminal response and is recorded once with
the commit. Preserve absent/null usage as unreported rather than synthesizing
counts from events. On failure, commit neither this turn's output nor its tool
calls or successful-inference usage; the accepted user prompt and earlier
commits remain. Preserve the existing TUI treatment of failed partial previews.

The adapter owns the HTTP body, decoder buffers, and provisional item storage
through success, error, timeout, and inference-future teardown. They remain
scoped to the request; no detached task may retain them. A detached observer
does not cancel inference, and backpressure must not become protocol authority.

Preserve finite request and observer deadlines. Bound frame storage as well as
accumulated finalized-item bytes, item count, and association state. Avoid
allocation proportional to an unchecked remote index. Limit violations are
observable errors, not silent truncation. Concrete limits and accounting must
be stated with validation before cutover; a per-event limit alone does not
bound a stream containing many completed items.

Diagnostics must distinguish HTTP rejection, upstream failed/incomplete status,
missing success, missing finalized output, source conflict, framing failure,
and resource exhaustion. Preserve available upstream error codes and incomplete
reasons. Include bounded event/index/count context where useful without logging
credentials, full requests, or opaque reasoning data by default. This does not
require a new public error taxonomy or general tracing subsystem.

## Adapter structure

The current adapter mixes HTTP lifetime, SSE byte framing, event interpretation,
and wire conversion. Finalized-item reconciliation adds a distinct protocol
obligation whose tests should not require a network server or TUI. A same-owner
split is justified by those responsibilities, not by file length:

```text
src/inference/openai_responses/
  mod.rs       backend, HTTP lifetime, deadlines, observer delivery, error surface
  sse.rs       byte framing, line endings, UTF-8, data fields, frame limits
  stream.rs    Responses events, finalized items, terminal validation, adjudication
  wire.rs      request encoding, output projection, usage, opaque continuation
```

Keep `sse` independent of Responses semantics and tools. Keep `stream`
independent of network access, session storage, and terminal rendering. The
backend drives decoding and observation through narrow private interfaces;
there is no new subsystem owner or public facade for internal storage. Preserve
existing public imports and keep child visibility as narrow as practical.
Unit tests remain inline with their owner according to
[the coding style](../../CODING_STYLE.md).

The structural move must be separately reviewable as behavior-preserving.
Moving files alone does not satisfy compatibility acceptance or authorize a
semantic cutover. Additional wrappers or layers require a concrete obligation
within the agreed target.

## Acceptance evidence

Baseline CLI and terminal HTTP fixtures primarily emit text deltas followed by
a populated terminal response. They do not establish reconstruction from item
completion. Preserve their frontend coverage while adding evidence for the
actual compatibility target.

Before cutover, establish:

1. **Protocol:** both supported result forms; multiple items and content parts;
   interleaved indexes; missing, duplicate, or conflicting completion; partial
   terminal arrays; valid enrichment and conflicting continuation data;
   refusal projection; legitimate empty and tool-only output; absent/null usage.
2. **Transport and limits:** arbitrary byte boundaries, split UTF-8 and CRLF,
   SSE comments and multiline data, malformed JSON/framing, frame limits,
   accumulated-output and item limits, oversized indexes, terminal ordering,
   and stream exhaustion. Ordinary JSON generation success remains unsupported.
3. **Failure and lifecycle:** partial text or a finalized tool call followed by
   failed/incomplete status, malformed data, EOF without success, or timeout
   does not commit or execute. Earlier commits survive. Observer detachment,
   backpressure, and scoped response cleanup retain their existing semantics.
4. **Runtime and frontends:** reconstructed tools execute exactly once and their
   results replay with intact identities; usage commits once; subsequent turns
   receive all continuation data; CLI and TUI use the same final result without
   duplicate publication; previews remain provisional on failure.
5. **Live compatibility:** small synthetic relay probes for text, a harmless
   tool call and result replay, and multi-turn reasoning continuation where the
   provider exposes it. Revalidate the existing MiniMax text/tool/replay path.
   Report input-token counting independently. Record what was observed, including
   absent reasoning data; a text probe alone does not establish continuation or
   full provider compatibility.

Use focused inline tests for decoder and adjudication behavior, runtime tests
for commit and execution invariants, and HTTP/PTY evidence for external timing
and presentation. Source review may establish scoped cleanup and dependency
direction where a synthetic test would add production hooks solely for testing.
Retain sanitized representative fixtures and record provider/model, validation
revision, outcomes, and limits without credentials. Official schema conformance
and relay/MiniMax evidence do not establish a live test against OpenAI itself.

Run the repository checks relevant to the eventual implementation, and the
recommended local suite before submitting that change. Do not substitute
provider-specific success fixtures for failure coverage or weaken an oracle
to accommodate an unsupported stream.

## Stop conditions and cutover

Stop and resolve the decision before cutover if the implementation requires:

- Competing result authorities, committing previews, or executing tools before
  validated response success and session commit.
- Silent loss of text, calls, refusal, usage provenance, or continuation data;
  a guessed merge; or treating incomplete output as success.
- Provider-name/client-identity branches, implicit request retries, a new
  transport fallback, or a broader protocol compatibility promise.
- Wire state leaking into runtime/TUI, public interface growth to expose
  internal storage, unowned cleanup, or unbounded buffering/waiting.
- Changing completion/exhaustion, cancellation, or loop policy beyond the
  decisions above, or acceptance evidence that cannot prove the claimed scope.

At validated cutover, inspect
the actual diff and directly affected paths for architecture friction under
the project guidance. Resolve blocking correctness/ownership findings and
report any residual concrete engineering debt with its repair boundary.

Update only affected effective contracts and current-limitations documentation
after implementation and validation. A small inference contract may record
the durable supported result forms, final authority, commit boundary, and
failure behavior; private decoder and file-layout details belong in code.
Keep this ECP as provenance, with implementation revisions, acceptance evidence,
remaining scope limits, and cutover outcome. Do not mark it closed solely because
the adapter has been split or the initial text reproduction passes.
