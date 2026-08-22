---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Publish and read a serving heartbeat so a child abandons a dead parent

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | low |
| **Kind** | absent |
| **Upstream area** | authority: forwarding liveness |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream lets a forwarding child give up after a short grace window when the target session is
provably not draining its inbox (in-process registry, or a filesystem heartbeat with pid +
staleness for a separate process); the port's child has no liveness channel at all and always
waits out the full 10-minute timeout.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

src/authority/forwarding-liveness.ts:1-17 (problem statement), :47-59
(`SERVING_HEARTBEAT_REFRESH_MS` / `SERVING_HEARTBEAT_STALE_MS`), :78-118 (`HeartbeatState`,
`TargetServingLookup`, `ServingObservation`), :135-167 (`ForwardingLivenessJudge` routes registry
vs heartbeat by target source), :218-401 (`ServingHeartbeatStore` publish/withdraw/classify/sweep-
dead-pids); src/authority/permission-forwarding.ts:19-20
(`PERMISSION_FORWARDING_SERVING_GRACE_MS`); src/authority/approval-escalator.ts:385-406 and
:434-441 (poll loop abandons after the grace window and logs
`forwarded_permission.no_serving_session` with channel/state/servingIds);
src/authority/forwarding-manager.ts:63-81 and :104-142 (announce/refresh/withdraw while polling)

**Port** (`crates/cyrup-permission-system`):

`rg -in "heartbeat" /home/user/cyrup/crates/cyrup-permission-system/src` returns 0 matches; `rg
-in "serving" /home/user/cyrup/crates/cyrup-permission-system/src` matches only the word
"preserving" in jsonc.rs/common.rs/ordered.rs/manager.rs/types.rs/ext_config.rs/wildcard.rs.
/home/user/cyrup/crates/cyrup-permission-system/src/forwarding.rs:749-802 — the child poll loop
tests only `response_path.exists()` and the deadline; /home/user/cyrup/crates/cyrup-permission-
system/src/forwarding.rs:1257+ — `spawn_forwarding_watcher` announces nothing.

## Why it matters

Every ask forwarded to a parent that has exited, been killed, or stopped polling stalls the
subagent for the full `PERMISSION_FORWARDING_TIMEOUT` (10 minutes) and then resolves to a denial
nobody made, so a single dead parent can wedge a subagent run for minutes per tool call with no
diagnostic naming which channel saw what.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

CONFIRMED ABSENT. `grep -rin "heartbeat" src` and `grep -rin "serving" src` (only the substring in
"preserving") return nothing relevant; the child poll loop at /home/user/cyrup/crates/cyrup-
permission-system/src/forwarding.rs:749-800 tests only response_path.exists() and the deadline.
SEVERITY LOWERED high->low: this is an availability/latency defect with no gate consequence — the
failure mode is a stall that terminates in denied() (forwarding.rs:824), i.e. strictly more
restrictive than upstream, never less. The finder's own security_impact describes wedged subagent
runs, not an escape. Worth fixing for diagnosability (upstream's
forwarded_permission.no_serving_session names channel/state/servingIds, approval-
escalator.ts:385-406), but it does not belong in the same tier as a gate hole.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
