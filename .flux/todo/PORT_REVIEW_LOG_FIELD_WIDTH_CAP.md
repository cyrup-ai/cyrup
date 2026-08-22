---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Port the review-log field width cap and its config knob

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | high |
| **Kind** | absent |
| **Upstream area** | logging, redaction and log hygiene |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream narrows every string in a review-log detail record (recursing through plain objects and
arrays) to reviewLogFieldMaxWidth, defaulting to 1000, at writeLine — the single place a line is
produced; the port writes every string at full length.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

/home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/log-field-cap.ts:24-82
(DEFAULT_REVIEW_LOG_FIELD_MAX_WIDTH = 1000, resolveReviewLogFieldWidth, capLogFieldWidths/capValue
recursion, ellipsis marker); applied at logging.ts:62-65 and logging.ts:111-117 (review passes
`resolveReviewLogFieldWidth(config)`, debug deliberately passes none); config key at config-
schema.ts:211-217; tool-preview-formatter.ts:31,146 states the producer no longer bounds because
the writer does

**Port** (`crates/cyrup-permission-system`):

`rg -n
"cap_log_field_widths|resolve_review_log_field_width|review_log_field_max_width|FIELD_MAX_WIDTH"
/home/user/cyrup/crates/cyrup-permission-system/src` returns nothing.
/home/user/cyrup/crates/cyrup-permission-system/src/logging.rs:191-217 copies `details` into the
record untouched, and /home/user/cyrup/crates/cyrup-permission-system/src/extension/audit.rs:64-78
inserts the raw `prompt`, `command` and whole `toolInput` Value (dedup.rs:57 `pub tool_input:
serde_json::Value`). The port's only truncation is the 200/80-char PROMPT-side
`truncate_inline_text` (gate.rs:283-321), which never reaches the writer. The config struct has no
such field (ext_config.rs:39-73 lists only
enabled/debug/yolo_mode/forwarded_prompt_timeout_seconds).

## Why it matters

An unbounded JSONL trail: a single `write` tool call persists its entire file body, and a long
here-string bash command persists whole, so log growth and on-disk exposure are a side effect of
input size rather than an operator decision. Combined with the missing redactor, a large secret-
bearing payload is stored complete rather than clipped at 1000 characters.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
