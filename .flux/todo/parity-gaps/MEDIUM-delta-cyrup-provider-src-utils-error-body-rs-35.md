---
title: "CYRUP-DELTA capability gap at crates/cyrup-provider/src/utils/error_body.rs:35"
priority: MEDIUM
crate: cyrup-provider
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-provider/src/utils/error_body.rs:35`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

`truncateErrorText` slices by UTF-16 code units (JS `String.length` / `String.slice`) and reports `[truncated N chars]` in those units.

## What cyrup does

Counts and slices Unicode scalar values.

## What a caller sees

For an error body over the cap containing astral characters (emoji — common in gateway/HTML error pages), the truncation point and the reported `N` differ between the two: same upstream error, different displayed message and different dropped-character count. Low impact and the Rust choice is the sound one (a `String` cannot hold a lone surrogate), but the difference is constructible, so it is not mechanism-only by your definition.

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
