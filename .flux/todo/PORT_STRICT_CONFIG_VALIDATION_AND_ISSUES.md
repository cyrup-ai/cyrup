---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Add strict config validation and accumulated config issues

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | medium |
| **Kind** | absent |
| **Upstream area** | config loading / validation and reporting |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream validates each config against a strict schema (unknown top-level keys are an error),
rejects the whole scope fail-closed on violation, accumulates path-qualified issue messages, and
adds two standing detectors (permissive-bash fallback, deprecated preview caps); the port parses
JSONC and silently ignores every unknown key and every malformed value, with no issue list.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

config-schema.ts:157-158 (`strictObject`); config-loader.ts:165-190 (`validateUnifiedConfig`
rejects the whole config on failure; `formatConfigIssues` renders `Unrecognized config key '<k>'`
and `Invalid config value at '<path>'`); config-loader.ts:399-419 `detectPermissiveBashFallback`;
config-loader.ts:433-446 `detectDeprecatedPreviewCaps`; policy-loader.ts:191-201
(`accumulateConfigIssues` / `getConfigIssues`); permission-manager.ts:152-164 (issues surfaced to
the operator)

**Port** (`crates/cyrup-permission-system`):

`rg -n 'config_issues|get_config_issues|unrecognized|unknown key' /home/user/cyrup/crates/cyrup-
permission-system/src` returns nothing. /home/user/cyrup/crates/cyrup-permission-
system/src/manager.rs:1050-1060 silently skips any pattern value that is not a parseable state
string; manager.rs:487-497 only reports whole-file read/parse failure through the single
`on_warning` callback (manager.rs:130-137), and unknown top-level keys are simply never read.

## Why it matters

A typo in a surface or key name (`"denny"`, `"permisson"`, `"skils"`) yields a config that loads
clean and enforces nothing where the operator believes it is enforcing a deny, with no warning
anywhere. The permissive-bash-fallback footgun (`"*": "allow"` with no bash policy, so every bash
command inherits allow) is likewise never flagged.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
