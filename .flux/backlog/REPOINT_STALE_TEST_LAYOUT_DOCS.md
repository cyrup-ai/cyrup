---
stage: aug
status: done
updated: 2026-08-27 04:55
---

# Repoint The Crate's Prose At The Test Layout That Actually Exists

## Verification headline (read this first)

**Nothing in this task has been fixed. Every stale pointer it named is still there.** Verified
against the working tree at `df64e81` (`origin/main` = `d2c5b1e` is an ancestor;
`git diff d2c5b1e..HEAD -- crates/cyrup-ext-subagents` is **empty**, so this crate is byte-identical
to `main`).

What *has* moved is the task's own bookkeeping, and it moved enough to matter:

* **Every line number in the original Evidence paragraph is wrong except three.** Only
  `src/exec/output.rs:1694`, `src/extension/host/registration.rs:267` and `Cargo.toml:15-21` survived
  unchanged.
* **One cited *file* no longer exists.** The task's `src/exec/mod.rs:1312` / `:7058` are now
  [`src/exec/spawn_plan.rs:39`](../../crates/cyrup-ext-subagents/src/exec/spawn_plan.rs) and
  [`:3265`](../../crates/cyrup-ext-subagents/src/exec/spawn_plan.rs) — `exec/mod.rs` was split.
* **Two Evidence claims are now false and are corrected below** (§4, §5): the fixture bins are *not*
  unlinted, and the `builtin_catalog()` consumer list was incomplete.
* **One stale site the task missed**: [`Cargo.toml:157`](../../crates/cyrup-ext-subagents/Cargo.toml)
  cites `crates/cyrup-tui/tests/assembled_render.rs`, which is now `crates/cyrup-tui/src/tests/`.

Crate root for every bare `src/…` / `Cargo.toml` path below:
[`crates/cyrup-ext-subagents`](../../crates/cyrup-ext-subagents).

## Description

Two commits (`63d729a` "move 199 integration tests into their crates as unit tests" and `c3982b5`
"add cyrup-it integration harness") drained this crate's `tests/` directory, and nothing updated the
prose that describes it. `ls crates/cyrup-ext-subagents` is still exactly `Cargo.toml resources src`
— no `tests/` — yet 22 source comments and five `Cargo.toml` comment blocks still route readers
there.

This matters more here than usual because these comments are the only map of which test location
covers a given seam, and the crate now has **four** of them (inline `#[cfg(test)] mod tests`; the
`#[path]` sibling [`src/extension/tool/routing_tests.rs`](../../crates/cyrup-ext-subagents/src/extension/tool/routing_tests.rs)
declared at [`routing.rs:1598`](../../crates/cyrup-ext-subagents/src/extension/tool/routing.rs);
[`src/tests/`](../../crates/cyrup-ext-subagents/src/tests); and `crates/cyrup-it/tests/{subagents,permission}/`).
Following a `tests/…` pointer now dead-ends, so a reader checking whether a spawn-env or
permission-forwarding contract is proven anywhere cannot find the proof.

Worst of all, the file whose job is to *state* the placement rule contradicts itself:
[`src/tests/mod.rs:10-12`](../../crates/cyrup-ext-subagents/src/tests/mod.rs) says env-mutating files
"can NOT move here … Those files stay in `tests/`" — naming the deleted directory as the
destination, so the rule cannot be applied by anyone adding a test today.

## 1. What the layout actually is now

Confirmed by reading the tree, not by pattern. Four locations, and the choice between them is forced
by what the test needs to reach:

| Location | When | Evidence |
|---|---|---|
| Inline `#[cfg(test)] mod tests` beside the code | needs private items/fields, or must sit next to a process-global `static` and the lock serializing it | the crate's dominant form; e.g. [`src/background/control.rs`](../../crates/cyrup-ext-subagents/src/background/control.rs), [`src/tui/render.rs`](../../crates/cyrup-ext-subagents/src/tui/render.rs) |
| `#[path]` sibling `*_tests.rs` | one module's tests are large enough to want their own file but still need its private surface | [`src/extension/tool/routing_tests.rs:4`](../../crates/cyrup-ext-subagents/src/extension/tool/routing_tests.rs) states the reason; declared at [`routing.rs:1598`](../../crates/cyrup-ext-subagents/src/extension/tool/routing.rs) |
| **`src/tests/`** (12 modules) | a whole relocated file that drives the crate's own public API in-process — no process seam, no built artifact, no `CARGO_BIN_EXE_*` | [`src/tests/mod.rs`](../../crates/cyrup-ext-subagents/src/tests/mod.rs), declared `#[cfg(test)] mod tests;` at [`src/lib.rs:53-54`](../../crates/cyrup-ext-subagents/src/lib.rs) |
| **`crates/cyrup-it/tests/subagents/`** (35 files) | spawns a real child, or mutates this process's env so the in-process spawn resolver picks up a double — i.e. needs `unsafe { std::env::set_var }`, which `#![forbid(unsafe_code)]` blocks in `src/` | [`crates/cyrup-it/tests/subagents/main.rs:1-9`](../../crates/cyrup-it/tests/subagents/main.rs) |

