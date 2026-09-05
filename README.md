# Minuet

Minuet is a command-line agent for conversations with an OpenAI-compatible
model. It provides an interactive terminal interface and a one-shot `chat`
command, with a small set of built-in tools.

Minuet is early-stage software. Sessions are held in memory, the current
runtime uses one Responses-compatible provider protocol, and tools run as
trusted components in the process. See [current limitations](docs/current-limitations.md)
for the boundaries that matter when evaluating it.

## Quick start

Minuet needs a configuration file and an API key supplied through the
environment. The example below uses MiniMax; other configured providers can
use their own endpoint and environment variable.

```console
mkdir -p .minuet
cp config.example.toml .minuet/config.toml
export MINUET_HOME=.minuet
export MINIMAX_API_KEY='your-api-key'
nix develop -c cargo run
```

`MINUET_HOME` points to the directory containing `config.toml`. Minuet does not
fall back to an operating-system configuration directory. API keys are read
from the environment named by the provider configuration and are not written
to the configuration file.

## Using Minuet

With a terminal, `cargo run` opens the interactive interface. Enter submits a
prompt; `Ctrl-O` inserts a newline. The interface renders model responses as
Markdown and displays tool progress while a run is active. Type `/help` to see
the available interactive commands.

For a single request, use `chat`:

```console
minuet chat "Explain this error"
minuet chat - < prompt.txt
```

The first form takes one prompt argument. The second reads one UTF-8 prompt from
standard input until EOF. Each `chat` invocation starts a new in-memory session
and prints the final answer to standard output; diagnostics are written to
standard error.

The command-line interface is also available without configuration:

```console
minuet --help
minuet --version
```

## Development

Enter the development shell and build the project:

```console
nix develop
cargo build
```

Before submitting a change, run the checks relevant to it. The full local suite
is:

```console
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
nix flake check --all-systems --no-build
```

See [Contributing](CONTRIBUTING.md) for commit and pull request guidance.

## Further documentation

- [Current limitations](docs/current-limitations.md) — supported boundaries
  and intentionally absent features.
- [Architecture](docs/architecture.md) — ownership and subsystem design.
- [Contributing](CONTRIBUTING.md) — development workflow and verification.

## License

Minuet is licensed under either the MIT license or the Apache License, Version
2.0, at your option. See [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE).
