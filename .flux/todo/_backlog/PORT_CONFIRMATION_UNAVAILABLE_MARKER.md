---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Mark abandoned forwards as confirmation-unavailable with a reason

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | low |
| **Kind** | partial |
| **Upstream area** | authority: escalation outcomes |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream's `abandon()` returns a denial flagged `confirmationUnavailable` and carrying a
`denialReason` naming which forwarding path gave up; the port returns a bare `denied()` with
`denial_reason: None` on all four abandonment paths, so a request no human ever saw is recorded
and reported as an ordinary user denial.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

src/authority/approval-escalator.ts:118-126 (`abandon(denialReason)` sets
`confirmationUnavailable: true`, `denialReason`, `decidedBy: {kind:"unavailable"}`), :236-247 /
:255-262 / :291 / :402-406 / :422-424 (the five distinct reasons: unresolvable target,
undeliverable dirs, unwritable request, target not serving, timeout with the timeout in seconds);
src/authority/permission-dialog.ts:20-30 (`confirmationUnavailable` contract — "a user who was
never asked denied nothing"); src/authority/permission-prompter.ts:131-133 (review resolution
becomes `confirmation_unavailable`)

**Port** (`crates/cyrup-permission-system`):

/home/user/cyrup/crates/cyrup-permission-system/src/forwarding.rs:631-633 — `fn denied() ->
PermissionPromptDecision { approved: false, state: Denied, denial_reason: None }`, returned
unchanged at forwarding.rs:667, :676, :685, :722 and :824 (the five failure paths);
/home/user/cyrup/crates/cyrup-permission-system/src/ask.rs:46-50 — `PermissionPromptDecision` has
no `confirmation_unavailable` field; `rg -n "confirmation_unavailable"
/home/user/cyrup/crates/cyrup-permission-system/src/forwarding.rs` returns 0 matches.

## Why it matters

The invoking model is told it was denied with no reason instead of learning the forwarding path
broke, so it cannot self-correct or retry; and an operator auditing the review log cannot
distinguish a real human denial from a parent that never answered, which is precisely the signal
that a forwarding misconfiguration is silently blocking every subagent action.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

CONFIRMED PARTIAL. denied() at /home/user/cyrup/crates/cyrup-permission-
system/src/forwarding.rs:631-633 carries denial_reason: None and is returned at :667, :676, :685,
:722 and :824; PermissionPromptDecision (ask.rs:44-50) has no confirmation_unavailable field;
`grep -n confirmation_unavailable forwarding.rs` -> 0. CORRECTION to the finder's impact: the
audit half is NOT missing. Every abandonment path already writes a distinguishing record —
audit.forwarding_error(...) at forwarding.rs:667/676/685/722 and
forwarded_permission.response_timed_out at :812-820 — so an operator CAN separate a broken forward
from a human denial in the review log. Note also that the port DOES have the confirmation-
unavailable concept on its local tiers (gate.rs:653 format_ask_unavailable_reason, gate.rs:795
format_external_directory_unavailable_reason, extension/decide.rs:255 and :356 emit "resolution":
"confirmation_unavailable"); it is only the forwarding paths that do not use it. SEVERITY LOWERED
medium->low: the only real loss is model-facing — apply_decision (extension/prompt.rs:288-296)
renders gate::format_user_denied_reason with no reason, so the model is told a user denied it when
no user was asked. Fail-closed, audit-visible, model-misinforming.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
