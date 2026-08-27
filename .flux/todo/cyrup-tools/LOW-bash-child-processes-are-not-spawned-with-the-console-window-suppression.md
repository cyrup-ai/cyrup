---
title: Bash child processes are not spawned with the console-window suppression flag on Windows
priority: LOW
tool: bash
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: exec
status: in-progress
updated: 2026-08-27
---

# Bash child processes are not spawned with the console-window suppression flag on Windows

## Core objective

Every process `cyrup-tools` spawns whose Pi counterpart passes `windowsHide: true` must be created
with the Windows `CREATE_NO_WINDOW` creation flag, so that on a Windows host no console window
flashes (and, in a GUI/embedded host, no console window *appears*) for a `bash` tool invocation, for
the shell-detection PATH probe, or for the `taskkill` cleanup spawns.

The mirror-image half of the objective is equally load-bearing: the ONE spawn whose Pi counterpart
deliberately omits `windowsHide` — the direct-argv `exec` capability path — must keep omitting it.
This is a parity task, and the crate's existing discipline is that each spawn is 1:1 with its own
real Pi consumer, not with the crate's other spawns.

## What pi does — verified against the checked-out upstream

The shell spawn shared by the `bash` and `powershell` tools
([pi bash.ts:99-105](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts)):

```ts
const commandFromStdin = shellConfig.commandTransport === "stdin";
const child = spawn(shellConfig.shell, commandFromStdin ? shellConfig.args : [...shellConfig.args, command], {
    cwd,
    detached: process.platform !== "win32",
    env: env ?? getShellEnv(),
    stdio: [commandFromStdin ? "pipe" : "ignore", "pipe", "pipe"],
    windowsHide: true,
});
```

The Windows arm of the shell-detection PATH probe
([pi shell.ts:25-42](../../../tmp/pi/packages/coding-agent/src/utils/shell.ts)) — note the unix
`which` arm at `shell.ts:47` does NOT pass the option, because the option is meaningless there:

```ts
if (process.platform === "win32") {
    const result = spawnSync("where", [executable], {
        encoding: "utf-8",
        timeout: 5000,
        windowsHide: true,
    });
```

The process-tree kill ([pi shell.ts:216-233](../../../tmp/pi/packages/coding-agent/src/utils/shell.ts)):

```ts
const child = spawn(
    join(process.env.SystemRoot ?? "C:\\Windows", "System32", "taskkill.exe"),
    ["/F", "/T", "/PID", String(pid)],
    { stdio: "ignore", detached: true, windowsHide: true },
);
```

And the one that does NOT — `execCommand`, the real consumer behind the WASM `exec` capability
([pi exec.ts:41-45](../../../tmp/pi/packages/coding-agent/src/core/exec.ts)):

```ts
const proc = spawn(command, args, {
    cwd,
    shell: false,
    stdio: ["ignore", "pipe", "pipe"],
});
```

Node's default is `windowsHide: false`, so this spawn genuinely shows a console window upstream.

## What cyrup-tools does today — verified, with the original citations corrected

[`build_command`](../../../crates/cyrup-tools/src/ops/local/command.rs) at
[command.rs:14-51](../../../crates/cyrup-tools/src/ops/local/command.rs) sets program, args, cwd,
env removals-then-overrides, and all three stdio handles, then installs the unix `setsid` process
group at [command.rs:37-49](../../../crates/cyrup-tools/src/ops/local/command.rs) — and returns.
There is no Windows arm at all. The spawn path adds nothing afterwards:
[proc.rs:127-132](../../../crates/cyrup-tools/src/ops/local/proc.rs) is

```rust
let std_cmd = build_command(&spec);
let mut cmd = tokio::process::Command::from(std_cmd);
cmd.kill_on_drop(true);
let mut child = cmd
    .spawn()
    .map_err(|e| error::io(&format!("spawn {}", error::show(&spec.shell.program)), &e))?;
```

