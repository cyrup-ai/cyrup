---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Port the per-session decision audit and its session_summary line

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | medium |
| **Kind** | absent |
| **Upstream area** | logging, redaction and log hygiene |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream counts every terminal allow/block/error at the fail-closed boundary and emits one
permission.session_summary debug line at shutdown, warning when toolCalls != allowed + blocked +
errors; the port has no counters and emits no summary.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

/home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/decision-audit.ts:33-74
(DecisionAudit, recordDecision, recordError, writeSummary emitting `permission.session_summary`
and the invariant warning); wired at index.ts:289 (`const audit = new DecisionAudit()`),
handlers/tool-call-boundary.ts:47 and :83, handlers/lifecycle.ts:122
(`this.audit.writeSummary(this.logger)`)

**Port** (`crates/cyrup-permission-system`):

`rg -n "session_summary|DecisionAudit|decision_audit|record_decision|record_error"
/home/user/cyrup/crates/cyrup-permission-system/src` returns nothing. The port's shutdown path
(extension/native.rs:279, runtime_api/status teardown citing `index.ts:1868-1871`) tears down the
runtime API and status pill but writes no counters; extension/audit.rs:38-119 contains only the
two stream writers and the decision record shaper.

## Why it matters

The trail cannot distinguish an evaluated-and-allowed call from a never-evaluated one, and the
cheap structural self-check that catches a re-opened silent exit (a tool call that resolved
without a recorded terminal decision) is gone. A gate path that bypasses logging entirely leaves
no evidence anywhere in the audit output.

## Also reported independently

Other area agents found this same gap from a different angle:

- **Record per-session decision counters and the shutdown summary** (handlers: tool_call boundary / session shutdown) — Upstream's fail-closed tool_call boundary records exactly one terminal outcome per call
(allow/block/error), emits a per-call `permission.decision` debug trace, and writes a
`permission.session_summary` line at session_shutdown that warns when toolCalls != allowed +
blocked + errors; the port has none of the three, and no `gate_error` record for a gate that
fails.
- **Port the per-session decision audit and per-call decision trace** (decision audit) — Upstream records exactly one terminal decision per tool call (allow/block/error), traces it to
the debug log, and writes a `permission.session_summary` on shutdown with a self-check warning
when the counts do not reconcile; the port records nothing for allowed calls and keeps no
counters.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
