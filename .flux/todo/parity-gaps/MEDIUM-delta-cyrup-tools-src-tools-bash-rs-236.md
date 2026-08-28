---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/bash.rs:236"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-tools/src/tools/bash.rs:236`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

The bash and powershell prompt guideline reads `"You can inspect PI_* environment variables for current model and session details."` (bash.ts:49, powershell.ts:20 @e8682309), and the tool actually injects `PI_SESSION_ID` / `PI_SESSION_FILE` / `PI_PROVIDER` / `PI_MODEL` / `PI_REASONING_LEVEL` into the child (bash.ts:171-181).

## What cyrup does

Emits `"You can inspect CYRUP_* environment variables ..."` and injects `CYRUP_SESSION_ID` / `CYRUP_SESSION_FILE` / `CYRUP_PROVIDER` / `CYRUP_MODEL` / `CYRUP_REASONING_LEVEL`, while `config::session_env_scrub_keys()` DELETES the five `PI_*` names from every child unconditionally.

## What a caller sees

System-prompt text differs, and — more consequentially — any user script, hook, or `.bashrc` that reads `PI_SESSION_ID` (or the other four) gets nothing under cyrup; the variables are actively scrubbed, not merely absent. A pi user's existing shell tooling silently stops working. Deliberate and self-consistent, but squarely caller-visible.

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