`tokio::process::Command::from` wraps the `std::process::Command` value rather than rebuilding it,
so creation flags set inside `build_command` DO survive the conversion — the fix belongs in the
builder, which is the single funnel, and not at the spawn site.

The other reachable-on-Windows spawns in the crate, all confirmed present and all missing the flag:

| Site | Pi counterpart | Pi passes `windowsHide`? |
| --- | --- | --- |
| [command.rs:15](../../../crates/cyrup-tools/src/ops/local/command.rs) `build_command` | `bash.ts:99-105` | yes |
| [command.rs:65](../../../crates/cyrup-tools/src/ops/local/command.rs) `build_argv_command` | `exec.ts:41-45` | **no** |
| [shell.rs:88](../../../crates/cyrup-tools/src/ops/shell.rs) `find_bash_on_path` (`where bash.exe`) | `shell.ts:28-32` | yes |
| [signal.rs:42](../../../crates/cyrup-tools/src/ops/local/signal.rs) `kill_process_tree` | `shell.ts:220-228` | yes |
| [signal.rs:75](../../../crates/cyrup-tools/src/ops/local/signal.rs) `send_sigkill_tree` | `shell.ts:220-228` | yes |
| [signal.rs:144](../../../crates/cyrup-tools/src/ops/local/signal.rs) `kill_pid` | none (port-side `taskkill`) | n/a — see below |

Corrections to the original write-up:

- The claim "cyrup-core contains no process-spawn code at all" is **confirmed**: there is no
  `Command::new` anywhere under [crates/cyrup-core/src](../../../crates/cyrup-core/src), and its
  module list (`cancel`, `constrained_sampling`, `diagnostics`, `error`, `event_stream`,
  `keyed_lock`, `message`, `tool`) contains no process module. No shared data shape changes, so
  **nothing in `cyrup-core` is touched by this task.**
- [signal.rs:39-41](../../../crates/cyrup-tools/src/ops/local/signal.rs) is indeed a comment that
  *cites* `windowsHide: true` while the code three lines below does not apply it — the comment is
  currently a lie about its own spawn, and the fix makes it true.
- The original text's reference to
  [no_inherited_harness_stdio.rs:25-28](../../../crates/cyrup-tools/src/tests/no_inherited_harness_stdio.rs)
  as "states the crate cross-compiles clean for Windows" is a mis-paraphrase; that passage explains
  why the stdio invariant is enforced by a SOURCE SCAN rather than a runtime probe (it must cover
  `#[cfg(windows)]` `taskkill` spawns that cannot execute on the build host). The real
  cross-compilation statement is at
  [shell.rs:163-170](../../../crates/cyrup-tools/src/ops/shell.rs). Both matter here, for different
  reasons — see *Invariants the implementation must not break*.

## Why only the console half of `windowsHide` is implemented — and this is not a shortcut

Node's `windowsHide: true` becomes libuv's `UV_PROCESS_WINDOWS_HIDE`, which is TWO distinct
suppressions:

1. the process-creation flag `CREATE_NO_WINDOW` (`0x0800_0000`), which stops a **console**-subsystem
   child from allocating a console; and
2. `STARTUPINFO.wShowWindow = SW_HIDE` together with `STARTF_USESHOWWINDOW`, which hides the first
   window of a **GUI**-subsystem child.

Half 1 is `std::os::windows::process::CommandExt::creation_flags`, `#[stable(feature =
"windows_process_extensions", since = "1.16.0")]`, and it is a **safe** `fn` taking a `u32`, so the
crate's `#![deny(unsafe_code)]` at [lib.rs:16](../../../crates/cyrup-tools/src/lib.rs) is untouched.

