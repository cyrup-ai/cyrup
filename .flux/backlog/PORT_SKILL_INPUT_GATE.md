---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Gate /skill:<name> user input instead of granting a bypass

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | high |
| **Kind** | behaviour-drift |
| **Upstream area** | handlers: input hook / skill gate |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream's `input` handler runs a real skill gate on a `/skill:<name>` invocation (deny → UI
warning + swallow the input, ask → prompt), whereas the port only records the name in a set that
then unconditionally bypasses the skill-read gate for that skill for the rest of the session.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

handlers/permission-gate-handler.ts:79-107 (handleInput: extract skill name, build notifier,
skillInputPipeline.evaluate, block → {action:"handled"}); handlers/gates/skill-input-gate-
pipeline.ts:56-74 (raw checkPermission + deny notify + runner.run) and :86-93
formatSkillDenyNotice; index.ts:341 registers it. `rg -n "explicitlyRequested" src` in upstream
returns nothing — the old explicit-request bypass no longer exists.

**Port** (`crates/cyrup-permission-system`):

src/extension/native.rs:168-178 — the Input arm only inserts the name into
`explicitly_requested_skill_names` and returns HookOutcome::Noop; src/extension/decide.rs:199-201
`let explicitly_requested =
guard(&self.explicitly_requested_skill_names).contains(&read_skill.name); if !explicitly_requested
{ … }` skips the deny/ask enforcement entirely. `rg -n "skill_input"
/home/user/cyrup/crates/cyrup-permission-system/src` returns no gate, only doc text.

## Why it matters

A skill the policy denies can still be invoked: typing `/skill:<denied>` is neither blocked nor
reported, and it additionally disarms the skill-read gate for that skill, so the denied skill's
files are read with no prompt and no review-log entry for the whole session. The deny rule is
defeated by the user typing the skill's name.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

Could not refute. Upstream confirmed: handlers/permission-gate-handler.ts:79-106 runs
skillInputPipeline.evaluate and returns {action:"handled"} on block; handlers/gates/skill-input-
gate-pipeline.ts:52-73 does the raw checkPermission + deny notify + runner.run; `grep -rn
explicitlyRequested` over the upstream src returns NOTHING, so the old bypass is genuinely
deleted, not renamed. Port confirmed: src/extension/native.rs:168-178 Input arm only does
`guard(&self.explicitly_requested_skill_names).insert(name)` then HookOutcome::Noop, and
src/extension/decide.rs:199-201 wraps the WHOLE deny+ask block in `if !explicitly_requested`.
`grep -rn "skill_input"` over the port src returns nothing. Also confirmed there is no
compensating gate elsewhere: the port has no handlers/gates analog and the only skill enforcement
is the read-path one in decide.rs:180-260. The port's own comment cites `pi :2243` — it is a
faithful port of the OLD upstream, not a marked CYRUP-DELTA, so this is silent drift. High is
correct, not critical: the skill's own files still go through the read gate for any skill NOT
typed by the user, and the bypass requires the operator to type the skill name.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
