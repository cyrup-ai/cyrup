---
title: Find accepts a search path that is a regular file and reports "No files found"; pi rejects it
priority: MEDIUM
tool: find
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# Find accepts a search path that is a regular file and reports "No files found"; pi rejects it

## What pi does

In pi's default (fd) branch there is NO existence/type pre-check at all — `ops.exists` is only called in the custom-operations branch (/home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/find.ts:170-172). The absolute search path is handed to fd as its root (find.ts:267 `args.push("--", effectivePattern, searchPath)`), and fd validates it: a path that is missing or is not a directory makes fd write `[fd error]: Search path '<p>' is not a directory.` to stderr and exit non-zero with no stdout, so find.ts:304-309 rejects with that exact stderr text.

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tools/src/tools/find.rs:123-129 only checks that `fs.metadata(&search_root)` succeeds, mapping failure to `Path not found: <p>`; there is no `meta.is_dir` check (contrast /home/user/cyrup/crates/cyrup-tools/src/tools/ls.rs:93-98, which does have `if !meta.is_dir { … "Not a directory: …" }`). A regular-file search root therefore passes, `fs.walk` yields exactly that one path, find.rs:188-190 skips it as `w.path == search_root`, and the tool falls into the empty branch at find.rs:223-230.

## User-visible impact

`find(pattern: "*.ts", path: "src/index.ts")` — the model mistakenly passing a file instead of a directory — returns the successful text `No files found matching pattern` in cyrup, so the model believes the tree is empty and stops looking. Upstream the same call raises an error naming the mistake, which the model can correct. Separately, for a genuinely missing path the two sides emit different error text (`Path not found: <p>` vs fd's `[fd error]: Search path '<p>' is not a directory.`).

## Parity action

After the metadata call in find.rs:123-129, reject a non-directory root the way fd does, e.g. `if !meta.is_dir { return Err(error::invalid(format!("[fd error]: Search path '{}' is not a directory.", error::show(&search_root)))) }`, and use the same fd-shaped text for the missing-path case so both errors match what pi surfaces from fd's stderr.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Confirmed absent after exhaustive search. crates/cyrup-tools/src/tools/find.rs:122-129 calls fs.metadata(&search_root) purely for its Err arm (Path not found: <p>) and discards the Metadata value — is_dir is never consulted. Repo-wide ripgrep for is_dir across crates/cyrup-tools/src and crates/cyrup-core/src returns only one tool-level directory guard, ls.rs:93-98 (Not a directory: …); every other hit is the Metadata/WalkItem field itself (ops/mod.rs:37,254), its population (ops/local/fs.rs:166,178,231-232), find.rs:208's trailing-slash formatting, or grep.rs:398's dir-skip while walking. Searches for "Not a directory|ENOTDIR|NotADirectory" find only ls.rs and lock.rs's isMissingPathError errno set (unreachable from find). No wrapper supplies it: isolation/traversal.rs:133-142 walk only confines against root escape, isolation/protected.rs:150-156 walk is a passthrough, and ops/local/fs.rs:209-241 builds WalkBuilder::new(&root) unconditionally — a file root yields exactly one entry, which find.rs:188-190 drops as w.path == search_root, falling into the empty branch at find.rs:223-230 and returning the success text "No files found matching pattern". Upstream verified: find.ts's ops.exists guard is inside the customOps?.glob branch only; the default branch hands searchPath to fd (find.ts:267) and rejects on non-zero exit with trimmed stderr, and fd rejects a non-directory search path. Nuance: pi's own custom-ops glob branch is exists-only and behaves exactly like cyrup, so the divergence is against the fd branch that cyrup's comments mirror. Severity medium, not high: it requires the model to pass a file where a directory is expected, and the harm is a misleading success (model concludes the tree is empty) rather than data loss; the separate error-text difference for a genuinely missing path is cosmetic.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
