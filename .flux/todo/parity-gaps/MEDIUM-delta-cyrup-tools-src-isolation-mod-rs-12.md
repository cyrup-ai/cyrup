---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/isolation/mod.rs:12"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-tools/src/isolation/mod.rs:12`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi has no protected-path concept anywhere. `core/tools/write.ts:195-225` writes whatever absolute path it is handed, including `.env`, `.git/`, `node_modules/`.

## What cyrup does

Ships `ProtectedFs`/`ProtectedPaths`, a backend-seam decorator that blocks writes/edits to those paths. Off by default (`SessionConfig::protect_paths: false`, ADR-0003 D5).

## What a caller sees

A divergence in the ADD direction: with `protect_paths: true` an embedder's agent gets a hard error writing `.env` where pi would have written the file. Also note the decorator covers only the fs seam — `bash 'echo x >> .env'` still succeeds — so the guard an embedder switches on is partial, which is itself a caller-visible property. Flagging because an added, partial safety mechanism absent upstream is exactly the kind of scope change that needs your sign-off, not an agent's.

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
