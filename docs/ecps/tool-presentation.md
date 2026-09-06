# Tool presentation snapshots and execution-first TUI

Status: closed; implementation and validated cutover complete.

Date: 2026-09-07. Baseline: `35b88e4`.

The project owner authorized friendly tool-owned argument/result display, gray
indented tool bodies, quieter live output, implementation and one commit.
The baseline prints pending call listings before actual execution and displays
protocol JSON as tool results. Indentation is coupled to the text color.

## Target and boundaries

Tools supply explicit, pure plain-text argument and successful-result formatters.
The tool boundary formats errors. Execution still validates inputs; display
must not decide execution status or alter model-facing structured results.
The loop captures the call label before execution and pairs it with the actual
return's display. Session commits immutable display snapshots with outcomes;
replay requires neither registry lookup nor tool execution. Skips capture a
label and policy reason without invoking work. Unresolved history remains
honest about the absence of a committed result and display snapshot.

Live displays complete committed messages followed by actual Running/Ran/result
observations, not an advance call list or normal Pending notices. This explicitly
replaces the previous ECP's identical live/replay message-call interleaving
requirement. Transcript order, message boundaries, and grouped replay order stay
unchanged. Results and streams are gray and every visual row is indented;
failure headings remain red. Layout is independent of color and NO_COLOR.

Repository commit, execution opportunity, observer lifetime and serial kernel
ownership remain unchanged. No persistence, per-tool commit, retries, recovery
state machine, terminal-aware tool interface, or generic formatting framework.
Public Rust tool/outcome/presentation types may change together; provider wire
formats, CLI answer output and session commands remain compatible.

## Acceptance and stops

Prove builtin displays use actual values, structured model context is unchanged,
success/failure/skip snapshots replay without execution, and no normal pending
listing remains. Per the owner's subsequent direction, use source tracing where
sufficient and update existing tests for changed contracts; do not force new
tests. Existing PTY evidence covers live versus replay ordering, separate
Markdown documents, streaming, resize and NO_COLOR. Trace gray indented body
rows through both scrollback insertion and unfinished-tail drawing. Run
formatting, check, strict clippy, tests and actual-diff ownership
review before updating affected effective contracts and closing this proposal.

Stop for duplicate execution/history authority, presentation-driven protocol
decisions, hidden failures, replay requiring current tools, or expansion beyond
the above target. Do not revise the closed historical ordered-output ECP.

## Cutover record

Completed 2026-09-07. Existing source/directory owners remain in place.
`Tool` now requires `display_arguments` and `display_result`; the builtin tools
explicitly render quoted echo text, inclusive range bounds, actual integer
results and the returned UTC timestamp. Registry and snapshot invocation share
the same boundary result/error construction. No default JSON renderer, tool
presentation registry, or execution state machine was introduced.

`ToolResult` separates context output from `ToolDisplay`; typed `ToolOutcome`
variants carry it into the existing atomic batch commit. Terminal transcript
states retain only the historical display alongside the original request;
provider context retains structured results. `ToolStarted` carries the captured
call label; `ToolActivity` carries display instead of encoded output. These are
coordinated Rust API changes, not provider protocol changes.

The owner-centered change review followed builtin invocation through runtime,
repository commit, observations and both Screen paths. Session remains the
only committed-history owner. Display snapshots cannot change execution status,
readiness or next-inference context; replay has no tool capability. The existing
single-use, failure and cleanup protocol is unchanged. TUI helpers that became
internal after removing the advance call list now have private visibility.

Source tracing establishes color/layout behavior: `tool_fragment` selects gray
and two-space indentation independently; `tool_body` uses that same path.
Both `Screen::append` and unfinished-tail drawing read `OutputTail::indent`;
line completion preserves it, and NO_COLOR affects style only. Builtin result
formatters read actual returned values without execution or recomputation.

Existing tests were adapted, not expanded into a new test suite. The execution
timeline/replay PTY case checks live messages before actual Running, no advance
list, ordered replay labels, independent Markdown documents, one result per
execution, and replay without execution. Existing kernel tests still check the
structured next-inference result independently from the friendly observation.
Success/failure/skip, repository atomicity, streaming/resize, session lifecycle,
NO_COLOR, detachment and CLI behavior remain covered by the existing suite.

Validation in the Nix development environment passed:

- `cargo fmt --all -- --check`
- `cargo check --locked --all-targets --all-features`
- `cargo clippy --locked --all-targets --all-features -- -D warnings`
- `cargo test --locked --all-targets --all-features`: 143 passed, 1 ignored.
- `git diff --check`

The ignored live MiniMax test was not run. This change claims source-reviewed
presentation behavior and existing fixture-backed regression coverage, not new
live-provider validation. Affected session/loop contracts, architecture and
limitations were updated at cutover; historical ECPs remain unchanged.
