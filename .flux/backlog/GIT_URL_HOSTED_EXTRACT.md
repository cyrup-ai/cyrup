---
stage: new
status: done
updated: 2026-08-22 23:09
---

# Extract The hosted-git-info Port Out Of package/git_url.rs

**Owns files:** `crates/cyrup-resources/src/package/git_url.rs` ->
`crates/cyrup-resources/src/package/git_url/`

## Description

`src/package/git_url.rs` is 985 lines, of which roughly 561 are a self-contained port of npm's
`hosted-git-info` (the github/gitlab/bitbucket shorthand table and its resolution rules). That port
is a distinct body of ported third-party behavior sitting inside what is otherwise cyrup's own URL
parsing and security validation.

**Fix:** convert to `src/package/git_url/` with:

- **`mod.rs`** — cyrup's own `parse_git_url`, `has_unsafe_git_install_part`, `ParsedGitUrl`,
  the security validation, and the `#[cfg(test)]` module
- **`hosted.rs`** — the hosted-git-info port and its own tests

Re-export from `mod.rs` so `src/package/mod.rs` and `src/lib.rs` are unchanged.

Extract by whole-line range copy; re-derive the ranges from the file at execution time rather than
trusting a number written here.

## Notes

This finding survived adversarial verification, but at lower confidence than the `discovery.rs`
split — the exact seam between "cyrup's parsing" and "the port" was not line-mapped. **Map the seam
before cutting**, and if the two turn out to be interleaved rather than contiguous, stop and report
that instead of forcing a split.

There is one clippy finding in this file already (`git_url.rs:626`, manual `unwrap_or`). Fix it in
passing only if it lands in a file you are already rewriting — otherwise leave it.

## Acceptance Criteria

- [ ] `src/package/git_url.rs` is gone; `src/package/git_url/{mod,hosted}.rs` exist
- [ ] Neither `src/package/mod.rs` nor `src/lib.rs` is modified
- [ ] Test bodies are byte-identical to what they replaced
- [ ] `cargo test -p cyrup-resources` unchanged: `103 passed; 0 failed; 1 ignored`
