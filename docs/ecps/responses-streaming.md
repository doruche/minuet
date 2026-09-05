# Stream-only Responses inference and live TUI previews

Status: closed; cutover complete.

Decision date: 2026-09-05.

Authorization: the project owner agreed to the target and behavioral decisions
below and subsequently authorized implementation, appropriate commits, and
subagent review. Implementation boundaries may adapt to evidence within this
target; scope expansion or a distorted target requires renewed agreement and
an ECP amendment. Cutover remains subject to the acceptance below.

## Baseline and motivation

Baseline provenance: `62e3fe025fad2c83896aef4486c2e077734a895c`.

Responses generation is non-streaming: the adapter buffers the HTTP response
and returns a complete result. The client has a 120-second total request
timeout. The inference interface has no incremental observation capability.

The runtime commits complete model output and usage before making its tool
calls available to loop policy. The session owns
committed history; an accepted user prompt remains committed if inference fails.
The kernel serializes commands and owns execution after enqueue. Dropping a
result future or progress receiver does not cancel accepted work.

Run observation already provides ordered, optional, bounded tool progress. A
full queue applies backpressure; closing the receiver releases pending sends.
The returned run result remains authoritative for overall outcome, and its text
contains only the last model turn's text.

The TUI displays tool fragments during execution but renders model text only
after the run returns. It parses the complete answer as Markdown to resolve
references across blocks, then publishes formatted blocks to native terminal
scrollback, which the application cannot subsequently rewrite.

The proposed change improves compatibility with streaming-capable Responses
providers and makes model output visible during generation. Maintaining one
generation transport reduces the compatibility surface. This is a Minuet
support-policy choice; it does not assert that OpenAI has removed non-streaming
or that every intermediary implements the same streaming dialect.

## Target and non-goals

All Responses generation requests use HTTP SSE with `stream: true`. There is no
non-streaming generation option, automatic fallback, or silent retry of a request
whose output may already have been observed. A provider that returns an ordinary
JSON success body instead of the requested stream is unsupported on this path.
JSON HTTP error bodies remain diagnosable.

Generation exposes incremental assistant body text and a complete authoritative
result. Retaining a final `InferenceResponse` does not retain non-streaming HTTP
support. Input-token counting remains a separate JSON operation. The one-shot
CLI continues printing only the final answer, while its backend uses streaming.

The TUI previews assistant body text from every model turn, including text before
tool calls. It publishes each committed turn once as complete Markdown. Tool
arguments are not a live execution capability. Reasoning continuation data is
retained and replayed opaquely; displaying reasoning content is outside scope.

This proposal does not add cancel-current-run, concurrent tool execution,
speculative tool execution, reconnect/resume, automatic generation retries,
additional provider protocols, session persistence, a full-screen transcript
browser, or an application-owned replacement for native scrollback.

## Ownership and handoff

| Owner | Authority and responsibility |
| --- | --- |
| `inference/openai_responses` | HTTP lifetime, SSE framing, Responses event interpretation, protocol validation, terminal success/failure, and conversion to internal output |
| `inference` | Narrow protocol-neutral observation capability and final inference result contract |
| Kernel execution through `LoopContext` | Model-turn commit ordering, publication of committed-turn observations, and preservation of session/tool invariants |
| `agent_loop` policy | When to infer, execute committed tool calls, or stop at the model-turn limit |
| `session` | Committed conversation and usage |
| `tui` | Temporary display projections, rendering, and terminal publication/cleanup |

The adapter receives only the inference request and an inference observation
capability. It does not receive kernel handles, session storage, TUI objects, or
the run event sender directly. Responses event names, wire indexes, and JSON
assembly remain at the adapter boundary. Internal correlation, when needed for
text routing, has an explicit scope and owner rather than masquerading as a
diagnostic-only field.

The inference operation emits provisional observations while consuming SSE and
returns a complete result only after validated response success. An individual
text, argument, or output-item `done` event is not whole-response success.
Prefer the complete response supplied by `response.completed` as the final
output source. Do not maintain a competing behavioral result assembled from
deltas. If a target provider requires reconstruction from finalized items, the
source and reconciliation rule must be explicitly decided before adopting that
compatibility behavior.

The existing session commit remains the handoff from validated inference output
to committed model history. Only after successful commit may the runtime publish
a committed-turn observation and expose that turn's tool calls for execution.
Usage is recorded once with that commit. Model-turn-limit handling continues to
commit explicit skipped tool results rather than executing pending calls.

Run observation must cover turn start, provisional text, and successful turn
commit with the final text needed for presentation. Exact types and correlation
mechanics remain implementation choices. `RunOutcome` remains authoritative for
overall run success or failure; event-channel closure is not a success signal.
A committed-turn observation does not promise that subsequent tools or model
turns will succeed.

