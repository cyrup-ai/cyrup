---
stage: new
status: done
updated: 2026-08-22 23:08
---

# Dedupe The Git Subprocess Runner In tests/resources/fixtures.rs

**Owns files:** `crates/cyrup-resources/src/tests/resources/fixtures.rs`

## Description

The recent `tests/resources.rs` decomposition gathered three copies of the same git-subprocess runner
into one file, which made a duplication that was previously spread across 800 lines suddenly visible:

```
 98:    use std::process::Command;          <- inside make_local_git_repo
102:    let git = |args: &[&str]| -> bool {   <- copy 1
127:    use std::process::Command;          <- inside make_local_git_repo_two_commits
131:    let git = |args: &[&str]| -> bool {   <- copy 2 (byte-identical body)
159: pub(super) fn git_in(dir: &Path, args: &[&str]) -> bool {   <- copy 3, already a real fn
```

`git_in` at 159 already does exactly what both closures do; it just takes `dir` as a parameter
instead of capturing it.

**Fix:** delete both closures (102-109, 131-138) and both `use std::process::Command;` lines (98,
127), then rewrite each call site from `git(&[...])` to `git_in(&dir, &[...])`. `git_in` stays where
it is.

This is a test-fixture-only change with no effect on any assertion.

## Acceptance Criteria

- [ ] Exactly one git-subprocess runner remains in the file (`git_in`)
- [ ] No `use std::process::Command;` outside `git_in`
- [ ] `cargo test -p cyrup-resources` unchanged: `103 passed; 0 failed; 1 ignored`
- [ ] The two `make_local_git_repo*` fixtures still return `None` when the `git` CLI is unavailable
