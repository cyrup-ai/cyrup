---
stage: exec
status: done
updated: 2026-08-22 17:53
---

# Fix All Errors In `cyrup-it --features it`

## Description

`cargo check -p cyrup-it --features it --all-targets` does not compile. The failure is **one
family only**: struct literals in the integration-test crate that have fallen behind fields added
to the types they construct. There is no enum drift, no signature drift, no renamed type, and no
production bug. This is pure test-side catch-up.

### Why it went unnoticed

Every `cyrup-it` `[[test]]` target is `required-features = ["it"]`
([Cargo.toml](../../crates/cyrup-it/Cargo.toml)), and `it` is OFF by default — deliberately, so the
merge gate (`cargo test --workspace`) does not build, link or run any seam test.
`cargo check --workspace --all-targets` therefore reports clean while these targets have drifted
into not building at all. The CI half of that hole belongs to
[BUILD_FEATURE_COMBINATIONS.md](./BUILD_FEATURE_COMBINATIONS.md), not here.

### Corrections to the original report

The original write-up was based on an 8-line truncated `cargo check` dump. Three things in it are
wrong and must not be carried forward:

1. **The breakage is NOT confined to the `subagents` target.**
   [`tests/intercom/child_bridge_activation.rs:131`](../../crates/cyrup-it/tests/intercom/child_bridge_activation.rs)
   is a broken `RunOptions` literal, and that file is a module of the **`intercom`** target
   ([`tests/intercom/main.rs:53`](../../crates/cyrup-it/tests/intercom/main.rs)). Two of the eight
   targets are red: **`subagents`** (33 sites) and **`intercom`** (1 site).
2. **A third type is affected, not two.** `BackgroundStepsSpec` is also missing `usage_budget` at
   [`tests/subagents/subagent_persona_and_depth_integration.rs:1020`](../../crates/cyrup-it/tests/subagents/subagent_persona_and_depth_integration.rs).
3. **`..Default::default()` is not a hazard here — it is a second compile error.** None of
   `RunnerConfig` ([runner_main.rs:111](../../crates/cyrup-ext-subagents/src/background/runner_main.rs)),
   `RunOptions` ([exec/mod.rs:514](../../crates/cyrup-ext-subagents/src/exec/mod.rs)) or
   `BackgroundStepsSpec` ([extension.rs:637](../../crates/cyrup-ext-subagents/src/extension.rs))
   derives or implements `Default`. Functional-update syntax will not compile against any of them.
   Every field must be written out.

The `mcp`, `bin`, `ext`, `session_svc`, `misc` and `permission` targets are unaffected.
[`tests/permission/forwarding_spawn_env.rs:162`](../../crates/cyrup-it/tests/permission/forwarding_spawn_env.rs)
was **already updated** for all three fields and is the canonical precedent this task copies.

---

## The three drifted types

### `RunnerConfig` — 29 fields

[`crates/cyrup-ext-subagents/src/background/runner_main.rs:113`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs)

`run_id`:118 · `mode`:120 · `steps`:128 · `cwd`:130 · `session_file`:134 · `session_id`:149 ·
`global_concurrency_limit`:153 · `worktree_base_dir`:156 · `max_subagent_depth`:161 ·
`async_root`:172 · `results_dir`:183 · `resolved_agents`:203 · `original_task`:211 · `chain_dir`:218 ·
`orchestrator_intercom_target`:228 · `inherited_session_model`:239 · `turn_budget`:252 ·
**`usage_budget`:262** · `model_scope`:278 · `nested_route`:287 · `nested_self`:292 ·
`dynamic_fanout_max_items`:301 · `control`:320 · `include_progress`:335 · `timeout_ms`:350 ·
`deadline_at_ms`:365 · `share`:376 · `artifacts_dir`:387 · `artifact_config`:397

Derives at [runner_main.rs:111](../../crates/cyrup-ext-subagents/src/background/runner_main.rs):
`Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize`. **No `Default`.**

