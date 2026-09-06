# Loop execution and committed publication

Kernel serializes runs and session commands. Accepted work remains owned by
the kernel when a caller or observer detaches; shutdown waits behind that work.
There is no current-run cancellation or automatic tool retry. Forced process
termination does not promise external rollback or durable recovery.

Session owns committed history and pending-result association. `LoopContext`
holds immutable requests returned by successful model commit and owns their
single execution opportunity. Policy chooses inference, invocation or explicit
step-limit skipping through the current context; it cannot transport a round
into another run. Consuming requests does not establish that results committed.

New runs check repository readiness before accepting user input. Every inference
checks readiness before contacting the backend, reads a fresh committed-context
snapshot, and applies `ContextStrategy`. No run-local incremental conversation
is independently assembled. Model commit precedes request exposure and ordered
committed-message publication; opaque input history never comes from transcript.

Invocation or skipping consumes the current opportunity once. Tools execute in
call order after the entire model response commits. Ordinary execution errors
are typed failed results. One batch commit atomically publishes context outputs
and invocation terminal states; skipped work commits an explicit reason and
model-visible result without executing a tool. An absent or already consumed
round is an error, never a successful no-op.

Kernel passes the policy result through the context's completion check before
replying. Unconsumed requests or repository-owned unresolved results prevent
success. If policy already failed, completion preserves that original error
and reports the outstanding obligation as well. Failed commit or a policy's
dropped operation future does not restore consumed requests. Subsequent input
and inference cannot bypass an unresolved round. Internal protocol defects stay
observable; there is no automatic skip, retry, clear, or user recovery protocol.

One run's optional bounded event channel is attached at submission and ordered;
it does not support mid-run attachment or reattachment. The publication rules
are:

- `ModelTurnCommitted` carries ordered immutable entries from successful
  repository commit, preserving complete message boundaries and call positions.
- `ToolExecutionFinished` reports an executed call's return immediately, even
  while later calls are still running. It does not claim repository commitment.
- `ToolRoundCommitted` carries repository-confirmed invocation terminal
  snapshots in call order. Every completed/failed result has a preceding
  execution-finished observation on this channel. Skipped results have no
  execution-finished observation and are published only after commit.

These rules let the inline TUI show each executed result once on observation,
and only skipped results on batch commitment, without maintaining an execution
registry. Replay uses stored final states; it does not reconstruct live timing
or promote streamed fragments into results. Grouping a result under a call
does not imply that later messages in the same model response saw that result.
The TUI publishes each committed model message once; the run reply supplies
completion status, not another copy of its body. `RunOutcome.text` remains a
lossy final-turn summary for consumers such as the one-shot CLI.

Observer backpressure may pause execution, but observation is not authority.
Closing the receiver releases pending sends and does not undo or cancel work.
Runtime owns request/result temporaries for the round; adapter requests and
tool resources retain their respective cleanup owners. The TUI releases its
observer on completion or exit and restores terminal modes independently of
the kernel's eventual shutdown.

Evidence: inline runtime tests cover ordered handoffs, gated tool observations,
commit rejection, dropped policy operations, single-use requests, and fresh
context reads under full and selective strategies. Kernel tests cover invalid
policy completion, new-input rejection, detachment and shutdown. PTY tests
cover message order, Markdown boundaries, single publication and replay.
