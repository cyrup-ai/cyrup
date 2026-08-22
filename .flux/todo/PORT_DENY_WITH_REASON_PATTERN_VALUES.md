---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Support deny-with-reason pattern values

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | high |
| **Kind** | absent |
| **Upstream area** | policy model / rule kinds |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream accepts `{"action": "deny", "reason": "…"}` as a pattern value and carries the custom
reason into the check result shown to the agent; the port's record normalizer accepts only bare
strings, so such an entry is silently discarded rather than becoming a deny.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

config-schema.ts:39-58 (denyWithReasonSchema, part of patternValueSchema); types.ts:78-87
`isDenyWithReason`; normalize.ts:29-37 (emits `{action:"deny", reason}` rules); rule.ts:37-38
(`Rule.reason`); types.ts:47-48 + permission-manager.ts:363 (`reason` on PermissionCheckResult)

**Port** (`crates/cyrup-permission-system`):

/home/user/cyrup/crates/cyrup-permission-system/src/manager.rs:1050-1060
`normalize_permission_record`: `if let Some(state) =
val.as_str().and_then(PermissionState::parse)` — a non-string (object) value is skipped with no
rule emitted. types.rs:457-465 `PermissionCheckResult` has no `reason` field. `rg -in
'deny_with_reason|DenyWithReason' /home/user/cyrup/crates/cyrup-permission-system/src` returns
nothing.

## Why it matters

A deny rule written in the documented object form contributes no rule at all, so the pattern falls
through to whatever broader (often `allow`) rule or default applies — a deny silently becomes a
permit. Even when a plain-string deny is used, the operator's custom denial reason never reaches
the agent.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
