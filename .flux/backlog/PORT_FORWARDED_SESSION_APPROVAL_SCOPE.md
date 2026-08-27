---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Relay the session-approval suggestion and offer the grant-scope choice

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | low |
| **Kind** | absent |
| **Upstream area** | authority: forwarded grant scope |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream relays the child's suggested session-approval pattern on the forwarded request and, when
present, asks the approver whether a "for this session" grant applies to the requesting subagent
only or to the whole serving session — recording the pattern into the serving node's rules for the
latter; the port relays no pattern and offers no scope choice.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

src/authority/permission-forwarding.ts:84-96 (`ForwardedSessionApproval` surface + patterns),
:162-168 (`sessionApproval` on the request); src/authority/permission-dialog.ts:3-8
(`approved_for_serving_session` state), :101-113 and :140-160 (`sessionScope` second select,
cancelled scope falls back to the least-privilege subagent scope); src/authority/local-user-
authorizer.ts:72-88 (`buildRequestOptions` offers the scope only for a forwarded ask carrying a
suggestion); src/authority/forwarded-request-server.ts:362-388 (`applyGrantScope` records the
pattern into the serving node's `SessionRules` and downgrades the wire state to plain `approved`)

**Port** (`crates/cyrup-permission-system`):

`rg -in "approved_for_serving_session|serving_session|session_scope|sessionScope"
/home/user/cyrup/crates/cyrup-permission-system/src` returns 0 matches;
/home/user/cyrup/crates/cyrup-permission-system/src/forwarding.rs:99-112 — no `sessionApproval`
field on the forwarded request; /home/user/cyrup/crates/cyrup-permission-system/src/ask.rs:128-140
— the forwarded prompt reuses `LocalAskChannel::confirm`'s fixed four options (Allow Once / Allow
Always / Reject / Reject with Reason) with no second scope select; /home/user/cyrup/crates/cyrup-
permission-system/src/ask.rs:35-42 — `PermissionDecisionState` has no serving-session variant.

## Why it matters

A parent-side "Allow Always" on a forwarded ask is relayed to the child and persisted there with
no explicit scope decision by the approver, so the human cannot see or choose whether they are
granting the one subagent or the whole serving session, and the serving node records nothing it
could later audit or revoke.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

CONFIRMED ABSENT as a feature: `grep -rin
"approved_for_serving_session|serving_session|session_scope"` -> 0; no sessionApproval field
(forwarding.rs:99-112); the forwarded prompt reuses LocalAskChannel::confirm's fixed four options
(ask.rs:97-105, 128-160); PermissionDecisionState (ask.rs:35-42) has the six v0.8.0 states only.
BUT the finder's security_impact is INVERTED and should not be repeated to the fixer: a forwarded
Always is relayed to the child and persisted by the CHILD into its own session store only —
extension/prompt.rs:295-306 apply_decision -> session_approvals.approve_always(check.tool_name,
subject) — which is precisely upstream's least-privilege fallback when the approver cancels the
scope select (permission-dialog.ts:140-160). Absence of the choice therefore produces the NARROWER
grant, never a broader one; nothing extra is granted and no unaudited serving-session rule is
created (the port records permission_request.approval_persisted with approvalScope,
extension/prompt.rs:262-278). SEVERITY LOWERED medium->low: missing operator affordance whose
default is the safe one.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
