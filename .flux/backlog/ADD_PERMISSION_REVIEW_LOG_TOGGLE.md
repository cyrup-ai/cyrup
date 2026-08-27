---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Add the permissionReviewLog config toggle

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | low |
| **Kind** | absent |
| **Upstream area** | logging, redaction and log hygiene |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream gates the review stream on a dedicated permissionReviewLog config flag (default true)
that is independent of debugLog; the port writes review entries unconditionally with no operator
control.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

/home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/logging.ts:102-118 (`if
(!config.permissionReviewLog) return undefined;`); extension-config.ts:16-17,38-39,70-71 (defaults
`debugLog: false`, `permissionReviewLog: true`, normalized as `raw.permissionReviewLog !==
false`); config-schema.ts:169-175 and config-modal.ts:102-114,136-139 expose it in the settings
modal

**Port** (`crates/cyrup-permission-system`):

/home/user/cyrup/crates/cyrup-permission-system/src/logging.rs:167-169 — `review` is a bare
`self.write_line("review", ...)` with no config read. `rg -n
"permission_review_log|permissionReviewLog" /home/user/cyrup/crates/cyrup-permission-system/src`
returns nothing, and ext_config.rs:39-73 / EXTENSION_CONFIG_KEYS (ext_config.rs:118) carry only
`debug`, `yoloMode`, `forwardedPromptTimeoutSeconds`.

## Why it matters

An operator on a shared or sensitive host has no supported way to stop the trail from accumulating
command strings and tool input on disk — the only escape is redirecting
CYRUP_PERMISSION_SYSTEM_LOGS_DIR. Conversely, a config file written by upstream tooling with
`permissionReviewLog: false` is silently ignored, so the operator believes logging is off while it
continues.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
