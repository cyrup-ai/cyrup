---
title: "CYRUP-DELTA capability gap at crates/cyrup-provider/src/api/google_vertex.rs:41"
priority: MEDIUM
crate: cyrup-provider
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-provider/src/api/google_vertex.rs:41`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi throws bare `Error`s from this module; the catch runs `formatProviderError(normalizeProviderError(error))`, which returns `error.message` unchanged. The user sees e.g. `Vertex requires a project` verbatim.

## What cyrup does

Routes the same messages through `ProviderError::Transport`, whose `Display` prepends `"transport error: "`.

## What a caller sees

Every Vertex configuration error reaches the user with a `transport error: ` prefix pi does not emit — on messages (`resolveProject`/`resolveLocation`) that were ported verbatim precisely because they are what a misconfigured user reads. The marker notes the same prefix is already applied in `google_generative_ai.rs:93-98`, so the divergence is wider than this one module.

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
