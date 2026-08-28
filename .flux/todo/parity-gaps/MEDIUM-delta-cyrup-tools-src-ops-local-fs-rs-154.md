---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/ops/local/fs.rs:154"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-tools/src/ops/local/fs.rs:154`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi's write tool declares `WriteOperations { writeFile, mkdir }` (core/tools/write.ts:31-37) — TWO members — with `defaultWriteOperations` at :38-41 and `WriteToolOptions.operations` at :43-46. `WriteOperations` is a public export of the extension API (packages/coding-agent/src/index.ts:331). `execute` calls `await ops.mkdir(dir)` then `throwIfAborted()` then `await ops.writeFile(...)` (write.ts:221-225).

## What cyrup does

`FsOps` has no `mkdir` member at all (ops/mod.rs:395-458). `LocalFs::write_in_place` folds `tokio::fs::create_dir_all(parent)` into the write.

## What a caller sees

Two observable consequences. (1) A backend/extension supplier can override `writeFile` but cannot override, intercept, or suppress `mkdir` independently — a remote/SSH or read-only-audit backend that in pi could refuse directory creation while allowing writes has no seam in cyrup. (2) pi's abort check between mkdir and writeFile is gone: a write cancelled in that window leaves pi with the directory created and no file, cyrup with both done. Error contexts also differ (`mkdir <path>` vs pi's raw Node mkdir rejection).

## Decision required

One of:

1. **Close it** — bring cyrup to pi's behaviour.
2. **Accept it** — David explicitly accepts the divergence; the marker stays and is
   annotated as authorized, with the reason.
3. **Reshape it** — the divergence is right but the current form is wrong.

Do not silently keep option 2 by leaving the marker as-is; that is how this became a
backlog in the first place.

## Definition of done

1. The gap is closed, or the marker records an explicit authorized acceptance.
2. If closed, a test fails without the change.
3. No behaviour regression in the owning crate.
