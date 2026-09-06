# Session lifecycle

The kernel owns the active session ID and serializes session commands with runs.
A run binds to one session for its entire agent loop. Creating a session uses
startup defaults and activates it; switching validates the target and affects
subsequent runs. The active session cannot be deleted. Deleting another session
removes it immediately. Clearing removes history and usage while preserving ID
and configuration.

The repository owns all session records, configuration, history, usage, and
summary projections. Consumers use immutable snapshots. Session IDs are opaque
UUIDs. Startup creates a fresh session; no previous session is resumed.

Model/provider, loop, and context strategy are startup-fixed. Session-owned
runtime policy currently includes reasoning effort and enabled tool names.
Tool implementations are process-level registry capabilities; each turn uses
one immutable snapshot resolved from the active session's selection.
