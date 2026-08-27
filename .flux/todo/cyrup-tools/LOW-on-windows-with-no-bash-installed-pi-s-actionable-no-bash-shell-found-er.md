---
title: On Windows with no bash installed, pi's actionable No bash shell found error is replaced by an opaque spawn failure
priority: LOW
tool: bash
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# On Windows with no bash installed, pi's actionable No bash shell found error is replaced by an opaque spawn failure

## What pi does

`createLocalBashOperations` calls `getShellConfig(shellPath)` *inside* `exec` (`/home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/bash.ts:91,158-160`), and `utils/shell.ts:100-106` throws a fully-formed repair recipe when no Git Bash candidate exists and `where bash.exe` finds nothing: `"No bash shell found. Options:\n  1. Install Git for Windows: https://git-scm.com/download/win\n  2. Add your bash to PATH (Cygwin, MSYS2, etc.)\n  3. Set shellPath in settings.json\n\nSearched Git Bash in:\n  <each candidate>"`. That error reaches the model as the tool result for every bash call.

## What cyrup-tools does

`ToolRegistry::with_builtins` resolves the shell once at construction with the *infallible* `ShellConfig::detect()` (`/home/user/cyrup/crates/cyrup-tools/src/registry.rs:57`; same at `/home/user/cyrup/crates/cyrup-session-svc/src/builder.rs:843`). `/home/user/cyrup/crates/cyrup-tools/src/ops/shell.rs:237-244` `detect()` swallows the `No bash shell found` error from `try_detect()` and degrades to a bare `bash -c`. `BashTool::execute` only re-resolves (and can surface an error) when `opts.shell_path` is `Some` (`/home/user/cyrup/crates/cyrup-tools/src/tools/bash.rs:302-305`), so with no `shellPath` setting the failure instead appears at spawn as `error::io("spawn bash", e)` (`/home/user/cyrup/crates/cyrup-tools/src/ops/local/proc.rs:130-132`).

## User-visible impact

A Windows user with no bash gets a generic `spawn bash: … (os error 2)` per command instead of the three-step install/PATH/settings recipe plus the list of searched Git Bash locations, with no indication of how to fix it.

## Parity action

Call `ShellConfig::try_detect()` at the `registry.rs:57` / `builder.rs:843` construction sites and propagate the error, or re-resolve inside `BashTool::execute` via `try_detect()` on every exec so the `No bash shell found` message is what surfaces as the tool error.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Searched hard and could not refute the reachability half of the claim, though the claim overstates it as a missing capability. The capability's substance DOES exist in Rust: `/home/user/cyrup/crates/cyrup-tools/src/ops/shell.rs:169-191` (`ShellConfig::windows_detect_from`) implements pi's exact Windows order — ProgramFiles/ProgramFiles(x86) Git Bash candidates, then `where bash.exe` (`find_bash_on_path`, shell.rs:78-127, with pi's 5s probe timeout), then the verbatim three-option repair recipe plus the two-space-indented "Searched Git Bash in:" list — and it is unit-tested on every host at shell.rs:337-366 (`windows_arm_without_bash_errors_with_pis_repair_recipe`). `ShellConfig::try_detect` (shell.rs:194-226) returns it, and `ShellConfig::resolve(None)` (shell.rs:142-150) forwards to it.

What is genuinely missing is the wiring: no production call site reaches the fallible path when `shellPath` is unset. `ShellConfig::resolve` is called only with `Some(p)` at `/home/user/cyrup/crates/cyrup-tools/src/tools/bash.rs:302-305` and `/home/user/cyrup/crates/cyrup-session-svc/src/session/bash.rs:94-99`; the only `resolve(None)` caller, `LocalBashOperations::exec` (`/home/user/cyrup/crates/cyrup-tools/src/ops/mod.rs:535`), is constructed nowhere outside that file's own tests (mod.rs:640,694,704,729). Every real construction site — `ToolRegistry::with_builtins` (registry.rs:57), `cyrup-session-svc/src/builder.rs:843`, `Backend::default` (ops/mod.rs:579) — uses the infallible `ShellConfig::detect()` (shell.rs:237-244), whose own doc comment concedes the point ("Prefer `try_detect` at every real construction site so a Windows box with no bash reports Pi's `No bash shell found` at session construction"). So on Windows with no bash and no `shellPath`, the recipe is dead code and the user gets `spawn bash: … (os error 2)` from `/home/user/cyrup/crates/cyrup-tools/src/ops/local/proc.rs:129-132`. No fallback interpreter is substituted and no other surface (config_value.rs's independent `get_shell_config`, any CLI doctor/preflight) emits the recipe.

Severity corrected down to low: the blast radius is a Windows host with no bash installed anywhere, where the bash tool cannot function under either agent; the failure is loud and immediate, not silent or wrong-result — a bare `bash -c` is still bash, never a substituted `cmd.exe`, so nothing executes under different semantics. Only the actionability/wording of a fatal-environment diagnostic differs, and the fix is a one-line swap of `detect()` for `try_detect()?` at the two construction sites since the message, the candidate list and its tests already exist.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