Half 2 is only reachable through `std::os::windows::process::CommandExt::show_window`, which is
`#[unstable(feature = "windows_process_extensions_show_window", issue = "127544")]` — nightly-only,
and therefore not available to this workspace (`rust-version = "1.96"`, stable toolchain). It is not
a choice being deferred; it is not compilable. This is the identical, already-recorded delta carried
by [cyrup-mcp secrets.rs:164-182](../../../crates/cyrup-mcp/src/secrets.rs), and the shells this
crate spawns (`bash.exe`, `sh`, `where.exe`, `taskkill.exe`) are console-subsystem binaries to a
one, so half 1 is the half that governs every call site here.

## Required change

### 1 · New file — `crates/cyrup-tools/src/ops/win.rs`

One definition, one funnel. Create
[crates/cyrup-tools/src/ops/win.rs](../../../crates/cyrup-tools/src/ops/win.rs) with exactly this:

```rust
//! `windowsHide: true` — the console half, for every `std::process::Command` this crate builds
//! whose real Pi counterpart passes the option.
//!
//! Node's `windowsHide: true` lowers to libuv's `UV_PROCESS_WINDOWS_HIDE`, which is TWO
//! suppressions: the creation flag `CREATE_NO_WINDOW` (a CONSOLE-subsystem child never allocates a
//! console) and `STARTUPINFO.wShowWindow = SW_HIDE` + `STARTF_USESHOWWINDOW` (the first window of a
//! GUI-subsystem child). Only the first is reachable from stable Rust:
//! `CommandExt::creation_flags` is stable since 1.16 and safe, while `CommandExt::show_window` is
//! `#[unstable(feature = "windows_process_extensions_show_window", issue = "127544")]`. Every
//! program spawned through this crate (`bash.exe`, `sh`, `where.exe`, `taskkill.exe`) is a
//! console-subsystem binary, so the console half is the half that governs all of them. RECORDED
//! DELTA: a GUI-subsystem child would show its first window here where Pi hides it.
//!
//! NOT applied to `super::local::command::build_argv_command` — see that function's doc comment.

/// `CREATE_NO_WINDOW` (`winbase.h`).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Apply `windowsHide: true`'s console half to `cmd`. A no-op everywhere but Windows, so call sites
/// stay `cfg`-free and cannot drift between platforms.
///
/// `CommandExt::creation_flags` ASSIGNS the flag word (`self.flags = flags` in
/// `std::sys::process::windows`), it does not OR into it — std then ORs in its own
/// `CREATE_UNICODE_ENVIRONMENT` before `CreateProcessW`. Nothing else under
/// `crates/cyrup-tools/**` sets creation flags today; anything that later needs one MUST pass
/// `CREATE_NO_WINDOW | …` in a single call rather than adding a second call, which would silently
/// replace this one.
pub(crate) fn windows_hide(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        let _ = cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}
```

Why crate-local and not a shared helper: the three existing copies of this constant live in
[cyrup-mcp secrets.rs:138-140](../../../crates/cyrup-mcp/src/secrets.rs),
[cyrup-intercom spawn.rs:185-187](../../../crates/cyrup-intercom/src/transport/spawn.rs) and
[cyrup-ext-subagents spawn_detached.rs:91-107](../../../crates/cyrup-ext-subagents/src/background/spawn_detached.rs),
none of which `cyrup-tools` depends on — its only runtime workspace dependency is `cyrup-core`
([Cargo.toml](../../../crates/cyrup-tools/Cargo.toml)), and `cyrup-core` is deliberately
process-free. Inverting a crate dependency to share a `u32` is the wrong trade; one funnel inside
this crate is the right one.

### 2 · `crates/cyrup-tools/src/ops/mod.rs` — declare the module

Current, at [ops/mod.rs:9-10](../../../crates/cyrup-tools/src/ops/mod.rs):

```rust
pub mod local;
pub mod shell;
```

Replacement:

```rust
pub mod local;
pub mod shell;
pub(crate) mod win;
```

`ops` is the common ancestor of both consumers (`ops::shell` and `ops::local::{command, signal}`).
Nothing is re-exported from [lib.rs](../../../crates/cyrup-tools/src/lib.rs) — this is internal.

### 3 · `crates/cyrup-tools/src/ops/local/command.rs` — the `bash` path gets it, the argv path does not

In `build_command`, current tail
([command.rs:37-51](../../../crates/cyrup-tools/src/ops/local/command.rs)):

```rust
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid` only detaches the child into its own session/process group before exec;
        // it touches no parent memory and is async-signal-safe. This makes the child the group
        // leader (pgid == pid) so the whole tree can be killed via `killpg` (R-03-027).
        unsafe {
            std_cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    std_cmd
}
```