Final text snapshots in events derive from committed output and are immutable.
The TUI's draft derives from received observations, may lag execution, and is
reconciled to the final text when commit is observed. The TUI alone refreshes or
discards its display projection. Neither snapshots nor draft text drive session
commits, tool execution, input admission, or cancellation.

## Failure, backpressure, and cleanup

EOF without validated success, `response.failed`, `response.incomplete`, stream
errors, malformed required protocol data, and timeouts remain failures. Error
messages preserve the distinction and available upstream failure or incomplete
reason. Partial text or individually finalized items cannot turn these into
success.

On failed inference, that turn's output and tool calls are not committed or
executed. The accepted user prompt and all earlier successful model/tool commits
remain. Failed-generation usage is not silently invented or represented as a
successful committed inference; this proposal adds no separate billing ledger.

If the TUI remains operational, it preserves received partial body text as a
readable failed output, explicitly marked incomplete and not written to the
conversation. It must distinguish that uncommitted draft from text belonging to
an earlier committed turn when a later tool or inference operation fails.
Terminal write failure may prevent this presentation and remains an I/O error.

Observation remains optional and bounded, including fragment sizes as well as
queue length. An attached slow observer applies documented backpressure;
detachment releases pending and future observation writes without cancelling
inference or changing its result. No unbounded queue or detached forwarding task
is introduced merely to avoid choosing a backpressure policy.

Inference needs an explicit finite total deadline and a documented policy for
network idle time and observer waiting. Observer backpressure must not be
misreported as upstream network inactivity. Simply removing the existing total
timeout is not acceptable. Exact durations, frame limits, batching, and redraw
cadence are implementation choices, but the resulting wait and memory bounds
must be stated and supported by appropriate evidence before cutover. This does
not introduce a deadline for trusted tool execution or promise bounded duration
for an entire agent run.

The adapter owns response-body release on success, protocol failure, transport
failure, timeout, and inference-future teardown. Runtime forwarding must not
leave background work retaining the response or observer after the operation.
Closing the TUI drops its observer; the kernel continues accepted work. The
existing ordered shutdown joins execution after it finishes. User cancellation
and forced termination semantics remain outside this proposal.

## TUI publication and Markdown boundary

During generation, body text occupies a replaceable preview region. Layout and
Markdown interpretation may change as more text arrives. Preview height is
limited by the terminal viewport; a long answer need not be fully visible until
publication. Preview rendering is batched or rate-limited rather than requiring
a whole-document parse and terminal write for every token.

After a model turn commits, the screen withdraws the preview and publishes the
authoritative complete text once through whole-document Markdown rendering.
Publication precedes presentation of that turn's tool activity. The final run
reply must not print that text a second time. Preview content that differs from
final text cannot override the committed result.

This preserves the current completed-output dialect, cross-block reference
resolution, terminal escaping, syntax highlighting, `NO_COLOR`, and hyperlink
behavior. It does not promise final Markdown interpretation for unfinished
input or immediate publication of an entire unfinished answer into scrollback.
The preview is an intended presentation mode, not a temporary non-streaming
fallback.

Screen remains the sole owner of preview withdrawal, native scrollback
publication, viewport reanchoring, and terminal restoration. Resize, partial
publication errors, and interface exit must preserve cleanup ownership. The
terminal cannot guarantee rollback of bytes already published after an I/O
failure; such a failure must remain observable.

## Compatibility evidence and acceptance

Available documentation establishes the intended transport, not completed
Minuet compatibility:

