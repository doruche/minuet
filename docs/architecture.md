# Architecture

Minuet is one crate organized around a micro-kernel. The kernel owns lifecycle
and state transitions; injected components own replaceable policy and external
adaptation. Module boundaries are semantic owner boundaries, not a promise
that each module will become a crate.

## Owners and dependency direction

- `kernel` is the command sequencer and sole owner of runtime transitions.
  Consumers receive `KernelHandle`, never the store, registry, or their locks.
- `session` owns the repository, provider context, semantic transcript, session
  settings, usage, and derived summaries. Kernel owns the active session ID;
  each turn receives immutable session and tool snapshots. Transcript reads are
  immutable presentation snapshots. Execution snapshots are taken during
  serialized kernel processing; frontends may retain stale presentation views.
- `agent_loop` owns inference/tool sequencing policy through a narrow
  `LoopContext` capability. The context retains single-use execution requests;
  session alone determines whether their results have committed. Kernel checks
  run completion through that capability before replying.
- `context` selects the model-visible view of committed history without a
  second writable conversation.
- `tool` owns plain-text argument/result formatting and the compiled tool set,
  and resolves immutable invocation snapshots from session-owned tool selection.
  JSON is the internal heterogeneous tool
  value protocol; provider adapters own wire encoding.
- `inference` defines the protocol-neutral backend capability; the
  `openai_responses` adapter owns OpenAI Responses JSON and HTTP behavior.
- `config` resolves provider credentials at startup into a private snapshot.
  Provider identity and protocol selection remain separate.
- `cli` resolves command grammar and frontend selection. `main` constructs the
  runtime and owns final kernel shutdown.
- `tui` owns terminal input, presentation, and observation state. Execution and
  session transitions remain in the kernel.

## Stable cross-boundary rules

Rules that callers and future changes must preserve live in
[`contracts/`](contracts/). They are organized by subsystem and contain only
the smallest durable closure needed by their consumers. Proposed changes to
those rules live in [`ecps/`](ecps/) until validation and cutover.

Implementation-specific limits and deliberately unsupported capabilities live
in [`current-limitations.md`](current-limitations.md). Local invariants remain
in code, tests, or nearby comments.

## Runtime shape

At runtime, the kernel serializes commands and owns their state transitions. A successful
enqueue transfers work to the kernel task; shutdown is an ordered barrier and
`RunningKernel::shutdown` joins that task. Runs expose progress through an
optional bounded observer while `RunOutcome` remains the authority for result
and completion.

The memory session owns two histories with different contracts. Provider
context is the canonical input for the next model turn and retains opaque
continuation items. The semantic transcript is the canonical presentation
history for user messages, model text, and grouped tool invocations. Repository
commit operations update the matching facts together; neither history is
derived from the other after commit.

Model commit returns the ordered presentation entries and confirmed execution
requests. Runtime publishes those entries without another semantic projection;
each inference reads fresh committed context through `ContextStrategy`. Loop
policy cannot retain or resubmit a tool round. Execution consumes the context's
requests once, while repository readiness independently checks the result-commit
obligation. Tool execution observations precede the atomic batch-result commit.

The TUI has one terminal input reader and restores terminal modes on normal,
error, and interrupted exits. Live progress is presentation data, not session
history. Live committed messages and replay share Markdown rendering and
document boundaries. Live tool output follows execution; replay groups stored
call/result display snapshots at their original semantic positions without
consulting current tools. Color and per-row indentation belong to TUI, not tool
formatters. The final run reply does not republish model text; its
text summary remains available to the one-shot CLI. TUI owns neither history
mutation nor execution cancellation.
