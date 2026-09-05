# Architecture

Minuet begins as one crate with module boundaries that can become crate
boundaries after the basic chain is stable. Its center is a micro-kernel: the
kernel owns lifecycle and state transitions, while injected components own
replaceable policy or external adaptation.

## Ownership

- `kernel` is the sole command sequencer. A consumer receives `KernelHandle`,
  not the store, registry, or their locks.
- `session` owns committed conversation items, per-session reasoning effort,
  and usage reported by successful inference calls. Its snapshots are immutable
  and are used only during the kernel's serialized command processing.
- `agent_loop` owns inference/tool sequencing policy. The initial `ReactLoop`
  receives a narrow `LoopContext`; that capability enforces commit ordering and
  tool visibility without exporting store internals.
- `context` owns selection of the model-visible view of committed history. The
  initial `FullContext` returns an ordered snapshot without maintaining a second
  writable conversation.
- `tool` owns the compiled tool set and its runtime enabled subset. The initial
  function tools are `echo`, `random_integer`, and `current_datetime`.
- `inference` defines a protocol-neutral backend capability.
  `openai_responses` alone owns OpenAI Responses wire JSON and HTTP behavior.

Provider identity and protocol are deliberately separate. A configured provider
selects an endpoint and credential environment variable; `openai-responses`
selects the adapter. `config` resolves the selected provider's environment
variable once at load and owns a private credential snapshot, redacted in debug
output. The runtime provider configuration carries the resolved value rather
than an environment variable reference; changing credentials requires loading
a new configuration and constructing a new backend. Model names and reasoning
effort are opaque strings passed to that adapter, not entries in a locally
maintained capability matrix.

## Kernel commands and lifetime

`kernel::command` owns the handle and internal request protocol. Each command
variant fixes its payload and result types through a shared `Envelope<P, R>`;
the reply channel belongs to the envelope, not to the business payload. There
is no untyped response enum or caller-side downcast. The kernel task remains the
only command sequencer and retains all state transitions in `kernel/mod.rs`.

Successful enqueue transfers work to the task. Dropping a waiting request before
enqueue abandons submission; dropping it afterwards only abandons the reply.
Shutdown is an ordered barrier: earlier commands finish, later queued commands
are rejected with `KernelError::Stopped`. `RunningKernel::shutdown` also joins
the task so completion includes resource cleanup. Dropping `RunningKernel`
without awaiting shutdown detaches it; task exit then depends on the last
handle closing and queued work finishing, including event backpressure.

## Conversation and context

The memory session history is the canonical conversation. The initial context
strategy sends that full history on every inference call: there is no local
compression, truncation, token estimation, context-window metadata, or
`previous_response_id` chain.

Responses output may contain messages, reasoning state, and function calls in
the same array. The adapter projects only the text and callable fields needed by
the loop, while retaining every complete output item as an opaque continuation.
The kernel can replay these items but cannot inspect or mutate their wire form.
This preserves provider-specific continuation state without spreading the
Responses representation across the core.

`/context info` invokes the provider's `/responses/input_tokens` operation for
the committed history and currently enabled tools. Separately, the session sums
only usage actually returned by inference responses and marks the sum partial if
any successful call omitted usage. Minuet does not store a context-window size.

## Current limits

The user-visible and operational limits of this implementation are maintained
in [`current-limitations.md`](current-limitations.md). Keeping that list in one
place avoids treating a transient module shape as a product guarantee.

## Run observation

`KernelHandle::run` accepts either a prompt alone or a `RunRequest` carrying an
optional caller-owned bounded event channel. The caller must consume progress
concurrently with awaiting the result. Detaching either
observer or result does not cancel accepted execution; a closed observer releases
blocked sends and disables further delivery.

The runtime owns inference and tool-call lifecycle events. A tool borrows only a
`ToolOutput` capability for execution-time UTF-8 fragments. It cannot fabricate
completion events or retain the capability beyond its invocation. Writes apply
backpressure and preserve text without adding newlines; large writes are split
at UTF-8 boundaries. Progress text is presentation data, not session history.
The tool's final return value remains the sole source of its committed result.

A `ToolFinished` event reports execution (or an explicit skip), not successful
session commit or overall run success. Later commit or inference failures remain
observable through the run's returned error. `RunOutcome` retains its immutable
summary for callers that do not need live observation.

## CLI and terminal ownership

`cli::app` owns pending user requests and coordinates input, progress and display.
Its status label is only a projection of received events; the pending request
controls input admission. Run progress is drained before displaying the returned
outcome, and the outcome's tool summary is not replayed after live observation.
Command modules own their subcommand grammar, execution and result text; the
root only assembles and dispatches command families. They share no session
storage or tool registry and invoke the existing narrow kernel operations.
The clap declaration is the single source for grammar, aliases, help and usage.
A shared help renderer derives layout from that declaration; bare groups are
informational help requests, while invalid leaf arguments remain errors.
The output boundary owns terminal styling and line endings.

`cli::input` owns one editing component and its buffer. Grapheme-aware operations
adapt the component's scalar cursor positions through its editing API. There is
no second writable input string. `cli::render` owns presentation conventions;
its unfinished output line is a bounded-by-display-width rendering tail, not a
second conversation. Completed display rows are handed to terminal scrollback.

`cli::terminal` owns terminal modes and output. Setup establishes its cleanup
guard before fallible viewport initialization; normal exit, errors and unwinding
restore terminal modes. The interaction task is the sole terminal input reader,
including synchronous cursor-position queries made by inline rendering. A
competing asynchronous terminal reader would steal those replies.

Ctrl-C and SIGINT both exit this interaction layer. Returning drops the observer
and pending request, restores the terminal, and lets `main` run existing kernel
shutdown. This is not cancellation of an accepted run. The pipeline adapter and
inline renderer consume the same run events and command effects; neither owns
execution or session transitions. The pipe reader owns only process-lifetime
stdin and a bounded input sender, so a blocked external read cannot retain the
kernel or prevent Tokio runtime teardown.