Two supporting facts a rewrite must not get wrong:

* `crates/cyrup-it/tests/subagents/` files **no longer carry `#![cfg(feature = "test-fixtures")]`**.
  [`main.rs:22-31`](../../crates/cyrup-it/tests/subagents/main.rs) records that the attribute was
  deliberately removed (re-spelled inside `cyrup-it` it would name *that* crate's features, where no
  such feature exists, and all 23 modules would compile to nothing and pass vacuously). Fixture
  availability is now a **build-script postcondition**, not a compile-time cfg.
* The whole `cyrup-it` suite is off by default behind the `it` feature
  ([`crates/cyrup-it/Cargo.toml:53`](../../crates/cyrup-it/Cargo.toml)); `build.rs` no-ops unless
  `CARGO_FEATURE_IT` is set ([`build.rs:95-97`](../../crates/cyrup-it/build.rs)).

The workspace-level statement of the same rule is
[`docs/TEST-ARCHITECTURE.md` §0/§9.1](../../docs/TEST-ARCHITECTURE.md) — "every crate keeps unit
tests only (`#[cfg(test)]` inside `src/`); integration tests move to a single separate crate".

## 2. Current state — every doc site, verified

Line numbers below were each read in the working tree. "Task said" is the original Evidence
paragraph's citation.

### 2a. Source comments (22 pointers, 12 files) — **all still stale**

| # | Verified site | Task said | Cited path | Correct target |
|---|---|---|---|---|
| 1 | [`src/tests/mod.rs:3`](../../crates/cyrup-ext-subagents/src/tests/mod.rs) | 10-12 | `crates/cyrup-ext-subagents/tests/` | past tense, keep — but must not read as a live destination |
| 2 | [`src/tests/mod.rs:12`](../../crates/cyrup-ext-subagents/src/tests/mod.rs) | 10-12 ✓ | "Those files stay in `tests/`" | `crates/cyrup-it/tests/subagents/` |
| 3 | [`src/lib.rs:51`](../../crates/cyrup-ext-subagents/src/lib.rs) | — (missed) | "relocated out of `tests/`" | historical; disambiguate |
| 4 | [`src/exec/output.rs:1694`](../../crates/cyrup-ext-subagents/src/exec/output.rs) | 1694 ✓ | `tests/exec_run_sync_integration.rs` | `crates/cyrup-it/tests/subagents/exec_run_sync_integration.rs` — `message_end_line` is still real, at [`:149`](../../crates/cyrup-it/tests/subagents/exec_run_sync_integration.rs) |
| 5 | [`src/exec/spawn_plan.rs:39`](../../crates/cyrup-ext-subagents/src/exec/spawn_plan.rs) | `exec/mod.rs:1312` | `cyrup-permission-system/tests/forwarding_spawn_env.rs` | `crates/cyrup-it/tests/permission/forwarding_spawn_env.rs` |
| 6 | [`src/exec/spawn_plan.rs:3265`](../../crates/cyrup-ext-subagents/src/exec/spawn_plan.rs) | `exec/mod.rs:7058` | same | same |
| 7 | [`src/discovery/skills.rs:90`](../../crates/cyrup-ext-subagents/src/discovery/skills.rs) | 90 ✓ | `cyrup-resources`' `tests/resources.rs` | `crates/cyrup-resources/src/tests/resources/` (module dir) |
| 8 | [`src/discovery/skills.rs:758`](../../crates/cyrup-ext-subagents/src/discovery/skills.rs) | 758 ✓ | `tests/resources.rs`'s `root/global` | [`crates/cyrup-resources/src/tests/resources/skills.rs:129-131`](../../crates/cyrup-resources/src/tests/resources/skills.rs) |
| 9 | [`src/tui/render.rs:666`](../../crates/cyrup-ext-subagents/src/tui/render.rs) | 686 | `crates/cyrup-tui/tests/assembled_render.rs` | [`crates/cyrup-tui/src/tests/assembled_render.rs`](../../crates/cyrup-tui/src/tests/assembled_render.rs) |
| 10 | [`src/background/spawn_detached.rs:265`](../../crates/cyrup-ext-subagents/src/background/spawn_detached.rs) | 270-271 | "files under `tests/`" | true as a *Cargo* fact; keep the fact, drop the implication that this crate has such files |
| 11 | [`src/background/spawn_detached.rs:270`](../../crates/cyrup-ext-subagents/src/background/spawn_detached.rs) | 270 ✓ | `tests/background_spawn_detached_integration.rs` | `crates/cyrup-it/tests/subagents/background_spawn_detached_integration.rs` |
| 12 | [`src/background/spawn_detached.rs:271`](../../crates/cyrup-ext-subagents/src/background/spawn_detached.rs) | 271 ✓ | `tests/exec_run_sync_integration.rs` | `crates/cyrup-it/tests/subagents/exec_run_sync_integration.rs` |
| 13-15 | [`src/background/runner_main.rs:3746, :3801, :3928`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs) | 3746/3801/3928 ✓ | `tests/background_runner_main_integration.rs` ×3 | `crates/cyrup-it/tests/subagents/background_runner_main_integration.rs` |
| 16 | [`src/background/runner_main.rs:3845-3861`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs) | 3849/3859 | that file + `tests/background_spawn_detached_integration.rs` + "files under `tests/`" | as above |
| 17 | [`src/background/runner_main.rs:4165`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs) | 4165 ✓ | `tests/run_state_signal_and_stop_parity.rs` | `crates/cyrup-it/tests/subagents/run_state_signal_and_stop_parity.rs` |
| 18 | [`src/background/control.rs:2730`](../../crates/cyrup-ext-subagents/src/background/control.rs) | 2748 | `tests/steer_delivery_integration.rs` | **not** cyrup-it — [`src/tests/steer_delivery_integration.rs`](../../crates/cyrup-ext-subagents/src/tests/steer_delivery_integration.rs), in this crate |
| 19 | [`src/extension/testsupport.rs:10`](../../crates/cyrup-ext-subagents/src/extension/testsupport.rs) | 10 ✓ | `tests/cyrup_home_env_sandboxed_tests.rs` | `crates/cyrup-it/tests/subagents/cyrup_home_env_sandboxed_tests.rs` |
| 20 | [`src/extension/testsupport.rs:12`](../../crates/cyrup-ext-subagents/src/extension/testsupport.rs) | — (missed) | "every other `tests/*_integration.rs` file's … convention **in this crate**" | doubly wrong: no such dir, and the convention now lives in `cyrup-it` |
| 21 | [`src/extension/executor/paths.rs:748`](../../crates/cyrup-ext-subagents/src/extension/executor/paths.rs) | 755 | `tests/cyrup_home_env_sandboxed_tests.rs` | as #19 |
| 22 | [`src/extension/host/registration.rs:253`](../../crates/cyrup-ext-subagents/src/extension/host/registration.rs) | — (missed) | "the `tests/` integration file" | `crates/cyrup-it/tests/subagents/subagents_optin_gate_integration.rs` |
| 23 | [`src/extension/host/registration.rs:267`](../../crates/cyrup-ext-subagents/src/extension/host/registration.rs) | 267 ✓ | `tests/subagents_optin_gate_integration.rs` | as #22 |
| 24 | [`src/tests/dynamic_collect_record_fidelity.rs:22`](../../crates/cyrup-ext-subagents/src/tests/dynamic_collect_record_fidelity.rs) | 22 ✓ | `tests/dynamic_group_acceptance_parity.rs` | **sibling in the same directory**: [`src/tests/dynamic_group_acceptance_parity.rs`](../../crates/cyrup-ext-subagents/src/tests/dynamic_group_acceptance_parity.rs) |
| 25 | [`src/tests/dynamic_collect_record_fidelity.rs:25`](../../crates/cyrup-ext-subagents/src/tests/dynamic_collect_record_fidelity.rs) | 25 ✓ | `tests/chain_step_child_detail_integration.rs` | `crates/cyrup-it/tests/subagents/chain_step_child_detail_integration.rs` |

