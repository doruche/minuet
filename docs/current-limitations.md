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

Minuet currently uses one stream-only, OpenAI-compatible Responses protocol for
generation. Input-token counting remains a separate JSON operation.
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

Ctrl-C and process SIGINT request interface exit followed by this same shutdown;
there is no cancel-current-run action or second-key force-abort shortcut.

## Terminal interface

The interactive interface uses the terminal's main screen and scrollback. It has
basic multiline editing but no input history, completion, selection menus,
application-owned transcript browser or input queue during execution. TUI mode
requires terminal stdin and stdout; there is no plain-text REPL fallback.

Tool progress is UTF-8 text, not a terminal emulation protocol. Newlines are
preserved, tabs are displayed as spaces, and other control characters (including
ANSI escapes and carriage returns) are displayed literally. Progress is separate
from the final result; both can contain overlapping content. Model text is shown
in a replaceable preview while it is generated and published as complete
Markdown only after the model turn commits. Failed partial output remains
visible as an incomplete draft and is not committed.

Model previews reveal grapheme clusters in small batches, accelerating for
larger bursts. Up to six recent wrapped rows remain visible, subject to terminal
height. Commit replaces the preview immediately with authoritative Markdown;
failure publishes all received draft text literally with an incomplete marker.
Presentation pacing never delays event reception, tool execution, or shutdown,
and does not continue playing an animation after completion. Short responses
may complete before a preview frame is drawn. There is no pacing configuration.

The TUI parses each completed model answer as one Markdown document so reference
links can resolve across paragraphs, then formats and publishes one top-level
block at a time. It does not incrementally parse model tokens as final Markdown;
the in-progress preview is replaceable and intentionally provisional. The supported
dialect is CommonMark with tables, task lists and strikethrough; raw HTML stays
literal and images are text placeholders. Recognized code languages use bundled
syntax highlighting; absent or unknown language labels use ordinary code text.
Tables are clipped at the terminal's right edge without responsive layout or
horizontal scrolling. Links require OSC 8 support, with no detection or fallback.
`NO_COLOR` disables Markdown styling too, while preserving hyperlinks.

On horizontal shrink, the currently displayed screen is moved into terminal
scrollback before redraw because the current renderer otherwise clears it. This
preserves output but may also retain a snapshot of the previous input/status
area. There is no application-level reflow of already published scrollback.

## One-shot CLI

`chat` runs a single user turn in a fresh in-memory session. It has no session
resume, JSON output, progress option, or CLI-specific model/tool overrides;
configuration supplies the same runtime settings as the TUI. `chat -` buffers
all UTF-8 stdin until EOF as one prompt, without a size limit. Empty prompts are
rejected. The former implicit line-by-line pipe conversation has been removed.

Only final text is printed to stdout, without terminal escaping; a missing final
newline is added. Step-limit text may be partial and is accompanied by a nonzero
exit and stderr diagnostic. Agent completion does not promise that every tool
succeeded: tool errors can be model-visible results from which the agent recovers.
SIGINT during execution requests shutdown and returns 130 after accepted work
finishes; it does not cancel that work. Before kernel startup, stdin reads retain
the normal process-level interrupt behavior.

## Other deliberately absent features

The current application does not provide a policy engine, subagent
orchestration, a full-screen TUI, third-party component loading, or durable
runtime configuration changes.
