---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/bash.rs:214"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-tools/src/tools/bash.rs:214`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi @e8682309 bash.ts:49 reads `"You can inspect PI_* ..."` — the same wording cyrup ships. At the nominal ported baseline v0.83.0 (bash.ts:330) it read the bare imperative `"Inspect PI_* environment variables ..."`.

## What cyrup does

Ships the later (v0.84.x) wording.

## What a caller sees

Against the audit reference commit e8682309 there is NO observable difference — cyrup and the reference tree agree. Listed as a gap rather than folded into the mechanism count so it stays visible: it is a model-facing prompt string that is knowingly ahead of the tag the rest of the port targets, i.e. the project is running a mixed baseline. Your call whether that is acceptable; it is not a mechanism detail.

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