Replacement:

```rust
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid` only detaches the child into its own session/process group before exec;
        // it touches no parent memory and is async-signal-safe. This makes the child the group
        // leader (pgid == pid) so the whole tree can be killed via `killpg` (R-03-027).
        unsafe {
            std_cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    // The Windows counterpart of the `setsid` block above, and the exact partner of the
    // `detached: process.platform !== "win32"` on the same spawn: Pi passes `windowsHide: true`
    // (bash.ts:104) for BOTH shell tools, so `bash -c` / `bash -s` never flashes a console.
    // `creation_flags` is safe and stable, so this adds no `unsafe` beyond the block above.
    windows_hide(&mut std_cmd);
    std_cmd
}
```

and extend the import at [command.rs:10](../../../crates/cyrup-tools/src/ops/local/command.rs):

```rust
use crate::ops::win::windows_hide;
use crate::ops::{ArgvSpec, ExecSpec, Transport};
```

`build_argv_command` gets **no** call, and gets a comment saying so, appended to the existing
"deliberately NO `setsid`" note at
[command.rs:87-91](../../../crates/cyrup-tools/src/ops/local/command.rs):

```rust
    // Deliberately NO `setsid`/process-group setup here — Pi's real `execCommand` spawn
    // (`exec.ts:41-45`) never sets `detached`, so the child stays in the caller's own process
    // group and must be signaled by single pid only, never `killpg` (see the doc comment above and
    // `super::proc`'s module doc).
    //
    // Deliberately NO `windows_hide` either, for the same 1:1-with-its-own-consumer reason: that
    // same `exec.ts:41-45` spawn passes no `windowsHide`, and Node's default is `false`, so this
    // child SHOWS a console window upstream. Adding the flag here would be a behavior Pi does not
    // have. Contrast `build_command` above, whose consumer (`bash.ts:104`) does pass it.
    std_cmd
}
```

### 4 · `crates/cyrup-tools/src/ops/shell.rs` — the `where bash.exe` probe

Current, at [shell.rs:88-94](../../../crates/cyrup-tools/src/ops/shell.rs):

```rust
    let mut child = std::process::Command::new(cmd)
        .arg(arg)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