### `RunOptions` — 36 fields

[`crates/cyrup-ext-subagents/src/exec/mod.rs:515`](../../crates/cyrup-ext-subagents/src/exec/mod.rs)

`cwd`:516 · `deadline_at`:520 · `timeout_ms`:529 · `output_path`:530 · `output_mode`:531 ·
`reads`:545 · `structured_output_schema`:546 · `model_override`:550 · `preferred_provider`:551 ·
`available_models`:552 · `cancel`:554 · `interrupt`:560 · `share`:567 · `session_dir`:572 ·
`skills`:577 · `runtime_cwd`:582 · `include_progress`:596 · `agent_scope`:597 · `model_scope`:606 ·
`acceptance`:609 · `fork_context`:613 · `live_events`:619 · `parent_session_id`:628 · `clarify`:638 ·
`orchestrator_intercom_target`:649 · `run_id`:654 · `child_index`:659 · `steer_inbox_dir`:674 ·
**`steer_ack_dir`:687** · **`steer_capability_path`:699** · `control_config`:708 ·
`artifacts_dir`:719 · `on_control_event`:724 · `turn_budget`:733 · `enforce_hard_turn_limit`:739 ·
**`usage_budget`:747**

Derives at [exec/mod.rs:514](../../crates/cyrup-ext-subagents/src/exec/mod.rs): `Debug, Clone`.
**No `Default`.**

### `BackgroundStepsSpec` — 15 fields

[`crates/cyrup-ext-subagents/src/extension.rs:637`](../../crates/cyrup-ext-subagents/src/extension.rs)

`steps`:639 · `mode`:641 · `session_file`:644 · `resolved_agents`:647 · `original_task`:649 ·
`chain_dir`:652 · `control`:664 · `include_progress`:669 · `run_id`:684 · `timeout_ms`:687 ·
`share`:690 · `artifacts_dir`:693 · `artifact_config`:696 · `turn_budget`:704 ·
**`usage_budget`:709**

No derive attribute at all. **No `Default`.**

---

## Complete error enumeration

Enumerated statically by parsing every struct literal of these three types under
`crates/cyrup-it/tests/` and diffing its field set against the definition. **35 construction sites
exist; 34 need an edit; 56 field insertions in total.** Three distinct field names are missing,
across five `(type, field)` pairs.

| type | missing field | sites |
|---|---|---|
| `RunnerConfig` | `usage_budget` | 22 |
| `RunOptions` | `usage_budget` | 11 |
| `RunOptions` | `steer_ack_dir` | 11 |
| `RunOptions` | `steer_capability_path` | 11 |
| `BackgroundStepsSpec` | `usage_budget` | 1 |

### A. `RunnerConfig` sites — 22, each missing only `usage_budget`

All in the **`subagents`** target.

| file | literal at | anchor `turn_budget: None,` at | indent |
|---|---|---|---|
| [background_runner_main_integration.rs](../../crates/cyrup-it/tests/subagents/background_runner_main_integration.rs) | 220 | 221 | 8 |
| ″ | 327 | 328 | 8 |
| ″ | 470 | 471 | 8 |
| ″ | 590 | 591 | 8 |
| ″ | 711 | 712 | 8 |
| ″ | 839 | 840 | 8 |
| ″ | 984 | 985 | 8 |
| ″ | 1114 | 1115 | 8 |
| ″ | 1228 | 1229 | 8 |
| ″ | 1356 | 1357 | **12** |
| ″ | 1465 | 1466 | 8 |
| ″ | 1575 | 1576 | 8 |
| ″ | 1638 | 1639 | 8 |
| ″ | 1731 | 1732 | 8 |
| [background_spawn_detached_integration.rs](../../crates/cyrup-it/tests/subagents/background_spawn_detached_integration.rs) | 537 | 538 | 8 |
| ″ | 750 | 751 | 8 |
| [subagent_persona_and_depth_integration.rs](../../crates/cyrup-it/tests/subagents/subagent_persona_and_depth_integration.rs) | 179 | 180 | 8 |
| ″ | 355 | 356 | 8 |
| ″ | 639 | 640 | 8 |
| [acceptance_memo_key_and_live_wiring.rs](../../crates/cyrup-it/tests/subagents/acceptance_memo_key_and_live_wiring.rs) | 834 | 835 | 8 |
| [background_cascade_integration.rs](../../crates/cyrup-it/tests/subagents/background_cascade_integration.rs) | 203 | 204 | 8 |
| [run_state_signal_and_stop_parity.rs](../../crates/cyrup-it/tests/subagents/run_state_signal_and_stop_parity.rs) | 621 | 622 | 8 |

