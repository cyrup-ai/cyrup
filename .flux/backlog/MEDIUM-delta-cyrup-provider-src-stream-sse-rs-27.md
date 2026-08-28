---
title: "CYRUP-DELTA capability gap at crates/cyrup-provider/src/stream/sse.rs:27"
priority: MEDIUM
crate: cyrup-provider
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-provider/src/stream/sse.rs:27`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi installs a global undici dispatcher (cli.ts:18, main.ts:538/:802) with SEPARATE `headersTimeout`, `bodyTimeout`, and undici's default 10 s `connectTimeout`.

## What cyrup does

One `reqwest::ClientBuilder::read_timeout` per client covers headers+body; the marker states plainly that undici's separate 10 s `connectTimeout` is NOT reproduced and the connect phase is covered by the same (much larger) idle deadline.

## What a caller sees

A request to an unreachable or black-holed endpoint (bad proxy, dropped SYN, misconfigured base URL) fails after ~10 s under pi and after the full idle timeout under cyrup — and NEVER, if the idle timeout is configured to 0, which both systems treat as 'disabled'. The user sees a hung turn where pi returns a connect error promptly. This one is worth a decision rather than a note.

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
