# Current limitations

This document records intentional runtime and product boundaries of the current
Minuet implementation. It describes mechanisms and observable behavior rather
than source layout or implementation debt. The architecture document remains
the source for ownership and dependency rationale.

## Session lifetime

Sessions exist only in process memory. Restarting Minuet loses conversation
history, session settings, and usage summaries. There is no persistence,
session export, or cross-process session sharing.

The configuration file is read when the process starts. Session settings and
tool toggles changed during an interactive run are not written back to disk.

## Inference and context

Minuet currently uses one non-streaming, OpenAI-compatible Responses protocol.
Provider-specific continuation data is retained and replayed, but no other
provider protocol is exposed by the application.

The full committed conversation is sent on every model turn. Minuet does not
provide local history compression, truncation, token estimation, or a local
context-window guarantee. Context token information is available only when the
upstream provider supports its input-token operation.

API-provided built-in tools and system instructions are not exposed as
application features.

## Agent run limits

Tool calls execute sequentially. A run is limited by the configured number of
model turns. When that limit is reached, the model response is retained, any
pending tool calls are explicitly recorded as skipped, and no further tool
call is executed for that run. The run reports a limit stop rather than
silently pretending that a tool ran.

The user may submit another prompt in the same session. The next model turn
can see the skipped-call results, but Minuet does not automatically replay the
skipped tools.

## Tool trust and isolation

Tools are compiled into the process and treated as trusted components. Minuet
does not currently provide a sandbox, per-tool capability isolation, or a
general rollback guarantee for external side effects. Untrusted dynamic tool
loading is not supported.

## Run interruption and shutdown

Kernel commands are serialized. A run already in progress is not cancelled by
closing the interactive terminal or by dropping the caller's pending result;
shutdown may wait for the active inference or tool operation to finish. Process
level forced termination is an emergency exit and does not promise normal
cleanup or rollback of external effects.

## Other deliberately absent features

The current application does not provide a policy engine, subagent
orchestration, a full-screen TUI, third-party component loading, or durable
runtime configuration changes.