```

Replacement — the chain is broken into a binding so the flag can be applied before `spawn`, and the
`Command::new(` / `.stdin(` / `.stdout(` / `.stderr(` tokens all stay on their own lines within one
window (see *Invariants* below):

```rust
    let mut probe = std::process::Command::new(cmd);
    probe
        .arg(arg)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    // Pi's win32 probe is `spawnSync("where", [executable], { …, windowsHide: true })`
    // (shell.ts:28-32); its unix `which` arm (shell.ts:47) passes no such option, which is exactly
    // what `windows_hide` compiles to off Windows. Shell detection runs during session
    // construction, so without this a console flashes before the agent has printed anything.
    crate::ops::win::windows_hide(&mut probe);
    let mut child = probe.spawn().ok()?;
```

Everything below (the `try_wait` deadline loop, `child.stdout.take()`, the `#[cfg(not(unix))]`
`path.exists()` re-check) is unchanged and still operates on `child`.

### 5 · `crates/cyrup-tools/src/ops/local/signal.rs` — all three `taskkill` spawns

Apply the helper at each of the three `#[cfg(not(unix))]` arms. Pi's `killProcessTree` — the
counterpart of the first two — passes `windowsHide: true` (`shell.ts:226`), and the comment at
[signal.rs:39-41](../../../crates/cyrup-tools/src/ops/local/signal.rs) already claims it.

`kill_process_tree`, current [signal.rs:37-48](../../../crates/cyrup-tools/src/ops/local/signal.rs):

```rust
    #[cfg(not(unix))]
    {
        // Pi's win32 arm is a fire-and-forget `spawn("taskkill", ["/F","/T","/PID", …], {stdio:
        // "ignore", detached: true, windowsHide: true})` (`shell.ts:203-212`) — NOT a blocking
        // wait, which matters because this runs inside a signal handler.
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
```

Replacement:

```rust
    #[cfg(not(unix))]
    {
        // Pi's win32 arm is a fire-and-forget `spawn("taskkill", ["/F","/T","/PID", …], {stdio:
        // "ignore", detached: true, windowsHide: true})` (`shell.ts:220-228`) — NOT a blocking
        // wait, which matters because this runs inside a signal handler.
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // The `windowsHide: true` the comment above has always cited. This drain runs from the
        // shutdown signal handler, so a console window per tracked child is the worst possible
        // moment for one.
        crate::ops::win::windows_hide(&mut cmd);
        let _ = cmd.spawn();
    }
```

`send_sigkill_tree`, current [signal.rs:72-79](../../../crates/cyrup-tools/src/ops/local/signal.rs):

```rust
    #[cfg(not(unix))]
    {
        if let Some(pid) = child.id() {
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output();
        }
    }
```

Replacement (still `.output()`, which pins all three stdio handles by construction):

```rust
    #[cfg(not(unix))]
    {
        if let Some(pid) = child.id() {
            let mut cmd = std::process::Command::new("taskkill");
            cmd.args(["/F", "/T", "/PID", &pid.to_string()]);
            // `killProcessTree`'s `windowsHide: true` (shell.ts:226) — this fires on every cancel
            // and every bash timeout, i.e. on the hot path the user actually watches.
            crate::ops::win::windows_hide(&mut cmd);
            let _ = cmd.output();
        }
    }
```

`kill_pid`, current [signal.rs:142-148](../../../crates/cyrup-tools/src/ops/local/signal.rs):

```rust
    #[cfg(not(unix))]
    {
        std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output()?;
        Ok(())
    }
```

Replacement:

```rust
    #[cfg(not(unix))]
    {
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/F", "/PID", &pid.to_string()]);
        // No direct Pi counterpart — upstream's single-pid kill is `proc.kill("SIGKILL")`
        // (exec.ts:59), and this `taskkill /F /PID` exists only because Windows has no such
        // primitive for a pid we do not hold a `Child` for. It is a port-side spawn standing in for
        // a Pi call that spawns NOTHING, so it must be at least as invisible as what it replaces;
        // every `taskkill` Pi does spawn carries `windowsHide: true` (shell.ts:226).
        crate::ops::win::windows_hide(&mut cmd);
        cmd.output()?;
        Ok(())
    }
```

## Concurrency notes

Nothing in this change is async or shared-state. `windows_hide` takes `&mut std::process::Command`
by unique borrow and returns before the command is ever moved into `tokio::process::Command`, so it
cannot race the tokio reactor; the async `select!` loops in
[proc.rs:176-250](../../../crates/cyrup-tools/src/ops/local/proc.rs) and the `KillTreeOnDrop` guard
armed at [proc.rs:136](../../../crates/cyrup-tools/src/ops/local/proc.rs) are untouched. The two
`signal.rs` sites reached from the shutdown signal handler keep their existing shapes exactly —
`kill_process_tree` stays a non-blocking fire-and-forget `spawn()` (never `output()`, which would
block a signal handler), and `send_sigkill_tree` stays a blocking `output()` on the async
cancel/timeout path where blocking briefly is already the accepted behaviour.

## Explicitly out of scope

Do not "fix" the bare `"taskkill"` program name into Pi's
`join(process.env.SystemRoot ?? "C:\\Windows", "System32", "taskkill.exe")`
([shell.ts:221](../../../tmp/pi/packages/coding-agent/src/utils/shell.ts)). That is a separate
PATH-trust parity gap with a separate risk profile; folding it in here mixes two changes in one file.
Do not add `DETACHED_PROCESS` or `CREATE_NEW_PROCESS_GROUP` — Pi's shell spawn is explicitly
`detached: process.platform !== "win32"`, i.e. NOT detached on Windows, so this crate must not
detach there either.

## Invariants the implementation must not break

1. **The stdio source-scan audit.** [`no_inherited_harness_stdio.rs`](../../../crates/cyrup-tools/src/tests/no_inherited_harness_stdio.rs)
   scans every `.rs` under `crates/cyrup-tools/{src,tests}` for the literal token `Command::new(`
   and requires that the following window (bounded by the next `Command::new(`, or 60 lines) either
   names all three of `.stdin(` / `.stdout(` / `.stderr(` or contains `.output()`; it also requires
   at least 6 matched sites. This is why every call site above KEEPS its literal
   `std::process::Command::new(...)` and why the flag is applied through a `&mut Command` helper
   rather than by funnelling construction through a `win::command(program)` constructor — that
   refactor would erase the tokens the audit counts.
2. **`#![deny(unsafe_code)]`** at [lib.rs:16](../../../crates/cyrup-tools/src/lib.rs). `creation_flags`
   is a safe fn; no new `unsafe` block and no new `#[allow(unsafe_code)]` is introduced. The existing
   `#[allow(unsafe_code)]` on `build_command` stays exactly as it is, covering only `pre_exec`.
3. **`cfg(windows)` vs `cfg(not(unix))`.** The crate's existing arms select the `where`/`taskkill`
   programs with `#[cfg(not(unix))]`, a strictly wider predicate than `windows`. `windows_hide` is
   internally gated on `cfg(windows)` precisely so those wider arms can call it unconditionally
   without breaking a hypothetical non-unix, non-Windows target.
4. **Windows cross-compilation stays green.** Per
   [shell.rs:163-170](../../../crates/cyrup-tools/src/ops/shell.rs) this crate is expected to compile
   for `x86_64-pc-windows-gnu`; the `#[cfg(windows)]` body of `windows_hide` is the only code in this
   change compiled exclusively there, so it must be written correctly the first time —
   `use std::os::windows::process::CommandExt as _;` inside the block, `let _ =` on the returned
   `&mut Command`, and the const declared under the same `cfg` so it is not dead code elsewhere.

## Definition of done

1. On a Windows host, running a `bash` tool command spawns the shell with `CREATE_NO_WINDOW` set: no
   console window appears or flashes for the duration of the command, and this holds for both
   transports — argv (`bash -c <command>`) and the WSL-legacy stdin transport (`bash -s`).
2. Session construction on a Windows host resolves the shell without a console window appearing for
   the `where bash.exe` probe.
3. Cancelling or timing out a `bash` command on a Windows host, and the shutdown drain of tracked
   detached children, terminate the process tree without any `taskkill` console window appearing.
4. `LocalProc::exec_argv` (the direct-argv `exec` capability path) is spawned WITHOUT the flag,
   matching `exec.ts:41-45`, which passes no `windowsHide`.
5. On unix the observable behaviour is byte-for-byte unchanged: same program, args, cwd, env ordering
   (removals then overrides), same three stdio handles, same `setsid` process group, same
   `killpg`-based escalation, and the same `which bash` probe.
6. Exactly one definition of the `CREATE_NO_WINDOW` constant exists under `crates/cyrup-tools/**`, in
   `ops/win.rs`, and every spawn in the crate whose Pi counterpart passes `windowsHide: true` routes
   through `windows_hide`.
7. `crates/cyrup-core` is unmodified.
