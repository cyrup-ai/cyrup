---
title: "CYRUP-DELTA capability gap at crates/cyrup-provider/src/auth/google_adc.rs:300"
priority: MEDIUM
crate: cyrup-provider
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-provider/src/auth/google_adc.rs:300`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

google-auth-library constructs a client for `external_account` / `impersonated_service_account` / `gdch_service_account`.

## What cyrup does

Returns `ProviderError::Transport("Unsupported Google credentials type: {other}. cyrup mints Vertex bearers from `authorized_user` and `service_account` credentials only (see auth/google_adc.rs's CYRUP-DELTA note)")`.

## What a caller sees

Same gap as google_adc.rs:19 — this is the exact user-facing error string. Note it also references an internal source file in a message shown to end users.

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
