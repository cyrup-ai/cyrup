---
title: Shell detection is cached at tool construction instead of re-resolved on every command
priority: LOW
tool: bash
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: aug
status: done
updated: 2026-08-27
---

# Shell detection is cached at tool construction instead of re-resolved on every command

## Ordering — read before doing anything

**This task is fully subsumed by its sibling.** It contributes **zero independent code change**.

Sibling: [LOW-on-windows-with-no-bash-installed-pi-s-actionable-no-bash-shell-found-er.md](./LOW-on-windows-with-no-bash-installed-pi-s-actionable-no-bash-shell-found-er.md)
(`stage: aug`, `status: done`).

That brief already prescribes, in full and in prescriptive form:

- deleting `ShellConfig::detect()` and `impl Default for ShellConfig`,
- dropping the cached `ShellConfig` out of `LocalProc`, `BashTool`, `Backend`, `SessionExtras` and
  `AgentSession`,
- collapsing [bash.rs:302-305](../../../crates/cyrup-tools/src/tools/bash.rs) to the single line
  `let shell = ShellConfig::resolve(self.opts.shell_path.as_deref())?;`,
- collapsing [session/bash.rs:93-100](../../../crates/cyrup-session-svc/src/session/bash.rs) to
  `let shell = ShellConfig::resolve(self.shell_path.as_deref())?;`.

Those four bullets **are** this task's parity action. Once the sibling lands, the divergence this
task describes no longer exists anywhere in the workspace.

**Required path:**

1. Land the sibling task exactly as written.
2. Then confirm the observable behaviours in *Definition of done* below.
3. If any of them does not hold, the correction belongs to the sibling's numbered step that owns
   that seam (mapped in *Ownership map*), not to a second edit here.

Do **not** implement this task independently. Re-resolving inside `BashTool::execute` while
`BashTool` still carries a `shell` field leaves a dead cached value on the struct, leaves
`ShellConfig::detect()` alive as the swallow that degrades a bash-less host to a bare `bash -c`, and
leaves the immediate-bash seam ([session/bash.rs](../../../crates/cyrup-session-svc/src/session/bash.rs))
still resolving once at session build. That is a strictly worse end state than either task's.

---

## The gap — verified against source

### pi resolves inside `exec`

`createShellToolDefinition` takes its operations from
`options?.operations ?? createLocalBashOperations({ shellPath: options?.shellPath })`
([pi bash.ts:343](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts)). That constructor
resolves nothing — it passes a **thunk**:

```ts
// pi bash.ts:158-160
export function createLocalBashOperations(options?: { shellPath?: string }): BashOperations {
	return createLocalShellOperations("bash", () => getShellConfig(options?.shellPath));
}
```

and the thunk is called **inside every `exec`**, after `resolveTimeoutMs` and after the abort check:

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

`getShellConfig` ([pi shell.ts:67-119](../../../tmp/pi/packages/coding-agent/src/utils/shell.ts))
re-runs the whole ladder each time: explicit `shellPath` → `existsSync("/bin/bash")` (shell.ts:110)
→ `findExecutableOnPath("bash")` (shell.ts:114) → `{ shell: "sh", args: ["-c"] }` (shell.ts:119);
on win32, the two `ProgramFiles` Git Bash candidates (shell.ts:78-92) → `where bash.exe`
(shell.ts:95) → the `No bash shell found` throw (shell.ts:100-106).

Nothing on pi's bash path resolves a shell earlier than that. `createAllToolDefinitions`
([pi index.ts:167,185](../../../tmp/pi/packages/coding-agent/src/core/tools/index.ts)) calls
`createBashToolDefinition(cwd, options?.bash)` and never touches `getShellConfig`.

### cyrup caches at construction

[registry.rs:57](../../../crates/cyrup-tools/src/registry.rs):

```rust
        let shell = ShellConfig::detect();
```

passed straight into `BashTool::new`
([registry.rs:70-75](../../../crates/cyrup-tools/src/registry.rs)), stored on the struct
([bash.rs:53-63](../../../crates/cyrup-tools/src/tools/bash.rs)), and consulted on the auto-detect
branch ([bash.rs:302-305](../../../crates/cyrup-tools/src/tools/bash.rs)):

