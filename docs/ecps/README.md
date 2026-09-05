# Engineering change proposals

An ECP records a proposed consequential semantic change, its authorization,
protected boundaries, acceptance evidence, and stop conditions. Use one when a
change alters ownership, handoff, failure or cleanup semantics, public behavior,
compatibility, durable rules, or requires risky or multiple cutovers. Ordinary
local changes and implementation plans do not need an ECP.

Keep proposals as topic files:

```text
ecps/<change-topic>.md
```

An ECP is not current truth or authorization for unrelated work. While it is
in progress, affected contracts remain unchanged. After validated cutover,
update only those contracts; retain the closed ECP as decision provenance. If
the change is abandoned or not cut over, leave contracts unchanged and record
that outcome in the ECP. Do not create ECPs retroactively for historical work.
