---
title: "CYRUP-DELTA capability gap at crates/cyrup-provider/src/api/openai_completions/params.rs:102"
priority: MEDIUM
crate: cyrup-provider
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-provider/src/api/openai_completions/params.rs:102`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

`ai/src/api/openai-completions.ts:716` sends `max_tokens` only when the caller supplied one (`if (options?.maxTokens)`). With no caller cap, no key is sent and the server applies its own default ceiling.

## What cyrup does

PROV-069: ALWAYS sends an output ceiling — the caller's when present, otherwise `model.max_tokens` from the embedded catalog (only omitted when that is 0).

## What a caller sees

Every completions-API request body carries a `max_tokens` pi would not send. Observable in both directions: it fixes real mid-sentence truncation on Together, and it caps replies at the catalog value on any provider whose own default ceiling is HIGHER than the catalog row — a stale or conservative catalog number now silently truncates generations that pi completes. The rule is borrowed from pi's anthropic-messages path (`options?.maxTokens ?? model.maxTokens`, anthropic-messages.ts:989), not from this API. Deliberate behaviour change, correctly marked, needs your sign-off.

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
