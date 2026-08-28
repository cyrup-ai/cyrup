---
stage: qa
status: completed
updated: 2026-08-28 05:01
---

# Repair the cyrup-it Feature-Gated Integration Build — Rework

The build repair itself is complete and verified; everything below is what remains.

## Completed, not to be redone

`cargo test --no-run -p cyrup-it --features it` compiles (exit 0). 35 `permission_rules: None`
and 24 `runner: None` applied, zero deviations, each value confirmed correct against the field's
own documentation. `resolve_context` passes the documented `true`. Suite runs: 465 tests, 462
pass; the 3 failures are triaged and none is caused by the repair. Workspace check clean, clippy
clean, workspace suite 8133/8133 with 8 skipped. 18 files changed, none outside
`crates/cyrup-it/tests/`.

Do not revisit any of the above. Do not change any `None` value, and do not touch production code.

## Outstanding 1 — the explanatory comment is on the LAST occurrence, not the first

Each file was to carry one explanatory comment, so a reader meets the rationale where the pattern
first appears. The edit applied sites in descending line order (correct, so insertions do not
shift later line numbers) but assigned the comment on that same descending pass, so it landed on
the final occurrence in every multi-occurrence file. In `background_runner_main_integration.rs`
thirteen bare `permission_rules: None,` lines precede the explanation.

Move the comment from the last occurrence to the first in each case below. Line numbers are as of
this review and shift as you edit — locate by first/last occurrence, not by number.

| file | field | comment is on | must move to |
|---|---|---|---|
| `subagents/background_runner_main_integration.rs` | `permission_rules` | 1786 | 223 |
| `subagents/subagent_persona_and_depth_integration.rs` | `permission_rules` | 1050 | 182 |
| `subagents/subagent_persona_and_depth_integration.rs` | `runner` | 1148 | 167 |
| `subagents/run_state_signal_and_stop_parity.rs` | `permission_rules` | 635 | 158 |
| `subagents/run_state_signal_and_stop_parity.rs` | `runner` | 569 | 147 |
| `subagents/background_spawn_detached_integration.rs` | `permission_rules` | 757 | 540 |

Exactly one commented occurrence per (file, field) must remain — moved, not duplicated.

The comment texts stay as they are:

```
// SUBA-073: no policy — the pre-field behaviour
// SUBA-074: the native child, as before
```

Files with a single occurrence of a field are already correct and must not be touched:
`child_bridge_activation.rs`, `forwarding_spawn_env.rs`, `acceptance_memo_key_and_live_wiring.rs`,
`acceptance_parser_state_model_interaction.rs`, `background_cascade_integration.rs`,
`chain_step_child_detail_integration.rs`, `child_protocol_stream_integration.rs`,
`child_stderr_drain_integration.rs`, `child_written_output_authorship.rs`,
`companions_wiring_proof.rs`, `exec_run_sync_integration.rs`,
`read_only_acceptance_inference.rs`, `startup_retry_lifecycle_integration.rs`.

## Outstanding 2 — trailing-comment spacing

The six inserted comments use two spaces before `//`. The suite's only pre-existing trailing field
comment uses one:

```rust
completion_guard: Some(false), // isolate this test from R-SA-034's own separate gate
```

Reduce the six to a single space, matching that precedent and rustfmt's default.

## Definition of done for this rework

- Each of the six (file, field) pairs has its comment on the FIRST occurrence, and exactly one
  commented occurrence remains for that pair.
- All six inserted comments use one space before `//`.
- Counts unchanged: 35 `permission_rules: None`, 24 `runner: None`, zero non-`None` values.
- `cargo test --no-run -p cyrup-it --features it` still compiles.
- `cargo check --workspace --all-targets` still clean.
- No file outside `crates/cyrup-it/tests/` modified.

Comments only — no behavioural change, so re-running the full `it` suite is not required. The
compile check above is sufficient.

## Not in scope, file separately

- The zombie-blind liveness check in `background_spawn_detached_integration.rs`: `pid_is_alive`
  uses `kill -0`, which succeeds on an unreaped exited process, so the test fails under an init
  that does not reap orphans. The runner does exit and writes correct terminal files.
- The two pre-existing `session_svc` failures the repair unmasked:
  `event_tier_set_model_and_thinking_take_effect_on_next_turn` (thinking `Medium`, expected `Off`)
  and `wasm_guest_set_active_tools_restricts_the_live_agent` (3 tools, expected 1).
- The suite is not rustfmt-clean, repo-wide and pre-existing (12/12 sampled untouched files fail
  `rustfmt --check`). There is no fmt gate. Do not reformat files wholesale as part of this rework.
