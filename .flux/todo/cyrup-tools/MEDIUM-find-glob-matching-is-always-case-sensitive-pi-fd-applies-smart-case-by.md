---
title: Find glob matching is always case-sensitive; pi (fd) applies smart-case by default
priority: MEDIUM
tool: find
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# Find glob matching is always case-sensitive; pi (fd) applies smart-case by default

## What pi does

pi shells out to `fd` and builds the argv at /home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/find.ts:235 as `["--glob", "--color=never", "--hidden"]`, and never adds `--case-sensitive`/`-s` or `--ignore-case`/`-i` anywhere in the rest of the argv construction (find.ts:235-267). fd's documented default is smart case: an all-lowercase pattern is matched case-INSENSITIVELY, a pattern containing an uppercase character is matched case-sensitively, and this applies to `--glob` patterns too (fd compiles the glob with `case_insensitive(!case_sensitive)`). So `find(pattern: "*.md")` upstream returns `README.MD`, `CHANGELOG.Md`, and `notes.md`.

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tools/src/tools/find.rs:131 calls `PatternMatcher::build(&input.pattern)`. /home/user/cyrup/crates/cyrup-tools/src/tools/globmatch.rs:37-40 builds the glob with `GlobBuilder::new(&effective).literal_separator(full_path).build()` — `.case_insensitive(...)` is never called, so globset's default (case-SENSITIVE) applies unconditionally. A crate-wide `rg 'case_insensitive|smart_case' /home/user/cyrup/crates/cyrup-tools/src` returns exactly one hit, grep.rs:309 (`grep`'s explicit `ignoreCase` parameter), so no smart-case path exists anywhere in the crate for `find`.

## User-visible impact

`find` silently returns fewer results than upstream on any case-varying tree: `*.md` misses `README.MD`, `*.ts` misses files on a case-preserving/case-insensitive filesystem (macOS/Windows), `makefile` misses `Makefile`. The model gets an empty or short result set and concludes the files do not exist. There is no parameter to opt back in, since pi exposes no case flag either.

## Parity action

Compile the find glob with fd's smart-case rule: `GlobBuilder::new(&effective).literal_separator(full_path).case_insensitive(!pattern.chars().any(|c| c.is_uppercase())).build()` in `PatternMatcher::build` (globmatch.rs:37-40). Note the case decision must be taken on the ORIGINAL user pattern, not on `effective` (the auto-prepended `**/` is lowercase and does not change the verdict). Do NOT change `RgGlob`, which reproduces ripgrep's override rule and is case-sensitive upstream.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Confirmed absent after an exhaustive search. globmatch.rs:36-41 `PatternMatcher::build` compiles the glob with `GlobBuilder::new(&effective).literal_separator(full_path).build()` and never calls `.case_insensitive(...)`, so globset's case-SENSITIVE default applies unconditionally. find.rs:131 is the sole call site and find.rs:207 `matcher.is_match(&abs_posix, &basename)` the sole match site; neither the pattern nor the basename is lowercased anywhere on the path. Crate-wide `rg -i 'case_insensitive|smart.?case|to_lowercase|eq_ignore_ascii|ignore_case'` across cyrup-tools/src AND cyrup-core/src yields only grep.rs:23/51/309 (grep's explicit `ignoreCase` parameter) and ls.rs:117-129/229 (collation sort) — no smart-case path exists for `find`, under any name. There is also no fd shell-out fallback: find.rs:1-2 states the tool uses `ignore::WalkBuilder` + `globset` in place of the fd binary, and every other `fd` hit in the crate is a comment or unrelated file-descriptor text. Upstream side verified independently: `rg 'case_sensitive|case_insensitive|smart'` over pi's find.ts returns zero hits and the argv construction at find.ts:235-267 adds only --glob/--color=never/--hidden/--no-require-git/--max-results/--full-path, so fd's documented smart-case default governs (all-lowercase pattern matches case-insensitively, including in --glob mode). I also checked pi's alternate `customOps.glob` branch (find.ts:167-176) to be sure cyrup was not porting that one instead — it is not; it ports the default fd branch. This is a genuinely missing capability, not a different implementation of it.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
