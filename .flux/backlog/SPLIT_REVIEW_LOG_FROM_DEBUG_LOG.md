---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Write the review stream to its own permission-review.jsonl

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | medium |
| **Kind** | behaviour-drift |
| **Upstream area** | logging, redaction and log hygiene |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream keeps two log files — the diagnostic debug stream and the security review stream — with
separate paths handed to the writer; the port routes both streams into the single debug JSONL
file.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

/home/user/cyrup/tmp/pi-packages/packages/pi-permission-system/src/config-paths.ts:5-6
(DEBUG_LOG_FILENAME vs REVIEW_LOG_FILENAME = `${EXTENSION_ID}-permission-review.jsonl`); session-
logger.ts:62-68 passes both `debugLogPath` and `reviewLogPath`; logging.ts:99 and :111-117 write
each stream to its own path

**Port** (`crates/cyrup-permission-system`):

/home/user/cyrup/crates/cyrup-permission-system/src/logging.rs:193 — `let path =
debug_path(&logs_dir);` inside `write_line`, used for BOTH streams (logging.rs:145 debug,
logging.rs:168 review), where `debug_path` (logging.rs:62-64) is always
`<EXTENSION_ID>-debug.jsonl`. The port's own test asserts the shared file (logging.rs:495-509
`entries_append_and_share_one_file_across_streams`). `rg -n "permission-
review.jsonl|REVIEW_LOG_FILENAME|review_path" <src>` returns nothing.

## Why it matters

The security-relevant decision trail is interleaved with opt-in diagnostics in one file, so it
cannot be given its own retention, shipping or file permissions, and any operator tooling that
reads the documented permission-review log finds no file at all. It also blocks the upstream split
where only the review stream is width-capped while debug is deliberately uncapped.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