The original report listed 5 of these 22 and missed 17 — including 10 more in
`background_runner_main_integration.rs` alone.

### B. `RunOptions` sites — 11, each missing all three fields

| target | file | literal at | anchor `steer_inbox_dir: None,` at |
|---|---|---|---|
| **intercom** | [child_bridge_activation.rs](../../crates/cyrup-it/tests/intercom/child_bridge_activation.rs) | 131 | 138 |
| subagents | [acceptance_parser_state_model_interaction.rs](../../crates/cyrup-it/tests/subagents/acceptance_parser_state_model_interaction.rs) | 103 | 132 |
| subagents | [child_protocol_stream_integration.rs](../../crates/cyrup-it/tests/subagents/child_protocol_stream_integration.rs) | 126 | 158 |
| subagents | [child_stderr_drain_integration.rs](../../crates/cyrup-it/tests/subagents/child_stderr_drain_integration.rs) | 125 | 157 |
| subagents | [child_written_output_authorship.rs](../../crates/cyrup-it/tests/subagents/child_written_output_authorship.rs) | 110 | 139 |
| subagents | [companions_wiring_proof.rs](../../crates/cyrup-it/tests/subagents/companions_wiring_proof.rs) | 144 | 173 |
| subagents | [exec_run_sync_integration.rs](../../crates/cyrup-it/tests/subagents/exec_run_sync_integration.rs) | 98 | 127 |
| subagents | [read_only_acceptance_inference.rs](../../crates/cyrup-it/tests/subagents/read_only_acceptance_inference.rs) | 121 | 150 |
| subagents | [run_state_signal_and_stop_parity.rs](../../crates/cyrup-it/tests/subagents/run_state_signal_and_stop_parity.rs) | 155 | 184 |
| subagents | [startup_retry_lifecycle_integration.rs](../../crates/cyrup-it/tests/subagents/startup_retry_lifecycle_integration.rs) | 148 | 180 |
| subagents | [subagent_persona_and_depth_integration.rs](../../crates/cyrup-it/tests/subagents/subagent_persona_and_depth_integration.rs) | 421 | 450 |

Every one of these is the file's single `base_run_options(cwd, model)` / `run_options(...)` helper,
so one edit per file fixes every test in it. Field indent is 8 at all 11.

### C. `BackgroundStepsSpec` site — 1, missing `usage_budget`

[`tests/subagents/subagent_persona_and_depth_integration.rs:1020`](../../crates/cyrup-it/tests/subagents/subagent_persona_and_depth_integration.rs),
anchor `turn_budget: None,` at line 1021, indent **16**.

### D. Already correct — do not touch

[`tests/permission/forwarding_spawn_env.rs:162`](../../crates/cyrup-it/tests/permission/forwarding_spawn_env.rs)
supplies all 36 `RunOptions` fields, with the rationale comments at lines 164-181. Copy its shape.

### E. Other drift classes — checked, none found

* **Enum variants**: every `Type::Variant` path used anywhere under `crates/cyrup-it/tests/`
  resolves to a live variant. Zero mismatches.
