---
stage: exec
status: done
updated: 2026-08-22 16:35
---

# Decompose modes.rs Test File Into Submodules

## Objective

[`crates/cyrup-modes/src/tests/modes.rs`](../../crates/cyrup-modes/src/tests/modes.rs) is the
largest Rust source file in the crate: **2,005 lines / 95 KB holding 36 tests**. It is one flat file
covering five unrelated concerns (PRINT mode, JSON mode, the RPC verb surface, the RPC extension-UI
transport, and the model registry). Split it into a `modes/` module directory whose submodules each
own one concern, with every shared fixture in a single `support` module.

This is a **pure move + dedup refactor**. No test is added, removed, renamed, re-scoped, or
`#[ignore]`d, and no assertion text changes. The suite that runs after must be byte-identical in
behaviour to the suite that runs now.

## Current state (verified)

| Fact | Value |
| --- | --- |
| File | `crates/cyrup-modes/src/tests/modes.rs`, 2,005 lines |
| Tests | 34 × `#[tokio::test]` + 2 × `#[test]` = **36** |
| Declared by | [`src/tests/mod.rs`](../../crates/cyrup-modes/src/tests/mod.rs) — `mod modes;` |
| Target | the crate **lib** target (`#[cfg(test)] mod tests;` in [`src/lib.rs`](../../crates/cyrup-modes/src/lib.rs)) — one test binary for the whole crate |
| Edition | 2024 (workspace `Cargo.toml:88`), rust-version 1.96 |
| Sibling style | flat files, each with a rich `//!` header — see [`src/tests/rpc_client.rs`](../../crates/cyrup-modes/src/tests/rpc_client.rs) |

### Three constraints that shape the split

1. **The file head carries a load-bearing inner attribute.** `modes.rs:6` is
   `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]`,
   and the workspace **denies all four** (root `Cargo.toml` `[workspace.lints.clippy]`). Lint levels
   propagate down the module tree, so this attribute goes at the top of `modes/mod.rs` **once** and
   covers every submodule file. Do not drop it and do not sprinkle per-test `#[allow]`s. (If a
   submodule still trips a denied lint, add the identical `#![allow(...)]` line to that file's head
   — never weaken the assertion to satisfy the lint.)
2. **`crate::rpc::extension_ui_effect_json` is `pub(crate)`** ([`src/rpc.rs:439`](../../crates/cyrup-modes/src/rpc.rs)),
   so the one test that calls it keeps working from any submodule with its existing fully-qualified path.
3. **Helpers cross module boundaries.** Everything that moves into `support` must be `pub(super)`
   (visible throughout the `modes` subtree, invisible outside it) — including the `Fixture` fields
   `cwd` and `agent_dir`, which tests read directly. Keep `_tmp` private; `fixture()` is its only
   constructor.

## Target layout

```
crates/cyrup-modes/src/tests/modes/
├── mod.rs                    ~35   suite doc + the #![allow] + `mod` declarations only
├── support.rs               ~165   Fixture, runtime builders, sink parsers, duplex transport
├── print_mode.rs            ~195   5 tests
├── json_mode.rs             ~160   3 tests
├── rpc_commands.rs          ~350   5 tests — the verb surface
├── rpc_errors.rs            ~150   6 tests — error responses + id correlation
├── rpc_command_parsing.rs    ~60   2 tests — SessionCommand serde, no runtime
├── rpc_bash.rs              ~330   5 tests
├── rpc_ui_dialogs.rs        ~290   4 tests — correlated request/response
├── rpc_ui_effects.rs        ~200   3 tests — fire-and-forget effects
├── rpc_extension_errors.rs   ~60   1 test
└── rpc_models.rs            ~140   2 tests
```

`src/tests/mod.rs` needs **no edit** — `mod modes;` resolves to `modes/mod.rs` unchanged.

## Test → module assignment (all 36, exhaustive)

Line numbers are the current `fn` line in `modes.rs`, for locating only.

### `print_mode.rs` — PRINT adapter: stdout/stderr routing and exit codes (R-11-005)
| line | test |
| --- | --- |
| 90 | `print_mode_emits_final_assistant_text` |
| 122 | `print_mode_prints_only_the_final_message_of_a_turn` |
| 156 | `print_mode_routes_a_failed_turn_to_stderr_and_suppresses_stdout` |
| 198 | `print_mode_exit_code_is_pis_zero_or_one_from_the_final_message` |
| 227 | `print_mode_aborted_turn_without_message_uses_the_request_reason_fallback` |

### `json_mode.rs` — JSONL event stream + session header (R-11-007)
| line | test |
| --- | --- |
| 337 | `json_mode_emits_ordered_event_stream` |
| 416 | `json_mode_writes_session_header_as_first_line` |
| 463 | `json_mode_writes_session_header_exactly_once_across_followups` |

