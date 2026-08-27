---
title: .rgignore files are honored by pi but ignored by cyrup
priority: LOW
tool: grep
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: aug
status: in-progress
updated: 2026-08-27
---

# .rgignore files are honored by pi but ignored by cyrup

## What pi does

pi searches with real ripgrep (/home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/grep.ts:177, :226), which registers `.rgignore` as a custom ignore filename in addition to `.ignore`/`.gitignore`. Verified with rg 14.1.0 in a git repo containing `a.txt`, `b.txt` and `.rgignore` = `b.txt`: `rg --json --hidden -- NEEDLE .` reports only `./a.txt`.

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tools/src/ops/local/fs.rs:213-226 builds the walker with `hidden`, `git_ignore`, `git_exclude`, `git_global`, `require_git`, `parents` and never calls `add_custom_ignore_filename(".rgignore")`; `grep` uses this walker at /home/user/cyrup/crates/cyrup-tools/src/tools/grep.rs:367-373. Verified by running the tool on the same layout: both `a.txt` and `b.txt` are searched. (`.ignore` IS honored on both sides — verified — so this is specific to `.rgignore`.)

## User-visible impact

A repository that uses `.rgignore` to keep generated or vendored files out of searches gets those files back in cyrup's results, and they consume the 100-match cap.

## Parity action

Add `.rgignore` as a custom ignore filename on the walker used by `grep` (ripgrep-only; `find`/fd does not read it, so it must not be added unconditionally for the find walker).

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Could not refute. Zero occurrences of `rgignore` or `add_custom_ignore_filename` anywhere in /home/user/cyrup/crates. cyrup-tools has exactly one WalkBuilder (ops/local/fs.rs:213-226) setting only hidden/git_ignore/git_exclude/git_global/require_git/parents; `.ignore` works solely via the `ignore` crate's `ignore(true)` default, while custom ignore filenames default to empty. grep.rs:367-373 uses that same walker in-process (no `rg` subprocess exists in the crate), so there is no alternate code path that could pick `.rgignore` up. The only filename-enumerating ignore code in the repo is a different subsystem (cyrup-resources/src/discovery/scan.rs:172, [".gitignore",".ignore",".fdignore"]) and it omits `.rgignore` as well. Vendored ignore-0.4.33 (src/walk.rs:653) confirms `.rgignore` is opt-in via add_custom_ignore_filename, which cyrup never calls. Severity corrected down to low: every match cyrup returns is still a genuine match in a real file — nothing is silently wrong — the only user-visible harm is extra files consuming the 100-match cap, and that requires a repo using `.rgignore` specifically, while `.gitignore` and `.ignore` (the near-universal choices) are honored identically on both sides.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
