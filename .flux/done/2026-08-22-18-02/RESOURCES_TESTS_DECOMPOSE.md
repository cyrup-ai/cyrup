---
stage: qa
status: completed
updated: 2026-08-22 18:58
---
# Repair the `file:line` citations the `resources.rs` split invalidated

## Objective

The 4,450-line `crates/cyrup-resources/src/tests/resources.rs` was split into
[`src/tests/resources/`](../../crates/cyrup-resources/src/tests/resources/) (14 submodules + `mod.rs`).
That refactor is done and verified — 94/94 tests preserved, suite unchanged at
`103 passed; 0 failed; 1 ignored`. **Nothing about the split needs revisiting.**

What it broke is elsewhere: **eleven `file:line` citations** in
[`docs/gap-analysis/05-cyrup-config-and-resources.md`](../../docs/gap-analysis/05-cyrup-config-and-resources.md)
point into the deleted file, and the new module doc pushed a count that same document tracks from
six to seven. Three edits close it — two in that document, one in the new test module.

## Verified citation inventory

Searched `*.md`, `*.rs`, `*.toml`, `*.yml`, `*.yaml` across the repo (excluding `target/`, `.flux/`).
**Exactly one file** carries citations that this refactor invalidated:

| File | Line | Row | What is stale |
| --- | --- | --- | --- |
| `docs/gap-analysis/05-cyrup-config-and-resources.md` | 345 | CFG-052 | `src/tests/resources.rs:2241` |
| `docs/gap-analysis/05-cyrup-config-and-resources.md` | 1490 | CFG-077 *Fix* | `tests/resources.rs:4022` + the "six source sites" count |
| `docs/gap-analysis/05-cyrup-config-and-resources.md` | 1492 | CFG-077 *Verify* | the path + nine `npt_*` line offsets |

Nothing in `spec/`, `.github/` or `xtask/` references this file. The two `spec/flux.md` hits for
`resources.rs` are `cyrup-ext-subagents/src/registration/resources.rs` — a different file.

### Out of scope — do not touch

These cite `cyrup-resources/tests/resources.rs`, the pre-`src/` path. They were already stale before
the split and fixing them is a different task:

- `docs/TEST-ARCHITECTURE.md:624`, `:919` (and the `:114` table, a point-in-time triage snapshot)
- `docs/gap-analysis/05-cyrup-config-and-resources.md:412`, `:454`, `:713`
- `docs/gap-analysis/07-cyrup-tui.md:2174`
- `crates/cyrup-ext-subagents/src/discovery/skills.rs:90`, `:758`

## The line-number mapping is a constant shift — verified

Every chunk moved as one contiguous block, so each file's citations shift by a single constant.
For the nine `npt_*` tests the shift is **exactly 4008** in all nine cases:

| Original | New | Test |
| --- | --- | --- |
| `:4033` | `:25`  | `npt_namespaced_names_case_and_expansion` |
| `:4089` | `:81`  | `npt_skip_rules_dir_names_only` |
| `:4123` | `:115` | `npt_depth_cap_warns_once_per_refused_dir` |
| `:4175` | `:167` | `npt_symlink_policy` |
| `:4225` | `:217` | `npt_load_with_root_derivation_edges` |
| `:4293` | `:285` | `npt_load_with_root_non_utf8_components_error` |
| `:4320` | `:312` | `npt_load_error_becomes_warning` |
| `:4352` | `:344` | `npt_precedence_shadowing_and_case_collision` |
| `:4375` | `:367` | `npt_all_directory_and_single_file_call_sites` |

