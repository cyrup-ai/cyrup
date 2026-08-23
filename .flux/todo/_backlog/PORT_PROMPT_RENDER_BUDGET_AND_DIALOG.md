---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Port the configurable prompt render budget and structured dialog render

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | medium |
| **Kind** | absent |
| **Upstream area** | presentation — dialog-renderer / prompt-payload |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream renders a structured PromptPayload into aligned `label : value` rows under an operator-
configurable budget (promptMaxRows / promptFieldMaxWidth) with per-field caps, whole-entry
evidence elision, an explicit elision marker, a complete-view escape and flagged-token
highlighting; the port still uses the pre-payload sentence prompt plus a fixed 32-line /
2200-character compaction with no configuration.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

/home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/presentation/dialog-
renderer.ts:22-48 (renderPromptDialog), :76-79 (DEFAULT_RENDER_BUDGET 24 rows / 400 chars), :88-94
(resolveRenderBudget over promptMaxRows/promptFieldMaxWidth), :116-122 (completeViewBudget — the
un-elided view an operator must be able to reach), :146-155 (capField), :165-188 (fitToRows, core
exempt, evidence dropped whole), :205-236 (coreFacts), :287-348 (label alignment + whole-token
highlight of the flagged element); presentation/prompt-payload.ts:11-116; presentation/line-
fitting.ts:118-130; knobs declared at extension-config.ts:26-28 and config-schema.ts:197-210

**Port** (`crates/cyrup-permission-system`):

`rg -ni "prompt_max_rows|promptMaxRows|prompt_field_max_width|promptFieldMaxWidth|PromptPayload|pr
ompt_payload|evidence" /home/user/cyrup/crates/cyrup-permission-system/src -g '!tests'` → no
presentation matches (only unrelated prose in forwarding.rs/ext_config.rs). The port's prompt is a
single sentence built at /home/user/cyrup/crates/cyrup-permission-system/src/gate.rs:620-645
(`format_ask_prompt`) and clipped by the v0.7.1 compaction at /home/user/cyrup/crates/cyrup-
permission-system/src/ask.rs:247-282 (`compact_permission_prompt_for_select`, fixed
PERMISSION_DIALOG_MAX_VISIBLE_LINES/CHARACTERS).

## Why it matters

The human deciding an ask sees a truncated sentence with no way to reach the complete view and no
operator control over how much is shown, and the decision-relevant value is not visually
distinguished from surrounding text — so a long or crafted command can push the part that matters
past the fixed 2200-character cut before the approval is granted.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
