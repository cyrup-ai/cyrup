---
stage: new
status: done
updated: 2026-08-23 00:06
---

# Drive the 131-site clippy baseline to zero and delete the blanket suppressions hiding another 71 warnings

> Found by an eight-lens workspace hygiene sweep. Every count below was reproduced
> against the tree before this task was filed.
> **Priority:** high · **Effort:** large
> **Crates:** `cyrup-provider`, `cyrup-ext-sdk`, `cyrup-ext`, `cyrup-tools`, `cyrup-modes`, `cyrup-permission-system`, `cyrup-session-svc`, `cyrup-intercom`, `cyrup-mcp`, `cyrup-core`, `cyrup-session`, `cyrup-resources`, `cyrup-ext-subagents`, `cyrup`, `cyrup-tui`

A workspace clippy run produces **131 unique warning sites**, and separate blanket `allow` attributes hide roughly **71 more**. Only `MUST_USE_BUILDER_METHODS` (11 sites in cyrup-tui) is queued against any of it.

**Live baseline (131 sites):**
- **cyrup-provider — 47 sites (36% of the whole baseline)**: 13 `unnecessary_get_then_check` (header/JSON `get(...).is_none()` probes in `api/google_vertex.rs`, `api/openai_completions.rs`, `api/openai_responses.rs`, `collection.rs`), 11 `doc_lazy_continuation` + 5 `doc_overindented_list_items` (13 of them in one doc block at `collection.rs:792-806`), 4 `chunks_exact_to_as_chunks` in the hand-rolled SHA-256 at `auth/oauth/sha256.rs`, plus `result_large_err` (416-byte Err on the bedrock stream driver), `collapsible_if`, `unnecessary_first_then_check`, `field_reassign_with_default`, and 10 `return_self_not_must_use`.
- **`return_self_not_must_use` — 56 sites total (43% of the baseline), the one lint the workspace deliberately opts into** in `[workspace.lints.clippy]` (verified present). 11 are the queued cyrup-tui task; the other **45** are in cyrup-ext-sdk (20 — `descriptor.rs:127,132,137,142,152,163,214,219,226`, `ctx/http.rs` ×3, `ctx/proc.rs` ×3, `provider.rs` ×3, `api.rs` ×2), cyrup-provider (10), cyrup-ext (9 — `host/services.rs:1522,1534,1654,1674,1699`, `event.rs`, `native.rs`, `host/limits.rs`), cyrup-core (3, `diagnostics.rs:43,48,53`), cyrup-session (2), cyrup-resources (1, `discovery.rs:659`).
- **cyrup-tools — 8**: 4 `cmp_owned` allocating a String per comparison, two of them in production shell dispatch (`src/ops/shell.rs:375,376`) plus their mirrors in `tests/shell_interpreter.rs:65,70`; 4 `default_constructed_unit_structs` in `src/tests/pi_tool_semantics.rs:35,39,232,258`. All 8 auto-fixable.
- **cyrup-modes — 6, all in one file**: `src/rpc_client.rs` — `collapsible_if` at :617, :648, :1056, :1162, :1167 and `single_match` at :342. `cargo clippy --fix --lib -p cyrup-modes` applies 5 of them.
- **Long tail — 23 across 7 crates**: `too_many_arguments` (8/7) at `cyrup-ext-subagents/src/exec/mod.rs:1558`, `cyrup-session-svc/src/bash.rs:138`, `cyrup-intercom/src/broker/test_support.rs:55`; `type_complexity` at `exec/mod.rs:5826`, `cyrup-permission-system/src/logging.rs:258`, `cyrup-session-svc/src/tests/round9_l5res.rs:578`; `drop_non_drop` at `spawn/parallel.rs:706` and `bash.rs:219` (both deliberate borrow-scoping — these want a commented `#[allow]`, not a rewrite); `result_large_err` (376 bytes) at `cyrup-mcp/src/runtime.rs:1696`; and the worst single spot, `cyrup-permission-system/src/dedup.rs:333` `mixed_attributes_style` plus three `duplicated_attribute` spans at `:338`, where the test module carries both an outer `#[allow(unwrap_used, expect_used, panic, indexing_slicing)]` and an inner `#![allow(...)]` repeating three of the four — a live "which one do I edit" hazard on the crate's panic-policy escape hatch.

