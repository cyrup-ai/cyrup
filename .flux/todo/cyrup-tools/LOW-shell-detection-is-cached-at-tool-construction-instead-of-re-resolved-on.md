---
title: Shell detection is cached at tool construction instead of re-resolved on every command
priority: LOW
tool: bash
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# Shell detection is cached at tool construction instead of re-resolved on every command

## What pi does

`/home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/bash.ts:91` calls `resolveShellConfig()` (→ `getShellConfig(shellPath)`, `:158-160`) *inside every* `exec`, immediately after `resolveTimeoutMs` and the abort check, so the `/bin/bash` → `which bash` → `sh` resolution is re-run per command.

## What cyrup-tools does

`/home/user/cyrup/crates/cyrup-tools/src/registry.rs:57` calls `ShellConfig::detect()` once and passes the result into `BashTool::new`; `/home/user/cyrup/crates/cyrup-tools/src/tools/bash.rs:302-305` only re-resolves when `opts.shell_path` is `Some(..)` and otherwise reuses the cached `self.shell`.

## User-visible impact

If bash appears or disappears during a session (a `/bin/bash` install, a PATH change, a container remount), pi's very next bash command picks up the new resolution while cyrup keeps using the shell it detected at startup until the process restarts.

## Parity action

Re-run `ShellConfig::try_detect()` (or `resolve(shell_path)`) inside `BashTool::execute` on every call rather than caching the construction-time value.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Searched every shell-resolution site in cyrup-tools, cyrup-core, cyrup-session-svc and the builder. The claim survives, but only for the auto-detect branch, and cyrup already has the per-call machinery.

What DOES exist (partial refutation):
- /home/user/cyrup/crates/cyrup-tools/src/ops/shell.rs:142 `ShellConfig::resolve(Option<&str>)` -> :194 `try_detect()` implements pi's exact `getShellConfig` order (existsSync /bin/bash -> `which bash` (5s bounded) -> `sh -c`, plus the Windows Git Bash / `where bash.exe` / `No bash shell found` arms). It is a pure, side-effect-free function callable per command.
- /home/user/cyrup/crates/cyrup-tools/src/ops/mod.rs:492-556 `LocalBashOperations` — the direct port of `createLocalBashOperations` — stores only `shell_path: Option<String>` and calls `ShellConfig::resolve(self.shell_path.as_deref())` INSIDE `exec` (:535), i.e. full re-detection on every command, including the `None`/auto branch. Its doc at :495-499 states the rule explicitly, and the test `local_bash_operations_resolves_the_shell_per_call_so_a_bad_path_fails_before_spawn` (ops/mod.rs:686-720) pins it. So the capability is implemented and reachable through the `BashOperations` override seam.
- /home/user/cyrup/crates/cyrup-tools/src/tools/bash.rs:302-305 and /home/user/cyrup/crates/cyrup-session-svc/src/session/bash.rs:93-99 both re-resolve per exec when a settings `shellPath` is set, in pi's exact ordering (after resolve_timeout_ms and the abort check), producing `Custom shell path not found` at the same point pi does.

What is genuinely absent: with no `shellPath` set (the common case), the default wiring resolves once and reuses it. `ShellConfig::detect()` is called at /home/user/cyrup/crates/cyrup-tools/src/registry.rs:57 and /home/user/cyrup/crates/cyrup-session-svc/src/builder.rs:843 (also ops/mod.rs:579 `Backend::default`), and both bash front-ends fall back to the cached copy (`None => self.shell.clone()`, bash.rs:304; session/bash.rs:99). There is no re-detect hook, settings-watcher rebuild, or lazy invalidation anywhere — grep for `detect()`/`try_detect(` finds only construction sites. cyrup-tools' own doc at ops/mod.rs:498 names this as the divergence the immediate-bash path carries. Nothing in cyrup-core touches shell selection at all.

Severity corrected to low: the divergence is only observable if the shell topology changes DURING a live session — e.g. a session starting in a minimal container with no bash (cached `sh -c`) and then `apk add bash` mid-run, or /bin/bash being removed under a running session. Steady-state behaviour is byte-identical to pi (same order, same transports, same errors), commands keep executing, and the failure mode when it does bite is a loud shell syntax/spawn error, not silent wrongness; a session restart clears it. cyrup also pays one detection instead of an `existsSync` + possible `which bash` spawn per command, so the cache is a deliberate cost trade rather than an oversight.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