```rust
        let shell = match self.opts.shell_path.as_deref() {
            Some(p) => ShellConfig::resolve(Some(p))?,
            None => self.shell.clone(),
        };
```

The immediate-bash seam has the same shape
([session/bash.rs:93-100](../../../crates/cyrup-session-svc/src/session/bash.rs)):

```rust
        let shell = match self.shell_path.as_deref() {
            Some(p) => match ShellConfig::resolve(Some(p)) {
                Ok(shell) => shell,
                // `_bash_guard` performs pi's `finally` removal on this path too.
                Err(e) => return Err(e.into()),
            },
            None => self.shell.clone(),
        };
```

fed from [builder.rs:843-844](../../../crates/cyrup-session-svc/src/builder.rs):

```rust
        let shell = ShellConfig::detect();
        let base = Backend::local(shell.clone());
```

which also bakes the same value into `LocalProc`
([proc.rs:47-64](../../../crates/cyrup-tools/src/ops/local/proc.rs)) and into
`SessionExtras.shell` ([builder.rs:1756](../../../crates/cyrup-session-svc/src/builder.rs) →
[session/mod.rs:99,245,370](../../../crates/cyrup-session-svc/src/session/mod.rs)).

Both replacement lines named in *Ownership map* delete every `None => self.shell.clone()` arm, and
so delete the cache.

### What is already correct

[`ShellConfig::resolve`](../../../crates/cyrup-tools/src/ops/shell.rs) (shell.rs:142-150) and
[`try_detect`](../../../crates/cyrup-tools/src/ops/shell.rs) (shell.rs:194-226) are pure,
side-effect-free and already implement pi's exact order and exact error strings. They need no
change; only their **call site** moves.

`LocalBashOperations` is already the correct shape and is the model the other two seams are brought
into line with ([ops/mod.rs:535](../../../crates/cyrup-tools/src/ops/mod.rs)):

```rust
        let shell = ShellConfig::resolve(self.shell_path.as_deref())?;
```

`cyrup-config`'s independent settings-shell path resolves per invocation too —
`get_shell_config()` is called from inside the exec helper at
[config_value.rs:280](../../../crates/cyrup-config/src/config_value.rs) and caches nothing. It is
out of scope and needs no change.

---

## Citation corrections against the pre-augmentation text

| original claim | verdict |
|---|---|
| pi bash.ts:91 calls `resolveShellConfig()` inside `exec` | correct |
| pi `createLocalBashOperations` at bash.ts:158-160 | correct |
| registry.rs:57 `ShellConfig::detect()` | correct |
| bash.rs:302-305 auto-detect fallback | correct |
| shell.rs:142 `resolve`, shell.rs:194 `try_detect` | correct |
| ops/mod.rs:535 per-call `resolve` inside `LocalBashOperations::exec` | correct |
| builder.rs:843 and ops/mod.rs:579 `detect()` construction sites | correct |
| "session/bash.rs:93-99" | **off by one** — the `match` runs 93-**100**; `None => self.shell.clone()` is line 99, closing brace 100 |
| "LocalBashOperations at ops/mod.rs:492-556" | **imprecise** — 492 is the first doc line, `pub struct` is at 502, `impl BashOperations` ends at 556 |
| "its doc at :495-499" | **off by one** — the per-call rule is stated at ops/mod.rs:**494-499** |
| "shell.rs:142 → :194 implements pi's exact `getShellConfig` order" | correct, and the 5 s probe bound is `BASH_PROBE_TIMEOUT` at [shell.rs:70](../../../crates/cyrup-tools/src/ops/shell.rs), applied in `find_bash_on_path` at [shell.rs:79-127](../../../crates/cyrup-tools/src/ops/shell.rs) |