**Not stale — do not touch.** These five `tests/…` hits are JSON-schema *example values* inside
prompt/report literals, not routing pointers:
[`src/exec/acceptance/model/report/parse.rs:589`](../../crates/cyrup-ext-subagents/src/exec/acceptance/model/report/parse.rs),
[`src/exec/acceptance/lattice/gate.rs:579`](../../crates/cyrup-ext-subagents/src/exec/acceptance/lattice/gate.rs) and
[`:581`](../../crates/cyrup-ext-subagents/src/exec/acceptance/lattice/gate.rs),
[`src/tests/acceptance_policy_parity.rs:171`](../../crates/cyrup-ext-subagents/src/tests/acceptance_policy_parity.rs) and `:174`.

### 2b. `Cargo.toml` (5 blocks) — **all still stale**

| Block | Verified lines | Task said | Defect |
|---|---|---|---|
| `test-fixtures` feature | [15-21](../../crates/cyrup-ext-subagents/Cargo.toml) | 15-21 ✓ | claims "this crate's own integration tests … via `cargo test`" |
| `cyrup-provider` rationale | [51-58](../../crates/cyrup-ext-subagents/Cargo.toml) | 37-44 | names `catalog::seed_catalog`, which does not exist |
| `[[bin]] cyrup-subagent-fixture` | [106-116](../../crates/cyrup-ext-subagents/Cargo.toml) | 93-99 | same "this crate's own integration tests" claim |
| `[[bin]] cyrup-subagent-orchestrator-sim` | [118-126](../../crates/cyrup-ext-subagents/Cargo.toml) | 105-109 | "an integration test can kill it" — no such test in this crate |
| `cyrup-test-support` dev-dep | [156-161](../../crates/cyrup-ext-subagents/Cargo.toml) | 147 | **missed by the task**: cites `crates/cyrup-tui/tests/assembled_render.rs` |
| `cyrup-session-svc` dev-dep | [162-170](../../crates/cyrup-ext-subagents/Cargo.toml) | 148-156 | justified by a file in another crate; zero code references |

## 3. `cyrup-session-svc` — re-verified, still an unjustified dev-dependency

