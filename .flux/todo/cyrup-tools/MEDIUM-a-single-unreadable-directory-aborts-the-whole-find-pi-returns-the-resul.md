---
title: A single unreadable directory aborts the whole find; pi returns the results it collected
priority: MEDIUM
tool: find
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# A single unreadable directory aborts the whole find; pi returns the results it collected

## What pi does

/home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/find.ts:297-310: on fd exit, `if (code !== 0) { const errorMsg = stderr.trim() || `fd exited with code ${code}`; if (!output) { settle(() => reject(new Error(errorMsg))); return; } }` — the error is only surfaced when fd produced NO output at all. fd itself continues traversing past directories it cannot read, so a permission-denied subtree (or an EIO / dangling mount) costs pi nothing: the matching paths fd did emit are relativized (find.ts:321-326) and returned as a normal successful result.

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tools/src/tools/find.rs:212 — inside the walk loop, `Some(Err(e)) => return Err(e)`. The producing walker at /home/user/cyrup/crates/cyrup-tools/src/ops/local/fs.rs:227-239 maps every per-entry `ignore::Error` into `Err(ToolError::new(format!("walk: {e}")))` and pushes it down the same channel as real results, so the first unreadable directory the walk touches makes `find` return a `walk: <path>: Permission denied (os error 13)` error and DISCARD every path already accumulated in `results` (find.rs:145, 209).

## User-visible impact

Running `find` anywhere above a directory the agent cannot read (a root-owned subtree, a stale NFS mount, a permissions-stripped `.git/objects`) fails the whole tool call with a `walk: …` error instead of returning the matches. Upstream the same call succeeds and returns the files. The error string `walk: …` also carries none of pi's stable message prefixes, so callers cannot classify it.

## Parity action

In find.rs's walk loop, treat a per-entry walk error as non-fatal the way fd does: `Some(Err(_)) => continue`, and keep collecting. Only surface an error when the walk produced no rows at all (mirroring pi's `if (!output)` guard at find.ts:306). The same one-line change is needed in grep.rs:428, which has the identical `Some(Err(e)) => return Err(e)`.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Searched all of crates/cyrup-tools/src and crates/cyrup-core/src for any error-tolerant walk path (Some(Err, is_partial, ignore::Error, Permission, skip/continue-on-error, every `fn walk` impl) and found none. LocalFs::walk (ops/local/fs.rs:227-239) is the only real producer and maps every per-entry ignore::Error into Err(ToolError "walk: {e}") down the same channel as results; the only two consumers in the tree, find.rs:212 and grep.rs:428, both do `Some(Err(e)) => return Err(e)`, dropping the locally accumulated `results` Vec (find.rs:145). The wrapper FsOps impls (isolation/protected.rs:150, isolation/traversal.rs:133) only delegate. Confirmed in the vendored ignore-0.4.26 (src/walk.rs:1098-1163) that Walk::next yields Some(Err(...)) for an unopenable directory and then continues traversal, so the error genuinely arrives mid-stream with valid entries both before and after it. Pi's find.ts gates its rejection on `if (!output)`, so any fd output at all returns a normal success. No equivalent tolerance exists anywhere in the Rust, and there is no test covering an unreadable directory for find or grep (the 0o300 fixtures cover ls/edit only). Capability genuinely absent. Severity lowered to medium: the failure is loud (a `walk: …` error, not silently wrong results) and requires an unreadable subtree under the search root, which is uncommon inside a normal project checkout.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
