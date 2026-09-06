# Architecture

Minuet is one crate organized around a micro-kernel. The kernel owns lifecycle
and state transitions; injected components own replaceable policy and external
adaptation. Module boundaries are semantic owner boundaries, not a promise
that each module will become a crate.

## Owners and dependency direction

- `kernel` is the command sequencer and sole owner of runtime transitions.
  Consumers receive `KernelHandle`, never the store, registry, or their locks.
- `session` owns the session repository, committed conversation items, session settings, usage, and derived summaries. Kernel owns the active session ID; each turn receives an immutable session and tool snapshot.
  Snapshots are immutable and consumed during serialized kernel processing.
- `agent_loop` owns inference/tool sequencing policy through a narrow
  `LoopContext` capability.
- `context` selects the model-visible view of committed history without a
  second writable conversation.
- `tool` owns the compiled tool set and enabled subset. JSON is the internal
  heterogeneous tool value protocol; provider adapters own wire encoding.
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

The memory session is the canonical conversation. The initial context sends
the full committed history on every model turn. Provider continuation items
remain opaque to the core, while adapters project only the fields needed by
the loop.

The TUI has one terminal input reader and restores terminal modes on normal,
error, and interrupted exits. Progress is presentation data, not session
history; terminal rendering does not own execution cancellation.