4008 = 4021 (the chunk's first source line) − 13 (where it lands, after the 12-line header). The nine
independently-derived deltas all agreeing is itself proof that no content drifted inside that chunk.

`cfg052_a_github_shorthand_is_a_local_path_exactly_as_upstream_leaves_it` shifts by 2048
(2054 − 6): `:2241` → **`git_url.rs:193`**.

> **The 4008 shift holds only while `prompt_namespaces.rs` keeps a 12-line header.** Edit 1 below is
> deliberately line-count preserving for this reason. Do Edit 1 first, then re-derive with the
> command in *Verification* before writing Edits 2 and 3 — never copy the table blind.

## Edit 1 — drop the duplicated spec citation from the new module doc

`crates/cyrup-resources/src/tests/resources/prompt_namespaces.rs` lines 1-2 currently read:

```rust
//! Namespaced prompt templates — recursive prompt scan, skip rules, depth cap, symlink policy
//! (spec/namespaced-prompt-templates.md). [CYRUP-DELTA]
```

The section banner at `:14` — original content, untouched by the split — already carries that
citation in full, along with the Pi divergence and the code-puppy reference. The module doc repeating
it is what took the count from six to seven.

Replace those **two lines with exactly two lines**:

```rust
//! Namespaced prompt templates — recursive prompt scan, skip rules, depth cap, symlink policy.
//! [CYRUP-DELTA] over Pi's flat scan; the governing spec citation is on the banner below.
```

Fix the count by removing the duplicate, **not** by editing "six" to "seven" in the document: the
five sites CFG-077 enumerates in `discovery.rs` and `prompt.rs` are still exact, and a test module
is not a *source* site for a spec citation in the sense that row means.

## Edit 2 — `docs/gap-analysis/05-cyrup-config-and-resources.md:345` (CFG-052)

One replacement on that line:

| from | to |
| --- | --- |
| `crates/cyrup-resources/src/tests/resources.rs:2241` | `crates/cyrup-resources/src/tests/resources/git_url.rs:193` |

## Edit 3 — `docs/gap-analysis/05-cyrup-config-and-resources.md:1490` and `:1492` (CFG-077)

**Line 1490** enumerates the six sites as
``(`discovery.rs:1753`, `:1759`; `prompt.rs:15`, `:34`, `:57`; `tests/resources.rs:4022`)``. The first
five are still correct — verified against the files. Replace only the sixth, matching the row's
existing crate-relative style:

| from | to |
| --- | --- |
| `` `tests/resources.rs:4022` `` | `` `tests/resources/prompt_namespaces.rs:14` `` |

`:14` is the banner line, which after Edit 1 is this file's single citation site.

**Line 1492** needs the path plus all nine offsets:

| from | to |
| --- | --- |
| `crates/cyrup-resources/src/tests/resources.rs` | `crates/cyrup-resources/src/tests/resources/prompt_namespaces.rs` |
| `` `:4033` `` … `` `:4375` `` | the nine values in the mapping table above |

Restrict every substitution to its own line — these four-digit values are unique on line 1492, but a
file-wide `sed` would be wrong. Leave the prose, the row's argument and every other citation on both
lines untouched; this is a pointer repair, not a rewrite.

## Verification

Re-derive the numbers from the files rather than trusting the table:

```bash
# Edit 1 landed and the header is still 12 lines (banner on 13/14, so the 4008 shift holds):
sed -n '1,2p;13,14p' crates/cyrup-resources/src/tests/resources/prompt_namespaces.rs

# The nine npt_* lines the doc must now cite:
grep -n '^\(async \)\?fn npt_' crates/cyrup-resources/src/tests/resources/prompt_namespaces.rs

# The CFG-052 test line:
grep -n 'fn cfg052_a_github_shorthand' crates/cyrup-resources/src/tests/resources/git_url.rs

# Citation-site count is back to six:
grep -rc 'spec/namespaced-prompt-templates\.md' crates/cyrup-resources/src/discovery.rs \
  crates/cyrup-resources/src/prompt.rs \
  crates/cyrup-resources/src/tests/resources/prompt_namespaces.rs | awk -F: '{s+=$2} END{print s}'

# No citation anywhere still points at the deleted file:
grep -rn 'src/tests/resources\.rs' --include='*.md' --include='*.rs' . | grep -v target | grep -v '\.flux'
```

## Definition of done

- [ ] `prompt_namespaces.rs` lines 1-2 no longer cite `spec/namespaced-prompt-templates.md`, and the
      file is still 442 lines with its banner on `:13`/`:14`
- [ ] The count command prints **6**
- [ ] The `src/tests/resources.rs` grep returns **nothing**
- [ ] Every `npt_*` offset on line 1492 matches `grep -n` output exactly; same for `git_url.rs:193`
      on line 345 and `prompt_namespaces.rs:14` on line 1490
- [ ] `cargo test -p cyrup-resources` still reports `103 passed; 0 failed; 1 ignored`
- [ ] Only two files changed: `prompt_namespaces.rs` and
      `docs/gap-analysis/05-cyrup-config-and-resources.md`
