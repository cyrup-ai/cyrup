---
stage: new
status: done
updated: 2026-08-23 00:06
---

# Make reconcile_before_control_op delegate to background::reconcile — 8 production control sites still skip the liveness probe

> Found by an eight-lens workspace hygiene sweep. Every count below was reproduced
> against the tree before this task was filed.
> **Priority:** high · **Effort:** medium
> **Crates:** `cyrup-ext-subagents`

`crates/cyrup-ext-subagents/src/background/control.rs:36-61` documents that full stale-run reconciliation is `background/reconcile.rs`'s job, states that module "does not exist yet as of this file", and commits to a concrete follow-up: *"When `reconcile.rs` lands, its fuller `reconcile()` … is the intended long-term replacement for this function's body; this module should be updated at that point to delegate to it rather than duplicate the probe."*

`background/reconcile.rs` has since landed — **1,225 lines**, exposing `check_pid_liveness`, `reconcile` and `reconcile_now` (verified by `wc -l`). But the body of `reconcile_before_control_op` (`control.rs:185-235`) still contains **zero** references to any of those three symbols (verified: `awk 'NR>=185 && NR<=235' … | grep -cE 'reconcile::reconcile|check_pid_liveness|reconcile_now'` → `0`). It returns `status.json` as-is whenever no `ResultFile` exists.

Every control op gated on that narrow probe therefore acts on a `Running` status for an already-dead pid, skipping R-SA-089/091/092 (zero-signal probe, 24h staleness→Failed, synthesized failure result). The affected production call sites are `control.rs:397` (interrupt), `:699` (resume), `:2091` (append_step), `run_status.rs:342/:504/:622`, and `extension/executor/control.rs:664/:782` — eight sites, against only four that use the full `reconcile_now`.

This is the one finding in the hygiene sweep with a live behavioral consequence rather than a stylistic one: a user interrupting or resuming a background run whose process died without writing a result file gets a no-op against a phantom Running state.

## Acceptance Criteria

- [ ] `reconcile_before_control_op` in crates/cyrup-ext-subagents/src/background/control.rs delegates to `background::reconcile` (grep for `reconcile::reconcile|reconcile_now|check_pid_liveness` inside its body returns a non-zero count) and no longer duplicates its own inline pid probe
- [ ] All 8 gated call sites (control.rs:397, :699, :2091; run_status.rs:342, :504, :622; extension/executor/control.rs:664, :782) go through the delegating path — verified by grep that none re-implements a local liveness check
- [ ] The stale module doc at control.rs:36-61 is rewritten: it no longer claims reconcile.rs "does not exist yet" and no longer describes the delegation as future work
- [ ] A regression test covers the previously-broken case: a run whose status.json says Running, whose pid is dead, and which has no ResultFile — a control op (interrupt or resume) must observe the reconciled Failed status and a synthesized failure result, per R-SA-089/091/092
- [ ] `cargo test -p cyrup-ext-subagents` passes with no new failures

## Verifying command

```bash
cd /home/user/cyrup/crates/cyrup-ext-subagents/src && sed -n '36,61p' background/control.rs && wc -l background/reconcile.rs && awk 'NR>=185 && NR<=235' background/control.rs | grep -cE 'reconcile::reconcile|check_pid_liveness|reconcile_now'
```
