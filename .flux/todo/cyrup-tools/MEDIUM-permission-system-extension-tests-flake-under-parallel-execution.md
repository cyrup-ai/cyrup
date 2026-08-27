---
stage: exec
status: done
priority: MEDIUM
tool: all
source: exec follow-up — observed independently by two agents, then reproduced
updated: 2026-08-27 21:05
---

# `cyrup-permission-system` extension tests flake under default parallelism

## Reproduction

`cargo test -p cyrup-permission-system --lib` fails intermittently. Measured on
this branch: **1 failure in 3 consecutive runs** at default parallelism, and
195/195 clean with `--test-threads=1`.

Two named tests carry it:
- `extension::tests::enabled_switch::the_install_probe_reads_the_same_resolved_config_as_the_enabled_switch`
- `extension::tests::install::auto_materialized_config_does_not_latch_the_gate_on`

This is **pre-existing** and unrelated to any task in this run. It was observed
independently by the powershell executor and the permission-manager executor —
each proved it was not their change — and then reproduced directly.

---

## Mechanism — corrected by the census below

The original framing ("`without_install_env` clears `CYRUP_PERMISSION_SYSTEM` and
an unlocked reader sees it gone") is *half* the defect, and it is the smaller
half. The census in the next section establishes three facts that change the fix:

1. **`INSTALL_ENV_VAR` has exactly ONE writer in the whole crate**
   ([`support.rs:74-88`](../../../crates/cyrup-permission-system/src/extension/tests/support.rs) —
   `without_install_env`), and it holds `env_lock`. So no *locked* test can be
   corrupted through that variable by another test in the crate.
2. **The variable that actually drives the flake is `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH`**
   ([`ext_config.rs:31`](../../../crates/cyrup-permission-system/src/ext_config.rs),
   `CONFIG_PATH_ENV_KEY`). It has three locked writers and **fifteen unlocked
   accessors**, several of which do not merely *read* it — they **write files
   through it**:
   - [`ExtensionConfig::load_with_result` → `ensure_on_disk`](../../../crates/cyrup-permission-system/src/ext_config.rs)
     (`ext_config.rs:168-175`, `:229-250`) **creates a file at the resolved path**;
   - [`ExtensionConfig::save`](../../../crates/cyrup-permission-system/src/ext_config.rs)
     (`ext_config.rs:434-435`) resolves the override *first* and **writes the
     merged document at the resolved path**.

   So an unlocked test that constructs an extension, or toggles a row in the
   config modal, reaches out of its own tempdir and writes into whatever file a
   *locked* sibling is currently pointing `CONFIG_PATH_ENV_KEY` at. That is a
   destructive cross-test write, not a stale read, and it is the direction that
   reaches
   `the_install_probe_reads_the_same_resolved_config_as_the_enabled_switch`,
   which is the one test in the crate that holds that override across a
   multi-step assertion sequence
   ([`enabled_switch.rs:137-152`, `:167-221`](../../../crates/cyrup-permission-system/src/extension/tests/enabled_switch.rs)).
3. **This is undefined behaviour, not merely a logical race.** The workspace is
   `edition = "2024"` ([`Cargo.toml:100`](../../../Cargo.toml), `rust-version = "1.96"`),
   which is exactly why `std::env::set_var` / `remove_var` are `unsafe` there.
   On glibc, `setenv`/`unsetenv` may reallocate and free the `environ` array
   while another thread is inside `getenv`. A mutex held by the *writer* does
   nothing about a reader that never takes it: the reader can observe freed
   memory, so the symptom is **nondeterministic and not attributable to a clean
   happens-before story**. Chasing "which assertion did it break" is therefore
   the wrong question — the fix must delete the mutation, not schedule it.

   The crate's own prose already says this in three places
   ([`env.rs:101-111`](../../../crates/cyrup-permission-system/src/extension/env.rs),
   [`ask.rs:475-485`](../../../crates/cyrup-permission-system/src/ask.rs),
   [`tests/mod.rs:11-27`](../../../crates/cyrup-permission-system/src/tests/mod.rs))
   and then does it anyway.

The `SAFETY` comment at
[`support.rs:77`](../../../crates/cyrup-permission-system/src/extension/tests/support.rs)
("serialized by `env_lock`") is therefore wrong twice over: the invariant holds
only among lock participants, and even a full-participation lock would not make
the *file* writes that hang off the resolved path safe.

---

## Census — every env accessor in `cyrup-permission-system`, with lock status

`env_lock` is
[`ext_config.rs:768-771`](../../../crates/cyrup-permission-system/src/ext_config.rs)
(`env_lock`), reached through
[`with_env_lock`](../../../crates/cyrup-permission-system/src/ext_config.rs)
(`ext_config.rs:795-798`),
[`with_config_env_lock`](../../../crates/cyrup-permission-system/src/extension/tests/support.rs)
(`support.rs:59-61`),
[`without_install_env`](../../../crates/cyrup-permission-system/src/extension/tests/support.rs)
(`support.rs:74-88`) and
[`trail_lock`](../../../crates/cyrup-permission-system/src/logging.rs)
(`logging.rs:418-420`).

### A. Production readers (all unlocked by construction — correct; production is single-threaded here)

| symbol | file:line | var(s) read |
|---|---|---|
| `env_truthy` | [`extension/env.rs:118-123`](../../../crates/cyrup-permission-system/src/extension/env.rs) | any (called with `INSTALL_ENV_VAR`) |
| `is_installed` | [`extension/install.rs:70-104`](../../../crates/cyrup-permission-system/src/extension/install.rs) | `INSTALL_ENV_VAR` (`:71`), `POLICY_AGENT_DIR_ENV_KEY` (via `:78`), `CONFIG_PATH_ENV_KEY` (via `:102`) |
| `permission_extension_for_env` | [`extension/install.rs:121-171`](../../../crates/cyrup-permission-system/src/extension/install.rs) | the above + the three subagent hint keys (`:153`) |
| `is_subagent_child` / `has_subagent_env_hint` | [`extension/env.rs:97-116`](../../../crates/cyrup-permission-system/src/extension/env.rs) | `CYRUP_SUBAGENT_CHILD`, `…_RUN_ID`, `…_AGENT_NAME` |
| `resolve_agent_name_from_env` | [`extension/env.rs:67-72`](../../../crates/cyrup-permission-system/src/extension/env.rs) | `CYRUP_SUBAGENT_AGENT_NAME` |
| `policy_agent_dir` | [`extension/paths.rs:34-44`](../../../crates/cyrup-permission-system/src/extension/paths.rs) | `POLICY_AGENT_DIR_ENV_KEY` |
| `ExtensionConfig::resolve_config_path` | [`ext_config.rs:146-154`](../../../crates/cyrup-permission-system/src/ext_config.rs) | `CONFIG_PATH_ENV_KEY` |
| `ExtensionConfig::load_with_result` → `ensure_on_disk` | [`ext_config.rs:168-175`](../../../crates/cyrup-permission-system/src/ext_config.rs), [`:229-250`](../../../crates/cyrup-permission-system/src/ext_config.rs) | `CONFIG_PATH_ENV_KEY`, **then writes that path** |
| `ExtensionConfig::save` | [`ext_config.rs:434-435`](../../../crates/cyrup-permission-system/src/ext_config.rs) | `CONFIG_PATH_ENV_KEY`, **then writes that path** |
| `resolve_logs_dir` | [`logging.rs:75-82`](../../../crates/cyrup-permission-system/src/logging.rs) | `LOGS_DIR_ENV_KEY`, **then writes under it** |
| `forwarding_root_dir` | [`forwarding.rs:148-153`](../../../crates/cyrup-permission-system/src/forwarding.rs) | `FORWARDING_AGENT_DIR_ENV` |
| `resolve_child_wait_timeout` | [`forwarding.rs:638-645`](../../../crates/cyrup-permission-system/src/forwarding.rs) | `CHILD_WAIT_TIMEOUT_ENV` |
| `ForwardingAskChannel::confirm` | [`ask.rs:339`, `:353`](../../../crates/cyrup-permission-system/src/ask.rs) | `PARENT_SESSION_ENV_VAR`, `AGENT_NAME_ENV_VAR` |
| `home_dir` | [`common.rs:29-32`](../../../crates/cyrup-permission-system/src/common.rs) | `HOME` / `USERPROFILE` (never mutated; out of scope) |

### B. Test WRITERS of process env — all five take `env_lock`

| test | file:line | var | lock |
|---|---|---|---|
| `without_install_env` (helper for 7 tests) | [`support.rs:74-88`](../../../crates/cyrup-permission-system/src/extension/tests/support.rs) | `INSTALL_ENV_VAR` | ✅ `env_lock` (`:75`) |
| `with_config_path_override` (helper) | [`enabled_switch.rs:137-152`](../../../crates/cyrup-permission-system/src/extension/tests/enabled_switch.rs) | `CONFIG_PATH_ENV_KEY` | ✅ inherited from enclosing `without_install_env` |
| `the_policy_agent_dir_override_moves_both_the_probe_and_the_engine` | [`install.rs:102-144`](../../../crates/cyrup-permission-system/src/extension/tests/install.rs) | `POLICY_AGENT_DIR_ENV_KEY` (`:123`, `:129-130`) | ✅ inherited |
| `a_blank_policy_agent_dir_override_is_not_an_override` | [`install.rs:148-166`](../../../crates/cyrup-permission-system/src/extension/tests/install.rs) | `POLICY_AGENT_DIR_ENV_KEY` (`:156`) | ✅ direct (`:150`) |
| `env_var_overrides_default_config_path` | [`ext_config.rs:1117-1136`](../../../crates/cyrup-permission-system/src/ext_config.rs) | `CONFIG_PATH_ENV_KEY` (`:1127`, `:1131`) | ✅ direct (`:1118`) |
| `save_honours_the_config_path_env_override` | [`ext_config.rs:1392-1410`](../../../crates/cyrup-permission-system/src/ext_config.rs) | `CONFIG_PATH_ENV_KEY` (`:1401`, `:1405`) | ✅ direct (`:1394`) |
| `logs_dir_env_var_overrides_the_default` | [`logging.rs:553-570`](../../../crates/cyrup-permission-system/src/logging.rs) | `LOGS_DIR_ENV_KEY` (`:558`, `:562`) | ✅ direct (`:554`) |
| `forwarding_channel_denies_when_no_parent_anchor_body` | [`ask.rs:519-530`](../../../crates/cyrup-permission-system/src/ask.rs) | `PARENT_SESSION_ENV_VAR` | ✅ via `with_env_lock` (`ask.rs:493`) |

**Conclusion: full writer participation is already achieved.** Fallback (2) in
the old plan is therefore *already implemented* and the suite still flakes —
which is the empirical proof that a reader/writer lock is not the fix.

### C. Test READERS that DO NOT take the lock — the actual hole

Each of these reads at least one of `CONFIG_PATH_ENV_KEY`,
`POLICY_AGENT_DIR_ENV_KEY`, `INSTALL_ENV_VAR` or the subagent hint keys, and the
ones marked **W** additionally *write a file through the env-derived path*.

| test | file:line | reaches env via | W |
|---|---|---|---|
| `installed_when_policy_file_present` | [`install.rs:35-40`](../../../crates/cyrup-permission-system/src/extension/tests/install.rs) | `is_installed` | |
| `registry_gate_fails_closed_with_no_attached_registry` | [`gate.rs:60-66`](../../../crates/cyrup-permission-system/src/extension/tests/gate.rs) | `PermissionSystemExtension::new` | **W** |
| `ask_fails_fast_without_ui_subagent_or_yolo` | [`gate.rs:83-88`](../../../crates/cyrup-permission-system/src/extension/tests/gate.rs) | `PermissionSystemExtension::new`; also asserts `CYRUP_SUBAGENT_CHILD` is absent | **W** |
| `session_start_rebuilds_manager_from_current_session_cwd` | [`config_reload.rs:70-80`](../../../crates/cyrup-permission-system/src/extension/tests/config_reload.rs) | `PermissionSystemExtension::new` | **W** |
| `parent_role_publishes_and_clears_the_process_parent_session_anchor` | [`watcher.rs:46-53`](../../../crates/cyrup-permission-system/src/extension/tests/watcher.rs) | `new_forwarding_parent` | **W** |
| `a_subagent_child_never_publishes_or_clears_the_parent_session_anchor` | [`watcher.rs:102-112`](../../../crates/cyrup-permission-system/src/extension/tests/watcher.rs) | `new_forwarding_child` | **W** |
| `a_fresh_extension_holds_no_watcher_config_handles` | [`watcher.rs:217-223`](../../../crates/cyrup-permission-system/src/extension/tests/watcher.rs) | `new_forwarding_parent` | **W** |
| `repeated_hooks_yield_exactly_one_forwarding_watcher` | [`watcher.rs:234-235`](../../../crates/cyrup-permission-system/src/extension/tests/watcher.rs) | `parent_ext` → `new_forwarding_parent` (`:204`) | **W** |
| `a_later_hook_arms_the_watcher_a_headless_session_start_could_not` | [`watcher.rs:292-293`](../../../crates/cyrup-permission-system/src/extension/tests/watcher.rs) | same | **W** |
| `a_detaching_ui_tears_the_forwarding_watcher_down` | [`watcher.rs:332-333`](../../../crates/cyrup-permission-system/src/extension/tests/watcher.rs) | same | **W** |
| `the_running_watcher_shares_the_extensions_live_config` | [`watcher.rs:364-365`](../../../crates/cyrup-permission-system/src/extension/tests/watcher.rs) | same | **W** |
| `apply_setting_matches_upstreams_switch_including_its_default_arm` | [`config_modal.rs:586-596`](../../../crates/cyrup-permission-system/src/config_modal.rs) | pure — safe | |
| `toggling_a_row_writes_the_config_and_updates_the_live_snapshot` | [`config_modal.rs:603-627`](../../../crates/cyrup-permission-system/src/config_modal.rs) | `ConfigController::set_config` → `ExtensionConfig::save` (`config_modal.rs:186`), and asserts on `get_config_path()` which is the **resolved** path (`config_modal.rs:162-166`) | **W** |
| `navigation_wraps_and_the_toggle_applies_to_the_selected_row` | [`config_modal.rs:630-648`](../../../crates/cyrup-permission-system/src/config_modal.rs) | same | **W** |
| `a_refused_write_leaves_the_row_and_the_live_config_unchanged` | [`config_modal.rs:677-692`](../../../crates/cyrup-permission-system/src/config_modal.rs) | same — and its *entire premise* (the parent component is a regular file, so the write must fail) is voided when the override redirects the write elsewhere | **W** |
| `the_rendered_frame_carries_the_rows_the_config_path_and_the_help_line` | [`config_modal.rs:695-720`](../../../crates/cyrup-permission-system/src/config_modal.rs) | `get_config_path()` | |

[`src/tests/mod.rs:22-27`](../../../crates/cyrup-permission-system/src/tests/mod.rs)
states the rule these violate in as many words: *"anything built through
`PermissionSystemExtension::new` must hold `crate::ext_config::env_lock`"*. Eleven
tests build through it without holding anything. The whole `config_modal` module
(8 tests, 0 lock references) predates the rule entirely.

### D. Out of scope for `--lib`, same bug class

[`tests/prompt_dedup.rs:117`](../../../crates/cyrup-permission-system/tests/prompt_dedup.rs)
and
[`tests/forwarding_persist.rs:96`, `:117`](../../../crates/cyrup-permission-system/tests/forwarding_persist.rs)
set `CYRUP_SUBAGENT_CHILD` process-wide (and `prompt_dedup` never restores it).
Those are separate integration binaries, so they cannot race the `--lib` suite;
they are covered by the same fix and should be converted in the same change.

---

## Required fix — one path, no options

Delete every process-environment mutation from this crate and route **every**
env read through one crate-internal accessor with a **thread-local** test
overlay. This is not a new convention: it is the convention the crate already
declared and then failed to apply. See
[`extension/env.rs:101-111`](../../../crates/cyrup-permission-system/src/extension/env.rs),
whose `has_subagent_env_hint` is already *"parameterized over the env reader so
the predicate is directly testable without `unsafe { std::env::set_var }` and the
cross-test races a process-global mutation brings"*.

A thread-local overlay is chosen over threading a reader parameter because the
env reads sit behind three **public** signatures — `is_installed`,
`permission_extension_for_env`, `ExtensionConfig::load` / `::save` — and the DoD
forbids production behaviour changes. `#[cfg(not(test))]` compiles the overlay
out entirely, so release code is a byte-identical `std::env::var`.

### Step 1 — the accessor, new file `src/envx.rs`

```rust
//! The ONE process-environment accessor for this crate.
//!
//! Production (`#[cfg(not(test))]`) is a bare `std::env::var`. Under `cfg(test)` the read first
//! consults a THREAD-LOCAL overlay, so a test pins a variable for its own thread without touching
//! the process environment.
//!
//! Why not a mutex around `set_var`: in edition 2024 `std::env::set_var`/`remove_var` are `unsafe`
//! because glibc's `setenv`/`unsetenv` may realloc and free the `environ` array while another
//! thread is inside `getenv`. A lock held by the WRITER cannot make a non-participating READER
//! safe, and this crate has fifteen non-participating readers (see the task census). The hazard is
//! undefined behaviour, not merely a stale value, so the mutation is removed rather than scheduled.

#[cfg(not(test))]
#[must_use]
pub(crate) fn var(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

#[cfg(test)]
thread_local! {
    /// Innermost-wins stack of `(key, value)` pins; `None` means "pinned to unset".
    static OVERLAY: std::cell::RefCell<Vec<(String, Option<String>)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
#[must_use]
pub(crate) fn var(key: &str) -> Option<String> {
    let pinned = OVERLAY.with_borrow(|stack| {
        stack.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    });
    match pinned {
        Some(value) => value,
        None => std::env::var(key).ok(),
    }
}

/// Pin `key` to `value` (`None` = unset) for the CURRENT THREAD until the guard drops.
///
/// No process state is mutated, so parallel tests never observe each other and no lock is needed.
///
/// **Constraint:** the pin is thread-local, so it does NOT reach work moved to another thread.
/// Keep the pinned body synchronous, or drive it on a `new_current_thread` runtime (`#[tokio::test]`
/// defaults to one). Never pin across `tokio::spawn` or a multi-thread runtime.
#[cfg(test)]
pub(crate) fn pin(key: &str, value: Option<&str>) -> EnvPin {
    OVERLAY.with_borrow_mut(|stack| stack.push((key.to_string(), value.map(str::to_string))));
    EnvPin
}

#[cfg(test)]
pub(crate) struct EnvPin;

#[cfg(test)]
impl Drop for EnvPin {
    fn drop(&mut self) {
        OVERLAY.with_borrow_mut(|stack| {
            stack.pop();
        });
    }
}
```

Register it in [`src/lib.rs`](../../../crates/cyrup-permission-system/src/lib.rs)
as `mod envx;` (private — nothing outside the crate may reach it).

### Step 2 — rewire every reader in section A

Replace `std::env::var(K).ok()` with `crate::envx::var(K)` at exactly these
sites. No other edit; the surrounding `.map`/`.filter`/`.trim` chains are
unchanged, so behaviour is identical.

| file | line | current |
|---|---|---|
| [`ext_config.rs`](../../../crates/cyrup-permission-system/src/ext_config.rs) | 147 | `std::env::var(CONFIG_PATH_ENV_KEY)` |
| [`logging.rs`](../../../crates/cyrup-permission-system/src/logging.rs) | 76 | `std::env::var(LOGS_DIR_ENV_KEY)` |
| [`extension/paths.rs`](../../../crates/cyrup-permission-system/src/extension/paths.rs) | 35 | `std::env::var(POLICY_AGENT_DIR_ENV_KEY)` |
| [`extension/env.rs`](../../../crates/cyrup-permission-system/src/extension/env.rs) | 68 | `std::env::var(AGENT_NAME_ENV_VAR)` |
| [`extension/env.rs`](../../../crates/cyrup-permission-system/src/extension/env.rs) | 98 | `has_subagent_env_hint(\|key\| std::env::var(key).ok())` → `has_subagent_env_hint(crate::envx::var)` |
| [`extension/env.rs`](../../../crates/cyrup-permission-system/src/extension/env.rs) | 120 | `std::env::var(name)` in `env_truthy` |
| [`ask.rs`](../../../crates/cyrup-permission-system/src/ask.rs) | 339, 353 | `PARENT_SESSION_ENV_VAR`, `AGENT_NAME_ENV_VAR` |
| [`forwarding.rs`](../../../crates/cyrup-permission-system/src/forwarding.rs) | 148 | `FORWARDING_AGENT_DIR_ENV` |
| [`forwarding.rs`](../../../crates/cyrup-permission-system/src/forwarding.rs) | 639 | `CHILD_WAIT_TIMEOUT_ENV` |

Leave [`common.rs:30-31`](../../../crates/cyrup-permission-system/src/common.rs)
(`HOME`/`USERPROFILE`) alone — no test mutates it and it is not part of this
contract.

Note that `has_subagent_env_hint` at
[`env.rs:112-116`](../../../crates/cyrup-permission-system/src/extension/env.rs)
takes `impl Fn(&str) -> Option<String>`, and `crate::envx::var` has exactly that
shape, so it is passed as a function item with no closure.

### Step 3 — rewrite the four test helpers, and DELETE the lock

`without_install_env`
([`support.rs:71-88`](../../../crates/cyrup-permission-system/src/extension/tests/support.rs))
becomes lock-free and `unsafe`-free:

```rust
/// Run `body` with [`INSTALL_ENV_VAR`] pinned UNSET for this thread only.
///
/// The pin is a thread-local overlay in [`crate::envx`], not a process mutation: nothing another
/// test can observe changes, so no lock is taken and none is needed. The previous implementation
/// held `ext_config::env_lock` around an `unsafe { std::env::remove_var }`, which serialized the
/// crate's five env WRITERS but not its fifteen unlocked READERS — and in edition 2024 a `getenv`
/// concurrent with `unsetenv` is undefined behaviour, which no writer-side lock can repair.
pub(super) fn without_install_env<T>(body: impl FnOnce() -> T) -> T {
    let _pin = crate::envx::pin(INSTALL_ENV_VAR, None);
    body()
}
```

`with_config_path_override`
([`enabled_switch.rs:137-152`](../../../crates/cyrup-permission-system/src/extension/tests/enabled_switch.rs)):

```rust
fn with_config_path_override<T>(path: &Path, body: impl FnOnce() -> T) -> T {
    let _pin = crate::envx::pin(
        crate::ext_config::CONFIG_PATH_ENV_KEY,
        Some(&path.display().to_string()),
    );
    body()
}
```

Its doc comment about `env_lock` non-reentrancy goes away with the lock — pins
nest freely (innermost wins, LIFO pop), which is what
`without_install_env` + `with_config_path_override` need.

Convert the same way, deleting every `unsafe` block and every `env_lock()`
acquisition:

- [`install.rs:121-132`](../../../crates/cyrup-permission-system/src/extension/tests/install.rs) and [`install.rs:150-164`](../../../crates/cyrup-permission-system/src/extension/tests/install.rs) — `POLICY_AGENT_DIR_ENV_KEY`
- [`ext_config.rs:1117-1136`](../../../crates/cyrup-permission-system/src/ext_config.rs) and [`ext_config.rs:1392-1410`](../../../crates/cyrup-permission-system/src/ext_config.rs) — `CONFIG_PATH_ENV_KEY`; drop the `_guard` line in each
- [`logging.rs:553-570`](../../../crates/cyrup-permission-system/src/logging.rs) — `LOGS_DIR_ENV_KEY`; then delete `trail_lock` ([`logging.rs:418-420`](../../../crates/cyrup-permission-system/src/logging.rs)) and its seven call sites, which exist only to serialize that one mutation
- [`ask.rs:519-530`](../../../crates/cyrup-permission-system/src/ask.rs) — `PARENT_SESSION_ENV_VAR`; `forwarding_channel_denies_when_no_parent_anchor` ([`ask.rs:492-494`](../../../crates/cyrup-permission-system/src/ask.rs)) then needs only the current-thread `block_on`, not `with_env_lock`

Then delete, in
[`ext_config.rs`](../../../crates/cyrup-permission-system/src/ext_config.rs):
`env_lock` (`:762-771`) and `with_env_lock` (`:773-798`); and in
[`support.rs`](../../../crates/cyrup-permission-system/src/extension/tests/support.rs):
`with_config_env_lock` (`:44-61`). Replace the ~15 `with_config_env_lock(fut)` /
`with_env_lock(fut)` call sites in
[`config_reload.rs`](../../../crates/cyrup-permission-system/src/extension/tests/config_reload.rs),
[`agent_start.rs`](../../../crates/cyrup-permission-system/src/extension/tests/agent_start.rs) and
[`events.rs`](../../../crates/cyrup-permission-system/src/extension/tests/events.rs)
with a plain local `block_on` helper:

```rust
/// Drive `body` on a current-thread runtime. (Formerly `with_config_env_lock`, which additionally
/// held `ext_config::env_lock`; there is no longer a process-env mutation anywhere in the crate for
/// that lock to serialize — see `crate::envx`.)
pub(super) fn block_on<F: std::future::Future>(body: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(body)
}
```

Keep `runtime_api::test_registry_lock`
([`runtime_api.rs:114`](../../../crates/cyrup-permission-system/src/runtime_api.rs))
— that guards a genuine `static RUNTIME_API` slot (`:64`) and is unrelated.

Two tests exist *only* to assert the deleted lock's `!Send` shape and must be
deleted with it:
[`config_reload.rs:347-351`](../../../crates/cyrup-permission-system/src/extension/tests/config_reload.rs)
(`the_env_locked_body_does_not_carry_the_guard_across_its_awaits`) and
[`ask.rs:507-511`](../../../crates/cyrup-permission-system/src/ask.rs)
(`the_env_locked_body_does_not_carry_the_guard_across_its_await`). Their subject
— a `MutexGuard` captured in a future's state — cannot recur once no such guard
is taken.

### Step 4 — the regrowth gate

Every `unsafe` block in this crate is one of these test-only env mutations
(`ext_config.rs:1126`, `:1130`, `:1400`, `:1404`; `logging.rs:557`, `:561`;
`enabled_switch.rs:142`, `:145`; `support.rs:78`, `:81`; `install.rs:123`,
`:127`, `:156`, `:159`; `ask.rs:525`, `:529`). There is **no `unsafe` in this
crate's production code at all**.

So after step 3, add to
[`src/lib.rs`](../../../crates/cyrup-permission-system/src/lib.rs):

```rust
#![forbid(unsafe_code)]
```

`cyrup-permission-system` is one of only three workspace crates lacking it (the
others are `cyrup-ext-sdk` and `cyrup-tools`), and this env mutation is the sole
reason. `cyrup-ext-subagents` already carries it
([`crates/cyrup-ext-subagents/src/lib.rs:25`](../../../crates/cyrup-ext-subagents/src/lib.rs))
and its `lib.rs:68` comment names exactly this trade-off. `forbid` — not `deny` —
because `forbid` cannot be re-allowed by an inner attribute, which is what makes
it a gate rather than a suggestion. This is what stops the defect from regrowing:
the next author who reaches for `std::env::set_var` in a test gets a compile
error instead of a flake.

---

## Fallbacks — evaluated and rejected

**(a) Make every reader acquire `env_lock`.** Rejected on evidence, not taste.
Three findings kill it:
- Section B shows **every writer already participates**. The suite flakes anyway.
- It does not close the hazard, it only narrows it. `getenv` racing `unsetenv` is
  UB; a lock that the C library's own internal readers do not take (and the ones
  in `std`, `tokio`, `notify`, `uuid`, `getrandom` do not) leaves the crate
  relying on "no dependency called `getenv` in that window". Nothing enforces
  that, and `notify::PollWatcher` and `tokio` both run background threads inside
  these tests.
- It would have to reach 16 more tests including all 8 of
  [`config_modal.rs`](../../../crates/cyrup-permission-system/src/config_modal.rs),
  and each new test forever after. The invariant is unenforceable — there is no
  lint for "this function transitively calls `getenv`" — which is precisely how
  the current 16 accumulated after
  [`tests/mod.rs:22-27`](../../../crates/cyrup-permission-system/src/tests/mod.rs)
  wrote the rule down. It also serializes the entire suite behind one mutex,
  buying the runtime cost of `--test-threads=1` while keeping the UB.

**(b) Move the two tests to their own integration binary.** Rejected. Cost:
`enabled_switch.rs` and `install.rs` reach `crate::ext_config::env_lock`,
`crate::ext_config::reset_load_count` / `load_count`
([`ext_config.rs:816-825`](../../../crates/cyrup-permission-system/src/ext_config.rs))
and `crate::extension::paths::{CONFIG_DIR, CONFIG_FILE, POLICY_FILE, PROJECT_AGENT_SUBDIR, policy_agent_dir}`
— all `pub(crate)` or `pub(super)`. Relocating them means widening that surface
to `pub` purely for tests, which
[`ext_config.rs:816`](../../../crates/cyrup-permission-system/src/ext_config.rs)'s
`#[cfg(test)] pub(crate)` instrumentation was deliberately shaped to avoid. It
also isolates only *today's* two tests: the other 14 unlocked accessors in
section C stay in the same process with the same UB, so the next test to
exercise a config-path override reintroduces the flake. Process isolation buys
nothing here anyway — a thread-local overlay gives strictly better isolation at
zero structural cost.

**(c) `--test-threads=1` in CI. Explicitly ruled out.** It hides the race for
every future test rather than removing it, converts a hard failure into a latent
one, and leaves `cargo test` on a developer's machine flaky while CI is green —
the worst possible split. It also does not remove the UB; it only makes the
window unlikely.

**(d) `#[ignore]` on the two tests.** Ruled out: they cover PERM-002 (the install
latch) and G130(b) (probe/switch config agreement), both security-relevant
regressions. Silencing them trades a flaky signal for no signal.

---

## Definition of done

1. **`cargo test -p cyrup-permission-system --lib` passes 20 consecutive runs at
   default parallelism.** Exact command — it stops at the first failure and
   reports the run number:

   ```bash
   for i in $(seq 1 20); do
     echo "=== run $i/20 ==="
     cargo test -p cyrup-permission-system --lib || { echo "FAILED on run $i"; exit 1; }
   done
   echo "20/20 clean at default parallelism"
   ```

   Do **not** pass `--test-threads`; the point is default parallelism.
2. `grep -rn 'set_var\|remove_var' crates/cyrup-permission-system/src` returns
   **zero** hits outside doc comments.
3. `grep -rn 'env_lock\|with_env_lock\|with_config_env_lock\|trail_lock' crates/cyrup-permission-system/src`
   returns **zero** hits — the lock is deleted, not merely unused.
4. `#![forbid(unsafe_code)]` is present in
   [`src/lib.rs`](../../../crates/cyrup-permission-system/src/lib.rs) and the
   crate compiles.
5. Every `std::env::var` in `src/` reads either through `crate::envx::var` or is
   the `HOME`/`USERPROFILE` pair in
   [`common.rs:30-31`](../../../crates/cyrup-permission-system/src/common.rs).
6. `cargo test -p cyrup-permission-system` (lib **and** the three integration
   binaries) is green, and
   [`tests/prompt_dedup.rs`](../../../crates/cyrup-permission-system/tests/prompt_dedup.rs)
   / [`tests/forwarding_persist.rs`](../../../crates/cyrup-permission-system/tests/forwarding_persist.rs)
   no longer call `set_var` either.
7. `cargo clippy -p cyrup-permission-system --all-targets` is clean under the
   workspace's `unwrap_used` / `expect_used` / `panic` / `indexing_slicing` denies
   (the test modules keep their existing `#![allow]`s).
8. **No production behaviour changes.** `#[cfg(not(test))] envx::var` is a bare
   `std::env::var(key).ok()`, and every call site keeps its existing
   trim/filter/default chain, so a release build resolves every path, timeout and
   opt-in exactly as it does today. The three public signatures — `is_installed`,
   `permission_extension_for_env`, `ExtensionConfig::load`/`::save` — are
   untouched.
