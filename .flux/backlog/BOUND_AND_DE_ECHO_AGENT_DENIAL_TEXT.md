---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Stop echoing the full bash command in agent-facing denial text

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | high |
| **Kind** | behaviour-drift |
| **Upstream area** | presentation — agent-renderer / denial reasons |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream's agent-facing renderer deliberately never reproduces the command and caps every agent-
supplied field it does render at the configured field budget; the port interpolates
`result.command`, `result.target` and the tool name verbatim and unbounded into all three denial
reasons.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

/home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/presentation/agent-
renderer.ts:14-33 ("The agent renderer identifies the call; it does not reproduce it… The command
is the one value never rendered — it is the payload that took over the viewport in #710 and the
context window on every denial"), :141-146 (flaggedClause returns "" for kind `bash`), :204-215
(`cap()` truncates the flagged element at `budget.fieldMaxWidth`), :44-78 (renderPolicyDenial /
renderUserDenial / renderUnavailableDenial all take an AgentRenderBudget); default budget at
presentation/dialog-renderer.ts:76-79

**Port** (`crates/cyrup-permission-system`):

/home/user/cyrup/crates/cyrup-permission-system/src/gate.rs:254-256 (`parts.push(format!("command
'{command}'"))`), :274-276 (`format!("User denied bash command '{command}'.")`), :653-660
(`format!("Running bash command '{command}' requires approval, …")`). No cap is applied on any of
these paths — `rg -n "cap\(|field_max_width" src/gate.rs` → 0 matches.

## Why it matters

Every denied bash call returns the agent's own (arbitrarily long, agent-authored) command string
back into the model context — the exact regression upstream removed. A here-string or multi-KB
pipeline denial floods the context window on each retry, and the block reason is unbounded
attacker-influenced text with no quantity cap.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
