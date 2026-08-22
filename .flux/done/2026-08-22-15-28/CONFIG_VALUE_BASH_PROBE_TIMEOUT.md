---
stage: qa
status: completed
updated: 2026-08-22 23:58
---

# Bound The Unbounded which/where Bash Probe In config_value.rs

## Description

`find_bash_on_path` in [`src/config_value.rs:409-429`](../../crates/cyrup-config/src/config_value.rs)
spawns the bash-discovery probe with a **blocking, unbounded** `Command::output()`:

```rust
let (program, arg) = if cfg!(windows) { ("where", "bash.exe") } else { ("which", "bash") };
let output = Command::new(program).arg(arg).output().ok()?;   // config_value.rs:415
```

Its own doc comment at `:406-408` states the contract it does not implement: *"Pi caps the lookup at
5s; `Command::output` blocks, but `where`/`which` return promptly."* A `which` wedged on a stale
automount or an unreachable network PATH entry hangs the caller indefinitely with no recovery.

### The bounded implementation already exists in this workspace

[`crates/cyrup-tools/src/ops/shell.rs`](../../crates/cyrup-tools/src/ops/shell.rs) ports the same Pi
source (`utils/shell.ts`) and bounds both probes:
`const BASH_PROBE_TIMEOUT: Duration = Duration::from_secs(5)` (`ops/shell.rs:62`), then a `try_wait`
poll loop at `:88-107` that on expiry does `child.kill()` + `child.wait()` and returns `None` —
mapping timeout to "no bash on PATH", exactly Pi's `spawnSync` semantics. The two files are
near-duplicate ports: `is_legacy_wsl_bash_path`, `ShellConfig`, `find_bash_on_path`, and
`get_bash_shell_config` (`ops/shell.rs:50`) vs `bash_shell_config` (`config_value.rs:390`).

### Runtime exposure is Windows-only — do not overstate it

`execute_shell` at `config_value.rs:236` gates the configured-shell path behind `cfg!(windows)`. On
unix builds `find_bash_on_path` compiles but is never reached from `execute_shell` (the unix arm of
`get_shell_config` at `:451-456` calls it, but `execute_shell` short-circuits to
`execute_with_default_shell`). A unix-hosted test cannot observe the hang through `execute_shell`;
cyrup-tools sidesteps this by driving its poll loop directly under `#[cfg(unix)]`
(`ops/shell.rs:400-430`).

**Scope:** port the `ops/shell.rs:88-107` poll loop into `config_value.rs::find_bash_on_path` —
roughly 25 lines, no dependency change. **Deleting the duplication is explicitly a NON-GOAL:**
`crates/cyrup-tools/Cargo.toml:15` lists `cyrup-core` as its only workspace dep, so a shared home
means relocating both ports into `cyrup-core` — a larger, separate change. Fix the timeout; leave
the duplication.

## Acceptance Criteria

- [ ] `find_bash_on_path` no longer calls `Command::output()`: `rg -n 'fn find_bash_on_path' -A 25 src/config_value.rs | rg -c '\.output\(\)'` returns 0
- [ ] It spawns with piped stdout and polls with `try_wait` against a deadline, killing and reaping on
      expiry. **Scope the check to the function** — a bare file-wide grep is useless here, because
      `run_with_timeout` (`:303-351`) already contains `try_wait` at `:331` and `.kill()` at `:345`:
      `rg -n 'fn find_bash_on_path' -A 40 src/config_value.rs | rg -c 'try_wait'` returns at least 1
- [ ] A named 5-second timeout constant exists in `src/config_value.rs` (matching `BASH_PROBE_TIMEOUT` at `ops/shell.rs:62`), and the timeout path returns `None` rather than propagating an error
- [ ] The stale doc at `config_value.rs:406-408` no longer claims the call is unbounded
- [ ] No change to `crates/cyrup-config/Cargo.toml` and no change to any file under `crates/cyrup-tools/`
- [ ] `cargo test -p cyrup-config` still reports 222 passed / 0 failed
- [ ] `cargo clippy -p cyrup-config --all-targets` 0 warnings; `cargo fmt -p cyrup-config -- --check` 0 hunks

## Implementer notes (from planning)

- **`crates/cyrup-tools` is NOT rustfmt-clean** — `cargo fmt -p cyrup-tools -- --check` reports **468**
  hunks, and the repo has no `rustfmt.toml`. So `ops/shell.rs` cannot be copied verbatim: mirror its
  *structure*, then format to the default profile, or the "0 hunks" criterion below fails. Never run a
  workspace-wide `cargo fmt` to fix this — it would reformat 468 hunks of someone else's crate.
- The claim that `crates/cyrup-tools/Cargo.toml:15` makes `cyrup-core` its "only workspace dep" is
  imprecise — ~20 further deps also use `workspace = true`. The accurate statement, which that crate's
  own comment makes at `Cargo.toml:51`, is that `cyrup-core` is its only **internal `cyrup-*` runtime**
  dependency. The non-goal conclusion is unaffected and stands.

## Outcome — completed

Landed in `83e73a1`. All seven acceptance criteria verified mechanically at QA.

`find_bash_on_path` now spawns with piped stdout and polls `try_wait` against a 5-second deadline,
killing and reaping on expiry and returning `None` — mapping timeout to "no bash on PATH", which is
what Node's `spawnSync` does. `BASH_PROBE_TIMEOUT` sits at `config_value.rs:409`, matching the
constant in `cyrup-tools`. No `Command::output()` remains in the function, `Cargo.toml` is untouched,
and nothing under `crates/cyrup-tools/` changed.

Structure was mirrored rather than copied, for the reason recorded in the implementer notes:
`cyrup-tools` carries 468 rustfmt hunks, so verbatim code would have broken this crate's zero-hunk
state.

Exposure is Windows-only — `execute_shell` gates the configured-shell path behind `cfg!(windows)` —
so no test claims to observe the hang on unix.

## Correction already folded in

This file's original AC for `try_wait` was defective: it grepped the whole file, and
`run_with_timeout` (`:303-351`) already contained `try_wait` at `:331` and `.kill()` at `:345`, so it
was satisfied by the untouched file and could never have detected the fix. It is now scoped to
`find_bash_on_path`. Found during planning, before implementation.
