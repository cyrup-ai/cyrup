---
title: "CYRUP-DELTA capability gap at crates/cyrup-provider/src/providers/fleet.rs:270"
priority: MEDIUM
crate: cyrup-provider
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-provider/src/providers/fleet.rs:270`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

At the ported baseline v0.83.0 the Groq model `qwen/qwen3-32b` carried a `thinkingLevelMap` (via the generator override at ai/scripts/generate-models.ts:837), so thinking levels mapped to specific `reasoning_effort` values.

## What cyrup does

Ships the v0.84.1 behaviour (no map on that row), enforced by an explicit entry in the catalog generator's `DELTAS` table (xtask/src/main.rs) against the b0c2a90e generation source.

## What a caller sees

Against v0.83.0: a caller selecting groq `qwen/qwen3-32b` with a low/medium/high thinking level gets a different `reasoning_effort` on the wire. Against the audit reference commit e8682309 there is no difference — I checked `packages/ai/src/providers/groq.models.ts` there and it contains no `thinkingLevelMap` and no `qwen` row at all. SEPARATE FINDING, no marker: cyrup still ships `qwen/qwen3-32b` in `providers/catalog/groq.json:118` although upstream has removed it — that catalog drift is undocumented by any CYRUP-DELTA.

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
