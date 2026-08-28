---
title: "CYRUP-DELTA capability gap at crates/cyrup-provider/src/api/compat.rs:781"
priority: MEDIUM
crate: cyrup-provider
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-provider/src/api/compat.rs:781`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi @e8682309 `packages/ai/src/api/openai-responses.ts:304-305` is `if (options?.temperature !== undefined) params.temperature = options.temperature;` — unconditional. The stripping rule cyrup ports lives in `pi-permission-system` @v0.8.0, a SEPARATE package that is not present anywhere in the reference tree, and upstream applies it only after that extension has loaded and run `session_start`.

## What cyrup does

`unsupported_temperature_reason` (compat.rs:832-862) is baked into the request builders: `temperature` is dropped for api `openai-codex-responses`, provider `openai-codex`, any openai-responses-family model whose id contains the token `codex`, AND any openai-responses-family model with `reasoning: true`.

## What a caller sees

A caller who sets `temperature` on a reasoning model over the Responses API (gpt-5, o-series — the common case, not an edge case) has it silently dropped by cyrup; base pi puts it on the wire. Divergent request body, divergent server response (pi may surface the provider's rejection). The marker's own text concedes a user can notice this by not loading the permission system. This is the single widest-reach capability gap in the provider crate.

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
