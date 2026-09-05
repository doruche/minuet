# Contributing to Minuet

Thank you for contributing to Minuet. Keep changes focused, explain the intent
behind them, and make the amount of documentation proportional to the change.

Before changing code, read and follow [`CODING_STYLE.md`](CODING_STYLE.md),
the authoritative source for Minuet's coding style conventions.

## Commits

Use the following form for commit subjects:

```text
type(scope): summary
```

The scope is optional. Use it when it adds useful context without requiring a
reader to inspect the diff.

Common types are:

- `feat`: new user-visible behavior
- `fix`: a bug fix
- `docs`: documentation-only changes
- `refactor`: behavior-preserving code changes
- `test`: test-only changes
- `perf`: performance improvements
- `build`: build system or dependency changes
- `ci`: continuous integration changes
- `chore`: maintenance that does not fit another type
- `revert`: a reverted change

Write the summary as a short imperative phrase, start it with a lowercase word
unless a name requires otherwise, and omit the trailing period. For example:

```text
feat(cli): add JSON output
fix: preserve task cancellation state
docs: explain contribution workflow
```

Use the commit body when the motivation, tradeoffs, compatibility effects, or
non-obvious constraints would otherwise be lost. Describe why the change is
needed rather than narrating the diff. Mark breaking changes with a
`BREAKING CHANGE:` footer.

Prefer commits that each represent one coherent intent. A commit does not need
to be artificially small, but unrelated changes should not be bundled merely
for convenience.

## Pull requests

Format pull request titles like commit subjects. This keeps the history useful
when a pull request title is used as a squash commit subject.

A pull request description should give reviewers the context that is relevant
to the change, which may include:

- what changed and why;
- how the change was verified;
- risks, compatibility concerns, or behavior intentionally left unchanged;
- related issues or follow-up work.

The pull request template is a prompt, not a form that must be completed in
full. Remove irrelevant sections. A small, self-explanatory change may need
only a sentence and its verification result.

Keep pull requests focused enough to review as one unit. Update tests and
documentation when behavior or user-facing interfaces change. For changes
involving state ownership, concurrency, subsystem boundaries, or resource
lifecycles, also follow the design guidance in [`AGENTS.md`](AGENTS.md).

## Verification

Run the checks relevant to the change before requesting review. The recommended
local check suite and development environment are documented in the
[Development section of the README](README.md#development).
