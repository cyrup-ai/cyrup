---
title: Pi aborts a read the instant the signal fires; cyrup checks the cancel token exactly once and never during the file read
priority: LOW
tool: read
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# Pi aborts a read the instant the signal fires; cyrup checks the cancel token exactly once and never during the file read

## What pi does

read.ts:232-241 rejects immediately if `signal.aborted` on entry and installs `signal.addEventListener("abort", onAbort, {once:true})` whose handler rejects the promise with `new Error("Operation aborted")` at any moment — including while `ops.readFile(absolutePath)` is still in flight. Guards at read.ts:246, 249 and 325 additionally stop the async body from resolving after an abort.

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tools/src/tools/read.rs:137-139 is the only `cancel.is_cancelled()` test in the whole tool; it sits between the `access` precheck and `self.fs.read(&abs)` (read.rs:143). There is no check after the read, none inside the image branch (read.rs:287-377), and no `select!` on the cancel token, so once `fs.read` starts the call always runs to completion. `error::aborted()` (/home/user/cyrup/crates/cyrup-tools/src/error.rs:117-119) does produce pi's `"Operation aborted"` text when it is reached.

## User-visible impact

Cancelling (Esc / interrupt) during a read of a very large file — or any read over a slow remote `FsOps` backend — returns "Operation aborted" promptly in pi, whereas cyrup finishes the read and returns the full successful result, so the cancelled turn still consumes the file content.

## Parity action

Race the `fs.read` (and the image-processing step) against the cancel token (`tokio::select!` on `cancel.cancelled()`), and re-check `cancel.is_cancelled()` before returning `Ok`, so a cancel arriving at any point yields `error::aborted()`.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Could not refute. `cancel` appears exactly twice in /home/user/cyrup/crates/cyrup-tools/src/tools/read.rs — the `execute` parameter (line 98) and one `is_cancelled()` at line 137, between the R_OK precheck and `self.fs.read(&abs)` (line 143). There is no post-read check, none in `read_image` (line 279+), and no `select!`. The capability cannot come from below either: `FsOps::read` (ops/mod.rs:323) takes no CancelToken, and neither `LocalFs::read` (= `tokio::fs::read`, ops/local/fs.rs:63-67) nor the `protected.rs:102` / `traversal.rs:89` decorators can observe a cancel. Nor from above: cyrup-agent/src/agent/run/tools/exec.rs awaits `tool.execute` to completion in both modes — the parallel path drains its JoinSet, and the sequential `tokio::select!` (lines 258-283) races only the update channel against `exec`, never the token; `is_cancelled()` is consulted only BETWEEN calls (exec.rs:90, 328). `wrap_registered_tool` (cyrup-ext/src/wrapper.rs:146) plainly forwards. Decisively, the same capability IS implemented for every sibling: biased `select!` on `cancel.cancelled()` in find.rs:176-184 and grep.rs:394-395, and post-mutation rechecks in write.rs:119 / edit.rs:280 (TOOL-041, tested in tests/pi_tool_semantics.rs:265,300). `read` simply never got it. Severity lowered to low: the content returned is correct, there is no side effect left inconsistent (unlike write/edit), the run still ends as StopReason::Aborted, and the window is bounded by a single `fs.read` — milliseconds for a local file. The only user-visible difference is that a cancelled turn's transcript carries the file text instead of "Operation aborted" (plus its token cost), which matters mainly for a slow remote FsOps backend.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
