---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Resolve forwarded requests against the parent's recorded authority

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | medium |
| **Kind** | absent |
| **Upstream area** | authority: forwarding (serving side) |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream's serving node resolves a forwarded child ask against its own composed ruleset via the
child-fixed `accessIntent` carried on the wire (allow auto-approves, deny auto-denies, only `ask`
prompts); the port's parent checks only request expiry and yolo, then always prompts the human.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

src/authority/forwarded-request-server.ts:66-68 (`ServingPolicy.resolve`), :455-493
(`resolveDecision` — recorded authority first, escalate only on `ask`), :459-483
(auto_approved/auto_denied with a `rule` DecisionSource); src/authority/permission-
forwarding.ts:106-136 (`ForwardedAccessFacts`/`ForwardedAccessIntent` — surface, matchValues,
boundaryValue, requesterCwd, principal), :169-174 (`accessIntent` on the request);
src/authority/approval-escalator.ts:314-346 (`buildForwardedRequest` stamps requesterCwd +
principal onto the child-fixed facts)

**Port** (`crates/cyrup-permission-system`):

/home/user/cyrup/crates/cyrup-permission-system/src/forwarding.rs:99-112 —
`ForwardedPermissionRequest` carries only
id/nonce/createdAt/requesterSessionId/targetSessionId/requesterAgentName/`message`;
/home/user/cyrup/crates/cyrup-permission-system/src/forwarding.rs:1082-1200 —
`resolve_forwarded_decision` goes expiry -> `config.yolo_mode` -> `LocalAskChannel::confirm`, with
no ruleset consultation. `rg -in
"access_intent|accessintent|match_values|matchvalues|boundary_value"
/home/user/cyrup/crates/cyrup-permission-system/src` returns 0 matches.

## Why it matters

The serving session's own recorded deny rules never apply to a subagent's forwarded ask — the only
gate is a human clicking through a dialog — and conversely every child ask the parent already
allows by policy still interrupts the human, which is exactly the prompt-fatigue pressure that
produces reflexive approvals.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

CONFIRMED ABSENT. resolve_forwarded_decision (/home/user/cyrup/crates/cyrup-permission-
system/src/forwarding.rs:1082-1200) is expiry -> config.yolo_mode -> LocalAskChannel::confirm, and
`grep -n "manager|check_permission|PermissionManager|evaluate" forwarding.rs` shows the serving
path never touches the rule engine — spawn_forwarding_watcher is not even constructed with a
PermissionManager (only agent_dir/services/config/audit/has_ui). No accessIntent field on
ForwardedPermissionRequest (forwarding.rs:99-112). SEVERITY LOWERED high->medium: this fails
CLOSED. Without the serving policy the parent prompts a human for every forwarded ask, so nothing
reaches the gate that a human did not explicitly approve; the real cost is (a) the parent's own
deny rules are not mechanically enforced against a child ask — a human can click through what
policy forbids — and (b) prompt fatigue from asks the parent's ruleset already allows. The port is
faithful to pi v0.8.0 here (index.ts:1170-1230), which is what its doc header claims to port; the
capability is a v27 addition (ADR 0008).

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
