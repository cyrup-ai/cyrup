---
title: On Windows with no bash installed, pi's actionable No bash shell found error is replaced by an opaque spawn failure
priority: LOW
tool: bash
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: qa
status: completed
updated: 2026-08-27 14:06
---

# On Windows with no bash installed, pi's actionable No bash shell found error is replaced by an opaque spawn failure

## Core objective

On a host where no bash exists, **every** bash invocation must fail with pi's three-option repair
recipe plus the searched-candidates list, delivered **as the tool error the model reads**, and no
cyrup code path may silently substitute a bare `bash -c` in its place.

The message, the candidate list and the Windows ordering are already written and already correct in
Rust ([shell.rs:169-191](../../../crates/cyrup-tools/src/ops/shell.rs)). The defect is purely
*reachability*: the single infallible entry point
[`ShellConfig::detect()`](../../../crates/cyrup-tools/src/ops/shell.rs) swallows that error, and
every production construction site calls it. This task deletes the swallow and moves shell
resolution to the moment of execution, where pi does it.

---

## What pi does — verified

`createShellToolDefinition` builds the tool with
`options?.operations ?? createLocalBashOperations({ shellPath: options?.shellPath })`
([pi bash.ts:343](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts)). That constructor
resolves **nothing** — it hands `createLocalShellOperations` a *thunk*:

```ts
// pi bash.ts:158-160
export function createLocalBashOperations(options?: { shellPath?: string }): BashOperations {
	return createLocalShellOperations("bash", () => getShellConfig(options?.shellPath));
}
```

The thunk is invoked **inside `exec`**, after `resolveTimeoutMs` and the abort check:

```ts
// pi bash.ts:84-91
export function createLocalShellOperations(shellName: string, resolveShellConfig: () => ShellConfig): BashOperations {
	return {
		exec: async (command, cwd, { onData, signal, timeout, env }) => {
			const timeoutMs = resolveTimeoutMs(timeout);
			if (signal?.aborted) {
				throw new Error("aborted");
			}
			const shellConfig = resolveShellConfig();
```

`getShellConfig` throws on the Windows no-bash arm
([pi shell.ts:100-106](../../../tmp/pi/packages/coding-agent/src/utils/shell.ts)):

```ts
throw new Error(
	`No bash shell found. Options:\n` +
		`  1. Install Git for Windows: https://git-scm.com/download/win\n` +
		`  2. Add your bash to PATH (Cygwin, MSYS2, etc.)\n` +
		"  3. Set shellPath in settings.json\n\n" +
		`Searched Git Bash in:\n${paths.map((p) => `  ${p}`).join("\n")}`,
);
```

and the tool's inner catch re-throws anything that is neither `"aborted"` nor `"timeout:"`
**verbatim**, with no status suffix appended
([pi bash.ts:458-468](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts)):

```ts
} catch (err) {
	const snapshot = await finishOutput();
	const { text } = formatOutput(snapshot, "");
	if (err instanceof Error && err.message === "aborted") { … }
	if (err instanceof Error && err.message.startsWith("timeout:")) { … }
	throw err;                      // ← the No-bash recipe reaches the model unmodified
}
```

Two consequences that fix the design of this task:

1. The recipe is the **tool result text** the model reads, on every bash call.
2. Pi's **session construction never fails** because of a missing bash.
   `createAllToolDefinitions` ([pi index.ts:182](../../../tmp/pi/packages/coding-agent/src/core/tools/index.ts))
   calls `getShellConfig` nowhere; the only call in the whole bash path is the one inside `exec`
   quoted above. A pi user on a bash-less Windows box still starts a session and still has
   `read`/`edit`/`write`/`grep`/`find`/`ls`.

---

## What cyrup-tools does today

`ShellConfig::detect()` is the swallow — it converts the fully-formed recipe into a bare `bash -c`
([shell.rs:228-250](../../../crates/cyrup-tools/src/ops/shell.rs)):

```rust
    pub fn detect() -> Self {
        Self::try_detect().unwrap_or_else(|_| ShellConfig {
            program: PathBuf::from("bash"),
            args: vec!["-c".to_string()],
            transport: Transport::Argv,
        })
    }
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self::detect()
    }
}
```

Every production shell comes from it:

| site | code | consequence |
|---|---|---|
| [registry.rs:57](../../../crates/cyrup-tools/src/registry.rs) | `let shell = ShellConfig::detect();` | baked into `BashTool` |
| [builder.rs:843-844](../../../crates/cyrup-session-svc/src/builder.rs) | `let shell = ShellConfig::detect(); let base = Backend::local(shell.clone());` | baked into `LocalProc` **and** into `SessionExtras.shell` |
| [ops/mod.rs:511](../../../crates/cyrup-tools/src/ops/mod.rs) | `LocalProc::new(ShellConfig::detect())` | `LocalBashOperations::new` backend |
| [ops/mod.rs:579](../../../crates/cyrup-tools/src/ops/mod.rs) | `Self::local(ShellConfig::detect())` | `Backend::default` |

`BashTool::execute` then only ever reaches the fallible path when a `shellPath` setting is present
([bash.rs:302-305](../../../crates/cyrup-tools/src/tools/bash.rs)):

```rust
        let shell = match self.opts.shell_path.as_deref() {
            Some(p) => ShellConfig::resolve(Some(p))?,
            None => self.shell.clone(),
        };
