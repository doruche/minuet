# Effective contracts

This directory contains current normative rules that must remain stable across
components or future changes. Each contract is the smallest useful closure for
one subsystem or cross-subsystem boundary, grouped as:

```text
contracts/<subsystem>/<contract>.md
```

Create a contract only for rules reused across owners, changes, or an external
interface. Keep local invariants in code, tests, or nearby comments. A contract
states effective behavior, ownership and handoff, failure and cleanup, visible
boundaries, and evidence. It is not a repository encyclopedia or a copy of an
ECP. The directory may remain sparse; add a subsystem directory when its first
contract is justified.