* **Function signatures**: `run_sync` ([exec/mod.rs:3653](../../crates/cyrup-ext-subagents/src/exec/mod.rs)),
  `SubagentExecutor::spawn_background_steps` ([extension.rs:2700](../../crates/cyrup-ext-subagents/src/extension.rs))
  and `background::runner_main::run` ([runner_main.rs:513](../../crates/cyrup-ext-subagents/src/background/runner_main.rs))
  all still take the arity/shape the tests call them with.
* **Renamed / removed types**: every `use cyrup_*::…` leaf imported by the test crate resolves to a
  live `pub` item or a `str_id!`-generated newtype
  ([cyrup-core/src/lib.rs:85](../../crates/cyrup-core/src/lib.rs)).
* **Other struct literals**: a full literal-vs-definition diff over all 8 targets flagged only the
  35 sites above. `SingleStepSpec`, `AgentConfig`, `DepthEnvelope`, `ArtifactConfig` and the rest
  are all complete.

---

## The value each field takes — and why it is `None` everywhere

The original note warned that a blanket default could silently change what a test proves. That
concern is correct in principle and does not apply to these three fields, for reasons that are
per-field and load-bearing. **All 56 insertions are `None`.** The justification below is what makes
that a decision rather than a shrug — record it in the code comments, not just here.

### `usage_budget: None` (34 insertions)

* Type is `Option<UsageBudgetConfig>`
  ([exec/usage_budget.rs:45](../../crates/cyrup-ext-subagents/src/exec/usage_budget.rs)). `None`
  means **unbudgeted**, which is what every run that does not ask for a budget gets — upstream
  ships no default budget.
* Production agrees: every in-crate construction passes `None`
  (e.g. [extension.rs:3294](../../crates/cyrup-ext-subagents/src/extension.rs),
  [runner_main.rs:2302](../../crates/cyrup-ext-subagents/src/background/runner_main.rs),
  [exec/mod.rs:4658](../../crates/cyrup-ext-subagents/src/exec/mod.rs)).
* On `RunnerConfig` the field carries
  `#[serde(default, skip_serializing_if = "Option::is_none")]`
  ([runner_main.rs:261](../../crates/cyrup-ext-subagents/src/background/runner_main.rs)), so `None`
  makes the on-disk `runner-config.json` **byte-identical** to what these tests wrote before the
  field existed. Every test that reads that file back, or round-trips it through `PartialEq`, is
  unchanged.
* At runtime `None` short-circuits: [runner_main.rs:1290](../../crates/cyrup-ext-subagents/src/background/runner_main.rs)
  threads it to the step executor, [runner_main.rs:2537](../../crates/cyrup-ext-subagents/src/background/runner_main.rs)
  onto each step's `RunOptions`, and [exec/mod.rs:4418-4432](../../crates/cyrup-ext-subagents/src/exec/mod.rs)
  leaves `SingleResult::usage_budget` `None` and the error message untouched.
