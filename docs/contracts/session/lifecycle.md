# Session lifecycle

The kernel owns the active session ID and serializes session commands with runs.
A run binds to one session for its entire agent loop. Creating a session uses
startup defaults and activates it; switching validates the target and affects
subsequent runs. The active session cannot be deleted. Deleting another session
removes it immediately. Clearing removes provider context, semantic transcript,
and usage while preserving ID and configuration.

The repository owns all session records, configuration, provider context,
semantic transcript, usage, and summary projections. Provider context is the
authoritative input history for inference and may retain opaque backend items.
The transcript is the authoritative presentation history and contains
Minuet-owned user messages, model messages, and grouped tool invocations with
typed pending, completed, failed, or skipped execution states. Neither history
is reconstructed from the other.

Consumers receive immutable transcript views whose entries may be shared with
the repository and may become stale after later mutations. The agent loop
commits matching context and transcript facts through repository operations;
frontends cannot mutate either history. Provider call IDs remain internal to
context bookkeeping and do not enter transcript entries. Session IDs are opaque
UUIDs. Startup creates a fresh session; no previous session is resumed.

Successful session creation and clear return an empty presentation view.
Successful switching returns the selected session's transcript from the same
serialized kernel command that activates it. A failed switch preserves the
active session and does not replace the frontend view. TUI creation, switching,
and clearing replace the visible terminal and request native scrollback purge;
switch replay renders stored semantics without executing model or tool work.

Model/provider, loop, and context strategy are startup-fixed. Session-owned
runtime policy currently includes reasoning effort and enabled tool names.
Tool implementations are process-level registry capabilities; each turn uses
one immutable snapshot resolved from the active session's selection.
