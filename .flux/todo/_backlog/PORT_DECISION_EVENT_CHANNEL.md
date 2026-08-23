---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Broadcast every gate resolution on the decision channel

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | low |
| **Kind** | partial |
| **Upstream area** | handlers: decision reporting |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream broadcasts a terminal PermissionDecisionEvent (requestId, surface, value, allow/deny,
resolution, origin, agentName, matchedPattern) after every gate resolution including pure policy
allows/denies; the port emits events only from the interactive prompt path, so a decision reached
without a prompt is never announced.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

permission-events.ts:33 PERMISSIONS_DECISION_CHANNEL, :145-176 PermissionDecisionEvent +
resolution enum (policy_allow/policy_deny/session_approved/gate_error/...); decision-
reporter.ts:39-52 GateDecisionReporter.emitDecision; index.ts:192 (one reporter shared by the gate
runner and the forwarded-request server); handlers/tool-call-boundary.ts:97-106

**Port** (`crates/cyrup-permission-system`):

src/extension/events.rs:23-63 — the only channel is PERMISSION_REQUEST_EVENT_CHANNEL and the only
payload is the prompt-state projection; `rg -n "emit_permission_state_event|emit_event" src` shows
its three call sites are all in the prompt path (src/extension/prompt.rs:149,169,205). No policy-
allow, policy-deny, session-approved or infrastructure-auto-allow decision reaches the bus.

## Why it matters

An external monitor or UI subscribing for an audit trail sees only the asks a human was shown;
silent policy denies and auto-allows are invisible, so the bus cannot be used to observe what the
gate actually decided.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

Could not refute the absence, but over-rated. Upstream confirmed: permission-events.ts:35
PERMISSIONS_DECISION_CHANNEL and :226, decision-reporter.ts:39-51,
handlers/gates/runner.ts:66-87,:134-144,:163-170,:233 (policy, session-approved and auto-approved
paths all emit). Port: src/extension/consts.rs holds only PERMISSION_REQUEST_EVENT_CHANNEL, and
`grep -rn emit_permission_state_event src/` gives exactly three non-test call sites, all in the
prompt path (src/extension/prompt.rs:149,169,205). Downgraded to low because the SECURITY trail is
not lost, only the bus projection of it: the port already writes the on-disk review record for a
policy deny (src/extension/decide.rs:131-143, resolution "policy_denied") and for a skill-read
deny (decide.rs:203-217), via write_review_entry/review_permission_decision in
src/extension/audit.rs:47-86. What is genuinely absent is a live subscriber's view of non-prompted
outcomes (and of plain policy ALLOWS, which the port records nowhere). No decision changes and
nothing extra passes the gate.

## Also reported independently

Other area agents found this same gap from a different angle:

- **Broadcast a terminal decision for every gate resolution** (decision reporter / events) — Upstream emits `permissions:decision` after every gate resolution — policy allow, policy deny,
session-approved, auto-approved, confirmation-unavailable, user decisions and gate errors —
carrying resolution, origin and matched pattern; the port emits events only from the prompt
path (waiting/approved/denied) and nothing at all for policy-resolved calls.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