* No test under `crates/cyrup-it/tests/` asserts on usage budget at all (verified by grep — the only
  hits are the precedent's own comments). Nothing to preserve, nothing at risk.
* `exec_run_sync_integration.rs` **does** exercise the sibling `turn_budget`, by mutating
  `opts.turn_budget` after construction at lines 1524 and 1621. That pattern is the model: the base
  helper stays `None`, and a test that wants a budget sets it locally. Do not put a budget in any
  base helper.

### `steer_ack_dir: None` and `steer_capability_path: None` (11 insertions each)

* Both are `Option<PathBuf>` naming files under a **background run directory**
  (`<run_dir>/control/steer-acks/<flatIndex>/` and
  `<run_dir>/control/steer-capabilities/<flatIndex>.json`,
  [background/control.rs:1234](../../crates/cyrup-ext-subagents/src/background/control.rs)).
* Only the detached hop-2 runner ever mints them, from a run dir it owns:
  [runner_main.rs:2362](../../crates/cyrup-ext-subagents/src/background/runner_main.rs) and
  [runner_main.rs:2645-2648](../../crates/cyrup-ext-subagents/src/background/runner_main.rs).
  The **foreground** production path passes `None`
  ([extension.rs:2254-2255](../../crates/cyrup-ext-subagents/src/extension.rs),
  [exec/mod.rs:4668-4669](../../crates/cyrup-ext-subagents/src/exec/mod.rs)).
* All 11 sites are foreground `run_sync` fixtures with no run directory. `None` is not a fallback
  there; it is the only correct value.
* This is the one place where the wrong value would be observable, and the direction matters:
  `build_attempt_spawn_plan` writes `STEER_CAPABILITY_ENV` / `STEER_ACK_DIR_ENV` into the child's
  env overlay **only when the option is `Some` and non-empty**
  ([exec/mod.rs:2227-2250](../../crates/cyrup-ext-subagents/src/exec/mod.rs)), and
  [exec/mod.rs:3906-3909](../../crates/cyrup-ext-subagents/src/exec/mod.rs) pre-creates those
  directories on the same gate. A `Some(...)` here would add two env keys to every spawned child
  and create two directories — which is exactly what
  [`forwarding_spawn_env.rs:172-181`](../../crates/cyrup-it/tests/permission/forwarding_spawn_env.rs)
  spells out as the reason it chose `None`, since that file asserts the overlay is byte-identical
  to a real foreground child's.
* [`child_bridge_activation.rs`](../../crates/cyrup-it/tests/intercom/child_bridge_activation.rs)
  is the other env-overlay assertion in the suite; its existing comment at lines 136-138 already
  made the same argument for `steer_inbox_dir: None`. The two new fields follow it verbatim.

---

## Prescribed edits

Two mechanical rules cover all 34 sites. Apply per file, and edit **bottom-up within each file** so
earlier line numbers stay valid.

**Rule 1 — every `RunnerConfig`, `RunOptions` and `BackgroundStepsSpec` literal listed above.**
Insert immediately after the literal's existing `turn_budget: None,` line, at that line's own
indent:

```rust
// SUBA-021: pi's `usageBudget` is an OPTIONAL param — upstream has no default budget, so a
// call that does not ask for one runs unbudgeted. This fixture asks for none.
usage_budget: None,
```

`turn_budget: None,` is the first field of every one of these literals, so the anchor is
unambiguous. Indent is 8 spaces everywhere except
`background_runner_main_integration.rs:1357` (12) and
`subagent_persona_and_depth_integration.rs:1021` (16).

**Rule 2 — the 11 `RunOptions` literals only.** Insert immediately after the existing
`steer_inbox_dir: None,` line, at 8-space indent:

```rust
// SUBA-049: the RETURN half of G90's steer channel. Both paths exist only under a background
// run directory; a foreground fixture like this one has none. Load-bearing: `build_attempt_
// spawn_plan` gates both env keys on presence (exec/mod.rs:2227-2250), so `None` keeps the
// child's env overlay byte-identical to a real foreground child's.
steer_ack_dir: None,
steer_capability_path: None,
```

Nothing else changes. **No production source is touched**, and no test assertion, helper, module
list or `Cargo.toml` entry is edited.

---

## Definition of done

- [ ] All 56 field insertions applied across the 34 sites in the 15 files listed in sections A, B
      and C — no site left out, no site edited twice.
- [ ] Every inserted value is `None`; no `..Default::default()` anywhere (it cannot compile against
      these three types).
- [ ] `crates/cyrup-it/tests/permission/forwarding_spawn_env.rs` is unmodified.
- [ ] No file outside `crates/cyrup-it/tests/` is modified.
- [ ] `cargo check -p cyrup-it --features it --all-targets` compiles clean — all 8 targets, with the
      `subagents` and `intercom` targets specifically confirmed to build.
- [ ] `cargo clippy -p cyrup-it --features it --all-targets` is warning-free under the workspace
      lint table.
- [ ] The CI gate that would have caught this is left to
      [BUILD_FEATURE_COMBINATIONS.md](./BUILD_FEATURE_COMBINATIONS.md); nothing about CI
      configuration is changed by this task.
