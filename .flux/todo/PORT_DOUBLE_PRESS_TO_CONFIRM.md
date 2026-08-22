---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Port the double-press-to-confirm approval guard

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | high |
| **Kind** | absent |
| **Upstream area** | presentation — config-modal / permission dialog |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream ships a `doublePressToConfirm` setting (default ON) requiring a confirming second press
of a decision hotkey in the inline TUI permission dialog, exposed as a toggle in the settings
modal; the port has neither the config field nor the modal row nor the dialog behaviour.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

/home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/extension-config.ts:20 (field),
:41 (default `true`), :73 (`raw.doublePressToConfirm !== false`); /home/user/cyrup/tmp/pi-
packages/packages/pi-permission-system/src/config-modal.ts:117-124 (settings row "Require a
confirming second press of a decision hotkey in the inline TUI permission dialog"), :140-141
(applySetting arm), :151-160 (syncSettingValues); config-schema.ts:183

**Port** (`crates/cyrup-permission-system`):

`rg -ni "double_press|doublePress|second press|confirm_twice" /home/user/cyrup/crates/cyrup-
permission-system/src` → 0 matches. /home/user/cyrup/crates/cyrup-permission-
system/src/ext_config.rs:39-74 — `ExtensionConfig` carries only `enabled`, `debug`, `yolo_mode`,
`forwarded_prompt_timeout_seconds`. /home/user/cyrup/crates/cyrup-permission-
system/src/config_modal.rs:75-89 — `build_setting_items` returns 2 rows (`debug`, `yoloMode`)
against upstream's 4.

## Why it matters

A single stray keypress at the permission dialog approves the pending call outright. Upstream's
default-on guard against accidental approval (including of a forwarded subagent ask) does not
exist, and an operator cannot turn it on because the setting is not read, stored, or offered.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
