---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Write a gate_error audit entry when the gate itself fails

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | low |
| **Kind** | partial |
| **Upstream area** | gate boundary |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream's fail-closed boundary catches a throwing gate, blocks, and records a
`permission_request.blocked` review entry with `resolution: "gate_error"` plus a decision
broadcast; the port relies on the host dispatcher's fail-closed block and writes nothing to the
permission review log.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

handlers/tool-call-boundary.ts:38-60 (try/catch → `{ block: true, reason:
formatGateErrorReason(error) }`) and :76-109 (`recordGateError`: mints a request id, writes
`permission_request.blocked` with `resolution: "gate_error"` and `decidedBy: { kind: "gate_error",
reason }`, then `reporter.emitDecision({... resolution: "gate_error" })`)

**Port** (`crates/cyrup-permission-system`):

`rg -n "gate_error|catch_unwind" /home/user/cyrup/crates/cyrup-permission-system/src` → 0 matches.
Containment lives in the host instead: /home/user/cyrup/crates/cyrup-ext/src/native.rs:875-887
catches the panic and /home/user/cyrup/crates/cyrup-ext/src/dispatch.rs:440-460 turns a fault on a
`fails_closed` kind into `Reduced::Blocked` — the call is blocked, but no permission-system review
entry or event is produced.

## Why it matters

The block is preserved, but a crashing or timing-out gate leaves no record in the permission
review log, so an operator auditing why a tool call was refused — or noticing that the gate is
repeatedly faulting rather than deciding — has nothing in the security trail to key on.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

Could not refute: `rg -n "catch_unwind|gate_error|AssertUnwindSafe"` over src/ (excluding tests)
returns only unrelated no-panic-policy prose (lib.rs:67-69, runtime_api.rs:66,
extension/mod.rs:230). The claim is self-limiting and correctly rated: cyrup-
ext/src/native.rs:875-887 and cyrup-ext/src/dispatch.rs:440-460 do block the call, so the fail-
closed OUTCOME is preserved and only the permission-system review-log record is missing. Fixer
note: the crate denies clippy::panic/unwrap and has a stated no-panic policy (lib.rs:67-69), so
the natural shape here is not catch_unwind inside the gate — it is a host-side hook or an explicit
error arm that funnels into write_review_entry("permission_request.blocked", {resolution:
"gate_error"}) via extension/audit.rs:44-49.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
