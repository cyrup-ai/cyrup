---
title: Symlinked files inside the search tree are searched by cyrup but skipped by pi
priority: MEDIUM
tool: grep
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# Symlinked files inside the search tree are searched by cyrup but skipped by pi

## What pi does

pi spawns ripgrep with no `--follow`/`-L` (arg list at /home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/grep.ts:220-224, spawn at :226). ripgrep does not follow symlinks during traversal and refuses to search a non-regular-file entry found by the walk. Verified with rg 14.1.0: a directory containing only `link.txt -> ../outside/real.txt` (which contains NEEDLE) yields zero match events from `rg --json --hidden -- NEEDLE .`.

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tools/src/tools/grep.rs:398 accepts every walk item that is not a directory (`Some(Ok(w)) if !w.is_dir`) and feeds it to `search_one`, which opens it with `FsOps::read_stream` (:93) — a symlink entry is not a directory, so it is opened and its target searched. The walker (/home/user/cyrup/crates/cyrup-tools/src/ops/local/fs.rs:209-226) never sets `follow_links`, so the symlink itself is yielded, and nothing filters on file type. Verified by running the tool on that same layout: output is `link.txt:1: NEEDLE`.

## User-visible impact

cyrup returns matches from files outside the search tree and duplicate matches for files reachable by more than one path; those extra hits also consume the 100-match limit. In a repo with symlinked vendor/build trees the result set differs from pi's for the same query.

## Parity action

Skip walk entries whose file type is not a regular file (ripgrep's `Subject::is_file` rule), while still searching a symlink named explicitly as the `path` argument — ripgrep searches explicitly-given paths even when they are links.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Genuine gap — could not refute. cyrup has no file-type filter anywhere in the grep traversal path. grep.rs:398 accepts every walk item with `!w.is_dir`, and WalkItem (ops/mod.rs:252-255) carries ONLY `is_dir`, so symlink-ness is structurally unavailable to any consumer — no filter could exist downstream even under another name. The walker (ops/local/fs.rs:209-240) sets hidden/git_ignore/git_exclude/git_global/require_git/parents but never follow_links, and computes `is_dir = entry.file_type().is_dir()`, collapsing a symlink to `is_dir=false`. search_one then opens it via read_stream = `std::fs::File::open` (fs.rs:73-80), which follows symlinks (no O_NOFOLLOW). Grepped ALL of crates/cyrup-tools/src and crates/cyrup-core/src for symlink|follow_links|is_symlink|read_link|file_type|is_file|is_regular|fifo|socket|device: cyrup-core has zero hits; cyrup-tools' only symlink-aware code is isolation/traversal.rs (opt-in TraversalFs decorator whose canonicalize escape guard applies to path ARGUMENTS, not walk items), lock.rs (mutation-queue keying) and tests/write_semantics.rs (write/edit follow tests) — none on the grep path. Verified both halves empirically: rg 14.1.0 on a dir containing only `link.txt -> ../outside/real.txt` returns matches:0 from `rg --json --hidden -- NEEDLE .` (exit 1 plain; `rg -L` finds it), matching ripgrep's SubjectBuilder which only searches entries whose file_type().is_file(); and a scratch program using the identical ignore::WalkBuilder config yields `tree/link.txt is_dir=Some(false) is_symlink=Some(true)`, which passes `!w.is_dir` and is opened. Severity lowered to medium: nothing is corrupted and the extra hits are real content, but the result set silently diverges (out-of-tree matches, duplicates via multiple paths, link-name labels) and those extras consume the global 100-match budget that breaks the fused walk at grep.rs:373-375, so genuine in-tree matches can be crowded out with no indication in the output — more than cosmetic, less than a wrong-answer/destructive bug. Fix is not local to grep.rs: WalkItem needs a file-type/is_file field populated at fs.rs:231 first. find.rs:208 uses the same flag but must be left alone, since fd does list symlinks and find is already at parity.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
