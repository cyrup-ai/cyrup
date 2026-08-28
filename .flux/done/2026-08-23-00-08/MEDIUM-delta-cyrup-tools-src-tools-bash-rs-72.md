---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/bash.rs:72"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: qa
status: completed
updated: 2026-08-28 10:45
---

# bash:72 — closed by verification; two documentation defects remain

## Settled — do not redo

The gap is **closed by verification**. The RED lever was executed, not predicted:

- Reverting `bash.rs:139` to `"Bash command to execute"` failed
  `all_eight_tool_schemas_match_pi_typebox_bytes` at `pi_schema.rs:72:5`, reached from `:84:5`,
  with `bash parameters() schema diverges from Pi's TypeBox input_schema`. One test of 335.
- The task's further prediction — that the `powershell` assertion at `:87` "stays green" — was
  **disproved**. It is unreachable (the bash assert panics first) and false in substance:
  `bash.rs:139` sits in `ShellTool::new`, which `powershell.rs:50` also routes through.
  Temporarily skipping the bash assertion produced `powershell parameters() schema diverges`
  carrying the same bytes. The shared literal makes it structurally impossible to diverge one
  shell without the other — which is the invariant `pi_schema.rs:59-62`'s comment claims to
  protect, now demonstrated.
- Both files restored byte-identically (`bash.rs` `6dc27e90…`, `pi_schema.rs` `ec376a63…`),
  re-verified at QA time. Suite green at 335. No source edit ships.
- The live sibling is filed as
  [MEDIUM-delta-cyrup-ext-sdk-tool-factory-descriptors.md](./MEDIUM-delta-cyrup-ext-sdk-tool-factory-descriptors.md).

Nothing above needs revisiting. What follows is documentation accuracy only — no source change.

---

## 1. `pi_schema.rs:59-70` is a wrong citation, and it is in TWO files

Appears at:

- `MEDIUM-delta-cyrup-tools-src-tools-bash-rs-72.md:150` (this file, §6)
- `MEDIUM-delta-cyrup-ext-sdk-tool-factory-descriptors.md:56`

Verified actual bounds in
[`pi_schema.rs`](../../crates/cyrup-tools/src/tests/pi_schema.rs): the schema constants run
**58–68** — `PI_READ` at `:58`, `PI_EDIT` at `:68`. `:59` misses `PI_READ` entirely; `:70`
overshoots into blank space before the description constants (which start at `:113`).

The accompanying phrasing is also wrong: *"pins all eight against `PI_READ` / `PI_SHELL` /
`PI_WRITE` / …"* implies eight constants. There are **seven** — `PI_READ`, `PI_SHELL`, `PI_GREP`,
`PI_FIND`, `PI_LS`, `PI_WRITE`, `PI_EDIT` — serving **eight** `assert_schema` calls, because
`bash` and `powershell` deliberately share `PI_SHELL`.

That sharing is the precise invariant this task spent a lever proving. Getting its arithmetic
wrong in the write-up undercuts the finding, and this task file is itself arguing that stale line
anchors are a problem worth a policy decision — so shipping a fresh task with a wrong anchor is
self-defeating.

**Fix:** in both files, cite `pi_schema.rs:58-68`, and say *seven constants covering eight tools,
because `bash` and `powershell` share `PI_SHELL`*.

## 2. §6 duplicates the child task and will drift

§6 of this file and the filed ext-sdk task now carry the same divergence tables, the same pi
citations, and the same decision text. Two copies of one set of facts, which is exactly the defect
the child task reports about `tool_factory` versus `cyrup-tools`.

**Fix:** collapse §6 to a short pointer — one sentence naming the defect, one line on why it was
split out, and a link to
[MEDIUM-delta-cyrup-ext-sdk-tool-factory-descriptors.md](./MEDIUM-delta-cyrup-ext-sdk-tool-factory-descriptors.md).
The child task is the single source of truth; the detail lives there.

## Definition of done

1. `pi_schema.rs:58-68` cited correctly in both files, with the seven-constants/eight-tools
   distinction stated.
2. §6 of this file is a pointer, not a copy.
3. No source file is touched — the close stands as verified.

## Still awaiting a decision from David (unchanged, not blocking)

- **`tool_factory`**: re-export the real `cyrup-tools` descriptors (pi's model, `sdk.ts:122-129`),
  or keep a second builder set pinned to the same constants? Option 2 is how the current drift
  arose. Detail in the child task.
- **Anchor policy**: should gap tasks anchor by symbol with the line as a hint? These anchors
  drifted by 8 before anyone first read them, and item 1 above is a third instance of the same
  class of error.
