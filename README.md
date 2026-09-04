# Minuet

Minuet is a general-purpose CLI agent harness.

## Development

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