```

The immediate-bash (`/bash`, RPC `executeBash`) seam has the identical shape
([session/bash.rs:93-101](../../../crates/cyrup-session-svc/src/session/bash.rs)):

```rust
        let shell = match self.shell_path.as_deref() {
            Some(p) => match ShellConfig::resolve(Some(p)) {
                Ok(shell) => shell,
                Err(e) => return Err(e.into()),
            },
            None => self.shell.clone(),
        };
```

So with no `shellPath` the degraded `bash -c` reaches the spawn, and what the model gets is
[proc.rs:130-132](../../../crates/cyrup-tools/src/ops/local/proc.rs):

```rust
        let mut child = cmd
            .spawn()
            .map_err(|e| error::io(&format!("spawn {}", error::show(&spec.shell.program)), &e))?;
```

i.e. `spawn bash: … (os error 2)` — no recipe, no candidate list, no remedy.

---

## Where the error must surface — and why not at construction

**Decision: the error must surface inside `BashTool::execute` and inside
`AgentSession::execute_bash`, as the returned `ToolError`. `ToolRegistry::with_builtins` keeps its
`-> Self` signature and stops resolving a shell at all.**

Propagating from the construction sites — turning `with_builtins` into
`Result<Self, ToolError>` — is the wrong answer for three verified reasons:

1. **It is behaviour pi does not have.** Pi resolves no shell at tool-definition time
   ([pi index.ts:182](../../../tmp/pi/packages/coding-agent/src/core/tools/index.ts),
   [pi bash.ts:343](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts)); a bash-less host
   still gets a working session. Failing session construction is a redesign, and this is a parity
   task.
2. **The model would never see the message.** The objective is that the recipe arrives as a *tool
   result*. A `with_builtins` error aborts before any model turn exists.
3. **It poisons bash-free tool sets.** [`read_only_tools`](../../../crates/cyrup-tools/src/registry.rs)
   (registry.rs:154-161) and [`coding_tools`](../../../crates/cyrup-tools/src/registry.rs)
   (registry.rs:144-151) both go through `with_builtins`; `read_only_tools` returns
   `{read, grep, find, ls}` — no bash at all — yet would fail to construct on a bash-less host.

The cascade that is thereby avoided is real and worth recording: `with_builtins -> Result` forces
`?`/`unwrap` at [registry.rs:145](../../../crates/cyrup-tools/src/registry.rs),
[:155](../../../crates/cyrup-tools/src/registry.rs), [:165](../../../crates/cyrup-tools/src/registry.rs),
makes `coding_tools`/`read_only_tools`/`all_tools` fallible, and pushes a `ToolError` into
`SessionBuilder::build`'s error type at
[builder.rs:885](../../../crates/cyrup-session-svc/src/builder.rs). None of that happens under the
prescribed path.

### Relationship to the sibling task

[LOW-shell-detection-is-cached-at-tool-construction-instead-of-re-resolved-on.md](./LOW-shell-detection-is-cached-at-tool-construction-instead-of-re-resolved-on.md)
prescribes re-resolving the shell per command at
[bash.rs:302-305](../../../crates/cyrup-tools/src/tools/bash.rs). This task's edit at that same seam
is **the identical replacement line** — `ShellConfig::resolve(self.opts.shell_path.as_deref())?` —
so the two tasks converge rather than conflict; whichever lands second finds the line already
written and only has to verify it. Everything else below (deleting `detect()`, the `LocalProc`
field, the `SessionExtras.shell`/`AgentSession.shell` fields, and the four construction sites) is
owned solely by this task.

---

## Required changes

### 1. `crates/cyrup-tools/src/ops/shell.rs` — delete the swallow

Delete `ShellConfig::detect()` and `impl Default for ShellConfig` outright
([shell.rs:228-250](../../../crates/cyrup-tools/src/ops/shell.rs)). Nothing calls
`ShellConfig::default()` anywhere in the workspace, and after changes 2-7 nothing calls `detect()`
either. Removing it is what makes the swallow *unwritable* rather than merely unwritten.

CURRENT:

```rust
    /// Infallible detection for the `Default` impls and for embedders that cannot propagate an
    /// error. Prefer [`ShellConfig::try_detect`] at every real construction site so a Windows box
    /// with no bash reports Pi's `No bash shell found` at session construction.
    ///
    /// [CYRUP-DELTA] Pi has no infallible entry point at all — `getShellConfig` throws
    /// (shell.ts:100-106) and every caller lives with that. Rust's `Default` cannot, so the
    /// unreachable-on-unix arm degrades to a bare `bash -c`: still bash, never a *different*
    /// interpreter, and it fails loudly at spawn ("program not found") rather than silently
    /// executing bash text under `cmd.exe` (ADR-0003 D4's implementation note).
    pub fn detect() -> Self {
        Self::try_detect().unwrap_or_else(|_| ShellConfig {
            program: PathBuf::from("bash"),
            args: vec!["-c".to_string()],
            transport: Transport::Argv,
        })
    }
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self::detect()
    }
}
```

REPLACEMENT — the whole block above is removed; the `impl ShellConfig` block now ends after
`try_detect`, and a note replaces the deleted doc so the deletion is not re-litigated:

```rust
    // NOTE: there is deliberately NO infallible `detect()` and no `Default for ShellConfig`.
    // Pi has no infallible entry point either — `getShellConfig` throws (shell.ts:100-106) and
    // every caller lives with that. An infallible wrapper can only degrade to a bare `bash -c`,
    // which turns Pi's actionable `No bash shell found` recipe into `spawn bash: … (os error 2)`
    // at the spawn site (ops/local/proc.rs). Resolve through `try_detect`/`resolve` at the point
    // of USE, where the error is the tool result the model reads (Pi bash.ts:91,457-468).
}
```

The message itself, `windows_detect_from`
([shell.rs:169-191](../../../crates/cyrup-tools/src/ops/shell.rs)), `find_bash_on_path`
([shell.rs:79-127](../../../crates/cyrup-tools/src/ops/shell.rs)), the 5 s probe bound
([shell.rs:70](../../../crates/cyrup-tools/src/ops/shell.rs)),
`try_detect` ([shell.rs:194-226](../../../crates/cyrup-tools/src/ops/shell.rs)) and
`resolve` ([shell.rs:142-150](../../../crates/cyrup-tools/src/ops/shell.rs)) are **unchanged** —
they are already byte-correct against pi. For reference, this is the text that must reach the
model verbatim (already produced by `windows_detect_from`, do not retype it):

```rust
        // shell.ts:100-106 verbatim, including the `  ${p}`-indented searched-path list.
        let searched = candidates
            .iter()
            .map(|p| format!("  {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n");
        Err(ToolError::new(format!(
            "No bash shell found. Options:\n  1. Install Git for Windows: \
             https://git-scm.com/download/win\n  2. Add your bash to PATH (Cygwin, MSYS2, \
             etc.)\n  3. Set shellPath in settings.json\n\nSearched Git Bash in:\n{searched}"
        )))
```

### 2. `crates/cyrup-tools/src/ops/local/proc.rs` — resolve the backend fallback lazily and fallibly

`LocalProc` caches a `ShellConfig` that is consulted only when an `ExecSpec` arrives with an empty
program. That cached value is the last place a degraded `bash -c` can hide. Drop the field and
resolve at the moment the fallback is actually needed, so a third-party `ExecSpec` producer gets
pi's error too.

CURRENT ([proc.rs:47-64](../../../crates/cyrup-tools/src/ops/local/proc.rs)):

```rust
/// Local process operations.
pub struct LocalProc {
    shell: ShellConfig,
    /// SIGTERM→SIGKILL grace period; overridable ONLY for tests ([`Self::with_kill_grace`]) so the
    /// escalation path is exercisable without a real test waiting 5+ real seconds — production
    /// always gets Pi's real 5s via [`Self::new`].
    kill_grace: Duration,
}

impl LocalProc {
    pub fn new(shell: ShellConfig) -> Self {
        Self::with_kill_grace(shell, DEFAULT_KILL_GRACE)
    }

    /// Build with a caller-supplied SIGTERM→SIGKILL grace period (tests only).
    pub fn with_kill_grace(shell: ShellConfig, kill_grace: Duration) -> Self {
        Self { shell, kill_grace }
    }
}
```

REPLACEMENT:

```rust
/// Local process operations.
///
/// Holds NO cached [`ShellConfig`]. An [`ExecSpec`] always carries the shell its producer resolved
/// (Pi resolves per `exec`, bash.ts:91); a spec that arrives with an empty program is resolved
/// here, fallibly, so a host with no bash reports Pi's `No bash shell found` recipe rather than
/// degrading to a bare `bash -c` and failing at spawn.
pub struct LocalProc {
    /// SIGTERM→SIGKILL grace period; overridable ONLY for tests ([`Self::with_kill_grace`]) so the
    /// escalation path is exercisable without a real test waiting 5+ real seconds — production
    /// always gets Pi's real 5s via [`Self::new`].
    kill_grace: Duration,
}

impl LocalProc {
    pub fn new() -> Self {
        Self::with_kill_grace(DEFAULT_KILL_GRACE)
    }

    /// Build with a caller-supplied SIGTERM→SIGKILL grace period (tests only).
    pub fn with_kill_grace(kill_grace: Duration) -> Self {
        Self { kill_grace }
    }
}

impl Default for LocalProc {
    fn default() -> Self {
        Self::new()
    }
}
```

The `Default` impl is mandatory, not decorative: `clippy::new_without_default` fires the moment
`new()` loses its argument, and this crate denies its lints.

CURRENT ([proc.rs:99-101](../../../crates/cyrup-tools/src/ops/local/proc.rs)):

```rust
        if spec.shell.program.as_os_str().is_empty() {
            spec.shell = self.shell.clone();
        }
```

REPLACEMENT:

```rust
        // A spec with no shell means "use the platform default", which is exactly Pi's
        // `getShellConfig(undefined)` (shell.ts:76-119). Resolve it HERE, per call and fallibly:
        // on a host with no bash this returns Pi's `No bash shell found` recipe (shell.ts:100-106)
        // and it becomes the caller's error, instead of a degraded `bash -c` that fails below as
        // `spawn bash: … (os error 2)`.
        if spec.shell.program.as_os_str().is_empty() {
            spec.shell = ShellConfig::try_detect()?;
        }
```

`ShellConfig` is already imported at
[proc.rs:26](../../../crates/cyrup-tools/src/ops/local/proc.rs); keep the import.

### 3. `crates/cyrup-tools/src/ops/mod.rs` — drop the shell from the backend constructors

CURRENT ([ops/mod.rs:509-514](../../../crates/cyrup-tools/src/ops/mod.rs)):

```rust
    pub fn new(shell_path: Option<String>) -> Self {
        Self {
            proc: Arc::new(local::LocalProc::new(ShellConfig::detect())),
            shell_path,
        }
    }
```

REPLACEMENT:

```rust
    pub fn new(shell_path: Option<String>) -> Self {
        Self {
            proc: Arc::new(local::LocalProc::new()),
            shell_path,
        }
    }
```

CURRENT ([ops/mod.rs:567-581](../../../crates/cyrup-tools/src/ops/mod.rs)):

```rust
impl Backend {
    /// The default local backend over tokio fs/process with the given shell.
    pub fn local(shell: ShellConfig) -> Self {
        Self {
            fs: Arc::new(local::LocalFs),
            proc: Arc::new(local::LocalProc::new(shell)),
        }
    }
}

impl Default for Backend {
    fn default() -> Self {
        Self::local(ShellConfig::detect())
    }
}
```

REPLACEMENT:

```rust
impl Backend {
    /// The default local backend over tokio fs/process. No shell is baked in — every `bash` seam
    /// resolves its own per call (Pi bash.ts:91), and [`local::LocalProc`] resolves the platform
    /// default itself for a spec that carries none.
    pub fn local() -> Self {
        Self {
            fs: Arc::new(local::LocalFs),
            proc: Arc::new(local::LocalProc::new()),
        }
    }
}

impl Default for Backend {
    fn default() -> Self {
        Self::local()
    }
}
```

`LocalBashOperations::exec` ([ops/mod.rs:535](../../../crates/cyrup-tools/src/ops/mod.rs)) already
calls `ShellConfig::resolve(self.shell_path.as_deref())?` per call and is left untouched — it is the
shape the other two seams are being brought into line with.

### 4. `crates/cyrup-tools/src/tools/bash.rs` — `BashTool` stops carrying a shell

CURRENT ([bash.rs:53-63](../../../crates/cyrup-tools/src/tools/bash.rs)):

```rust
pub struct BashTool {
    proc: Arc<dyn ProcOps>,
    shell: ShellConfig,
    cwd: PathBuf,
    opts: BashOpts,
    params: serde_json::Value,
}

impl BashTool {
    pub fn new(proc: Arc<dyn ProcOps>, shell: ShellConfig, cwd: PathBuf, opts: BashOpts) -> Self {
```

REPLACEMENT — the field and the parameter both go; `opts.shell_path` is the only shell input the
tool needs, exactly as pi's closure captures only `options?.shellPath`
([pi bash.ts:158-159](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts)):

```rust
/// Carries NO resolved [`ShellConfig`]. Pi's `createLocalBashOperations` captures only
/// `options?.shellPath` (bash.ts:158-159) and calls `getShellConfig` inside every `exec`
/// (bash.ts:91), so the shell — and any resolution error — belongs to the CALL, not to the tool.
pub struct BashTool {
    proc: Arc<dyn ProcOps>,
    cwd: PathBuf,
    opts: BashOpts,
    params: serde_json::Value,
}

impl BashTool {
    pub fn new(proc: Arc<dyn ProcOps>, cwd: PathBuf, opts: BashOpts) -> Self {
```

and the struct literal at the end of `new` drops its `shell,` line:

```rust
        Self {
            proc,
            cwd,
            opts,
            params,
        }
```

CURRENT ([bash.rs:296-305](../../../crates/cyrup-tools/src/tools/bash.rs)):

```rust
        // Resolve the shell per-exec, honoring an explicit settings `shellPath` (Pi's
        // `createLocalBashOperations` calls `getShellConfig(shellPath)` inside `exec`, AFTER
        // `resolveTimeoutMs` and the abort check, bash.ts:85-89); a missing custom path surfaces
        // as the `Custom shell path not found` error only after the initial empty update, the
        // timeout validation, AND the abort check have already happened, exactly like Pi.
        let shell = match self.opts.shell_path.as_deref() {
            Some(p) => ShellConfig::resolve(Some(p))?,
            None => self.shell.clone(),
        };
```

REPLACEMENT:

```rust
        // Resolve the shell per-exec (Pi's `createLocalBashOperations` calls
        // `getShellConfig(shellPath)` inside `exec`, AFTER `resolveTimeoutMs` and the abort check,
        // bash.ts:87-91). BOTH of Pi's resolution errors reach the model here as the tool result,
        // after the initial empty update, the timeout validation and the abort check, exactly like
        // Pi: `Custom shell path not found: …` when `shellPath` is set and missing (shell.ts:73),
        // and the three-option `No bash shell found. Options: …` recipe with its `Searched Git Bash
        // in:` list when it is unset and no bash exists (shell.ts:100-106). Pi's inner catch
        // re-throws both verbatim — neither is an `"aborted"` nor a `"timeout:"` message, so it
        // falls to `throw err` (bash.ts:468) with NO status appended — and so does this `?`.
        let shell = ShellConfig::resolve(self.opts.shell_path.as_deref())?;
```

That one line is the whole point of the task: `resolve(None)` forwards to `try_detect()`
([shell.rs:149](../../../crates/cyrup-tools/src/ops/shell.rs)), whose Windows arm is
`windows_detect_from` ([shell.rs:169-191](../../../crates/cyrup-tools/src/ops/shell.rs)).

### 5. `crates/cyrup-tools/src/registry.rs` — resolve nothing at construction

CURRENT ([registry.rs:53-57](../../../crates/cyrup-tools/src/registry.rs)):

```rust
    /// Build the default registry with the seven built-ins over `backend` (arch-03 §3.4).
    pub fn with_builtins(cwd: PathBuf, backend: Backend, opts: ToolsOptions) -> Self {
        let mut reg = Self::new();
        let locks = Arc::new(FileMutationLocks::new());
        let shell = ShellConfig::detect();
```

REPLACEMENT — signature unchanged (`-> Self`), the `detect()` line simply deleted:

```rust
    /// Build the default registry with the seven built-ins over `backend` (arch-03 §3.4).
    ///
    /// Resolves NO shell. Pi's `createAllToolDefinitions` (index.ts:182) does not either — the only
    /// `getShellConfig` call on the bash path is inside `exec` (bash.ts:91) — so a host with no
    /// bash still gets a working registry, and its `No bash shell found` recipe arrives as the
    /// `bash` TOOL RESULT rather than aborting session construction. Making this fallible would
    /// also break `read_only_tools`, which contains no bash tool at all.
    pub fn with_builtins(cwd: PathBuf, backend: Backend, opts: ToolsOptions) -> Self {
        let mut reg = Self::new();
        let locks = Arc::new(FileMutationLocks::new());
```

CURRENT ([registry.rs:70-75](../../../crates/cyrup-tools/src/registry.rs)):

```rust
        reg.insert(Arc::new(BashTool::new(
            backend.proc.clone(),
            shell,
            cwd.clone(),
            opts.bash,
        )));
```

REPLACEMENT:

```rust
        reg.insert(Arc::new(BashTool::new(
            backend.proc.clone(),
            cwd.clone(),
            opts.bash,
        )));
```

Drop `ShellConfig` from the `use crate::ops::{Backend, ShellConfig};` import at
[registry.rs:6](../../../crates/cyrup-tools/src/registry.rs), leaving `use crate::ops::Backend;`.

### 6. `crates/cyrup-session-svc/src/builder.rs` — stop detecting at session build

CURRENT ([builder.rs:841-844](../../../crates/cyrup-session-svc/src/builder.rs)):

```rust
        let shell_path_setting = settings.effective().shell_path();
        let shell_command_prefix_setting = settings.effective().shell_command_prefix();
        let shell = ShellConfig::detect();
        let base = Backend::local(shell.clone());
```

REPLACEMENT:

```rust
        let shell_path_setting = settings.effective().shell_path();
        let shell_command_prefix_setting = settings.effective().shell_command_prefix();
        // No shell is resolved at session build. Pi resolves inside every `exec` (bash.ts:91) and
        // its session start never fails on a bash-less host; both cyrup bash seams now do the same,
        // so a missing bash surfaces as Pi's `No bash shell found` recipe on the command that
        // needed it, not as a failed session.
        let base = Backend::local();
```

CURRENT ([builder.rs:1755-1758](../../../crates/cyrup-session-svc/src/builder.rs)):

```rust
            proc: bash_proc,
            shell,
            shell_path: shell_path_setting,
            shell_command_prefix: shell_command_prefix_setting,
```

REPLACEMENT:

```rust
            proc: bash_proc,
            shell_path: shell_path_setting,
            shell_command_prefix: shell_command_prefix_setting,
```

Remove `ShellConfig` from the file's imports if it becomes unused.

### 7. `crates/cyrup-session-svc/src/session/mod.rs` — delete the cached session shell

Three deletions, each a single line:

- `SessionExtras`: remove `pub shell: ShellConfig,`
  ([session/mod.rs:99](../../../crates/cyrup-session-svc/src/session/mod.rs)).
- `AgentSession`: remove `shell: ShellConfig,`
  ([session/mod.rs:245](../../../crates/cyrup-session-svc/src/session/mod.rs)).
- The constructor: remove `shell: extras.shell,`
  ([session/mod.rs:370](../../../crates/cyrup-session-svc/src/session/mod.rs)).

`self.shell` has exactly one reader in the whole crate —
[session/bash.rs:99](../../../crates/cyrup-session-svc/src/session/bash.rs) — which change 8
removes, so nothing else is affected. `SessionExtras` is constructed in exactly one place,
[builder.rs:1740](../../../crates/cyrup-session-svc/src/builder.rs), already handled by change 6.
Remove `ShellConfig` from this file's imports if it becomes unused.

### 8. `crates/cyrup-session-svc/src/session/bash.rs` — the immediate-bash seam resolves the same way

Pi's `executeBash` reaches the *same* `createLocalBashOperations({ shellPath })`
([pi bash.ts:158-159](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts)), so `/bash` and
the RPC `executeBash` must produce the identical recipe.

CURRENT ([session/bash.rs:88-101](../../../crates/cyrup-session-svc/src/session/bash.rs)):

```rust
        // Resolve the shell fresh on THIS call, honoring a custom `shellPath` setting (Pi's
        // `createLocalBashOperations({ shellPath })` resolves `getShellConfig(shellPath)` inside
        // `exec` on every `executeBash` invocation — bash.ts:69/89 — never baked in once at session
        // build time); a missing custom path surfaces the same `Custom shell path not found` error
        // as the agent-loop `bash` tool (`cyrup-tools/src/tools/bash.rs:108-111`).
        let shell = match self.shell_path.as_deref() {
            Some(p) => match ShellConfig::resolve(Some(p)) {
                Ok(shell) => shell,
                // `_bash_guard` performs pi's `finally` removal on this path too.
                Err(e) => return Err(e.into()),
            },
            None => self.shell.clone(),
        };
```

REPLACEMENT:

```rust
        // Resolve the shell fresh on THIS call (Pi's `createLocalBashOperations({ shellPath })`
        // resolves `getShellConfig(shellPath)` inside `exec` on every `executeBash` invocation —
        // bash.ts:91,159 — never baked in once at session build time). BOTH of Pi's errors surface
        // here, identically to the agent-loop `bash` tool: `Custom shell path not found: …` when
        // `shellPath` is set and missing (shell.ts:73), and the three-option `No bash shell found.
        // Options: …` recipe with its `Searched Git Bash in:` list when it is unset and the host
        // has no bash (shell.ts:100-106). `_bash_guard` performs Pi's `finally` removal on this
        // early-return path too.
        let shell = ShellConfig::resolve(self.shell_path.as_deref())?;
```

`SessionServiceError` already has a `From<ToolError>` conversion — the deleted arm used
`Err(e.into())` — so the bare `?` compiles. `run_bash`
([bash.rs:138-147](../../../crates/cyrup-session-svc/src/bash.rs)) keeps its
`shell: &ShellConfig` parameter and its call at
[session/bash.rs:122](../../../crates/cyrup-session-svc/src/session/bash.rs) is unchanged.

### 9. Mechanical call-site cascade

Deleting `ShellConfig::detect()` and the two constructor parameters makes the following existing
call sites stop compiling. Each is a pure argument drop, no logic change:

- `LocalProc::new(ShellConfig::detect())` / `LocalProc::with_kill_grace(ShellConfig::detect(), …)`
  → `LocalProc::new()` / `LocalProc::with_kill_grace(…)` in
  [ops/local/tests/exec.rs](../../../crates/cyrup-tools/src/ops/local/tests/exec.rs),
  [ops/local/tests/exec_argv.rs](../../../crates/cyrup-tools/src/ops/local/tests/exec_argv.rs),
  [ops/local/tests/tracking.rs](../../../crates/cyrup-tools/src/ops/local/tests/tracking.rs).
- `BashTool::new(proc, ShellConfig::detect(), cwd, opts)` → `BashTool::new(proc, cwd, opts)` in
  [tests/pi_schema.rs](../../../crates/cyrup-tools/src/tests/pi_schema.rs),
  [tests/tools.rs](../../../crates/cyrup-tools/src/tests/tools.rs),
  [tests/bash_session_env.rs](../../../crates/cyrup-tools/src/tests/bash_session_env.rs),
  [tests/isolation.rs](../../../crates/cyrup-tools/src/tests/isolation.rs),
  [tests/bash_env_scrub.rs](../../../crates/cyrup-tools/tests/bash_env_scrub.rs).
- `shell: ShellConfig::detect()` inside an `ExecSpec` literal
  ([ops/local/tests/mod.rs:36](../../../crates/cyrup-tools/src/ops/local/tests/mod.rs),
  [ops/mod.rs:692](../../../crates/cyrup-tools/src/ops/mod.rs)) →
  `ShellConfig::try_detect().expect("unix detection cannot fail (shell.ts:119)")`, the phrasing
  already used at [shell.rs:399](../../../crates/cyrup-tools/src/ops/shell.rs).
- [tests/shell_interpreter.rs:39-52](../../../crates/cyrup-tools/tests/shell_interpreter.rs) asserts
  against `ShellConfig::detect()`; retarget the same assertion at `ShellConfig::try_detect()`.

---

## Files changed

| file | change |
|---|---|
| [crates/cyrup-tools/src/ops/shell.rs](../../../crates/cyrup-tools/src/ops/shell.rs) | delete `detect()` and `impl Default for ShellConfig` (228-250) |
| [crates/cyrup-tools/src/ops/local/proc.rs](../../../crates/cyrup-tools/src/ops/local/proc.rs) | drop the `shell` field and both constructor params; add `impl Default`; empty-program fallback becomes `ShellConfig::try_detect()?` |
| [crates/cyrup-tools/src/ops/mod.rs](../../../crates/cyrup-tools/src/ops/mod.rs) | `LocalBashOperations::new` and `Backend::local`/`Backend::default` stop taking/resolving a shell |
| [crates/cyrup-tools/src/tools/bash.rs](../../../crates/cyrup-tools/src/tools/bash.rs) | `BashTool` loses its `shell` field and `new` param; `execute` resolves via `ShellConfig::resolve(self.opts.shell_path.as_deref())?` |
| [crates/cyrup-tools/src/registry.rs](../../../crates/cyrup-tools/src/registry.rs) | delete the `detect()` line and the `shell` argument; signature stays `-> Self` |
| [crates/cyrup-session-svc/src/builder.rs](../../../crates/cyrup-session-svc/src/builder.rs) | delete the `detect()` line; `Backend::local()`; drop `shell` from the `SessionExtras` literal |
| [crates/cyrup-session-svc/src/session/mod.rs](../../../crates/cyrup-session-svc/src/session/mod.rs) | delete the `shell` field from `SessionExtras` and `AgentSession` and its assignment |
| [crates/cyrup-session-svc/src/session/bash.rs](../../../crates/cyrup-session-svc/src/session/bash.rs) | `execute_bash` resolves via `ShellConfig::resolve(self.shell_path.as_deref())?` |

Existing test modules listed in §9 take mechanical argument drops only.

---

## Genuinely uncertain

- **Windows verification is unreachable from this workspace.** The affected arm is `#[cfg(not(unix))]`
  at [shell.rs:207-225](../../../crates/cyrup-tools/src/ops/shell.rs); `windows_detect_from` is
  compiled on every host but the branch that *feeds* it real `ProgramFiles` candidates is not.
  The reachability change (steps 4, 5, 8) is platform-independent and observable on unix by pointing
  `shellPath` at a missing file, but the exact `No bash shell found` payload on a real Windows box
  can only be argued from the code, not run here.
- **`Backend::local` and `LocalProc::new` are `pub`** and re-exported through
  [ops/mod.rs:17-21](../../../crates/cyrup-tools/src/ops/mod.rs). Dropping their parameters is a
  breaking change for any out-of-workspace embedder. No such embedder is visible in this repo, so
  the arity change is taken; if one exists, it is a rename-and-deprecate instead.
- **`Backend::default()` has no production caller** (every hit is a test module). Keeping the
  `Default` impl is a judgement call: it is retained because it is public API, and after this change
  it no longer hides a shell decision.
- **Per-command re-resolution cost.** `resolve(None)` on unix does one `Path::exists("/bin/bash")`
  per command in the common case and only reaches the bounded `which bash` probe when `/bin/bash` is
  absent ([shell.rs:196-206](../../../crates/cyrup-tools/src/ops/shell.rs)). That matches pi's
  `existsSync` per `exec` exactly, so it is parity rather than regression — but it is a real
  behavioural change from cyrup's current single detection, and it is the axis the sibling task owns.

---

## Definition of done

The following must hold as observable behaviour:

1. On a host where no bash can be found — no `/bin/bash`, nothing on `PATH`, no Git Bash candidate,
   and no `shellPath` setting — a `bash` tool call returns an error whose text begins
   `No bash shell found. Options:`, contains the lines
   `  1. Install Git for Windows: https://git-scm.com/download/win`,
   `  2. Add your bash to PATH (Cygwin, MSYS2, etc.)` and
   `  3. Set shellPath in settings.json`, and ends with `Searched Git Bash in:` followed by every
   searched candidate path indented by exactly two spaces. No status suffix
   (`Command exited with code …`, `Command aborted`, `Command timed out …`) is appended.
2. That same text is what `/bash` and the RPC `executeBash` return on the same host.
3. No cyrup path emits `spawn bash: … (os error 2)` for a host with no bash, and no path names
   `cmd.exe` or any non-bash interpreter as a substitute.
4. On that same host, session construction still succeeds and `read`, `edit`, `write`, `grep`,
   `find` and `ls` still work; `read_only_tools` and `coding_tools` still build.
5. With `shellPath` set to a path that does not exist, the error is still exactly
   `Custom shell path not found: <path>`, and it still appears only after the initial empty tool
   update, after timeout validation, and after the abort check — an already-cancelled call still
   yields `Command aborted` and never reaches shell resolution.
6. `ShellConfig::detect()` and `impl Default for ShellConfig` no longer exist, and no call site
   anywhere obtains a `ShellConfig` by a path that discards a resolution error.
7. On a normal host with bash present, the resolved shell, its args and its transport are
   byte-identical to today's — `/bin/bash -c` on unix, `bash -s` over stdin for a WSL-legacy
   `…\Windows\System32\bash.exe` — and command execution, streaming, truncation, timeout and
   cancellation behaviour are unchanged.
8. Behaviour pi does not have is not introduced: no shell resolution at registry or session
   construction, no environment-variable shell selector, no fallback interpreter.