**Suppressions hiding more (verified by grep):**
- `crates/cyrup-ext-sdk/src/guest.rs:12` carries `#![allow(clippy::all)]` over the whole **423-line** hand-written wasm guest routing module. Forcing lints back on (`--force-warn clippy::all`) shows it currently suppresses **nothing** — every forced warning attributed to guest.rs points at line 21, inside the `wit_bindgen::generate!` expansion, which has its own inner allow. It is a standing blanket with no present justification on the module every third-party extension build goes through.
- `crates/cyrup-ext-sdk/src/ctx/mod.rs:34` carries `#![allow(clippy::needless_return)]` with no reason attribute, hiding **50** machine-fixable occurrences across 10 submodules (ui.rs 12, base.rs 10, models.rs 6, proc.rs 6, tools.rs 4, command.rs 3, http.rs 3, session.rs 3, fs.rs 2, tool_call.rs 1). Three more sit outside it and warn on every build (`provider.rs:66,95,154`).
- **cyrup-ext-sdk is excluded from `default-members`** and its guest surface only compiles for wasm32, so no ordinary workspace `cargo clippy` ever lints it — that is why its 23 live warnings (20 `return_self_not_must_use` + 3 `needless_return`) are invisible despite the crate inheriting `[workspace.lints]`.
- **21 inert `allow` entries for pedantic-only lints** (20 attribute lines): `cast_precision_loss` ×11, `cast_possible_truncation` ×4, `too_many_lines` ×3 (incl. a file-level `#![allow]` at `cyrup-tui/src/markdown/latex.rs:16`), `cast_sign_loss` ×2, `float_cmp` ×1 (`cyrup-session/src/migrate.rs:163`). Nothing in the tree enables `clippy::pedantic` — the root lint table holds only the four denies plus `return_self_not_must_use`, there is no `clippy.toml`, no `.cargo/config.toml`, and the only "pedantic" mention outside `target/` is a completed .flux task that explicitly rejected adopting it. They falsely imply the numeric-cast surface has been audited.
- `crates/cyrup/src/main.rs:1563` is an unconditional `#[allow(dead_code)]` on `fn default_project_trust`, whose only occurrences workspace-wide are its own definition and its own body (verified) — real callers go through `cyrup_config::Effective::default_project_trust`.
- `crates/cyrup-tui/tests/zzz_scratch_perf_probe.rs` (**286 lines**, verified) is the workspace's only `#![allow(clippy::print_stdout)]`; its first line reads *"THROWAWAY perf probe (TUI-092 round 2). Not part of the suite — delete after measuring."* Its single `#[test] fn probe()` contains **zero** assertions (`grep -c assert` → 0) — it only prints a timing table, so it can never fail, yet it re-arms under `--all-features` and is one of two consumers cited in `cyrup-tui/Cargo.toml:27` to justify keeping the non-default `scrollback-accumulator` feature.
- Two `#[test]` fns in cyrup-ext-subagents assert nothing at all — `discovery/merge.rs:1865 output_spec_type_is_reachable_from_this_module` and `tests/discovery_integration.rs:581 path_buf_import_is_reachable` — whose bodies are a discarded `let _ = …` and whose comments admit they exist only to keep imports honest against unused-import lint drift. (`cyrup-tools/src/ops/mod.rs:704 bash_operations_is_object_safe` is a legitimate compile-time object-safety proof and must be left alone.)

## Acceptance Criteria

- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exits 0, and `cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2 --all-targets -- -D warnings` exits 0
- [ ] `#![allow(clippy::all)]` at cyrup-ext-sdk/src/guest.rs:12 and `#![allow(clippy::needless_return)]` at cyrup-ext-sdk/src/ctx/mod.rs:34 are deleted, and the 53 needless_return sites (50 hidden + provider.rs:66,95,154) are fixed rather than re-suppressed
- [ ] cyrup-ext-sdk is linted by the everyday build path: a documented command or CI/xtask step lints it for wasm32-wasip2, so its warnings can no longer hide behind its default-members exclusion
- [ ] All 21 pedantic-only allow entries (cast_precision_loss ×11, cast_possible_truncation ×4, too_many_lines ×3, cast_sign_loss ×2, float_cmp ×1) are deleted, OR clippy::pedantic is actually enabled in [workspace.lints.clippy] and each remaining allow carries a `reason = "…"`
- [ ] The duplicated panic-policy allow at cyrup-permission-system/src/dedup.rs:333/:338 is collapsed to one attribute (no mixed_attributes_style, no duplicated_attribute)
- [ ] Dead/assertion-free code is gone: crates/cyrup/src/main.rs `default_project_trust` and its #[allow(dead_code)]; crates/cyrup-tui/tests/zzz_scratch_perf_probe.rs (with the `scrollback-accumulator` rationale in cyrup-tui/Cargo.toml:27 updated); and the two `let _ = …` tests in cyrup-ext-subagents replaced by a real assertion or a scoped #[allow(unused_imports)]
- [ ] Every remaining `#[allow(clippy::…)]` introduced or kept by this task carries a `reason = "…"` or an inline comment stating the invariant it relies on — including the two deliberate `drop_non_drop` sites at spawn/parallel.rs:706 and session-svc/src/bash.rs:219
- [ ] `cargo test --workspace` shows no new failures and no net loss of meaningful test coverage

## Verifying command

```bash
cd /home/user/cyrup && cargo clippy --workspace --all-targets --message-format=short 2>&1 | grep -E ':[0-9]+:[0-9]+: warning:' | sort -u | wc -l && cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2 --message-format=short 2>&1 | grep -c 'warning:' && grep -rn 'allow(clippy::all)\|allow(clippy::needless_return)' --include='*.rs' crates
```
