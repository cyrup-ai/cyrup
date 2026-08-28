---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/find.rs:1"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-tools/src/tools/find.rs:1`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi spawns the real fd binary (find.ts:225 `ensureTool("fd")`, :269 `spawn(fdPath, args)`), downloading it if missing, and exposes `FindOperations { exists, glob }` (find.ts:55-71) so an extension can supply server-side globbing (`if custom operations provide glob(), use that instead of fd`, :168).

## What cyrup does

In-process `ignore::WalkBuilder` + `globset`, driven through `FsOps::walk`.

## What a caller sees

(a) fd's own glob dialect and traversal rules are replaced by globset/ignore — divergence here is version-dependent and unbounded rather than pinned. (b) pi's `fd is not available and could not be downloaded` / `Failed to run fd: ...` / `fd exited with code N` errors never occur. (c) pi's `FindOperations.glob` seam lets a remote backend do the glob remotely and return paths; cyrup's `FsOps::walk` forces enumeration-then-match, so a remote backend transfers the whole listing. See also path.rs:161 — the fd global-ignore file cyrup reproduces by hand is where these two diverge concretely on Windows.

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
