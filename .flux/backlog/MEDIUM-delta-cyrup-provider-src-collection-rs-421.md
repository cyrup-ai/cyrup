---
title: "CYRUP-DELTA capability gap at crates/cyrup-provider/src/collection.rs:421"
priority: MEDIUM
crate: cyrup-provider
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-provider/src/collection.rs:421`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi's `Models.refresh` always fans out over every refreshable provider, and threads a resolved `credential` plus a provider-scoped `ProviderModelsStore` into each `refreshModels` call (models.ts:287-303, :330-354).

## What cyrup does

`refresh_with(provider: Option<&str>, ...)` adds a single-provider form with no upstream counterpart, and threads neither `credential` nor `store`.

## What a caller sees

For an embedder or a third-party `Provider` implementor: an added API surface pi does not have (a widening nobody upstream reviewed), plus a narrowing — a custom provider's `refresh_models` cannot see the resolved credential or a scoped store, so it cannot authenticate its own catalog fetch the way a pi provider can. pi's `resolveRefreshCredential` bail is relocated to `crates/cyrup/src/provider.rs`, i.e. outside the library, so an embedder that does not replicate it gets different refresh behaviour.

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
