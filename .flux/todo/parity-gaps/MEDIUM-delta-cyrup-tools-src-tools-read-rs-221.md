---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/read.rs:221"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-tools/src/tools/read.rs:221`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

`const endLine = Math.min(startLine + limit, allLines.length)` (read.ts:282) then `allLines.slice(startLine, endLine)`. The addition is unclamped, so a negative `limit` makes `endLine` negative and JS `slice` applies its count-from-the-end rule: e.g. start=3, limit=-5 -> `slice(3, -2)` returns lines 4 .. len-2. pi returns a non-empty window and a continuation notice quoting a negative `offset=`.

## What cyrup does

`end = to_count(start + limit).clamp(start, total)` — a negative limit collapses to `end == start`.

## What a caller sees

For any negative `limit`, pi returns a real (possibly large) slice of the file and cyrup returns an empty window with a notice pointing back at `start + 1`. The model can reach this: `limit` is a bare `Type.Number` with no `minimum` and pi never validates tool arguments, so `limit: -5` is an input both implementations accept and answer differently.

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
