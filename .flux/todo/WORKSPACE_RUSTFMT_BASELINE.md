---
stage: new
status: done
updated: 2026-08-22 22:40
---

# Establish A rustfmt Baseline Across The Workspace

## Description

Scoped workspace-wide deliberately: cyrup-provider is **not** the outlier. All 22 workspace crates
are unformatted — 13,604 hunks total, with cyrup-provider ranking 12th at 369 (2.7% of the total).
Fixing one crate in isolation would just make the inconsistency less visible.

Measured with `cargo fmt -p cyrup-provider -- --check` (edition 2024, inherited from the workspace
manifest) and an out-of-tree reformat simulation (`rustfmt --edition 2024 --emit files` over a
scratch copy, then `diff -ru`):

```
cyrup-provider: 369 hunks across 76 of 129 .rs files (53 already clean)
applying it:    76 files changed, 1465 insertions, 634 deletions  (net +831, 0.83% of 75,634 lines)
workspace:      13,604 hunks across 22 crates
```

**The obvious objection does not survive measurement.** The fear is that formatting a port codebase
destroys `git blame` on its dense upstream-citation comments. In fact:

- Only **5 of the 2,087 changed lines are comment lines** — rustfmt's `wrap_comments` is off by
  default, so `///` citation blocks ride along as untouched context.
- Blame is already flat: **55,715 of 75,635 lines (74%) attribute to a single boundary commit**
  (`^b4bcc06`), and the crate's entire history is 25 commits across 13 days.

**It is also not deliberate.** There is no `rustfmt.toml` anywhere; there is no `.github/` directory
at all, so no fmt gate exists to have been skipped; rustfmt is a *declared required component* in
`rust-toolchain.toml` and is installed by `setup.sh`; and exactly one `#[rustfmt::skip]` exists in
the whole workspace (`auth/oauth/sha256.rs`) — the author knows the tool and uses its escape hatch.
The drift is simply that `cargo fmt` has never been run.

**Do not open a companion "add a fmt CI gate" task.** There is no CI to add it to.

## Sequencing

This rewrites nearly every file in the workspace, so it **conflicts with everything**. Run it
either strictly before or strictly after the cyrup-provider decomposition tasks, never beside them.
After is safer: the decompositions require pure code movement verified against a byte-level
baseline, and a reformat landing mid-flight destroys that property.

## Acceptance Criteria

- [ ] `cargo fmt --all` applied in **one commit that does nothing else**
- [ ] `.git-blame-ignore-revs` created at the repo root containing that commit's SHA, with a comment naming it as the formatting baseline
- [ ] `git config blame.ignoreRevsFile .git-blame-ignore-revs` documented in the README or setup script so it is not a private local trick
- [ ] `cargo fmt --all -- --check` exits 0 afterwards
- [ ] The one existing `#[rustfmt::skip]` in `auth/oauth/sha256.rs` still suppresses formatting there
- [ ] `cargo build --workspace --all-targets` — no new errors or warnings versus before the commit
- [ ] `cargo test --workspace` — the same tests pass as before; record both counts
- [ ] No source change of any kind rides along in that commit — verify with a whitespace-blind diff (`git show --ignore-all-space` shows nothing but whitespace)
