---
title: Bash child processes are not spawned with the console-window suppression flag on Windows
priority: LOW
tool: bash
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: aug
status: in-progress
updated: 2026-08-27
---

# Bash child processes are not spawned with the console-window suppression flag on Windows

## What pi does

`/home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/bash.ts:99-105` spawns the shell with `windowsHide: true`, suppressing the console window libuv would otherwise create for the child.

## What cyrup-tools does

`/home/user/cyrup/crates/cyrup-tools/src/ops/local/command.rs:14-51` (`build_command`) sets program, args, cwd, env, and stdio and installs `setsid` on unix, but never calls `creation_flags(CREATE_NO_WINDOW)` on Windows. A ripgrep for `creation_flags|CREATE_NO_WINDOW` across `/home/user/cyrup/crates/cyrup-tools` returns zero hits, even though other cyrup crates do apply it (`/home/user/cyrup/crates/cyrup-mcp/src/secrets.rs:213`, `/home/user/cyrup/crates/cyrup-intercom/src/transport/spawn.rs:187`).

## User-visible impact

On Windows a console window can flash on screen for every bash command the model runs, and in a GUI/embedded host the spawned shell can pop a visible console; pi never shows one.

## Parity action

In `build_command`, add a `#[cfg(windows)]` arm calling `std::os::windows::process::CommandExt::creation_flags(CREATE_NO_WINDOW)` (0x0800_0000), matching what `cyrup-mcp/src/secrets.rs` already does.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Confirmed absent. ripgrep for creation_flags|CREATE_NO_WINDOW|windowsHide|no_window|0x08000000|DETACHED_PROCESS across crates/cyrup-tools/src and crates/cyrup-core/src yields only two hits, both non-implementing: signal.rs:40 is a comment citing Pi's windowsHide, and command.rs:39 is `use std::os::unix::process::CommandExt` (the unix pre_exec/setsid import). cyrup-core contains no process-spawn code at all. I read every non-test Command::new site in the crate — command.rs:15 (build_command), command.rs:65 (build_argv_command), shell.rs:88 (the where bash.exe / which bash probe), and signal.rs:42/75/144 (the three taskkill spawns) — none apply creation_flags, and the spawn path build_command -> tokio::process::Command::from -> .spawn() (proc.rs:127-131) adds nothing in between, so no later layer supplies it. The gap is reachable on Windows: shell.rs:80 has a cfg(not(unix)) `where bash.exe` probe, signal.rs has cfg(not(unix)) taskkill arms, path.rs:89 has a cfg(windows) block, and src/tests/no_inherited_harness_stdio.rs:25 states the crate cross-compiles clean for Windows and scans the cfg(windows) taskkill spawns (that structural test pins stdio only, not creation flags). Other crates apply the flag (cyrup-mcp/src/secrets.rs:140,213; cyrup-intercom/src/transport/spawn.rs:185-187), so this is an unapplied known pattern rather than a different implementation. Severity downgraded to low: nothing is silently wrong — the command executes, output capture, exit codes, and kill/escalation are all unaffected; the only delta is a console window flashing on Windows hosts, which is purely cosmetic.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
