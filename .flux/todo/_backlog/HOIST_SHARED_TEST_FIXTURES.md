---
stage: new
status: done
updated: 2026-08-22 18:45
---

# Hoist the Duplicated Test Fixtures and Lint Allows into src/tests

## Problem

**1. Five copies of the same fixtures.** The `modes.rs` split (PR #44) gave `tests::modes` a single `support.rs`, but the four sibling test files were left with private copies of exactly those fixtures — and the shared module is scoped `pub(super)` to `tests::modes`, so nothing outside it can reach it. Confirmed duplication:

- `struct Fixture { _tmp, cwd, agent_dir }` + `fn fixture()` — 5 copies: `src/tests/modes/support.rs:15-28`, `src/tests/rpc_host_seam.rs:43-56`, `src/tests/rpc_agent_settled.rs:103-116`, `src/tests/rpc_output_decoupling.rs:134-147`, `src/tests/json_event.rs:36-53`. A `diff` of the first four (modulo the `pub(super)` prefix) is **empty**; json_event.rs's differs only in rustfmt line-wrapping of the struct literal.
- `fn parse_lines(&[u8]) -> Vec<Value>` — 5 copies: `modes/support.rs:81-87`, `json_event.rs:66-73`, `rpc_agent_settled.rs:148-155`, `rpc_host_seam.rs:84-91`, `rpc_output_decoupling.rs:164-171`. Normalising away string literals, all five differ only in whether `String::from_utf8(...).expect(...)` is bound or chained, and in the `expect` text ("utf8 output" / "utf8" / "each line is valid json" / "valid json line" / "each line is a complete json record").
- `fn type_of(v: &Value) -> &str` — 3 copies: `modes/support.rs:89-91`, `rpc_agent_settled.rs:157-159`, and `json_event.rs:108-110` where it is renamed `kind` and defaults to `"<none>"` instead of `""`.
- `fn base_config(fx)` — 3 named copies (`modes/support.rs:30-34`, `rpc_host_seam.rs:58-63`, `rpc_agent_settled.rs:118-123`) plus the same three lines inlined at `rpc_output_decoupling.rs:156-158` and `json_event.rs:57-58`. The only real variation: the two seam files also set `cfg.no_extensions = true`.
- `build_runtime` — 5 near-copies: `modes/support.rs:56-65`, `json_event.rs:55-64`, `rpc_host_seam.rs:71-74`, `rpc_agent_settled.rs:134-146` (`runtime_with`), `rpc_output_decoupling.rs:149-162` (`runtime`). All are `SessionFactory::new(provider, cfg)` -> `AgentSessionRuntime::create(factory, target).await.expect("build runtime")`, differing only in whether an `AnyFauxResolver`, a native extension, or preset faux responses are attached.

That is ~120 lines of scaffolding maintained in four places on top of the module that already exists for it. `json_event.rs:33` even carries the comment `// Fixture (same shape as \`modes.rs\`)`, so the copy was known at the time. The cost is drift: a fixture change (e.g. the ambient-credential injection TEST_FAILURES.md calls for at the `AuthStore` tier) has to be applied five times, and the five divergent `expect` strings already make failures read differently for identical causes.

**2. Six copies of the same lint opt-out.** Every test module repeats the identical inner attribute `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]` to opt out of the workspace denies (`/home/user/cyrup/Cargo.toml:98-101`): `src/tests/json_event.rs:11-16` (wrapped across lines), `rpc_agent_settled.rs:13`, `rpc_client.rs:16`, `rpc_host_seam.rs:20`, `rpc_output_decoupling.rs:33`, `modes/mod.rs:13`. `src/tests/mod.rs:1-9` carries no such attribute, so the one place that could state the policy once states it nowhere and each new test file must remember to copy it. Lint levels propagate down the module tree, so a single inner attribute at the `tests` module root covers every descendant, including all eleven files under `modes/`.

## Fix

1. Move `src/tests/modes/support.rs` up to `src/tests/support.rs`, declare `mod support;` in `src/tests/mod.rs`, and keep its items `pub(super)` (at `tests::support` that already reaches every sibling) or widen to `pub(crate)`. Have `modes/*.rs` reach it via `super::super::support`, or re-export from `modes/mod.rs` so their existing `use super::support::{...}` lines are untouched.
2. Delete the four sibling copies and point `json_event.rs`, `rpc_agent_settled.rs`, `rpc_host_seam.rs` and `rpc_output_decoupling.rs` at the shared `Fixture`/`fixture`/`parse_lines`/`type_of`.
3. Keep the two axes that genuinely vary explicit rather than collapsing them: give `base_config` a `no_extensions` knob (or add `base_config_no_ext`), and keep the per-file runtime builders as thin local wrappers over one shared `create_runtime(factory, target)`.
4. Keep `kind`'s `"<none>"` default as a separate helper if `json_event.rs`'s assertions depend on it. Do not change any assertion or `expect` message a test's failure output relies on beyond unifying the `parse_lines` text.
5. Add the `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]` inner attribute to `src/tests/mod.rs` (below the existing `//!` doc at lines 1-2, above the `mod` declarations) with a one-line comment saying why the test tree opts out of the workspace denies, and delete all six per-file copies.

`src/tests/rpc_client.rs` legitimately shares no fixtures — it drives a scripted host, not a runtime — so leave its body alone; only its `#![allow(...)]` line at `:16` goes.

Purely mechanical: no test behaviour changes and no new tests.

## Acceptance Criteria

- [ ] `src/tests/support.rs` exists and is declared from `src/tests/mod.rs`; `src/tests/modes/support.rs` no longer holds a second copy
- [ ] `grep -c 'struct Fixture' crates/cyrup-modes/src/tests -r` returns 1, and the same for `fn parse_lines`
- [ ] `json_event.rs`, `rpc_agent_settled.rs`, `rpc_host_seam.rs` and `rpc_output_decoupling.rs` contain no local `Fixture`/`fixture`/`parse_lines`/`type_of` definitions
- [ ] The `no_extensions = true` variation used by rpc_host_seam.rs and rpc_output_decoupling.rs is still applied where it was before
- [ ] `src/tests/mod.rs` carries the four-lint `#![allow(...)]` inner attribute with a rationale comment, and `grep -rn 'clippy::unwrap_used' crates/cyrup-modes/src/tests/` returns exactly that one line
- [ ] `cargo test -p cyrup-modes` still lists 75 tests with the same pass/fail set as before
- [ ] `cargo clippy -p cyrup-modes --all-targets --no-deps` reports the same finding count as before the change (no new warnings from the test tree)

## Source

- Identified by the cyrup-modes hygiene audit (workflow `cyrup-modes-hygiene-audit`)
- Severity: medium | Size: medium
