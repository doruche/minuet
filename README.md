# Minuet

Minuet is a general-purpose CLI agent harness.

The current implementation is an early, single-crate micro-kernel with a
non-streaming OpenAI-compatible Responses backend, an in-memory session store,
a replaceable ReAct loop, and runtime-toggleable function tools. See
[`docs/architecture.md`](docs/architecture.md) for its ownership model and
[`docs/current-limitations.md`](docs/current-limitations.md) for current runtime
and product limits.

## Running

Minuet requires `MINUET_HOME` and reads `$MINUET_HOME/config.toml`. It does not
fall back to XDG directories. Relative values are resolved from the process's
startup directory.

```console
mkdir -p .minuet
cp config.example.toml .minuet/config.toml
export MINUET_HOME=.minuet
export MINIMAX_API_KEY='...'
nix develop -c cargo run
```

API keys are never stored in `config.toml`; each provider configuration names
the environment variable from which its key is read. Configuration and all
session changes are read-only/in-memory in this version.

At the prompt, `/help` lists commands. In particular, `/model effort <value>`
passes an uninterpreted string to the upstream provider, while `/context info`
uses the upstream `input_tokens` operation and displays usage reported by prior
responses. No local model capability or context-window table is maintained.

## Terminal interaction

With terminal stdin and stdout, Minuet uses an inline TUI and leaves completed
output in the terminal's scrollback. Enter submits; Alt+Enter inserts a newline.
Bracketed multi-line paste is one edit and does not submit automatically. Editing
deletes and moves across whole grapheme clusters, including Chinese text. Input
history, completion menus, and accepting additional input during a run are not
implemented.

Tools can publish text while they execute. The display shows `Running`, live
fragments (including text without a trailing newline), and `Ran`, `Failed`, or
`Skipped` followed by a separately labelled final result. Process output is not
silently substituted for the result sent to the model. The current small builtin
tools return immediately and do not manufacture intermediate output.

`/help` lists commands; `/tools --help` and `/tools enable --help` show nested
usage. Slash-command arguments support shell-style quoting and escaping without
shell expansion. For example, `/model effort "vendor depth"` passes one opaque
value. Ordinary prompts retain their leading/trailing whitespace and newlines.

Ctrl-C exits the interface and enters the existing shutdown path. It does not
cancel the current run; shutdown may wait for inference or a tool to finish.
Terminal modes are restored before that wait. Setting a nonempty `NO_COLOR`
disables colors. With redirected stdin or stdout, the same commands and progress
use plain text without a prompt, animation, or ANSI controls. This path processes
one input line at a time. The conversation transcript goes to stdout; fatal
startup, I/O and shutdown errors go to stderr.

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
for streaming, Unicode editing, resize, pipes and shutdown. No live API is needed
for these checks.

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
