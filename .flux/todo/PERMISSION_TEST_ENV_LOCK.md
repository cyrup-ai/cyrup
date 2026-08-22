---
stage: new
status: done
updated: 2026-08-22 18:40
---

# Serialize the Six Permission Tests That Race on the Config-Path Env Var

## Description

`cargo test -p cyrup-permission-system --lib` fails intermittently — measured at 2/20 runs on the
pre-decomposition file and 1/20 to 1/6 after, varying with machine load. It is **not** caused by
the `extension.rs` decomposition; that was ruled out by restoring the original single file and
re-running it 20 times.

Root cause: six tests construct a `PermissionSystemExtension` — which resolves
`CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` on every `ExtensionConfig::load` — **without holding**
`crate::ext_config::env_lock`, while `tests/enabled_switch.rs`'s `with_config_path_override` sets
that variable process-wide. When they interleave, one test writes its pristine config template into
the other's override path. This is exactly the hazard `src/tests/mod.rs` documents.

The six, all under `crates/cyrup-permission-system/src/extension/tests/`:

- `gate.rs::registry_gate_fails_closed_with_no_attached_registry`
- `gate.rs::ask_fails_fast_without_ui_subagent_or_yolo`
- `config_reload.rs::session_start_rebuilds_manager_from_current_session_cwd`
- `watcher.rs::parent_role_publishes_and_clears_the_process_parent_session_anchor`
- `watcher.rs::a_subagent_child_never_publishes_or_clears_the_parent_session_anchor`
- `watcher.rs::a_fresh_extension_holds_no_watcher_config_handles`

Observed symptoms: `enabled_switch::the_install_probe_reads_the_same_resolved_config_as_the_enabled_switch`
("must read the OVERRIDE file"), `install::auto_materialized_config_does_not_latch_the_gate_on`
("must still materialize the editable config template"), and
`gate::ask_fails_fast_without_ui_subagent_or_yolo` (expected `Block`).

Route each through `crate::ext_config::with_env_lock` — the crate's existing helper, spelled locally
as `support::with_config_env_lock`. That means restructuring each `#[tokio::test] async fn` into the
`#[test] fn` + sync-lock + `block_on(body())` shape the rest of the module already uses, so the
guard is never held across an `.await`.

## Acceptance Criteria

- [ ] All six hold the env lock for the whole body, via `with_config_env_lock`
- [ ] The lock is never held across an `.await` (sync frame around `block_on`, per
      `ext_config::with_env_lock`'s doc)
- [ ] `cargo test -p cyrup-permission-system --lib` passes 30 consecutive runs
- [ ] No assertion is weakened, skipped, or deleted
