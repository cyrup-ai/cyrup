---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/ops/mod.rs:539"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-tools/src/ops/mod.rs:539`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi exposes `BashOperations` (packages/coding-agent/src/core/tools/bash.ts:62-80 @e8682309) as a PUBLIC extension type, and an extension returns one from a `user_bash` handler via `UserBashEventResult.operations` (core/extensions/types.ts:1117-1122) or from `BashToolOptions.operations`. `executeBash` resolves `options?.operations ?? createLocalShellOperations(...)` on every invocation, so a JS extension can redirect command execution to SSH/a container/a remote host.

## What cyrup does

The host-side trait exists and the consumer side is wired (`BashOptions::operations` -> `execute_bash`), but there is no WIT round-trip: `crates/cyrup-ext/wit/world.wit:345-346` `on-user-bash` returns a `hook-outcome` only; there is no registration import and no keyed dispatch export, so a WASM guest has nothing callable to register. `crates/cyrup-ext/src/lib.rs:100-120` states this openly (DRIFT-004 / SEAM-015). NOTE: the marker is tagged `[CYRUP-DELTA, mechanism]` — that tag is wrong.

## What a caller sees

CONFIRMED capability gap (this is the second item you asked about; the brief calls it `FsOps` but the trait is actually `BashOperations` — the substance is the same). An extension author porting a pi `user_bash` handler that returns `operations` gets: the JSON key survives into `UserBashReduction::Handled`, and then nothing happens — the command runs on the local host shell. A pi extension that transparently ran the user's shell over SSH is not expressible as a cyrup WASM extension at all. Only in-host Rust callers can supply a backend.

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
