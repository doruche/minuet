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
selects the adapter. Model names and reasoning effort are opaque strings passed
to that adapter, not entries in a locally maintained capability matrix.

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