Two stale pi line references survive **inside** cyrup source comments and are picked up by the
sibling's rewrites of those blocks: [bash.rs:299](../../../crates/cyrup-tools/src/tools/bash.rs)
says `bash.ts:85-89` and [session/bash.rs:90](../../../crates/cyrup-session-svc/src/session/bash.rs)
says `bash.ts:69/89`; the real coordinates are `bash.ts:87-91` (`resolveTimeoutMs` 87, abort check
88-90, `resolveShellConfig()` 91) and `bash.ts:91,159`. The comment at
[ops/mod.rs:496](../../../crates/cyrup-tools/src/ops/mod.rs) and
[ops/mod.rs:533](../../../crates/cyrup-tools/src/ops/mod.rs) also says `bash.ts:89`; that block is
untouched by either task and carries no behaviour.

---

## Cost of per-command re-resolution, and what caching must be retained

**Nothing must be retained. The cache is not paying for anything.**

Measured against the code, `ShellConfig::resolve(None)` per command costs:

| host state | work per command | pi's equivalent |
|---|---|---|
| unix, `/bin/bash` present (the overwhelming default) | one `Path::exists` → one `stat(2)`, then a `PathBuf` + one-element `Vec<String>` allocation ([shell.rs:196-198](../../../crates/cyrup-tools/src/ops/shell.rs)) | `existsSync("/bin/bash")`, shell.ts:110 — identical |
| unix, no `/bin/bash` | the above `stat`, then one `which bash` process spawn bounded at 5 s ([shell.rs:70,79-127](../../../crates/cyrup-tools/src/ops/shell.rs)) | `findExecutableOnPath("bash")` → `spawnSync("which","bash",{timeout:5000})` — identical |
| `shellPath` set | one `Path::exists` ([shell.rs:143](../../../crates/cyrup-tools/src/ops/shell.rs)) | `existsSync(customShellPath)`, shell.ts:70 — identical, and cyrup already does this per call today |
| windows, Git Bash installed | up to two `Path::exists` ([shell.rs:170-174](../../../crates/cyrup-tools/src/ops/shell.rs)) | two `existsSync`, shell.ts:87-91 — identical |

The default-case cost is a single `stat` against a command that is about to `fork`/`exec` a real
shell process and stream its output — three to four orders of magnitude larger. There is no hot
loop: resolution happens exactly once per bash tool call, per `/bash`, per RPC `executeBash`.

There is also no double resolution after the sibling lands. `LocalProc::exec` only resolves when the
incoming `ExecSpec` arrives with an empty program
([proc.rs:99-100](../../../crates/cyrup-tools/src/ops/local/proc.rs)); all three production seams
build the spec with an already-resolved `shell` field, so the `LocalProc` fallback fires only for a
third-party `ExecSpec` producer and never on the tool path.

The only case where re-resolution costs a process spawn — no `/bin/bash` — is exactly the case where
the cached value is most likely to be **wrong** (a minimal container where bash is installed
mid-session), and pi pays the same spawn there. Retaining any cache to avoid it would preserve the
divergence this task exists to close.

---

## Ownership map — every file that changes, and which sibling step owns it

Every row is owned and specified by the sibling. This task adds nothing to any row.

| file | change | sibling step |
|---|---|---|
| [crates/cyrup-tools/src/ops/shell.rs](../../../crates/cyrup-tools/src/ops/shell.rs) | delete `ShellConfig::detect()` (shell.rs:237-243) and `impl Default for ShellConfig` (shell.rs:246-250); `resolve`, `try_detect`, `windows_detect_from`, `find_bash_on_path` untouched | 1 |
| [crates/cyrup-tools/src/ops/local/proc.rs](../../../crates/cyrup-tools/src/ops/local/proc.rs) | drop the `shell` field and both constructor params; empty-program fallback becomes `ShellConfig::try_detect()?` | 2 |
| [crates/cyrup-tools/src/ops/mod.rs](../../../crates/cyrup-tools/src/ops/mod.rs) | `LocalBashOperations::new` and `Backend::local`/`Backend::default` stop resolving a shell | 3 |
| [crates/cyrup-tools/src/tools/bash.rs](../../../crates/cyrup-tools/src/tools/bash.rs) | **this task's parity action** — `BashTool` loses its `shell` field and `new` param; bash.rs:302-305 collapses to `ShellConfig::resolve(self.opts.shell_path.as_deref())?` | 4 |
| [crates/cyrup-tools/src/registry.rs](../../../crates/cyrup-tools/src/registry.rs) | delete registry.rs:57 and the `shell` argument; signature stays `-> Self`; drop `ShellConfig` from the registry.rs:6 import | 5 |
| [crates/cyrup-session-svc/src/builder.rs](../../../crates/cyrup-session-svc/src/builder.rs) | delete builder.rs:843; `Backend::local()`; drop `shell,` from the `SessionExtras` literal at builder.rs:1756 | 6 |
| [crates/cyrup-session-svc/src/session/mod.rs](../../../crates/cyrup-session-svc/src/session/mod.rs) | delete `pub shell: ShellConfig,` (:99), `shell: ShellConfig,` (:245) and `shell: extras.shell,` (:370) | 7 |
| [crates/cyrup-session-svc/src/session/bash.rs](../../../crates/cyrup-session-svc/src/session/bash.rs) | **this task's parity action, second seam** — session/bash.rs:93-100 collapses to `ShellConfig::resolve(self.shell_path.as_deref())?` | 8 |

