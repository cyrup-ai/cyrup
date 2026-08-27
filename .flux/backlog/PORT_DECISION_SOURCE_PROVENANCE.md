---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Stamp and relay a DecisionSource on every permission decision

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | low |
| **Kind** | absent |
| **Upstream area** | authority: decision provenance |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream requires every decision to name what decided it (human at dialog vs select, chain link,
rule, session grant, yolo, infrastructure read, unavailable, gate error, or a nested `forwarded`
source), and carries that record across the forwarding hop on the response file; the port's
decision type has no such field.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

src/authority/decision-source.ts:20-65 (the nine variants incl. recursive `forwarded`), :76-218
(`asDecisionSource` depth-bounded tolerant narrowing off disk); src/authority/permission-
dialog.ts:31-38 (`decidedBy` is required on `PermissionPromptDecision`); src/authority/permission-
prompt-component.ts:109-115 (`attributeToHuman` stamps dialog vs select); src/authority/forwarded-
request-server.ts:419-429 (`decidedBy` written onto the response file); src/authority/approval-
escalator.ts:139-150 (`relayDecision` nests the responder's source under `{kind:"forwarded",
responderSessionId}`)

**Port** (`crates/cyrup-permission-system`):

`rg -in "decided_by|decidedby|decision_source|decisionsource" /home/user/cyrup/crates/cyrup-
permission-system/src` returns 0 matches; /home/user/cyrup/crates/cyrup-permission-
system/src/ask.rs:44-50 — `PermissionPromptDecision { approved, state, denial_reason }` only;
/home/user/cyrup/crates/cyrup-permission-system/src/forwarding.rs:118-131 —
`ForwardedPermissionResponse` carries `responder_session_id` but nothing describing what inside
that session decided.

## Why it matters

An audit of an approved subagent action can establish which session answered but not whether a
human at the parent's dialog approved it, the parent's yolo mode auto-approved it, or a timeout
fallback produced it — the exact distinction needed to investigate an unexpected grant.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

CONFIRMED ABSENT. `grep -rin "decided_by|decidedby|decision_source|decisionsource" src` -> 0;
PermissionPromptDecision (ask.rs:44-50) and ForwardedPermissionResponse (forwarding.rs:118-131)
carry no provenance field. Partial mitigation the finder omits: on the SERVING side the port
already writes provenance-bearing review entries — forwarded_permission.auto_approved (yolo) at
forwarding.rs:1128, forwarded_permission.prompted at :1141, forwarded_permission.expired at :1108
— so the parent's own log distinguishes yolo from human from expiry. What is genuinely lost is
that none of that crosses the wire: the CHILD's record (forwarded_permission.response_received,
forwarding.rs:757-772) logs responderSessionId, approved and state but nothing about what inside
the parent decided, so reconstructing an approval requires correlating two sessions' logs by
requestId. SEVERITY LOWERED medium->low: audit/forensics only; no decision changes and nothing
extra passes the gate.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
