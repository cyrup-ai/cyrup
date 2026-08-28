---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/grep.rs:1"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: qa
status: completed
updated: 2026-08-28 18:55
---

# One clause in `rgconfig.rs` overstates what `ignore(false)` misses

QA verdict: **9/10**. The DoD is met and the substance is right: the custom ignore file is named,
all three descriptions of `--no-ignore-vcs` now agree, and the survival claim was verified against
the code rather than the doc. Tests green, workspace clippy empty.

One clause is false, and it was written into the fix for the previous false clause.

---

## The clause

[`rgconfig.rs`](../../crates/cyrup-tools/src/tools/rgconfig.rs), in the `no_ignore_vcs` doc:

> Naming the custom file is the point: **it is the one source `ignore(false)` does not reach**, so a
> description that stops at `.ignore` leaves a reader to infer the custom file dies with the
> gitignore family.

`ignore(false)` reaches `.ignore` and nothing else. **Two** sources lie outside it:

| source | reached by |
| --- | --- |
| `.ignore` | `ignore(false)` |
| the gitignore family | the three `git_*` switches |
| the custom ignore file | the registration gate |

So "the one source" is wrong, and the phrasing invites the inference that `ignore(false)` *does*
cover the gitignore family — the same shape of error as the one this task chain has spent three
rounds correcting, in the same file.

## Fix

The point the sentence is making is sound: the custom file is the one a reader would not expect to
need a separate mechanism, because the git switches are obviously separate. Say that, rather than
claiming it is the only thing outside `ignore(false)`.

One clause. Do not rewrite the surrounding paragraph — the rest is accurate.

## Definition of done

1. No clause in `rgconfig.rs` claims the custom ignore file is the only source `ignore(false)` does
   not reach.
2. The paragraph still explains why naming the custom file matters.
3. No behaviour change; `cargo test -p cyrup-tools` green; workspace clippy still empty.

---

## Note on this loop

The behaviour under this task has been correct for several rounds; everything since has been comment
accuracy. That is a real standard — a comment asserting the wrong mechanism is what caused the
`--no-ignore` bug in the first place — but the returns are visibly diminishing, and the last three
findings have each been introduced by the fix for the one before it.

If this round closes cleanly, the task is done. If it produces another finding of the same size, the
right call is to stop and accept the branch rather than keep iterating on prose.
