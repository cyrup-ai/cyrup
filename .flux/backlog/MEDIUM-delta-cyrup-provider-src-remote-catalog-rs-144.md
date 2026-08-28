---
title: "CYRUP-DELTA capability gap at crates/cyrup-provider/src/remote_catalog.rs:144"
priority: MEDIUM
crate: cyrup-provider
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-provider/src/remote_catalog.rs:144`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi filters remote catalog entries to `"id" in entry` and spreads whatever else the body carries, because its `Model` is structural — a partial entry still lands in the catalog.

## What cyrup does

`parse_catalog` requires the full `Model` shape (`name`/`api`/`baseUrl`/`cost`/...); an entry carrying an `id` that fails to deserialize is DROPPED.

## What a caller sees

A model advertised by a remote catalog (pi.dev, a self-hosted models endpoint, an OpenRouter-style feed) with any missing required field is selectable in pi and simply does not appear in cyrup's model list — no warning, no diagnostic. The user's `/model` picker is shorter than pi's against the same endpoint, and the failure mode is silent.

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