- [OpenAI streaming guide](https://developers.openai.com/api/docs/guides/streaming-responses)
  describes typed events over HTTP SSE.
- [OpenAI streaming events](https://developers.openai.com/api/reference/resources/responses/streaming-events)
  defines deltas, finalized items, and response terminal events.
- [MiniMax Responses documentation](https://platform.minimaxi.com/docs/api-reference/responses-create)
  advertises SSE via `stream: true`, but does not provide the full event sequence
  needed to establish all acceptance cases below.

MiniMax-M3 is the first live compatibility target. Live evidence must use small,
synthetic prompts and side-effect-free tools, retain no credentials in fixtures
or logs, and capture only the protocol data needed to assess compatibility.
Validate ordinary text, function calls and subsequent result replay, reasoning
continuation replay, final response shape, and usage. A successful text stream
alone does not establish full Responses or intermediary compatibility.

Acceptance concerns the claims below, not a mandatory automated test for every
claim. Choose evidence according to the risk and what it can establish:

- Focused tests are useful for framing, event interpretation, and reproducible
  failures; runtime probes or PTY/manual observations establish externally
  visible timing, provider behavior, and terminal interaction.
- Source review is sufficient where the implementation makes a claim directly
  auditable, such as ownership, dependency direction, commit ordering, bounded
  storage, or cleanup through a scoped lifetime. Record the reviewed revision,
  relevant paths, and reasoning in the implementation PR or validation record,
  including assumptions about library behavior.
- Use a combination when neither review nor observation alone is sufficient.
  Do not introduce production abstractions or test hooks solely to force an
  impractical automated test. A concrete source-review argument is acceptance
  evidence, not an exception requiring separate approval.

For each claim, record the evidence used and its limits in the implementation PR
or validation record. This ECP defines acceptance boundaries; it does not
maintain a source index or execution log. Unresolved behavior remains an
evidence gap; difficulty writing a test alone is not a blocker.

Before cutover, establish:

1. **Transport:** arbitrary byte boundaries, UTF-8 splits, multiple events in one
   read, SSE line endings and comments, malformed frames, frame limits, JSON
   HTTP errors, and rejection of non-streaming generation success bodies.
2. **Early observation:** text reaches the observer and TUI before the successful
   response terminal event, demonstrated by a gated upstream or another runtime
   observation that distinguishes early delivery from buffered completion.
3. **Final authority:** multiple output items and content parts, argument
   fragments, opaque reasoning items, absent/null usage, and final text are
   projected and replayed correctly without duplicate accumulation or commits.
4. **Failure:** partial output followed by each failure class remains observable
   without committing the failed turn or executing its tools. Earlier commits
   survive. An item-level `done` followed by failure still fails the turn.
5. **Lifecycle:** a full observer queue, observer/result detachment, a slow
   consumer, total/idle deadlines, and TUI exit do not leak response readers or
   prevent shutdown after accepted execution finishes.
6. **Run semantics:** committed-turn observations precede tool activity; tools
   execute only after commit and at most once per scheduled call; turn limits
   still record skipped calls. One-shot CLI output remains final-answer-only.
7. **Terminal behavior:** previews, long answers,
   incomplete Markdown, cross-block references, tables and code fences, Unicode,
   resize, failed partial output, publication without duplicates, and terminal
   restoration behave as specified. Use PTY or recorded manual checks for the
   interaction claims, supplemented by source review where appropriate. Retain
   the intent of existing tool-progress and completed-Markdown checks.

Convert HTTP fixtures in CLI and terminal tests to meaningful streaming
scenarios. Merely wrapping a complete JSON response in one SSE event does not
prove incremental delivery. Retain relevant existing deterministic failure and
lifecycle coverage even when live provider probes succeed. Add focused coverage
where useful rather than requiring a test for every acceptance item. Record
actual validation and provider limits when performed; none is claimed by this
proposal.

## Stop conditions and cutover

Stop and resolve the target or boundary before proceeding if implementation
requires any of the following:

- Committing drafts, executing tool arguments before response success and
  commit, or using observer delivery as execution authority.
- Competing output authorities, protocol representation leaking into the
  runtime/TUI, or cleanup dependent on an unowned background operation.
- Treating truncated streams as success, silently retrying or falling back, or
  weakening continuation replay to accommodate a provider.
- Narrowing completed Markdown behavior, losing partial-output failure honesty,
  replacing native scrollback ownership, or introducing run cancellation.
- Unbounded buffering/waiting in the new inference path, or validation that no
  longer demonstrates the claimed compatibility and lifecycle scope.

Provider deviations that require a new reconstruction or compatibility rule
must be recorded as a contract decision with observable failure behavior, not
introduced as an incidental provider-name branch. Concrete blocking correctness
or architecture findings from the actual diff must be repaired or resolved
before cutover.

At cutover, inspect the diff and directly affected paths for architecture
friction, and record validation evidence and any residual non-blocking debt in
the implementation PR or validation record. Update only affected effective
contracts and current-limitations documentation, including stream-only
generation, provisional versus committed observations, and the preview boundary.
The existing architecture's ownership direction remains protected.

Close this ECP with the achieved target, any material deviations from the agreed
decisions, the cutover outcome, and a reference to the implementation PR or
validation record. Preserve the original baseline as historical context rather
than updating it into a map of the resulting code. Mark the ECP closed only
after implementation, validation, and semantic cutover are complete.

## Cutover record

Implementation commits: `5bf0302`, `93ac437`, `5148167`, and `9e37a25`.

Validation completed on 2026-09-06:

- Formatting, clippy with `-D warnings`, all targets and features, and
  `nix flake check --all-systems --no-build` passed.
- Unit, CLI, and all 39 PTY/TUI tests passed. Fixtures exercise multiple SSE
  deltas before `response.completed`, tool continuation, failure retention,
  and final Markdown publication without duplication.
- The live MiniMax-M3 probe `responses_tool_loop_and_input_count` passed with
  the configured Responses endpoint, verifying streamed tool execution,
  subsequent final text, and input-token reporting.
- Final source review found no blocking architecture friction: inference owns
  wire framing and deadlines, runtime owns commit handoff, and TUI owns
  preview withdrawal and terminal publication.

The stream-only generation policy, provisional-versus-committed observation
boundary, and replaceable TUI preview are now cut over. This ECP is closed as
historical provenance; its baseline remains unchanged and it is not a source
path index.
