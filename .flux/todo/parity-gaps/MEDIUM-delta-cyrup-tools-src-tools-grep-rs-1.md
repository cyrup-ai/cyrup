---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/grep.rs:1"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: exec
status: done
updated: 2026-08-28 08:00
---

# grep `$RIPGREP_CONFIG_PATH` — third rework

QA verdict: **8/10**. The three items from the previous round are fixed correctly and are not
revisited. One defect remains, and it is user-visible.

---

## `--no-ignore` / `-u` does not actually turn the ignore files off

**Reproduced**, not inferred. Against a directory containing `a.txt`, `b.txt`, a `.gitignore`
naming `a.txt`, an `.ignore` naming `b.txt`, and an `.rgignore` naming both:

| walk | result |
| --- | --- |
| cyrup default | `[]` |
| cyrup with `WalkOpts::no_ignore` set | `[]` |
| what ripgrep `-u` produces | `["a.txt", "b.txt"]` |

The flag that means *show me everything* changes nothing in any tree that has a `.rgignore`.

### What ripgrep does

`--no-ignore`'s update function sets **five** flags
([`defs.rs:4262-4266`](../../tmp/ripgrep-14.1.0/crates/core/flags/defs.rs)):

```
no_ignore_dot, no_ignore_exclude, no_ignore_global, no_ignore_parent, no_ignore_vcs
```

and its own documentation (`defs.rs:4245-4249`) is explicit that `.rgignore` is included.
[`hiargs.rs:890-899`](../../tmp/ripgrep-14.1.0/crates/core/flags/hiargs.rs) applies them:

```rust
.parents(!self.no_ignore_parent)
.ignore(!self.no_ignore_dot)
...
if !self.no_ignore_dot {
    builder.add_custom_ignore_filename(".rgignore");
}
```

### What cyrup does

[`ops/local/fs.rs`](../../crates/cyrup-tools/src/ops/local/fs.rs) covers the vcs trio and the
`.ignore` half of `no_ignore_dot`, and misses the other two:

- **`.rgignore` survives.** `fs.rs:361-363` registers the flavor's custom ignore filename
  unconditionally. `WalkBuilder` has **no clearing setter** — only `add_custom_ignore_filename` —
  so this has to become a *skip*, not an undo. Verified: there is no
  `custom_ignore_filenames`-style reset in ignore 0.4.33.
- **`parents(true)` is hardcoded** at `fs.rs:315`. `--no-ignore` implies `--no-ignore-parent`, so
  it must become `parents(!opts.no_ignore)`.

This is the highest-precedence ignore source of the lot — `fs.rs`'s own comment at `:357-360` says
the custom ignore file "outranks `.ignore` and EVERY gitignore source (`dir.rs:580-585`)". So the
one source `--no-ignore` fails to clear is the one that wins. The whole `-u`/`-uu`/`-uuu` ladder
inherits the defect, since `resolve_u_ladder` feeds the same field.

### Fix

In `LocalFs::walk`:

1. Gate the custom-ignore registration on `!opts.no_ignore`, keeping the existing comment about
   why the custom file outranks the others — that reasoning is correct and is precisely why the
   gate is needed.
2. Change `.parents(true)` to `.parents(!opts.no_ignore)`.

Keep `--no-ignore-vcs` exactly as it is: ripgrep's `--no-ignore-vcs` does **not** imply
`no_ignore_dot` or `no_ignore_parent`, so `.ignore`, `.rgignore` and parent traversal must all
keep applying for that flag. The two switches are deliberately asymmetric.

### Guard

A tree carrying `.gitignore`, `.ignore` and `.rgignore` that each hide a different file, searched
twice — once with a plain config and once with `--no-ignore`. Every file must appear under
`--no-ignore` and be hidden without it. The `.rgignore` entry is the one that fails today, so the
test must include it; a fixture with only `.gitignore` and `.ignore` passes either way and proves
nothing.

---

## Why two QA rounds missed this

Both previous rounds reviewed this mapping by reading it and judging it plausible. It only
surfaced when a real tree containing all three ignore files was walked. `config_globs_filter_the_walk`
exercises the walk, but nothing exercises `--no-ignore` — the flag was wired, compiled, and never
run. Same failure mode as `--max-filesize` and `--sortr` in round one.

For the remaining flags in §6.1, prefer one fixture that exercises the flag end-to-end over any
amount of reading.

## Definition of done

1. `--no-ignore` yields the same file set as ripgrep `-u`, including past a `.rgignore` and past
   ignore files in ancestor directories.
2. `--no-ignore-vcs` still leaves `.ignore`, `.rgignore` and parent traversal in force.
3. A guard test using all three ignore-file kinds proves it, and fails without the fix.
4. The 13 existing config guards still pass, and
   `smart_case_in_config_changes_the_match_set` still fails without its `.case_smart` call.
