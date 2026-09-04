# Minuet

Minuet is a general-purpose CLI agent harness.

The current implementation is an early, single-crate micro-kernel with a
non-streaming OpenAI-compatible Responses backend, an in-memory session store,
a replaceable ReAct loop, and runtime-toggleable function tools. See
[`docs/architecture.md`](docs/architecture.md) for its boundaries and deliberate
initial limits.

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
