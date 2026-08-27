---
title: Glob: "!dir" prunes the whole directory in pi, but only filters files in cyrup
priority: MEDIUM
tool: grep
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# Glob: "!dir" prunes the whole directory in pi, but only filters files in cyrup

## What pi does

pi hands `glob` to real ripgrep verbatim (`args.push("--glob", glob)`, /home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/grep.ts:223, spawned at :226). ripgrep evaluates the override against BOTH files and directories, so a negated glob whose pattern names a directory ignores that directory and prunes the entire subtree. Verified against rg 14.1.0: with `node_modules/pkg/a.js` and `src/b.js` both containing NEEDLE, `rg --json --hidden --glob '!node_modules' -- NEEDLE .` returns only `./src/b.js`.

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tools/src/tools/grep.rs:398-414 only reaches the glob test for non-directory walk items (`Some(Ok(w)) if !w.is_dir` … `g.keeps_file(&glob_rel)`); directories are matched by the arm at :427 (`Some(Ok(_)) => {}`) and are never tested against the override, so the subtree is still descended. `RgGlob` (/home/user/cyrup/crates/cyrup-tools/src/tools/globmatch.rs:155-163) exposes only `keeps_file`, with no directory-matching entry point, and `only_dir` merely makes files never match. Verified by running the tool: `{"pattern":"NEEDLE","glob":"!node_modules"}` returns `src/b.js:1: NEEDLE` AND `node_modules/pkg/a.js:1: NEEDLE`.

## User-visible impact

The most common exclusion idiom (`!node_modules`, `!target`, `!dist`) silently does nothing in cyrup: excluded trees are still searched and their hits consume the 100-match cap, crowding out the real results the user asked for.

## Parity action

Give `RgGlob` a directory-matching path (ripgrep `Override::matched(path, is_dir=true)`: a negated/ignore glob that matches a directory prunes it; a whitelist miss on a directory does NOT prune) and apply it to `is_dir` walk items in grep.rs so the walk skips the subtree, matching ripgrep's override semantics.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Confirmed absent after searching all of cyrup-tools/src and cyrup-core/src. RgGlob (crates/cyrup-tools/src/tools/globmatch.rs:85-167) is a port of ignore-0.4.33 Override::matched(path, is_dir) that DROPPED the is_dir parameter: its only entry point is keeps_file(&self, rel_posix) at :162, and only_dir at :165 is used solely to force hit=false ("a dir-only glob never matches") rather than "matches when the candidate is a directory" — so glob:"!src/" is a no-op too. grep.rs:398-414 consults the glob only in the `Some(Ok(w)) if !w.is_dir` arm; directories fall into `Some(Ok(_)) => {}` at :427 untested, so the subtree is still descended. No pruning exists upstream either: WalkOpts (ops/mod.rs:245-249) carries only include_hidden and require_git, and the local walker (ops/local/fs.rs:209-234) builds WalkBuilder with hidden/git_ignore/git_exclude/git_global/require_git/parents and never calls .overrides(...) — the one `ignore` API that applies an override to directories; the FsOps trait cannot carry a glob to the walker at all. rg -n for keeps_file|keeps_dir|RgGlob|only_dir|overrides|Override across both crates finds RgGlob referenced only at grep.rs:334/412 and its own tests; find.rs uses the unrelated fd-rule PatternMatcher. Semantics check out: "!node_modules" compiles to "**/node_modules" with literal_separator(true), which does not match node_modules/pkg/a.js, so hit=false, negated=true, keeps_file=true — file kept. The existing test negation_inverts_and_backslash_escapes_it (globmatch.rs:259-265) covers only file patterns like "!*.ts", which is why this was never caught. Severity lowered from the claim's framing to medium: the walker honors .gitignore/.git/info/exclude/global gitignore, so the three idioms cited (node_modules, target, dist) are usually already pruned inside a git repo regardless of the glob. The defect bites for non-ignored trees (checked-in vendor/, third_party/, generated or fixture dirs) and for searches outside a git repo, where require_git:true (grep.rs:372) disables gitignore entirely and "!dir" is the only exclusion available — there the filter is a total no-op and the fused walk/search can consume the 100-match cap inside the excluded tree, hiding real results. Under-filtering with visible unwanted paths plus occasional cap-crowding is more than cosmetic but short of data corruption.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
