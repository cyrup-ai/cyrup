---
title: "CYRUP-DELTA capability gap at crates/cyrup-provider/src/collection.rs:534"
priority: MEDIUM
crate: cyrup-provider
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-provider/src/collection.rs:534`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

`Models.checkAuth` -> private `checkProviderAuth` (models.ts:364-392 @v0.83.0) calls `resolveProviderAuth`, which is PROVIDER-scoped — it needs no model. It also consults an optional `ApiKeyAuth.check?` hook (auth/types.ts:173, used at :373-382).

## What cyrup does

`ApiKeyAuth::resolve` takes a `&Model` (because `providers/cloudflare.rs` needs `model.base_url` for its `{CLOUDFLARE_ACCOUNT_ID}` substitution), so `check_auth` uses the provider's FIRST catalog row as the resolution subject; a provider with an empty catalog reports `None`. There is no `check` hook on the trait.

## What a caller sees

Two observable effects. (1) A correctly-configured provider whose catalog is empty at that moment (remote catalog fetch failed, offline start, a dynamic provider before its first refresh) is reported as UNAUTHENTICATED by cyrup and as authenticated by pi — the user is told to `/login` for a provider whose key is present and valid. (2) A third-party provider cannot implement pi's `check` hook to answer auth cheaply; the resolution path always runs.

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
