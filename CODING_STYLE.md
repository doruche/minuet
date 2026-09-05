# Minuet Coding Style

This file is the authoritative source for Minuet's coding style conventions.
Keep new conventions here and link to them from other project guidance instead
of duplicating the rules.

## Inline unit tests with their owner

Place unit tests at the bottom of the implementation file for the module or
owner they test, inside an inline `#[cfg(test)] mod tests { ... }` block. This
applies to both leaf files such as `parser.rs` and module roots such as
`parser/mod.rs`. Tests for a child module belong in that child's implementation
file, keeping the tested behavior and its tests together.

For example, at the bottom of `parser.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_empty_input() {
        assert!(parse("").is_empty());
    }
}
```

Do not extract unit tests into a separate `tests.rs` or `tests/` module. Renaming
the extracted file, using a `#[path = "..."]` attribute, or using `include!`
does not satisfy this rule. File length alone is not a reason to separate tests
from their owner.

Cargo integration tests in a crate's `tests/` directory are outside this rule;
they exercise the crate through its external interfaces.

## Use `mod.rs` when a module has a directory

Do not keep a module's `xxx.rs` alongside a corresponding `xxx/` directory.
When a module needs that directory, move its root implementation into
`xxx/mod.rs`. Keep leaf modules without a corresponding directory in `xxx.rs`.
This gives a module one location containing its root, inline tests, and child
modules.

For example, change:

```text
src/
├── parser.rs
└── parser/
    └── token.rs
```

to:

```text
src/
└── parser/
    ├── mod.rs
    └── token.rs
```

The parent still declares `mod parser;`. Preserve module paths and visibility,
and check file-relative references when moving the implementation.
