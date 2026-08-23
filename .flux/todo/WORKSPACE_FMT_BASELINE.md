---
stage: new
status: done
updated: 2026-08-23 00:06
---

# Format the workspace and add a rustfmt.toml plus a format gate — cargo fmt --all --check fails with 13,585 hunks across 1,068 of 1,291 .rs files

> Found by an eight-lens workspace hygiene sweep. Every count below was reproduced
> against the tree before this task was filed.
> **Priority:** high · **Effort:** large
> **Crates:** `cyrup-ext-subagents`, `cyrup-tui`, `cyrup-mcp`, `cyrup-it`, `cyrup-session-svc`, `cyrup-ext`, `cyrup-intercom`, `cyrup-tools`, `cyrup-session`, `cyrup-agent`, `cyrup-permission-system`, `cyrup-provider`, `cyrup-modes`, `cyrup`, `cyrup-ext-sdk`, `cyrup-test-support`, `cyrup-core`, `cyrup-resources`, `cyrup-flux`, `cyrup-sdk`, `xtask`

The workspace has never been run through rustfmt. Re-verified in this repo just now: `cargo fmt --all --check` exits **1** and emits **13,585** `Diff in` hunks touching **1,068** distinct files out of the 1,291 `.rs` files under `crates/` and `xtask/` (82.7%). Only `cyrup-config` is entirely clean (cleaned by the completed CLIPPY_FMT_DEAD_CODE_CLEANUP task).

Per-member hunk counts: cyrup-ext-subagents 3,015, cyrup-tui 2,701, cyrup-mcp 1,268, cyrup-it 1,115, cyrup-session-svc 1,060, cyrup-ext 862, cyrup-intercom 669, cyrup-tools 468, cyrup-session 468, cyrup-agent 450, cyrup-permission-system 426, cyrup-provider 369, cyrup-modes 193, cyrup 172, cyrup-ext-sdk 137, cyrup-test-support 84, cyrup-core 45, cyrup-resources 30, cyrup-flux 24, xtask 22, cyrup-sdk 7. Worst files: `crates/cyrup-mcp/src/proxy.rs` (320 hunks), `crates/cyrup-mcp/src/config.rs` (168), `crates/cyrup-ext-subagents/src/discovery/management.rs` (157), `crates/cyrup-session/src/tests/compaction.rs` (156), `crates/cyrup-mcp/src/ui.rs` (144).

Drift kinds over the 13,585 hunks: 11,931 long-line reflow (19,072 source lines exceed rustfmt's default `max_width` 100; the longest is 222 chars in `proxy.rs`), **652 pure `use`-ordering hunks in 429 files** (edition 2024 selects style_edition 2024, which sorts `{json, Map as JsonMap, Value}` → `{Map as JsonMap, Value, json}`), 530 multiline→one-line joins, 204 blank-line removals, 270 other.

There is also no contract and no gate: no `rustfmt.toml`/`.rustfmt.toml` anywhere (verified — and none in git history), no `.github` directory, no `.yml`/`.yaml` workflow, and no non-sample hook in `.git/hooks`. Widening defaults does not rescue the tree — `max_width=120` still gives 13,180 hunks, and `use_small_heuristics=Max` makes `cli.rs` strictly worse (36 hunks vs 13) — so the fix is to commit a config and format, not to bend the config to the drift.

The cost is compounding: prior decomposition tasks have had to write explicit "do not run cargo fmt" guardrails (`.flux/done/2026-08-22-16-57/BEDROCK_CONVERSE_STREAM_DECOMPOSE.md:152`, `.flux/done/2026-08-22-16-09/SESSION_RS_DECOMPOSE.md:130`).

**Suggested sequencing:** land the 652-hunk import-ordering slice first (semantically inert, reviewable by inspection, minimal collision with in-flight decomposition tasks), then the bulk reflow, then the config + gate.

## Acceptance Criteria

- [ ] A `rustfmt.toml` is committed at the workspace root with an explicit, justified config (defaults `max_width = 100` / style_edition 2024 unless a documented reason says otherwise)
- [ ] `cargo fmt --all --check` exits 0 with zero `Diff in` hunks across all 22 members
- [ ] A format gate exists and is wired to run on every change (CI workflow or a checked-in git hook plus an xtask target) and fails when `cargo fmt --all --check` fails
- [ ] The formatting commits are mechanical only: `git diff --stat` shows no line added or removed that is not rustfmt output — no logic, import removals, or attribute changes ride along
- [ ] `cargo build --workspace` and `cargo test --workspace` produce the same results before and after (no behavior change)
- [ ] Existing 'do not run cargo fmt' guardrail notes in in-flight/queued task files are removed or updated, since the guardrail is now obsolete

## Verifying command

```bash
cd /home/user/cyrup && cargo fmt --all --check > /tmp/fmt.txt 2>&1; echo "EXIT=$?"; grep -c '^Diff in ' /tmp/fmt.txt; grep '^Diff in ' /tmp/fmt.txt | sed 's|:[0-9]*:$||' | sort -u | wc -l
```