Also takes the two helpers used **only** by these three: `message_role` (394) and `raw_lines` (404).
They stay private to this module — do not promote single-consumer helpers into `support`.

### `rpc_commands.rs` — the verb surface driven over a `Cursor` script
| line | test |
| --- | --- |
| 275 | `rpc_thinking_levels_blank_lines_and_omitted_optional_state_match_pi` |
| 491 | `rpc_mode_drives_prompt_and_answers_queries` |
| 569 | `rpc_fork_at_entry_branches_and_rebinds` |
| 800 | `rpc_extended_command_surface` |
| 1726 | `rpc_compact_refusal_is_an_error_response_with_pi_s_reason` |

`rpc_thinking_levels_...` currently sits **inside the PRINT-mode banner** at line 275 — it is an RPC
test misfiled under the wrong heading. Moving it here is the single relocation the split corrects;
its body is unchanged.

### `rpc_errors.rs` — failure envelopes and id correlation
| line | test |
| --- | --- |
| 608 | `rpc_unknown_command_echoes_id_on_failure` |
| 625 | `rpc_malformed_command_echoes_id_on_failure` |
| 653 | `rpc_parse_error_emits_parse_command_without_id` |
| 679 | `rpc_unknown_command_echoes_real_type_and_message` |
| 707 | `rpc_numeric_id_executes_and_echoes_number` |
| 727 | `rpc_unknown_command_is_a_failure_not_a_panic` |

### `rpc_command_parsing.rs` — `SessionCommand` wire-shape deserialization (no runtime, sync `#[test]`)
| line | test |
| --- | --- |
| 746 | `session_command_parses_streaming_behavior` |
| 766 | `session_command_parses_new_command_shapes` |

### `rpc_bash.rs` — `bash` / `abort_bash` / `user_bash`
| line | test |
| --- | --- |
| 909 | `rpc_bash_backend_failure_is_not_fabricated_into_a_success` |
| 1468 | `rpc_abort_bash_interrupts_a_running_bash_command` |
| 1822 | `rpc_bash_delivers_user_bash_to_an_extension` |
| 1868 | `rpc_bash_honors_a_user_bash_result_override` |
| 1932 | `rpc_bash_honors_a_partial_user_bash_result_override` |

Takes `RpcUserBashProbe` (1766) — a probe specific to this concern, so it stays here rather than in
`support`. `rpc_abort_bash_interrupts_a_running_bash_command` is a G1 command-loop-concurrency test
whose only observable is `bash`'s `cancelled:true`; note that in the module doc so a later reader
does not "correct" its placement.

### `rpc_ui_dialogs.rs` — correlated `extension_ui_request` / `extension_ui_response`
| line | test |
| --- | --- |
| 971 | `rpc_extension_ui_request_response_round_trips` |
| 1088 | `rpc_malformed_extension_ui_response_is_swallowed_not_answered` |
| 1318 | `rpc_extension_ui_request_times_out_to_the_default_when_client_never_responds` |
| 1385 | `rpc_abort_does_not_force_resolve_a_pending_dialog` |

### `rpc_ui_effects.rs` — fire-and-forget UI effects and what must never reach the wire
| line | test |
| --- | --- |
| 1136 | `rpc_fire_and_forget_ui_effects_reach_the_wire` |
| 1261 | `rpc_header_footer_and_tools_expanded_effects_never_reach_the_wire` |
| 1993 | `set_widget_carries_pis_three_fields_and_no_widget_blob` |

`set_widget_...` is a direct unit test of `crate::rpc::extension_ui_effect_json` with no runtime at
all; it belongs with the effect-shape tests, not with the transport tests.

### `rpc_extension_errors.rs` — contained extension faults on the wire
| line | test |
| --- | --- |
| 1516 | `rpc_contained_extension_fault_surfaces_as_extension_error_event` |

Its `FaultyInputExt` is already declared inside the fn body — leave it there.

### `rpc_models.rs` — SEAM-004 / G39, the auth-filtered registry
| line | test |
| --- | --- |
| 1589 | `rpc_model_commands_span_the_full_auth_filtered_registry` |
| 1670 | `rpc_cycle_model_spans_the_full_auth_filtered_registry` |

## `support.rs` — exact contents

Everything below moves verbatim from `modes.rs` (doc comments included) and gains `pub(super)`.
`AnyFauxResolver` (1580) **must** live here: `build_runtime` constructs one, so it is not
model-specific despite sitting under the SEAM-004 banner today.

