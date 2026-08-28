---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/bash.rs:312"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-tools/src/tools/bash.rs:312`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi sets `process.env.PI_CODING_AGENT = "true"` and `process.env.AI_AGENT = "pi"` process-globally at cli.ts:13-14 (verified at e8682309), so EVERY child pi ever spawns — bash tool children, MCP stdio servers, anything — inherits both through `{...process.env}`.

## What cyrup does

Declines the global `std::env::set_var` and pushes both keys per-child inside the bash tool's `execute` (bash.rs:313, :325), with `AI_AGENT = "cyrup"`.

## What a caller sees

Two differences. (1) Value: a hook or script that branches on `AI_AGENT == "pi"` takes the other branch under cyrup. (2) Scope: children spawned outside the bash/powershell tool (MCP stdio servers, any other subprocess) inherit the markers under pi and do NOT under cyrup — a detection script inside an MCP server sees no agent marker. The marker documents (1) but not (2).

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
