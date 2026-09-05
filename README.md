# Minuet

Minuet is a general-purpose CLI agent harness.

The current implementation is an early, single-crate micro-kernel with a
non-streaming OpenAI-compatible Responses backend, an in-memory session store,
a replaceable ReAct loop, and runtime-toggleable function tools. See
[`docs/architecture.md`](docs/architecture.md) for its ownership model and
[`docs/current-limitations.md`](docs/current-limitations.md) for current runtime
and product limits.

## Running

Chat and TUI commands require `MINUET_HOME` and read `$MINUET_HOME/config.toml`.
Minuet does not fall back to XDG directories. Relative values are resolved from
the process's startup directory.

```console
mkdir -p .minuet
cp config.example.toml .minuet/config.toml
export MINUET_HOME=.minuet
export MINIMAX_API_KEY='...'
nix develop -c cargo run
```

API keys are never stored in `config.toml`; each provider configuration names
the environment variable from which its key is read. Loading configuration
resolves the selected provider's key once; missing, empty or non-Unicode values
fail startup. Environment changes after loading do not update that snapshot.
Configuration and all session changes are read-only/in-memory in this version.

## Command-line interface

```console
minuet --version
minuet --help
minuet chat "Explain this error"
minuet chat - < prompt.txt
minuet tui
```

Version and help work without configuration, credentials, or a running kernel.
`chat` creates a fresh in-memory session, runs one user turn (including any model
and tool rounds), then shuts down and exits. Quote the prompt as one argument;
use `minuet chat -- "--leading-dashes"` for text starting with a dash. Slash
commands such as `/help` are literal prompt text in `chat`.

`chat -` reads all of stdin as one UTF-8 prompt, preserving whitespace and
newlines. It waits for EOF and rejects empty or whitespace-only input. A prompt
argument does not read stdin. There is no session persistence between invocations.

Chat stdout contains only the final model text, with a final newline added if
needed; an empty answer produces no output. Text is emitted as data without TUI
escaping or styling. Intermediate model text, tool progress and tool results are
not printed. Errors go to stderr. Exit codes are 0 for a completed run, 1 for
startup, input I/O, execution, output or shutdown failure, and 2 for invalid
arguments or empty prompts. Reaching the model-turn limit returns 1, preserves
any text from that turn on stdout, and explains the incomplete run on stderr.
Tool errors that the agent handles and recovers from do not independently make
a completed run fail.

During execution, Ctrl-C/SIGINT stops waiting for the answer and requests kernel
shutdown; it returns 130 after accepted work finishes. This does not cancel an
inference or tool call. While reading `chat -` before startup, SIGINT retains its
normal process-level behavior.

With no subcommand, `minuet` opens the TUI. Both this default and `minuet tui`
require terminal stdin and stdout. **The former implicit, line-by-line pipe
session is no longer supported:** use `minuet chat -` for redirected input.
A multiline input now forms one prompt rather than multiple conversation turns.

## Terminal interaction

With terminal stdin and stdout, Minuet uses an inline TUI and leaves completed
output in the terminal's scrollback. Enter submits; Ctrl-O inserts a newline,
as does Shift-Enter when the terminal supports it. Bracketed multi-line paste
is one edit and does not submit automatically. Editing
deletes and moves across whole grapheme clusters, including Chinese text. Input
history, completion menus, and accepting additional input during a run are not
implemented.

Completed model answers render as Markdown, including headings, emphasis,
lists, quotes, tables, and syntax-highlighted code blocks. Paragraphs and code
wrap at the terminal width; tables keep their natural column widths and clip
at the right edge. Images display an `[img]` placeholder and their description.
Links display only their label and use OSC 8 hyperlinks; activation (such as
Ctrl+Click) is handled by your terminal. Minuet assumes a modern terminal and
does not probe hyperlink support. Tool output and command results remain
literal text; `chat` continues to return the original Markdown.

Tools can publish text while they execute. The display shows `Running`, live
fragments (including text without a trailing newline), and `Ran`, `Failed`, or
`Skipped` followed by a separately labelled final result. Process output is not
silently substituted for the result sent to the model. The current small builtin
tools return immediately and do not manufacture intermediate output.

While a request is pending, the status line shows an activity indicator, elapsed
waiting time and the current phase. Time runs from submission until the TUI
receives the result, across all model calls and tools in that run. The indicator
animates even without new output; it does not measure remote progress. When a run
returns, it leaves a `Completed`, `Stopped` (turn limit), or `Failed` time summary.
Commands use the same live status line without adding a completion summary.

`/help` lists commands. Bare groups (`/tools`, `/context`, `/model`) show
help; `/tools list`, `/context info`, and `/model info` perform queries.
`/tools enable <name>` and `/tools disable <name>` change tool availability.
Every command supports `--help`, including `/tools enable --help`. Slash-command
arguments support shell-style quoting and escaping without shell expansion.
For example, `/model effort "vendor depth"` passes one opaque value; the value
is required (`clear` restores the upstream default). Ordinary
prompts retain their leading/trailing whitespace and newlines.

Ctrl-C exits the interface and enters the existing shutdown path. It does not
cancel the current run; shutdown may wait for inference or a tool to finish.
Terminal modes are restored before that wait. Setting a nonempty `NO_COLOR`
disables colors. The conversation transcript goes to the terminal; fatal
startup, I/O and shutdown errors go to stderr. Use `chat` for redirected output.

`/model effort <value>` passes an uninterpreted string to the upstream provider,
while `/context info` uses its `input_tokens` operation and displays usage
reported by prior responses. No local model capability or context-window table
is maintained.

See [current limitations](docs/current-limitations.md) for terminal behavior and
[architecture](docs/architecture.md#run-observation) for the tool output contract.

## Development

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for commit and pull request guidance.

[Nix](https://nixos.org/) provides the development shell, while
[`rust-toolchain.toml`](rust-toolchain.toml) pins the Rust toolchain managed by
`rustup`.

Enter the development shell and build Minuet:

```console
nix develop
cargo build
```

Run the local checks with:

```console
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
nix flake check --all-systems --no-build
```

The test suite includes local provider fixtures and real pseudo-terminal checks
for tool progress, Unicode editing, resize and shutdown, plus actual CLI process
tests for arguments, pipes, output, exit codes and interruption. No live API is
needed for these checks.

The live MiniMax compatibility test is intentionally ignored by the default
suite. With `MINIMAX_API_KEY` set, run it explicitly:

```console
nix develop -c cargo test --test minimax_live -- --ignored
```

The development shell also provides `actionlint` and `act` for validating the
GitHub Actions workflow locally. Running the workflow with `act` requires a
Docker-compatible container engine.

```console
actionlint
act pull_request --job checks
```

## License

Minuet is licensed under either the MIT license or the Apache License (Version
2.0), at your option.

See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE) for details.
