---
title: "CYRUP-DELTA capability gap at crates/cyrup-provider/src/utils/retry.rs:43"
priority: MEDIUM
crate: cyrup-provider
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-provider/src/utils/retry.rs:43`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi's retryable-message list (packages/ai/src/utils/retry.ts:26-89) contains three Node/libuv DNS literals: `getaddrinfo`, `ENOTFOUND`, `EAI_AGAIN`.

## What cyrup does

Keeps all three AND adds a fourth literal, `"dns error"`, matching hyper-util's `ConnectError::dns` Display, because cyrup's transport never produces the Node wording.

## What a caller sees

cyrup's retryable set is a strict SUPERSET of pi's. Intent-preserving for transport failures (without it, DNS retries would be unreachable in cyrup — the addition is well argued), but the same list is also applied to assistant/provider error TEXT (`is_retryable_assistant_error`), so a provider whose error body happens to contain the substring `dns error` is retried by cyrup and not by pi. Low impact, genuinely observable, and an addition to upstream behaviour rather than a reproduction of it.

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
