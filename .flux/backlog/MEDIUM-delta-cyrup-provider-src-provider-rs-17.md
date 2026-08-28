---
title: "CYRUP-DELTA capability gap at crates/cyrup-provider/src/provider.rs:17"
priority: MEDIUM
crate: cyrup-provider
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-provider/src/provider.rs:17`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

`RefreshModelsContext` (packages/ai/src/models.ts:34-44 @v0.83.0) carries FIVE members, including `credential` (:36) and `store` (:38).

## What cyrup does

Carries three. The two missing ones are said to have 'nowhere useful to arrive' because `RemoteCatalog` owns its own store and auth context.

## What a caller sees

A third-party dynamic provider implemented against cyrup receives strictly less context than the same provider implemented against pi: it cannot read the effective credential for its own fetch, and it cannot persist through the caller's store. Any pi provider that used either member is not portable. Caller-visible as missing functionality on a public trait, not as an implementation detail.

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
