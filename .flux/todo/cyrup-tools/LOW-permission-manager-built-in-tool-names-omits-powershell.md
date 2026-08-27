---
stage: new
status: pending
priority: LOW
tool: powershell
source: QA follow-up from the powershell task
updated: 2026-08-27 14:20
---

# `BUILT_IN_TOOL_NAMES` in the permission manager still lists seven tools

`crates/cyrup-permission-system/src/manager.rs:36` declares
`BUILT_IN_TOOL_NAMES: [&str; 7]`, which does not include `powershell`, while
`cyrup_tools::BUILTIN_NAMES` is now `[&str; 8]`.

## Why this is benign today, and why it is still worth closing

`powershell` currently falls through to the extension-tool arm. That arm resolves
against the same `tools` rules and the same `DefaultCategory::Tools` default
(`Ask`) the built-in arm would apply. The built-in arm's only extra behaviour is
path action / resource sub-targets, which are meaningless for a shell tool. So
the effective permission outcome is identical — **not more permissive, not a
hole.**

It was correctly left out of the powershell task's scope: `manager.rs` is a port
of a permission-manager source that is **not present in the vendored pi 0.84.3
tree**, so there is no upstream to check parity against.

## Parity action

Two options; the second is preferred.

1. Add `"powershell"` to `BUILT_IN_TOOL_NAMES` and bump the array to 8.
2. Better: derive it from `cyrup_tools::BUILTIN_NAMES` so the two lists cannot
   drift again, mirroring the invariant test the powershell task added for
   `ALL_BUILTIN_TOOLS` (`every_builtin_is_gated_and_powershell_is_not_a_default`,
   `cyrup-session-svc/src/builder.rs:2410`), which iterates `BUILTIN_NAMES` and
   asserts membership in both directions.

If the crates cannot depend that way, add an equivalent test asserting the two
lists agree, so a ninth built-in cannot silently miss this one.

## Definition of done

1. `powershell` is recognised by the permission manager's built-in arm.
2. A test fails if `BUILTIN_NAMES` and `BUILT_IN_TOOL_NAMES` ever diverge.
3. No change to the resolved permission outcome for any existing tool.
