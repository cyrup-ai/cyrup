---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Log the resolved policy paths and legacy-file detection at start

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | low |
| **Kind** | absent |
| **Upstream area** | handlers: session lifecycle |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream writes a `config.resolved` review + debug entry at session_start naming every resolved
policy path and flagging detected-but-ignored legacy global/project policy and legacy extension-
config files; the port emits no such entry.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

handlers/lifecycle.ts:57 (session.logResolvedConfigPaths()); permission-session.ts:193-195;
config-store.ts:197-224 (getResolvedPolicyPaths + legacyGlobalPolicyDetected /
legacyProjectPolicyDetected / legacyExtensionConfigDetected, logger.review("config.resolved", …)
and the matching debug line)

**Port** (`crates/cyrup-permission-system`):

`rg -n "config\.resolved|legacy_global_policy|legacyProjectPolicy|resolved_policy_paths"
/home/user/cyrup/crates/cyrup-permission-system/src` returns nothing; the SessionStart arm
(src/extension/native.rs:179-229) writes only `lifecycle.reload`, and refresh_extension_config
(src/extension/config.rs:98-106) writes only `config.loaded`.

## Why it matters

When policy does not behave as expected the operator cannot see from the audit trail which files
were actually loaded, and a stale legacy policy file sitting on disk being silently ignored is
undetectable.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

Could not refute. Upstream confirmed at handlers/lifecycle.ts:57 and config-store.ts:193-223
(getResolvedPolicyPaths + the three legacy* flags + the paired logger.review/logger.debug
"config.resolved"). Port: `grep -rn
'config\.resolved|legacy_global_policy|legacy_project|resolved_policy_paths' src/` returns
nothing; the SessionStart arm (src/extension/native.rs:214-229) writes only `lifecycle.reload` and
only when reason=="reload", and src/extension/config.rs:98-106 writes only `config.loaded` with
created/warning/debug/yoloMode — no paths. Severity low is right. Scope note for the fixer: only
the RESOLVED-PATHS half transfers. The three legacy detections are upstream's migration probes for
renamed pi files, and cyrup has no such migration — src/extension/paths.rs:13-18 defines one
policy filename (cyrup-permissions.jsonc) and one config location, and the
`legacy_global_settings_path` in src/manager.rs:107 / paths.rs:65 is a different thing entirely (a
settings.json policy SOURCE that is actively loaded at manager.rs:538, not a detected-and-ignored
file). Porting the legacy flags verbatim would log three permanently-false booleans.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