[bash.rs:138-147](../../../crates/cyrup-session-svc/src/bash.rs) keeps its `shell: &ShellConfig`
parameter, and the call at
[session/bash.rs:124](../../../crates/cyrup-session-svc/src/session/bash.rs) is unchanged — the
value it receives is simply now resolved on that call instead of at session build.

---

## Residual after the sibling lands

**None.** The two replacement lines the sibling writes are byte-identical to what this task
prescribes, and the sibling additionally removes every remaining holder of a cached `ShellConfig` —
which this task's own text asks for under *Parity action* but does not otherwise specify.

The one thing this task adds that the sibling does not state is the **cost finding above**: the
sibling records per-command re-resolution as an open question it does not own. That question is now
answered — one `stat` per command in the default case, matching pi exactly, with no caching retained
anywhere.

---

## Definition of done

Observable behaviour, on a host where the sibling task has landed:

1. Starting a session on a unix host with no `/bin/bash` and no `bash` on `PATH`, then installing
   bash (`apk add bash`, `apt install bash`, or dropping a binary onto `PATH`) **without restarting
   the session**, causes the *next* `bash` tool call to run under the newly present bash. No session
   restart is required, and the previously selected `sh -c` is not reused.
2. The reverse holds: removing `/bin/bash` under a live session causes the next `bash` tool call to
   fall through to the `PATH` bash, and then to `sh -c`, on that call — not on a later restart.
3. Changing the `shellPath` setting from unset to a valid path, or from a valid path back to unset,
   takes effect on the very next `bash` tool call.
4. Behaviours 1-3 are identical for `/bash` and for the RPC `executeBash` as they are for the
   agent-loop `bash` tool.
5. On a host with no bash at all and no `shellPath`, a `bash` call returns the error text beginning
   `No bash shell found. Options:` with its three numbered options and its `Searched Git Bash in:`
   list, and no path emits `spawn bash: … (os error 2)`.
6. With `shellPath` pointing at a missing file, the error is exactly
   `Custom shell path not found: <path>`, and it still appears only *after* the initial empty tool
   update, *after* timeout validation, and *after* the abort check — an already-cancelled call still
   yields `Command aborted` and never reaches shell resolution.
7. Session construction never fails because no bash exists: `read`, `edit`, `write`, `grep`, `find`
   and `ls` still work on such a host, and the read-only and coding tool sets still build.
8. No `ShellConfig` is stored on `BashTool`, `LocalProc`, `Backend`, `SessionExtras` or
   `AgentSession`, and `ShellConfig::detect()` no longer exists — so no code path can obtain a shell
   by a route that discards a resolution error.
9. On a normal host with bash present, the resolved program, args and transport are unchanged
   (`/bin/bash -c` on unix; `bash -s` over stdin for a WSL-legacy
   `…\Windows\System32\bash.exe`), and streaming, truncation, timeout and cancellation behaviour is
   unchanged.
10. Nothing pi lacks is introduced: no shell resolution at registry or session construction, no
    environment-variable shell selector, no fallback interpreter, and no memoization of a resolved
    shell between commands.
