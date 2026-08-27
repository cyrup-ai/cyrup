---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Relay the child's prompt payload and approval suggestion on a forwarded ask

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | medium |
| **Kind** | partial |
| **Upstream area** | presentation — forwarded-ask-payload / permission-ui-prompt |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream's forwarded request carries the child's complete prompt payload plus its display
projection (source/surface/value) and its session-approval suggestion, so the serving parent re-
renders the child's facts under the parent's budget, emits a non-degraded ui_prompt, and can
record the child's pattern; the port's forwarded request carries only a pre-rendered `message`
string.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

/home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/authority/permission-
forwarding.ts:138-172 (ForwardedPermissionRequest: `payload?: PromptPayload`, `source?`,
`surface?`, `value?`, `sessionApproval?`); /home/user/cyrup/tmp/pi-packages/packages/pi-
permission-system/src/presentation/forwarded-ask-payload.ts:199-218 (projects the child's payload,
stamping the parent-authoritative requester), :226-249 (degraded fallback only when no payload
arrived); /home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/permission-ui-
prompt.ts:60-70 (buildUiPrompt honours the explicit surface/value so a forwarded broadcast stays
non-degraded)

**Port** (`crates/cyrup-permission-system`):

/home/user/cyrup/crates/cyrup-permission-system/src/forwarding.rs:100-113 —
`ForwardedPermissionRequest { id, response_nonce, created_at, requester_session_id,
target_session_id, requester_agent_name, message }`; there is no
payload/surface/value/sessionApproval field. `rg -ni
"ui_prompt|forwarded_ask_payload|build_ui_prompt" /home/user/cyrup/crates/cyrup-permission-
system/src` → 0 matches.

## Why it matters

The parent's human approves a sentence the child assembled under the child's own configuration:
the parent's field/row budget, its own labelling, and its highlight of the flagged element never
apply, and any UI subscribed to the prompt sees no surface/value/rule facts for a subagent ask —
so the approver at the serving node cannot see which rule fired or which operand tripped it in the
parent's own vocabulary.

## Also reported independently

Other area agents found this same gap from a different angle:

- **Relay the structured prompt payload and display projection over the wire** (authority: forwarding wire format) — Upstream forwards the child's complete structured `PromptPayload` plus a display projection
(source, surface, value) so the serving node renders the child's facts under the parent's own
budget and emits a non-degraded prompt broadcast; the port forwards a single pre-rendered
`message` string the child assembled under its own configuration.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
