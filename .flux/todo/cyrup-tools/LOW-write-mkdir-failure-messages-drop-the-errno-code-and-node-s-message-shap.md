---
title: Write/mkdir failure messages drop the errno code and Node's message shape
priority: LOW
tool: write
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# Write/mkdir failure messages drop the errno code and Node's message shape

## What pi does

pi's write ops are raw Node calls — `fsWriteFile(path, content, "utf-8")` (write.ts:39) and `fsMkdir(dir, {recursive:true})` (write.ts:40) — and their rejections propagate out of `execute` (write.ts:221/225) untouched. The agent loop surfaces `error.message` verbatim as the isError text (packages/agent/src/agent-loop.ts:661-665), so the model and the user see Node's SystemError message, which always leads with the errno code, e.g. `EACCES: permission denied, open '/x'`, `EISDIR: illegal operation on a directory, open '/x'`, `ENOSPC: no space left on device, write`.

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tools/src/tools/write.rs:108 propagates whatever `FsOps::write_in_place` returns, and the local backend wraps every failure with the plain `error::io` helper (/home/user/cyrup/crates/cyrup-tools/src/ops/local/fs.rs:101-102 `create dir {path}: {e}`, :106-107 and :110-111 and :115-116 `write {path}: {e}`), producing e.g. `write /x: Permission denied (os error 13)`. `error::io` is `format!("{context}: {e}")` with no code (/home/user/cyrup/crates/cyrup-tools/src/error.rs:22-24); the crate's own `error::io_errno` helper exists precisely to prepend Node's errno name (error.rs:26-35, :77-84) and is used for `access` and `read_dir` (fs.rs:145-148, :181-183) but NOT for the write path.

## User-visible impact

On a failed write the model/user gets `write /x: Permission denied (os error 13)` instead of pi's `EACCES: permission denied, open '/x'` — the errno token the model uses to distinguish EACCES / EROFS / EISDIR / ENOSPC / ENOTDIR is absent, and the message text differs from upstream for every failure mode of the tool.

## Parity action

Use `error::io_errno` (which already exists for exactly this reason) instead of `error::io` in `LocalFs::write_in_place` for the `create_dir_all`, `open`, `write_all` and `flush` failures, so the message leads with the errno name as Node's does.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Searched all of cyrup-tools/src and cyrup-core/src for the capability under any name. The errno-prefixing capability exists (error.rs:38-95 errno_name/io_errno/io_errno_code, shape `CODE: context: display`) but is wired to exactly six call sites — fs.rs:48, :149, :165, :191, :203 and lock.rs:159 — none of them the write path. LocalFs::write_in_place (ops/local/fs.rs:97-121) uses plain error::io on all four edges: create_dir_all :103, open :111, write_all :114, flush :119. No FsOps decorator re-wraps it (isolation/traversal.rs:109-111 and isolation/protected.rs:126-128 both delegate unchanged), tools/write.rs:108 propagates with a bare `?`, and there is no errno normalization anywhere in cyrup-core or cyrup-agent (no hits for errno_name/raw_os_error/ENOSPC/EROFS outside cyrup-tools). Partial mitigation only: the mutation-lock realpath keying (lock.rs:153-161) does emit an errno prefix for the pre-write classes it catches (EACCES on a parent's search bit, ELOOP, ENAMETOOLONG), but it deliberately swallows ENOENT/ENOTDIR and never sees EACCES-on-file, EISDIR, EROFS or ENOSPC, which all originate inside write_in_place. Severity corrected to low: the write still fails, the failure still reaches the model as isError, and Rust's Display carries the same semantic content plus the raw errno number (`Permission denied (os error 13)`, `Is a directory (os error 21)`, `No space left on device (os error 28)`), so the classes remain distinguishable. No consumer branches on the token for write — errno_code_of is read only by edit.rs:240 for its own access precheck, and FATAL_BASH_PATTERNS (cyrup-ext-subagents/src/exec/output.rs:420-432) is bash-only and matches "permission denied" case-insensitively regardless. Nothing is silently wrong; the divergence is message wording versus upstream.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