```rust
//! Shared fixtures for the mode-adapter suite: a tempdir project + agent dir, the wired
//! `AgentSessionRuntime` builders every case drives, the JSONL sink readers the assertions parse
//! with, and the in-memory duplex transport that stands in for real stdio.

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_provider::faux::FauxProvider;
use cyrup_provider::Provider;
use cyrup_session_svc::{AgentSessionRuntime, SessionConfig, SessionFactory};
use serde_json::Value;
use tempfile::TempDir;

pub(super) struct Fixture {
    _tmp: TempDir,
    pub(super) cwd: PathBuf,
    pub(super) agent_dir: PathBuf,
}

pub(super) fn fixture() -> Fixture { /* verbatim from modes.rs:36-43 */ }
pub(super) fn base_config(fx: &Fixture) -> SessionConfig { /* :45-49 */ }

/// A [`cyrup_session_svc::ProviderResolver`] that hands back an offline faux provider for any id.
pub(super) struct AnyFauxResolver;              // :1580, with its impl block (:1582-1586)

pub(super) async fn build_runtime(fx: &Fixture, faux: Arc<FauxProvider>) -> Arc<AgentSessionRuntime>
{ /* :61-71, keep the whole doc comment */ }

pub(super) async fn build_runtime_with_ext(
    fx: &Fixture,
    faux: Arc<FauxProvider>,
    ext: Arc<dyn cyrup_ext::NativeExtension>,
) -> Arc<AgentSessionRuntime> { /* :1804-1819 */ }

pub(super) fn parse_lines(bytes: &[u8]) -> Vec<Value> { /* :73-79 */ }
pub(super) fn type_of(v: &Value) -> &str { /* :81-83 */ }

pub(super) async fn read_json_line<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> Value
{ /* :950-968 */ }
```

### Plus one new helper: `spawn_rpc_duplex`

Six tests (971, 1088, 1136, 1261, 1318, 1385) open the identical in-memory transport with the
identical 14-line block. Extract it — it is the difference between `rpc_ui_dialogs.rs` reading as
four dialog scenarios and reading as four copies of transport boilerplate:

```rust
/// The in-memory bidirectional transport that stands in for stdio: returns the client's write half,
/// a buffered reader over the server's output, and the join handle of the loop itself. Drop the
/// write half to signal EOF, then `await` the handle.
pub(super) fn spawn_rpc_duplex(
    runtime: Arc<AgentSessionRuntime>,
) -> (
    tokio::io::DuplexStream,
    tokio::io::BufReader<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<()>,
) {
    let (client_tx, server_rx) = tokio::io::duplex(64 * 1024);
    let (server_tx, client_rx) = tokio::io::duplex(64 * 1024);
    let handle = tokio::spawn(async move {
        let reader = tokio::io::BufReader::new(server_rx);
        let mut writer = server_tx;
        crate::run_rpc(&runtime, reader, &mut writer).await.expect("rpc mode runs");
    });
    (client_tx, tokio::io::BufReader::new(client_rx), handle)
}
```

Each call site collapses to:

```rust
let (mut client_tx, mut client_reader, rpc) = spawn_rpc_duplex(runtime);
```

Every one of the six already takes `let session = runtime.session().await;` (and
`host_services`) **before** the spawn, so moving the `Arc` into the helper is sound — the two that
need neither (1088) just pass the runtime straight in. `run_rpc` takes `&AgentSessionRuntime`
([`src/rpc.rs:663`](../../crates/cyrup-modes/src/rpc.rs)) and `&Arc<T>` deref-coerces, so the body is
unchanged from what the tests write inline today.

### And one dedup that removes a real copy-paste

`rpc_model_commands_span_the_full_auth_filtered_registry` (1596-1607) and
`rpc_cycle_model_spans_the_full_auth_filtered_registry` (1680-1687) each inline a runtime
construction that is **line-for-line identical to `build_runtime`'s body** — same `base_config`, same
`AnyFauxResolver`, same `AgentSessionRuntime::create(...).expect("build runtime")`. The only thing
those tests do differently is write `auth.json` into `fx.agent_dir` first, which happens *before*
construction and does not change it. Replace both blocks with:

```rust
let fx = fixture();
std::fs::write(
    fx.agent_dir.join("auth.json"),
    r#"{"anthropic":{"type":"api_key","key":"sk-test"}}"#,
)
.expect("write auth.json");
let runtime = build_runtime(&fx, Arc::new(FauxProvider::new())).await;
```

That drops `Provider`, `SessionFactory`, `AgentSessionRuntime` and `base_config` from
`rpc_models.rs`'s import list entirely.

## `mod.rs` — exact contents

