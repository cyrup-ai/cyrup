---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/bash.rs:72"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-tools/src/tools/bash.rs:72`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi @e8682309 shares ONE `bashSchema` between both shell tools; `command`'s description is `"Shell command to execute"` (core/tools/bash.ts:42-45, verified in the reference tree).

## What cyrup does

`BASH_CONFIG.command_description = "Bash command to execute"` (bash.rs:104) — the v0.83.0 string — while `POWERSHELL_CONFIG` uses `"Shell command to execute"` (powershell.rs:35). The ground-truth constant in tests/pi_schema.rs:58 pins the v0.83.0 wording, so the test suite locks the divergence in.

## What a caller sees

The JSON tool schema sent to the model on every turn differs from pi's by one property description. Model-facing text, byte-diffable by anyone comparing cyrup and pi tool definitions; also breaks any prompt-cache prefix shared with a pi transcript.

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
