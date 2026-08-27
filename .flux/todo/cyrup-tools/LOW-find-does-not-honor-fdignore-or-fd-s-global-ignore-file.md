---
title: Find does not honor .fdignore or fd's global ignore file
priority: LOW
tool: find
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: aug
status: in-progress
updated: 2026-08-27
---

# Find does not honor .fdignore or fd's global ignore file

## What pi does

pi runs the real `fd` binary (/home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/find.ts:225 `const fdPath = await ensureTool("fd")`, resolved from the system `fd`/`fdfind` or downloaded from `sharkdp/fd` per /home/user/cyrup/tmp/pi/packages/coding-agent/src/utils/tools-manager.ts:30-48) and never passes `--no-ignore` or any of its variants (find.ts:235-267). fd's default ignore set is therefore in force, which — beyond `.gitignore`, `.git/info/exclude` and the global gitignore — also includes `.fdignore` files found in the tree and fd's own global ignore file at `$XDG_CONFIG_HOME/fd/ignore`.

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tools/src/ops/local/fs.rs:213-226 configures the walker with `.hidden(!include_hidden).git_ignore(true).git_exclude(true).git_global(true).require_git(opts.require_git).parents(true)` only. `ignore::WalkBuilder::add_custom_ignore_filename` is never called — `rg 'fdignore|custom_ignore|add_custom_ignore' /home/user/cyrup/crates/cyrup-tools/src` returns nothing — so `.fdignore` files and fd's global ignore file have no effect.

## User-visible impact

A repository that uses `.fdignore` to exclude build output, vendored trees or large generated directories from agent searches has those exclusions silently ignored: `find` returns the excluded paths, wasting the result budget (default 1000) and the 50KB byte cap on files the user deliberately hid.

## Parity action

Add `.add_custom_ignore_filename(".fdignore")` to the `WalkBuilder` in fs.rs:213-226 (and, for full fd parity, load `$XDG_CONFIG_HOME/fd/ignore` — falling back to `~/.config/fd/ignore` — via `WalkBuilder::add_ignore`). Gate it on the find call path if grep must keep ripgrep's ignore set, since `grep` (grep.rs:367-373) shares the same `walk` seam and ripgrep does not read `.fdignore`.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Could not refute. The only WalkBuilder in the tool path is crates/cyrup-tools/src/ops/local/fs.rs:213-224, configured with .hidden/.git_ignore(true)/.git_exclude(true)/.git_global(true)/.require_git/.parents(true) and nothing else; add_custom_ignore_filename and add_ignore are never called anywhere in crates/ (rg for add_custom_ignore|custom_ignore|fdignore|fd/ignore hits only crates/cyrup-resources/src/discovery/scan.rs:172, a separate hand-rolled skill-discovery scanner that never uses FsOps::walk). WalkOpts (ops/mod.rs:245-248) exposes only include_hidden and require_git, and FindOpts (config.rs:287-290) only limit and max_bytes, so no caller or config can inject ignore filenames. cyrup-core has no WalkBuilder/ignore:: usage at all. On the Pi side find.ts:235 builds only ["--glob","--color=never","--hidden"] plus --no-require-git/--max-results/--full-path, never --no-ignore, so fd's default ignore set is in force. SCOPE CORRECTION: ignore 0.4.26 (Cargo.lock:3883) defaults WalkBuilder::ignore to true and fs.rs never disables it, so cyrup DOES honor plain `.ignore` files like fd; the real delta is only `.fdignore` files and fd's global $XDG_CONFIG_HOME/fd/ignore. SEVERITY: low — `.fdignore` is a rare fd-specific file, the common tool-agnostic `.ignore` plus all gitignore sources are already honored, and nothing is silently wrong: find returns a superset of real correct paths. Harm requires the compound case where a repo uses `.fdignore` for exclusions not already covered by `.gitignore`/`.ignore` AND the extra noise pushes a wanted match past the 1000-result/50KB cap.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