```rust
//! Integration tests for the non-interactive adapters (arch-11 §2.2; func-11 R-11-005/007/011…016).
//!
//! Each test builds a real wired [`AgentSession`] over a scripted `FauxProvider` in a tempdir and
//! drives one adapter into an in-memory sink, then asserts on the produced bytes — exactly how the
//! binary will drive them over real stdio.
//!
//! Split by concern: [`print_mode`] and [`json_mode`] cover the two one-shot adapters; the `rpc_*`
//! modules cover the bidirectional protocol — its verb surface ([`rpc_commands`]), its failure
//! envelopes ([`rpc_errors`]), its request deserialization ([`rpc_command_parsing`]), its bash
//! surface ([`rpc_bash`]), its extension-UI transport ([`rpc_ui_dialogs`], [`rpc_ui_effects`]),
//! contained extension faults ([`rpc_extension_errors`]) and the model registry ([`rpc_models`]).
//! Every fixture they share lives in [`support`].
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod support;

mod json_mode;
mod print_mode;
mod rpc_bash;
mod rpc_command_parsing;
mod rpc_commands;
mod rpc_errors;
mod rpc_extension_errors;
mod rpc_models;
mod rpc_ui_dialogs;
mod rpc_ui_effects;
```

## Implementation steps

1. **Capture the baseline** — the parity proof for the whole change:
   ```bash
   cd /home/user/cyrup
   cargo test -p cyrup-modes --lib -- --list | sort > /tmp/modes-before.txt
   wc -l /tmp/modes-before.txt      # must include the 36 modes:: entries
   ```
2. **Preserve history**: `git mv crates/cyrup-modes/src/tests/modes.rs crates/cyrup-modes/src/tests/modes/mod.rs`
   so the largest surviving chunk keeps its blame, then extract outward from that file.
3. **Create `support.rs` first**, move the helpers listed above into it, add `pub(super)`, and check
   it compiles against a still-intact `mod.rs` (`cargo check -p cyrup-modes --tests`).
4. **Move one concern per commit-sized step**, in this order — cheapest first, so a mistake surfaces
   on a small module: `rpc_command_parsing` → `print_mode` → `json_mode` → `rpc_errors` →
   `rpc_extension_errors` → `rpc_models` → `rpc_ui_effects` → `rpc_ui_dialogs` → `rpc_bash` →
   `rpc_commands`. After each move `mod.rs` should shrink and `cargo check -p cyrup-modes --tests`
   should stay clean. When `mod.rs` holds only the doc + attribute + `mod` lines, the move is done.
5. **Reconstruct each module's `use` block** from the original top-of-file block — take only what
   that module references. Leave every **function-local** `use` (e.g. `use cyrup_ext::host::{...};`
   inside the UI tests) exactly where it is: keeping those untouched is what makes the diff a
   verifiable pure move.
6. **Apply the two dedups** (`spawn_rpc_duplex`, `build_runtime` in the model tests) as the **last**
   step, separate from the move, so a reviewer can diff the mechanical part independently.
7. **Prove parity**:
   ```bash
   cargo test -p cyrup-modes --lib -- --list | sort > /tmp/modes-after.txt
   diff <(sed 's/^tests::modes::[a-z_]*:://' /tmp/modes-before.txt | sort) \
        <(sed 's/^tests::modes::[a-z_]*:://' /tmp/modes-after.txt  | sort)
   cargo test -p cyrup-modes
   cargo clippy -p cyrup-modes --all-targets -- -D warnings
   ```
   The name diff must be empty modulo the module path prefix, and the count must be exactly 36.

## Definition of done

- `crates/cyrup-modes/src/tests/modes.rs` no longer exists; `modes/mod.rs` + 11 submodules do.
- `cargo test -p cyrup-modes --lib -- --list` reports the same **36** test names as before the
  split, differing only in their `tests::modes::<submodule>::` path prefix.
- `cargo test -p cyrup-modes` is green (or, if a case is red on `main` too, red for exactly the same
  reason and no other — record that in the QA note rather than "fixing" it inside this refactor).
- `cargo clippy -p cyrup-modes --all-targets -- -D warnings` is clean: no new `#[allow]` anywhere
  except the single `#![allow(...)]` line at the head of `modes/mod.rs`.
- No file has a body of more than ~350 lines; `mod.rs` is declarations only.
- `git diff --stat` touches nothing outside `crates/cyrup-modes/src/tests/`.

## Out of scope

- Any change to `src/rpc.rs`, `src/print.rs`, `src/json.rs`, `src/rpc_client.rs` or any other
  non-test source. `src/rpc.rs` is the second-largest file in the crate (1,736 lines) — decomposing
  it is a separate task, and touching it here would make this diff unreviewable.
- The other five test files under `src/tests/` — they are 300-600 lines each and already
  single-concern.
- Rewording assertion messages, tightening `Pi`-citation comments, or "improving" any test body.
  The comments in this file cite upstream `pi` source lines and encode why each assertion exists;
  they move verbatim.
- No third-party source is needed in `./tmp` for this task — it is entirely in-repo.