* The justifying file, `tests/extension_end_to_end_smoke.rs`, is now
  [`crates/cyrup-it/tests/subagents/extension_end_to_end_smoke.rs`](../../crates/cyrup-it/tests/subagents/extension_end_to_end_smoke.rs)
  — in a crate that carries its own `cyrup-session-svc` at
  [`crates/cyrup-it/Cargo.toml:92`](../../crates/cyrup-it/Cargo.toml) (task's citation ✓ still
  correct).
* `grep -rn 'cyrup_session_svc' src/` returns **one** hit, a `//!` doc line at
  [`src/exec/ndjson.rs:18`](../../crates/cyrup-ext-subagents/src/exec/ndjson.rs) (task said `:20` —
  drifted). Zero code references.
* It is already reachable through `cyrup-test-support`, which lists it in **`[dependencies]`** at
  [`crates/cyrup-test-support/Cargo.toml:20`](../../crates/cyrup-test-support/Cargo.toml) — a normal
  edge, so the dev-dependency at `Cargo.toml:161` already pulls it in. (Manifest-read verification;
  no `cargo tree` needed.)

The crate-boundary sentence the block carries ("never depends on the session-service facade in
production code") is worth keeping. Its home is the `//!` doc of
[`src/extension/mod.rs`](../../crates/cyrup-ext-subagents/src/extension/mod.rs), which already states
the adjacent rule at `:20-23` ("no extension-host session-access seam beyond the one, narrow,
sanctioned `crate::fork_context` dependency on `cyrup-session` (§6.6)"). Append it there.

## 4. `test-fixtures` — the claim is wrong, but **not in the way the task said**

Re-verified, and the original Evidence overstated it. Correct picture:

* **True**: a package cannot enable its own feature from its dev-dependency closure. This crate has
  **no self-dev-dependency** (`grep -n cyrup-ext-subagents Cargo.toml` finds only `name =` and one
  prose mention at `:167`), unlike `crates/cyrup`. So `cargo test -p cyrup-ext-subagents` never
  enables `test-fixtures`, and `cargo check -p cyrup-ext-subagents --all-targets` never builds
  `src/bin/` (460 + 191 = **651** lines — count re-verified). The three comment blocks' claim is
  therefore false.
* **False, and must not be repeated in the rewrite**: that those 651 lines "go unlinted". There are
  **two** real consumers, not one:
  1. [`crates/cyrup-it/build.rs:67-77`](../../crates/cyrup-it/build.rs) — the `BINS` table runs
     `cargo build -p cyrup-ext-subagents --features test-fixtures` (grouped by package+features) into
     `$OUT_DIR/it-bins`, and armed only when `CARGO_FEATURE_IT` is set.
  2. [`xtask/src/features.rs:145-151`](../../xtask/src/features.rs) — the `feature-matrix` row
     `cargo check -p cyrup-ext-subagents --features test-fixtures --all-targets`, run by hand via
     `cargo run -p xtask -- feature-matrix` ([`README.md:162-166`](../../README.md)). This *does*
     type-check the bins. The repo also documents
     `cargo clippy --workspace --all-targets --features test-fixtures`
     ([`spec/flux/README.md:117`](../../spec/flux/README.md)).

So the rewrite should say: *the bins are built by `cyrup-it`'s `build.rs` and type-checked by
`xtask feature-matrix`; nothing in this crate's own default `cargo test`/`cargo check` path builds
them.*

## 5. `cyrup-provider` — `seed_catalog` is fiction; the real coupling is wider

* `grep -rn seed_catalog --include=*.rs --include=*.toml crates/` finds **no definition** — only
  [`Cargo.toml:53`](../../crates/cyrup-ext-subagents/Cargo.toml) and
  [`crates/cyrup-it/tests/subagents/registration_commands_integration.rs:112`](../../crates/cyrup-it/tests/subagents/registration_commands_integration.rs)
  calling it "the retired 2-model `seed_catalog()` stub".
* `load_catalog` exists at
  [`crates/cyrup-provider/src/catalog.rs:17`](../../crates/cyrup-provider/src/catalog.rs) but is
  never called from this crate.
* The real API is `builtin_catalog()`
  ([`catalog.rs:40`](../../crates/cyrup-provider/src/catalog.rs)). **Three** production call sites in
  this crate — the task listed two:
  * [`src/extension/models/mod.rs:59`](../../crates/cyrup-ext-subagents/src/extension/models/mod.rs) (`registry_models()`)
  * [`src/watchdog/model_selection.rs:165`](../../crates/cyrup-ext-subagents/src/watchdog/model_selection.rs) and [`:172`](../../crates/cyrup-ext-subagents/src/watchdog/model_selection.rs)
  * [`src/extension/executor/reports.rs:549`](../../crates/cyrup-ext-subagents/src/extension/executor/reports.rs) — **missed by the task**

  Plus doc references at [`src/extension/models/probe.rs:6`](../../crates/cyrup-ext-subagents/src/extension/models/probe.rs)
  and [`src/watchdog/model_selection.rs:27`](../../crates/cyrup-ext-subagents/src/watchdog/model_selection.rs).

**Decision: rewrite the prose, do not move code.** The task's alternative — routing the watchdog's
catalog access through `extension/models` to restore the stated narrow boundary — is rejected
because the boundary was never real: `reports.rs:549` calls `builtin_catalog()` directly too, so
"the ONLY … static-catalog read" for two slash commands has been false for longer than the watchdog
has existed. Restoring it would be a three-site refactor of production code inside a
comment-repointing task. Out of scope; note it and move on.

## 6. Exact edits

Every replacement below is verbatim-current text on the left. Paths on the right were each confirmed
to exist with `ls`.

### 6.1 The rule file — do this one first, the rest must agree with it

[`src/tests/mod.rs:1-14`](../../crates/cyrup-ext-subagents/src/tests/mod.rs) — replace lines 3 and
10-12, and add the routing rule.

**Before** (`:3` and `:10-12`):
```rust
//! These files previously lived under `crates/cyrup-ext-subagents/tests/` as separate Cargo
...
//! Files that mutate the process environment (`std::env::set_var`/`remove_var`) can NOT move here:
//! this crate is `#![forbid(unsafe_code)]` (src/lib.rs) and Rust 2024 requires `unsafe` for those
//! calls, and `forbid` cannot be locally overridden. Those files stay in `tests/`.
```

**After** (`:3` keeps the history but names the directory as gone; `:10-12` gains the live
destination and the full three-way rule):
```rust
//! These files previously lived under a `crates/cyrup-ext-subagents/tests/` directory — since
//! deleted — as separate Cargo
...
//! # Where a new test goes
//!
//! - Needs private items/fields, or must sit beside a process-global `static` and its lock ->
//!   inline `#[cfg(test)] mod tests` in the module itself. If that module's tests grow their own
//!   file, use a `#[path]` sibling (`extension/tool/routing_tests.rs`, declared at
//!   `extension/tool/routing.rs:1598`), which still sees the module's private surface.
//! - A whole file driving this crate's own public API in-process, with no process seam, no built
//!   artifact and no `CARGO_BIN_EXE_*` -> here, `src/tests/`.
//! - Needs `unsafe { std::env::set_var }` (this crate is `#![forbid(unsafe_code)]`, `src/lib.rs`,
//!   and Rust 2024 requires `unsafe` for env mutation; `forbid` cannot be locally overridden), or
//!   spawns a real child through `CYRUP_SUBAGENT_BINARY` -> `crates/cyrup-it/tests/subagents/`.
//!   That crate's suite is off by default behind its `it` feature; the fixture binaries are built
//!   by its `build.rs` (`crates/cyrup-it/build.rs:67-77`), NOT by a `#![cfg(feature = ...)]`
//!   attribute — see `crates/cyrup-it/tests/subagents/main.rs:22-31`.
//!
//! This crate has no `tests/` directory. Never route a reader to one.
```

### 6.2 Same-crate repoint (one site, and the only one whose destination is *not* `cyrup-it`)

[`src/background/control.rs:2730`](../../crates/cyrup-ext-subagents/src/background/control.rs):

* before: ``/// `tests/steer_delivery_integration.rs` fail about one run in four, which is the more``
* after:  ``/// `src/tests/steer_delivery_integration.rs` fail about one run in four, which is the more``

[`src/tests/dynamic_collect_record_fidelity.rs:22`](../../crates/cyrup-ext-subagents/src/tests/dynamic_collect_record_fidelity.rs)
— the referenced file is a **sibling in this very directory**:

* before: ``//! `tests/dynamic_group_acceptance_parity.rs` uses: what is under test is the walker's fold, and a``
* after:  ``//! `dynamic_group_acceptance_parity.rs` (this directory) uses: what is under test is the walker's fold, and a``

### 6.3 `crates/cyrup-it/tests/subagents/` repoints (prefix rewrite, 15 sites)

Mechanical `tests/X.rs` -> `crates/cyrup-it/tests/subagents/X.rs`, **but confirm each with `ls`** —
do not run a blanket `sed`, because §6.2 and §6.4 are exceptions to it.

| File:line | Cited file (all now under `crates/cyrup-it/tests/subagents/`) |
|---|---|
| [`src/exec/output.rs:1694`](../../crates/cyrup-ext-subagents/src/exec/output.rs) | `exec_run_sync_integration.rs` |
| [`src/background/spawn_detached.rs:270`](../../crates/cyrup-ext-subagents/src/background/spawn_detached.rs) | `background_spawn_detached_integration.rs` |
| [`src/background/spawn_detached.rs:271`](../../crates/cyrup-ext-subagents/src/background/spawn_detached.rs) | `exec_run_sync_integration.rs` |
| [`src/background/runner_main.rs:3746, :3801, :3849, :3928`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs) | `background_runner_main_integration.rs` |
| [`src/background/runner_main.rs:3859`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs) | `background_spawn_detached_integration.rs` |
| [`src/background/runner_main.rs:4165`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs) | `run_state_signal_and_stop_parity.rs` |
| [`src/extension/testsupport.rs:10`](../../crates/cyrup-ext-subagents/src/extension/testsupport.rs), [`src/extension/executor/paths.rs:748`](../../crates/cyrup-ext-subagents/src/extension/executor/paths.rs) | `cyrup_home_env_sandboxed_tests.rs` |
| [`src/extension/host/registration.rs:253, :267`](../../crates/cyrup-ext-subagents/src/extension/host/registration.rs) | `subagents_optin_gate_integration.rs` |
| [`src/tests/dynamic_collect_record_fidelity.rs:25`](../../crates/cyrup-ext-subagents/src/tests/dynamic_collect_record_fidelity.rs) | `chain_step_child_detail_integration.rs` |

Three of these carry a *second* defect beyond the path — the sentence around them is now wrong:

**[`src/background/spawn_detached.rs:263-272`](../../crates/cyrup-ext-subagents/src/background/spawn_detached.rs)**

* before: ``//! ordinary Cargo **integration** tests (files under `tests/`), not to a library's own``
  … ``//! `tests/background_spawn_detached_integration.rs`, mirroring this crate's own established``
  ``//! convention for the identical constraint (`tests/exec_run_sync_integration.rs`'s module``
* after: keep the Cargo fact but name where such files live now —
  ``//! ordinary Cargo **integration** tests (a `tests/` target of some crate — for this code that is``
  ``//! `crates/cyrup-it`, since this crate has no `tests/` directory), not to a library's own``
  … ``//! `crates/cyrup-it/tests/subagents/background_spawn_detached_integration.rs`, mirroring the``
  ``//! convention for the identical constraint (that suite's``
  ``//! `exec_run_sync_integration.rs` module``

**[`src/background/runner_main.rs:3849-3860`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs)**
— same treatment: `:3852`'s "(files under `tests/`)" and `:3858`'s "a `tests/*.rs` file is its own
separate compilation unit" are true statements about Cargo that must stop implying *this* crate has
such files. Name `crates/cyrup-it/tests/subagents/` once and refer to it.

**[`src/extension/testsupport.rs:12`](../../crates/cyrup-ext-subagents/src/extension/testsupport.rs)**

* before: ``// file's module doc for the full rationale (matches every other `tests/*_integration.rs` file's``
  ``// identical env-mutation convention in this crate).``
* after:  ``// file's module doc for the full rationale (matches the env-mutation convention of every``
  ``// file in `crates/cyrup-it/tests/subagents/` — see that suite's `main.rs:35-40` env caveat).``

### 6.4 Other crates' relocated tests (3 sites)

| File:line | Before | After |
|---|---|---|
| [`src/exec/spawn_plan.rs:39`](../../crates/cyrup-ext-subagents/src/exec/spawn_plan.rs) | ``(`cyrup-permission-system/tests/forwarding_spawn_env.rs` drives a real child process off THIS`` | ``(`crates/cyrup-it/tests/permission/forwarding_spawn_env.rs` drives a real child process off THIS`` |
| [`src/exec/spawn_plan.rs:3265`](../../crates/cyrup-ext-subagents/src/exec/spawn_plan.rs) | ``/// `cyrup-permission-system/tests/forwarding_spawn_env.rs`; this test is the other half of that`` | ``/// `crates/cyrup-it/tests/permission/forwarding_spawn_env.rs`; this test is the other half of that`` |
| [`src/tui/render.rs:666`](../../crates/cyrup-ext-subagents/src/tui/render.rs) | ``// Mirrors crates/cyrup-tui/tests/assembled_render.rs's whole-buffer text-grid pattern, scoped`` | ``// Mirrors crates/cyrup-tui/src/tests/assembled_render.rs's whole-buffer text-grid pattern, scoped`` |

[`src/discovery/skills.rs:90`](../../crates/cyrup-ext-subagents/src/discovery/skills.rs):

* before: ``/// (mirroring `cyrup-resources`' own `tests/resources.rs`, which roots `global` under a temp dir).``
* after:  ``/// (mirroring `cyrup-resources`' own `src/tests/resources/`, which roots `global` under a temp dir).``

[`src/discovery/skills.rs:757-758`](../../crates/cyrup-ext-subagents/src/discovery/skills.rs):

* before: ``/// test's assertions never see the developer's real `~/.cyrup/skills` (mirrors``
  ``/// `cyrup-resources` `tests/resources.rs`'s `root/global` isolation).``
* after:  ``/// test's assertions never see the developer's real `~/.cyrup/skills` (mirrors``
  ``/// `cyrup-resources`' `src/tests/resources/skills.rs:129-131` `root/global` isolation).``

### 6.5 `src/lib.rs:51`

* before: ``/// Crate-internal test modules relocated out of `tests/` (see [`tests`]'s own module doc):``
* after:  ``/// Crate-internal test modules relocated out of the crate's since-deleted `tests/` directory``
  ``/// (see [`tests`]'s own module doc, which states where a NEW test goes):``

### 6.6 `Cargo.toml`

**(a) `test-fixtures` feature, [15-21](../../crates/cyrup-ext-subagents/Cargo.toml)** — before:

```toml
# Builds the scripted-NDJSON test-double binary (`cyrup-subagent-fixture`, arch-SA §11) used by
# this crate's own integration tests and by later phases' spawn-boundary/model-fallback/
# background-job tests. Never enabled for the real `cyrup` binary or any non-test build — the
# `[[bin]]` target below is gated on `required-features` so it is not even compiled, let alone
# shipped, unless a caller (this crate's own dev-dependency closure, via `cargo test`) explicitly
# opts in.
```

after:

```toml
# Builds the two scripted test-double binaries below (`cyrup-subagent-fixture`,
# `cyrup-subagent-orchestrator-sim`; arch-SA §11). This crate CANNOT enable it for itself — a
# package's own feature is not reachable from its dev-dependency closure, and there is no
# self-dev-dependency here — so `cargo test -p cyrup-ext-subagents` and `cargo check -p
# cyrup-ext-subagents --all-targets` never build `src/bin/` at all. The two callers that do:
#   * `crates/cyrup-it/build.rs:67-77` — its `BINS` table runs
#     `cargo build -p cyrup-ext-subagents --features test-fixtures` and hands the paths to the
#     seam suite as `CYRUP_IT_BIN_*` env. Armed only under cyrup-it's `it` feature.
#   * `xtask/src/features.rs:145-151` — `cargo check -p cyrup-ext-subagents --features
#     test-fixtures --all-targets`, the row that type-checks these bins
#     (`cargo run -p xtask -- feature-matrix`).
# Never enabled for the real `cyrup` binary or any non-test build: both `[[bin]]` targets below
# carry `required-features`, so they are not compiled, let alone shipped, without an explicit opt-in.
```

**(b) `[[bin]] cyrup-subagent-fixture`, [107-113](../../crates/cyrup-ext-subagents/Cargo.toml)** —
replace ``# target (R-SA-045 tier 1) in this crate's own integration tests and later phases'
spawn-boundary/`` … with a pointer to the real driver:
``# target (R-SA-045 tier 1) by the seam tests in `crates/cyrup-it/tests/subagents/`, which reach it
# through `CYRUP_IT_BIN_CYRUP_SUBAGENT_FIXTURE` (built by `crates/cyrup-it/build.rs:67-77`) —``.

**(c) `[[bin]] cyrup-subagent-orchestrator-sim`, [119-123](../../crates/cyrup-ext-subagents/Cargo.toml)** —
replace ``# separate, killable OS process so an integration test can kill it mid-background-run`` with
``# separate, killable OS process so `crates/cyrup-it/tests/subagents/background_spawn_detached_integration.rs`
# can kill it mid-background-run``.

**(d) `cyrup-test-support` dev-dep, [156-158](../../crates/cyrup-ext-subagents/Cargo.toml)** —
`crates/cyrup-tui/tests/assembled_render.rs` -> `crates/cyrup-tui/src/tests/assembled_render.rs`.
*(This site is new relative to the original Evidence.)*

**(e) `cyrup-provider` rationale, [51-58](../../crates/cyrup-ext-subagents/Cargo.toml)** — before:

```toml
# `/subagents-models` and `/subagents-refresh-provider-models` (R-SA-129/130 gap closure): the
# only genuine, already-built provider/model-catalog surface in the workspace today is this
# crate's static, embedded seed catalog (`catalog::seed_catalog`/`load_catalog`) — full
# models.dev live-probe generation/refresh is explicitly DEFERRED (func-SA §9 item 31, arch-SA
# §12 item 11), so this dependency is used ONLY for that already-real static-catalog read, never
# to invent the deferred live-probe algorithm. `cyrup-ext` (already a direct dependency above)
# itself depends on `cyrup-provider`, so this adds no new layering cycle.
```

after:

```toml
# The static, embedded model registry: `cyrup_provider::catalog::builtin_catalog()`
# (`crates/cyrup-provider/src/catalog.rs:40`). Full models.dev live-probe generation/refresh is
# explicitly DEFERRED (func-SA §9 item 31, arch-SA §12 item 11), so this dependency is used only
# for the already-real static read, never to invent the deferred live-probe algorithm.
# Three production call sites, and they are NOT all slash-command surface:
#   * `src/extension/models/mod.rs:59` — `registry_models()`, behind `/subagents-models` and
#     `/subagents-refresh-provider-models` (R-SA-129/130).
#   * `src/extension/executor/reports.rs:549` — the doctor/report registry read.
#   * `src/watchdog/model_selection.rs:165,172` — model-SELECTION policy, i.e. this edge carries
#     behaviour, not just a display list. Do not narrow this comment back to "two slash commands".
# `cyrup-ext` (already a direct dependency above) itself depends on `cyrup-provider`, so this adds
# no new layering cycle.
```

**(f) `cyrup-session-svc` dev-dep, [162-170](../../crates/cyrup-ext-subagents/Cargo.toml)** — delete
the whole block including the `cyrup-session-svc = { workspace = true }` line. Move its surviving
sentence into [`src/extension/mod.rs`](../../crates/cyrup-ext-subagents/src/extension/mod.rs)'s `//!`
doc, next to the existing seam rule at `:20-23`, as:

```rust
//! Crate-boundary rule this module tree also holds: `cyrup-ext-subagents` never depends on the
//! session-service facade (`cyrup-session-svc`) in production code. `src/exec/ndjson.rs:31` states
//! the wire-schema half of the same rule.
```

## 7. Out of scope — record, do not fix here

Three adjacent citations drifted in the same reorganisation. They are in *other* crates and would
widen this task; log them rather than fixing them.

* [`crates/cyrup-it/build.rs:44`](../../crates/cyrup-it/build.rs) cites
  `crates/cyrup-ext-subagents/Cargo.toml:102,112` for the two `[[bin]]` gates — actually 106-116 and
  118-126.
* [`xtask/src/features.rs:148`](../../xtask/src/features.rs) cites `Cargo.toml:92-112` for the same
  two bins.
* Several `crates/cyrup-it/tests/subagents/*.rs` module docs still say "Gated on the `test-fixtures`
  Cargo feature" (e.g. [`extension_end_to_end_smoke.rs:33`](../../crates/cyrup-it/tests/subagents/extension_end_to_end_smoke.rs),
  [`registration_commands_integration.rs:12`](../../crates/cyrup-it/tests/subagents/registration_commands_integration.rs)),
  which [`main.rs:22-31`](../../crates/cyrup-it/tests/subagents/main.rs) says was removed.
  [`extension_end_to_end_smoke.rs:88`](../../crates/cyrup-it/tests/subagents/extension_end_to_end_smoke.rs)
  also calls its siblings "this crate's own integration tests".

Five more cross-crate `file:line` citations in this crate's prose point at modules that were
themselves split up in the same reorganisation — a *different* drift from the test-layout one, and
deliberately not in scope: `crates/cyrup-agent/src/agent.rs`, `crates/cyrup-core/src/message.rs`,
`crates/cyrup-resources/src/discovery.rs`, `crates/cyrup-session/src/manager.rs`,
`crates/cyrup/src/cli.rs`. Each is now a directory module or renamed file. Worth its own task.

Also rejected (see §5): moving `watchdog/model_selection.rs`'s catalog access behind
`extension/models`.

## Definition of Done

Run from `crates/cyrup-ext-subagents/`.

1. **No pointer to a non-existent `tests/` directory.** Returns nothing:
   ```sh
   grep -rnoE '[A-Za-z0-9_./-]*tests/[A-Za-z0-9_./-]*' src/ Cargo.toml \
     | grep -vE 'cyrup-it/tests/|cyrup-tui/src/tests/|cyrup-resources/src/tests/|src/tests/' \
     | grep -vE 'tests/(foo|file|regression)\.rs'
   ```
2. **Every path this task rewrites exists.** Scoped to test-layout citations — returns nothing:
   ```sh
   grep -rhoE 'crates/[A-Za-z0-9_./-]*tests?/[A-Za-z0-9_./-]+\.rs' src/ Cargo.toml | sort -u \
     | while read -r p; do [ -e "../../$p" ] || echo "MISSING: $p"; done
   ```
   Today this prints `crates/cyrup-tui/tests/assembled_render.rs` (twice-cited: `src/tui/render.rs:666`
   and `Cargo.toml:157`). **Do not widen this to all `crates/**.rs` citations** — that form surfaces
   five further drifted paths in unrelated crates (see §7), none of them this task's business.
3. **The rule file states the rule in force.**
   `grep -c 'Where a new test goes' src/tests/mod.rs` = 1, and
   `grep -n 'stay in `tests/`' src/tests/mod.rs` returns nothing.
4. **`cyrup-session-svc` is gone from the manifest and its sentence survives.**
   `grep -c cyrup-session-svc Cargo.toml` = 0, and
   `grep -c 'session-service facade' src/extension/mod.rs` >= 1.
5. **The `test-fixtures` prose names its real consumers.**
   `grep -c "this crate's own integration tests" Cargo.toml` = 0;
   `grep -c 'cyrup-it/build.rs' Cargo.toml` >= 1; `grep -c 'xtask' Cargo.toml` >= 1.
6. **`seed_catalog` is gone and the real consumers are named.**
   `grep -c seed_catalog Cargo.toml` = 0;
   `grep -c 'builtin_catalog' Cargo.toml` >= 1;
   `grep -c 'model_selection.rs' Cargo.toml` >= 1.
7. **Nothing broke.** `cargo check -p cyrup-ext-subagents --all-targets` and
   `cargo test -p cyrup-ext-subagents --no-run` both succeed;
   `cargo check -p cyrup-ext-subagents --features test-fixtures --all-targets` still succeeds
   (proves the bins still exist for `cyrup-it`'s `build.rs`, which fails by name if a target
   disappears).
8. **No test, benchmark or new document was added.** `git diff --stat` touches only
   `crates/cyrup-ext-subagents/Cargo.toml` and files under `crates/cyrup-ext-subagents/src/`, and no
   file is created or deleted.

## Source

- Identified by the `subagents-hygiene-survey` workflow (13 agents, 21 raw findings, 16 confirmed
  after adversarial verification).
- Effort: small · survey priority: 6 of 6
- Re-verified 2026-08-27 against `df64e81` (crate identical to `origin/main` `d2c5b1e`): 0 of the
  named sites fixed, 22 source pointers + 6 manifest blocks still stale, 1 new stale site found,
  2 Evidence claims corrected, all line numbers restated.
