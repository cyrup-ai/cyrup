---
title: The entire powershell built-in tool is missing from cyrup
priority: MEDIUM
tool: powershell
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: aug
status: in-progress
updated: 2026-08-27
---

# The entire `powershell` built-in tool is missing from cyrup

> **Merged finding.** Two independent lanes reported this. They agreed on every fact and disagreed
> on severity: one rated it **medium**, the other **low**. Both assessments are preserved below and
> both survived re-verification. The downgrade argument is not discarded — it is what fixes the
> *shape* of the work: this tool must ship **opt-in**, must fail loudly and precisely off-Windows,
> and must not become a second default shell.

## Core objective

`powershell` becomes cyrup's **eighth** built-in tool, in pi's literal position (immediately after
`bash`), sharing one execution engine with `bash` and differing only in the parameters pi's
`ShellToolConfig` carries. It resolves `pwsh.exe` then `powershell.exe`, applies
`-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command`, prepends the UTF-8
`[Console]::OutputEncoding` line to every command, refuses to run off Windows with pi's exact
sentence, and is **never active by default** — `read`/`bash`/`edit`/`write` remain the default set.

---

## Ordering dependency — read this first

This task **builds on** the finished sibling brief
[LOW-on-windows-with-no-bash-installed-pi-s-actionable-no-bash-shell-found-er.md](./LOW-on-windows-with-no-bash-installed-pi-s-actionable-no-bash-shell-found-er.md),
and **must land after it**. That task:

- deletes `ShellConfig::detect()` and `impl Default for ShellConfig` from
  [ops/shell.rs](../../../crates/cyrup-tools/src/ops/shell.rs),
- drops the cached `ShellConfig` from `LocalProc`, `BashTool`, `ToolRegistry::with_builtins`,
  `Backend::local`, `SessionExtras` and `AgentSession`,
- moves resolution to the execute seam: `BashTool::execute` becomes
  `let shell = ShellConfig::resolve(self.opts.shell_path.as_deref())?;`, and
  `ToolRegistry::with_builtins` keeps `-> Self` while resolving nothing.

Every `CURRENT` block below is quoted **as it will read once that task has landed**, not as it
reads today. Two concrete consequences:

1. `BashTool::new(proc, cwd, opts)` already has three parameters here. If this task lands first,
   the `shell` argument is still present and the rename below must carry it — do not do that;
   land the sibling first.
2. The sibling's whole point — that a bash-less host still gets a working session and reads pi's
   repair recipe as a *tool result* — is what makes this task's impact story true. Before it, a
   Windows box with no bash could not construct a session at all, so `powershell` would have had
   nothing to be useful *inside*. After it, `powershell` is the actual capability restored.

The two tasks touch [ops/shell.rs](../../../crates/cyrup-tools/src/ops/shell.rs),
[registry.rs](../../../crates/cyrup-tools/src/registry.rs) and
[tools/bash.rs](../../../crates/cyrup-tools/src/tools/bash.rs) in different places; there is no
line-level conflict, only this ordering.

---

## What pi does — verified against the vendored tree (pi 0.84.3)

### One factory, two shells

Pi does **not** have two shell tools. It has one factory and two configs.
[bash.ts:328-336](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts) declares the
parameter bag:

```ts
export interface ShellToolConfig {
	name: string;
	label: string;
	shellName: string;
	prompt: string;
	promptSnippet: string;
	promptGuidelines?: readonly string[];
	tempFilePrefix: string;
}
```

[bash.ts:338-345](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts) consumes it, and
every model-facing string is interpolated from it:

```ts
export function createShellToolDefinition(cwd: string, config: ShellToolConfig, options?: BashToolOptions) {
	const ops = options?.operations ?? createLocalBashOperations({ shellPath: options?.shellPath });
	…
		name: config.name,
		label: config.label,
		description: `Execute a ${config.shellName} command in the current working directory. …`,   // :350
		promptSnippet: config.promptSnippet,                                                        // :351
		promptGuidelines: exposeSessionEnvironment && config.promptGuidelines ? […] : undefined,    // :352
		parameters: bashSchema,                                                                     // :353
```

`bash` is then just one instantiation
([bash.ts:519-534](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts)):

```ts
const bashToolConfig: ShellToolConfig = {
	name: "bash", label: "bash", shellName: "bash", prompt: "$",
	promptSnippet: bashToolSystemPromptContribution.snippet,
	promptGuidelines: bashToolSystemPromptContribution.guidelines,
	tempFilePrefix: "pi-bash",
};
export function createBashToolDefinition(cwd, options?) { return createShellToolDefinition(cwd, bashToolConfig, options); }
```

and `powershell` is the other
([powershell.ts:39-57](../../../tmp/pi/packages/coding-agent/src/core/tools/powershell.ts)):

```ts
const powershellToolConfig: ShellToolConfig = {
	name: "powershell", label: "powershell", shellName: "PowerShell", prompt: "PS>",
	promptSnippet: powershellToolSystemPromptContribution.snippet,
	promptGuidelines: powershellToolSystemPromptContribution.guidelines,
	tempFilePrefix: "pi-powershell",
};
export function createPowerShellToolDefinition(cwd, options?) {
	return createShellToolDefinition(cwd, powershellToolConfig, {
		...options,
		operations: options?.operations ?? createLocalPowerShellOperations(),
	});
}
```

The snippet and guideline live at
[powershell.ts:18-21](../../../tmp/pi/packages/coding-agent/src/core/tools/powershell.ts):

```ts
export const powershellToolSystemPromptContribution = {
	snippet: "Execute PowerShell commands",
	guidelines: ["You can inspect PI_* environment variables for current model and session details."],
} as const;
```

The schema is **shared**: one module-level `bashSchema` serves both tools
([bash.ts:42-45](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts)), and at 0.84.3 its
`command` description reads `"Shell command to execute"`.

### The UTF-8 preamble, and exactly where it is applied

[powershell.ts:16,32-37](../../../tmp/pi/packages/coding-agent/src/core/tools/powershell.ts):

```ts
const UTF8_OUTPUT_PREFIX = "try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}\n";

export function createLocalPowerShellOperations(): PowerShellOperations {
	const operations = createLocalShellOperations("PowerShell", getPowerShellConfig);
	return {
		exec: (command, cwd, options) => operations.exec(`${UTF8_OUTPUT_PREFIX}${command}`, cwd, options),
	};
}
```

The prefix is applied **inside `operations.exec`**, i.e. downstream of everything the factory does
first: `commandPrefix` concatenation (bash.ts:340), then `resolveSpawnContext` (bash.ts:341,
:168-190), then the `spawnHook`, and only then
`ops.exec(spawnContext.command, …)` at
[bash.ts:451](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts). So the preamble goes
on **after** the spawn hook, immediately before the child is built. Getting that order wrong lets a
hook rewrite or double the preamble.

`createLocalPowerShellOperations()` takes **no options at all** — no `shellPath`, no
`commandPrefix`. That is not an oversight; it is mirrored in the public options type
([powershell.ts:29-30](../../../tmp/pi/packages/coding-agent/src/core/tools/powershell.ts)):

```ts
export interface PowerShellToolOptions
	extends Pick<BashToolOptions, "operations" | "exposeSessionEnvironment" | "spawnHook"> {}
```

### Resolution and the two error strings

[shell.ts:122-136](../../../tmp/pi/packages/coding-agent/src/utils/shell.ts), verbatim:

```ts
export const POWERSHELL_ARGS = ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"] as const;

/** Resolve PowerShell on Windows, preferring PowerShell 7 when available. */
export function getPowerShellConfig(): ShellConfig {
	if (process.platform !== "win32") {
		throw new Error("The powershell tool is only available on Windows.");
	}

	const shell = findExecutableOnPath("pwsh.exe") ?? findExecutableOnPath("powershell.exe");
	if (!shell) {
		throw new Error("No PowerShell executable found. Install PowerShell or add powershell.exe/pwsh.exe to PATH.");
	}

	return { shell, args: [...POWERSHELL_ARGS] };
}
```

`findExecutableOnPath`
([shell.ts:24-45](../../../tmp/pi/packages/coding-agent/src/utils/shell.ts)) is the **generic** form
of the probe cyrup already has: `where <exe>` on win32 with an `existsSync` verification and a
5000 ms `spawnSync` timeout, `which <exe>` elsewhere. Pi calls it with `"bash"` / `"bash.exe"` /
`"pwsh.exe"` / `"powershell.exe"`.

Note the transport: no `commandTransport` is set, so PowerShell is **argv** — the command becomes
the trailing argument after `-Command`.

### The `shellName` also names the cwd error

`createLocalShellOperations` takes `shellName` as its first parameter purely to build one message
([bash.ts:84-95](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts)):

```ts
export function createLocalShellOperations(shellName: string, resolveShellConfig: () => ShellConfig): BashOperations {
	return {
		exec: async (command, cwd, { onData, signal, timeout, env }) => {
			const timeoutMs = resolveTimeoutMs(timeout);
			if (signal?.aborted) { throw new Error("aborted"); }
			const shellConfig = resolveShellConfig();
			try { await fsAccess(cwd, constants.F_OK); }
			catch { throw new Error(`Working directory does not exist: ${cwd}\nCannot execute ${shellName} commands.`); }
```

So on a missing cwd pi says `Cannot execute PowerShell commands.`, not `Cannot execute bash
commands.` The resolution thunk is called **inside** `exec`, after `resolveTimeoutMs` and the abort
check — the identical seam the sibling task just moved cyrup's bash resolution to.

### Registration, defaults, help, prompt

- `ToolName` is the eight-name union
  ([index.ts:95](../../../tmp/pi/packages/coding-agent/src/core/tools/index.ts)) and `allToolNames`
  lists all eight in order `read, bash, powershell, edit, write, grep, find, ls`
  ([index.ts:96-105](../../../tmp/pi/packages/coding-agent/src/core/tools/index.ts)).
- `ToolsOptions.powershell?: PowerShellToolOptions`
  ([index.ts:110](../../../tmp/pi/packages/coding-agent/src/core/tools/index.ts)).
- Constructible by name at
  [index.ts:124-125](../../../tmp/pi/packages/coding-agent/src/core/tools/index.ts) and
  [index.ts:147-148](../../../tmp/pi/packages/coding-agent/src/core/tools/index.ts); present in
  `createAllToolDefinitions` at
  [index.ts:186](../../../tmp/pi/packages/coding-agent/src/core/tools/index.ts) and in
  `createAllTools` at
  [index.ts:217](../../../tmp/pi/packages/coding-agent/src/core/tools/index.ts) — both in third
  position. It is absent from `createCodingTools` and `createReadOnlyTools`.
- It is **not** a default tool. `defaultActiveToolNames` is `["read", "bash", "edit", "write"]`
  ([sdk.ts:256](../../../tmp/pi/packages/coding-agent/src/core/sdk.ts)), and the system prompt's
  own fallback is the same four
  ([system-prompt.ts:81](../../../tmp/pi/packages/coding-agent/src/core/system-prompt.ts)).
- Help ([args.ts:437-445](../../../tmp/pi/packages/coding-agent/src/cli/args.ts)):

  ```
  Built-in Tool Names:
    read       - Read file contents
    bash       - Execute bash commands
    powershell - Execute PowerShell commands on Windows
    edit       - Edit files with find/replace
    write      - Write files (creates/overwrites)
    grep       - Search file contents (read-only, off by default)
    find       - Find files by glob pattern (read-only, off by default)
    ls         - List directory contents (read-only, off by default)
  ```

- The system prompt's file-exploration fallback is a **three-way** branch
  ([system-prompt.ts:97-113](../../../tmp/pi/packages/coding-agent/src/core/system-prompt.ts)):

  ```ts
  const hasBash = tools.includes("bash");
  const hasPowerShell = tools.includes("powershell");
  …
  if ((hasBash || hasPowerShell) && !hasGrep && !hasFind && !hasLs) {
  	if (hasBash && hasPowerShell) {
  		addGuideline("Use bash or PowerShell for file operations like listing, searching, and finding files");
  	} else if (hasPowerShell) {
  		addGuideline("Use PowerShell for file operations like listing, searching, and finding files");
  	} else {
  		addGuideline("Use bash for file operations like ls, rg, find");
  	}
  }
  ```

- The TUI renders the call with `config.prompt` —
  `text.setText(formatShellCall(args, config.prompt))`
  ([bash.ts:488](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts), helper at
  [bash.ts:238-244](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts)) — so a PowerShell
  call draws `PS> <command>`, never `$ <command>`.

---

## What cyrup does today — verified, with the finding's citations corrected

Everything the two adversaries reported is true. Line numbers re-derived this pass:

| claim | verified location |
|---|---|
| seven built-ins only | `pub const BUILTIN_NAMES: [&str; 7]` — [registry.rs:20](../../../crates/cyrup-tools/src/registry.rs) |
| `with_builtins` inserts exactly seven | [registry.rs:53-100](../../../crates/cyrup-tools/src/registry.rs) |
| no `powershell` module | [tools/mod.rs:1-20](../../../crates/cyrup-tools/src/tools/mod.rs) |
| `ToolsOptions` has no `powershell` | [config.rs:316-325](../../../crates/cyrup-tools/src/config.rs) |
| no pwsh probe, no `POWERSHELL_ARGS` | [ops/shell.rs](../../../crates/cyrup-tools/src/ops/shell.rs) resolves bash only — `find_bash_on_path` [:79-131](../../../crates/cyrup-tools/src/ops/shell.rs), `try_detect` [:194-226](../../../crates/cyrup-tools/src/ops/shell.rs), `windows_detect_from` [:169-192](../../../crates/cyrup-tools/src/ops/shell.rs) |
| `BashTool` is concrete, not a factory | `fn name -> "bash"` [bash.rs:85-87](../../../crates/cyrup-tools/src/tools/bash.rs), hardcoded description [:99-103](../../../crates/cyrup-tools/src/tools/bash.rs), hardcoded schema [:65-72](../../../crates/cyrup-tools/src/tools/bash.rs) |
| CLI help lists seven | [help.rs:213-220](../../../crates/cyrup/src/cli/help.rs) |
| no UTF-8 preamble anywhere | repo-wide search for `OutputEncoding` returns nothing under `crates/` |

**Citation corrections** (the finding's numbers, fixed here so the implementer does not chase them):

- `createTool`'s powershell case is **index.ts:147-148**, not 146-147.
- `createAllTools`'s powershell entry is **index.ts:217**, not 220.
- `ToolsOptions` begins at **config.rs:316** (the derive), not 317.
- `registry.rs`'s own comment cites `index.ts:156-166` for `createAllToolDefinitions`; at 0.84.3
  that function is **index.ts:182-193**. Fix the comment while editing the block.
- [builder.rs:315](../../../crates/cyrup-session-svc/src/builder.rs) cites
  `cyrup-tools/src/registry.rs:45-67` for `with_builtins`; it is **registry.rs:53-100**.

**One correction to the downgrade argument, recorded rather than argued away.** Point (3) —
"nothing is silently wrong; `--tools powershell` fails loudly as an unknown tool name" — does not
hold in cyrup. There is no unknown-tool-name validation on either side: pi's `--tools` list is a
filter (`initialActiveToolNames`,
[sdk.ts:256-262](../../../tmp/pi/packages/coding-agent/src/core/sdk.ts)) and so is cyrup's
`select_active_tools` ([builder.rs:326-365](../../../crates/cyrup-session-svc/src/builder.rs)),
whose allowlist arm is `allow.iter().any(|a| a == name)`. So `cyrup --tools powershell` today
produces a session with **zero** model-visible tools and no diagnostic — quiet, not loud. The rest
of the downgrade argument stands unchanged and is honoured by the design below: opt-in,
Windows-only, loud where it matters (the off-Windows sentence and the no-executable sentence are
both tool-result text the model reads), and `shellPath` remains a workaround that this task does
not remove.

Explicitly **out of scope**, verified as unrelated rather than overlooked:

- [cyrup-config/src/config_value.rs:358-403](../../../crates/cyrup-config/src/config_value.rs) has a
  second, private `ShellConfig` for settings-file `$(…)` substitution. Pi's config-value
  substitution is bash-only; nothing there changes.
- [cyrup-ext-sdk/src/tool_factory.rs:17-33](../../../crates/cyrup-ext-sdk/src/tool_factory.rs) is a
  guest-authored descriptor surface with its own (already divergent) shapes.
- The `/bash` slash command and the `executeBash` RPC
  ([session/bash.rs](../../../crates/cyrup-session-svc/src/session/bash.rs)) stay bash-only —
  pi's `executeBash` reaches `createLocalBashOperations` and has no PowerShell counterpart.

---

## The decision: one config-driven shell tool, not a parallel `PowerShellTool`

**Required path: introduce `ShellToolConfig` + `ShellTool` in
[tools/bash.rs](../../../crates/cyrup-tools/src/tools/bash.rs), replacing `BashTool`, and add
`tools/powershell.rs` holding nothing but the PowerShell config and its constructor.** This is pi's
own file split — the factory lives in `bash.ts`, `powershell.ts` imports it — and it is the only
option that is code-correct here. The justification is in the real code, not in taste:

1. **`BashTool::execute` is ~350 lines and none of it is bash-specific.** From the initial empty
   update ([bash.rs:265-273](../../../crates/cyrup-tools/src/tools/bash.rs)) through
   `resolve_timeout_ms` (:279), the abort check (:281-295), the spawn-context assembly with the
   `CYRUP_*` injection and the unconditional scrub (:193-257), the 100 ms leading+trailing throttle
   (:315-387), the truncation footers (:401-445) and the four exit arms (:473-516) — every line is
   `createShellToolDefinition`'s body verbatim. A parallel `PowerShellTool` duplicates all of it,
   and every future bash fix has to be applied twice or silently is not.
2. **The differences are exactly seven values**, all of which pi already names: `name`, `label`,
   `shellName`, `promptSnippet`, `promptGuidelines`, `tempFilePrefix`, `prompt` — plus the
   resolution thunk and, for PowerShell only, the command preamble. Nine `&'static` fields against
   350 duplicated lines settles it.
3. **The sibling task already removed the one thing that would have made a shared tool awkward.**
   With the cached `ShellConfig` gone from `BashTool` and resolution moved into `execute`, the only
   remaining per-shell input at the execute seam is *which resolver to call* — a function pointer.
   Had the shell still been a constructor argument, the two tools would have needed different
   construction paths as well.
4. **`BashOpts` is already pi's shared options bag.** Upstream's factory signature is
   `createShellToolDefinition(cwd, config, options?: BashToolOptions)` — `BashToolOptions`, for
   both shells. `PowerShellToolOptions` is only a narrowed *public* surface on the powershell
   constructor. Rust mirrors that exactly with a `PowerShellOpts` struct and
   `From<PowerShellOpts> for BashOpts`, so nothing has to be re-plumbed.

`BashTool` therefore does not survive as a type. It is renamed `ShellTool`, and
`BashTool::new(proc, cwd, opts)` becomes `ShellTool::bash(proc, cwd, opts)`. Its only production
call site is [registry.rs:70](../../../crates/cyrup-tools/src/registry.rs).

---

## Required changes

### 1. `crates/cyrup-tools/src/ops/shell.rs` — carry the shell's human name, generalise the PATH probe, add PowerShell resolution

**1a. `ShellConfig` gains `shell_name`.** Pi passes `shellName` alongside the resolved config into
`createLocalShellOperations` purely to build the cwd error (bash.ts:84,95). In cyrup that error is
raised by the backend, three seams downstream of the tool, so the name has to travel with the
resolved shell. This is the smallest correct carrier: `ShellConfig` is built by exactly three
literals, all private to this file.

CURRENT ([shell.rs:23-29](../../../crates/cyrup-tools/src/ops/shell.rs)):

```rust
/// A resolved shell configuration.
#[derive(Clone, Debug)]
pub struct ShellConfig {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub transport: Transport,
}
```

REPLACEMENT:

```rust
/// A resolved shell configuration.
#[derive(Clone, Debug)]
pub struct ShellConfig {
    /// The shell's human name, as Pi passes it to `createLocalShellOperations` (bash.ts:84,159;
    /// powershell.ts:33): `"bash"` or `"PowerShell"`. Its ONLY consumer is the missing-cwd error
    /// `Cannot execute {shell_name} commands.` (bash.ts:95), which cyrup raises in the process
    /// backend rather than in the tool — so the name has to ride along with the resolved shell.
    /// It is the TOOL's name for its shell, not the resolved program's: bash's unix `sh -c`
    /// fallback (shell.ts:119) still reports `bash`, exactly as upstream does.
    pub shell_name: &'static str,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub transport: Transport,
}
```

CURRENT ([shell.rs:48-64](../../../crates/cyrup-tools/src/ops/shell.rs)):

```rust
/// `getBashShellConfig` (shell.ts:20-22): stdin transport for the WSL-legacy launcher, argv `-c`
/// otherwise.
fn get_bash_shell_config(program: PathBuf) -> ShellConfig {
    if is_legacy_wsl_bash_path(&program.to_string_lossy()) {
        ShellConfig {
            program,
            args: vec!["-s".to_string()],
            transport: Transport::Stdin,
        }
    } else {
        ShellConfig {
            program,
            args: vec!["-c".to_string()],
            transport: Transport::Argv,
        }
    }
}
```

REPLACEMENT:

```rust
/// `getBashShellConfig` (shell.ts:20-22): stdin transport for the WSL-legacy launcher, argv `-c`
/// otherwise.
fn get_bash_shell_config(program: PathBuf) -> ShellConfig {
    if is_legacy_wsl_bash_path(&program.to_string_lossy()) {
        ShellConfig {
            shell_name: "bash",
            program,
            args: vec!["-s".to_string()],
            transport: Transport::Stdin,
        }
    } else {
        ShellConfig {
            shell_name: "bash",
            program,
            args: vec!["-c".to_string()],
            transport: Transport::Argv,
        }
    }
}
```

The `sh` fallback literal inside `try_detect`
([shell.rs:204-208](../../../crates/cyrup-tools/src/ops/shell.rs)) takes the same one-line addition:

CURRENT:

```rust
            Ok(ShellConfig {
                program: PathBuf::from("sh"),
                args: vec!["-c".to_string()],
                transport: Transport::Argv,
            })
```

REPLACEMENT:

```rust
            // Still `bash` by NAME (Pi's `createLocalShellOperations("bash", …)`, bash.ts:159, is
            // unaware that `getShellConfig` degraded to `sh` at shell.ts:119).
            Ok(ShellConfig {
                shell_name: "bash",
                program: PathBuf::from("sh"),
                args: vec!["-c".to_string()],
                transport: Transport::Argv,
            })
```

**1b. `find_bash_on_path` becomes `find_executable_on_path`.** Pi's probe is already generic
(`findExecutableOnPath(executable)`, shell.ts:24-45); cyrup hardcoded the one executable it needed.
Parameterise the name; the `which`/`where` choice and the Windows-only existence verification stay
exactly as they are.

CURRENT ([shell.rs:72-84](../../../crates/cyrup-tools/src/ops/shell.rs)):

```rust
/// `findBashOnPath` (shell.ts:24-58): `which bash` on unix / `where bash.exe` on Windows. Returns
/// the first match (verified to exist on Windows, where `where` can print stale paths).
///
/// Bounded at [`BASH_PROBE_TIMEOUT`] exactly like Pi's `spawnSync` timeout: a `which` wedged on a
/// stale automount PATH entry must not wedge session construction. Node's `spawnSync` kills the
/// child on expiry and reports a non-zero status, which lands in Pi's `result.status === 0` guard
/// (shell.ts:48) — i.e. "no bash on PATH" — so expiry maps to `None` here.
fn find_bash_on_path() -> Option<PathBuf> {
    #[cfg(not(unix))]
    let (cmd, arg) = ("where", "bash.exe");
    #[cfg(unix)]
    let (cmd, arg) = ("which", "bash");
```

REPLACEMENT:

```rust
/// `findExecutableOnPath` (shell.ts:24-58): `which <exe>` on unix / `where <exe>` on Windows.
/// Returns the first match (verified to exist on Windows, where `where` can print stale paths).
///
/// Generic in the executable because Pi's is: it is called with `"bash"` / `"bash.exe"` for the
/// bash tool (shell.ts:95,114) and with `"pwsh.exe"` / `"powershell.exe"` for the powershell tool
/// (shell.ts:130). The unix/Windows split is the PROBE COMMAND only; the caller supplies the name.
///
/// Bounded at [`BASH_PROBE_TIMEOUT`] exactly like Pi's `spawnSync` timeout: a `which` wedged on a
/// stale automount PATH entry must not wedge a command. Node's `spawnSync` kills the child on
/// expiry and reports a non-zero status, which lands in Pi's `result.status === 0` guard
/// (shell.ts:48) — i.e. "not on PATH" — so expiry maps to `None` here.
fn find_executable_on_path(executable: &str) -> Option<PathBuf> {
    #[cfg(not(unix))]
    let cmd = "where";
    #[cfg(unix)]
    let cmd = "which";
    let arg = executable;
```

The body below is unchanged. The two existing call sites become:

- [shell.rs:201](../../../crates/cyrup-tools/src/ops/shell.rs) inside `try_detect`'s unix arm:
  `if let Some(found) = find_bash_on_path() {` → `if let Some(found) = find_executable_on_path("bash") {`
- [shell.rs:224](../../../crates/cyrup-tools/src/ops/shell.rs) inside `try_detect`'s Windows arm:
  `Self::windows_detect_from(&candidates, find_bash_on_path)` →
  `Self::windows_detect_from(&candidates, || find_executable_on_path("bash.exe"))`

**1c. PowerShell resolution.** Add, directly after
[`try_detect`](../../../crates/cyrup-tools/src/ops/shell.rs) and inside the same `impl ShellConfig`
block:

```rust
    /// `POWERSHELL_ARGS` (shell.ts:122), verbatim and in order. The command is delivered as the
    /// argument AFTER `-Command`, i.e. [`Transport::Argv`] — Pi sets no `commandTransport` on the
    /// PowerShell config (shell.ts:135), so the WSL-legacy stdin path is unreachable here.
    const POWERSHELL_ARGS: [&'static str; 5] = [
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
    ];

    /// Pi `getPowerShellConfig`'s body AFTER the `win32` guard (shell.ts:130-135), with the PATH
    /// probe hoisted into an argument so the arm that SHIPS to Windows is compiled on every host —
    /// the same treatment [`ShellConfig::windows_detect_from`] already gets, and for the same
    /// reason: this workspace has no Windows box, so an arm that only exists under
    /// `#[cfg(windows)]` is an arm nobody has ever built.
    ///
    /// `pwsh.exe` FIRST, `powershell.exe` second: Pi's `??` prefers PowerShell 7 over Windows
    /// PowerShell 5.1 (shell.ts:124 "preferring PowerShell 7 when available").
    fn powershell_detect_from(
        find_on_path: impl Fn(&str) -> Option<PathBuf>,
    ) -> Result<Self, ToolError> {
        let found = find_on_path("pwsh.exe").or_else(|| find_on_path("powershell.exe"));
        let Some(program) = found else {
            // shell.ts:132 verbatim.
            return Err(ToolError::new(
                "No PowerShell executable found. Install PowerShell or add powershell.exe/pwsh.exe \
                 to PATH.",
            ));
        };
        Ok(ShellConfig {
            shell_name: "PowerShell",
            program,
            args: Self::POWERSHELL_ARGS
                .iter()
                .map(|a| (*a).to_string())
                .collect(),
            transport: Transport::Argv,
        })
    }

    /// Pi `getPowerShellConfig` (shell.ts:124-136). Called per-command from the `powershell` tool's
    /// `execute`, never at construction — Pi's thunk is the bare `getPowerShellConfig` reference
    /// (powershell.ts:33), invoked inside `exec` (bash.ts:91).
    ///
    /// Takes NO `shellPath`: `createLocalPowerShellOperations()` accepts no options at all
    /// (powershell.ts:32-33) and `PowerShellToolOptions` does not include `shellPath`
    /// (powershell.ts:29-30), so the settings `shellPath` — which points at a BASH — must never
    /// steer this tool.
    pub fn resolve_powershell() -> Result<Self, ToolError> {
        #[cfg(not(windows))]
        {
            // shell.ts:127 verbatim. Pi gates on `process.platform !== "win32"`, so the Rust gate
            // is `windows`, not `not(unix)`.
            Err(ToolError::new(
                "The powershell tool is only available on Windows.",
            ))
        }
        #[cfg(windows)]
        {
            Self::powershell_detect_from(find_executable_on_path)
        }
    }
```

### 2. `crates/cyrup-tools/src/ops/local/proc.rs` — the cwd error names the right shell

CURRENT ([proc.rs:113-120](../../../crates/cyrup-tools/src/ops/local/proc.rs)):

```rust
        // Pi checks the cwd exists before spawning (bash.ts:70-74) so the model gets an actionable
        // message instead of a raw spawn error.
        if tokio::fs::metadata(&spec.cwd).await.is_err() {
            return Err(error::invalid(format!(
                "Working directory does not exist: {}\nCannot execute bash commands.",
                error::show(&spec.cwd)
            )));
        }
```

REPLACEMENT:

```rust
        // Pi checks the cwd exists before spawning (bash.ts:92-96) so the model gets an actionable
        // message instead of a raw spawn error. The shell's name comes from the resolved
        // `ShellConfig`, mirroring Pi's `createLocalShellOperations(shellName, …)` closure capture
        // (bash.ts:84,95): a `powershell` call on a missing cwd must say
        // `Cannot execute PowerShell commands.`, not `bash`.
        if tokio::fs::metadata(&spec.cwd).await.is_err() {
            return Err(error::invalid(format!(
                "Working directory does not exist: {}\nCannot execute {} commands.",
                error::show(&spec.cwd),
                spec.shell.shell_name
            )));
        }
```

### 3. `crates/cyrup-tools/src/tools/bash.rs` — `BashTool` becomes the shared `ShellTool`

**3a. The config type and the bash instantiation.**

CURRENT ([bash.rs:53-81](../../../crates/cyrup-tools/src/tools/bash.rs), post-sibling):

```rust
pub struct BashTool {
    proc: Arc<dyn ProcOps>,
    cwd: PathBuf,
    opts: BashOpts,
    params: serde_json::Value,
}

impl BashTool {
    pub fn new(proc: Arc<dyn ProcOps>, cwd: PathBuf, opts: BashOpts) -> Self {
        // Byte-for-byte Pi's TypeBox emission (bash.ts:24-27): verbatim descriptions,
        // `type:"number"`, no `minimum`, no `additionalProperties`.
        let params = serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": { "type": "string", "description": "Bash command to execute" },
                "timeout": { "type": "number", "description": "Timeout in seconds (optional, no default timeout)" }
            }
        });
        Self {
            proc,
            cwd,
            opts,
            params,
        }
    }
}
```

REPLACEMENT:

```rust
/// The per-shell parameters of Pi's shared shell-tool factory (`ShellToolConfig`, bash.ts:328-336).
/// Everything that differs between `bash` and `powershell` lives here; [`ShellTool`] below is Pi's
/// `createShellToolDefinition` (bash.ts:338-517) and is instantiated once per config.
pub struct ShellToolConfig {
    /// `config.name` (bash.ts:346).
    pub name: &'static str,
    /// `config.label` (bash.ts:347).
    pub label: &'static str,
    /// `config.shellName` — interpolated into the tool description (bash.ts:350). It is ALSO the
    /// name the missing-cwd error uses (bash.ts:95); that copy travels on [`ShellConfig`] because
    /// cyrup raises the error in the process backend.
    pub shell_name: &'static str,
    /// The `command` property's schema description.
    ///
    /// [CYRUP-DELTA — version lag, per-tool instead of shared] Pi shares ONE `bashSchema` between
    /// both shell tools (bash.ts:42-45), and at v0.84.3 its text is `"Shell command to execute"`.
    /// cyrup's ported baseline is v0.83.0, where `bash` alone existed and the text was
    /// `"Bash command to execute"` — which cyrup still emits. Keeping that per-config rather than
    /// shared lets `powershell` be byte-exact against the tag it is ported FROM without silently
    /// rewriting a model-facing `bash` string that no other part of this task touches. A later
    /// v0.84.x uplift collapses both to `"Shell command to execute"`.
    pub command_description: &'static str,
    /// `config.promptSnippet` (bash.ts:351).
    pub prompt_snippet: &'static str,
    /// `config.promptGuidelines` (bash.ts:352), emitted only when
    /// [`BashOpts::expose_session_environment`] is set.
    pub prompt_guidelines: &'static [&'static str],
    /// `config.tempFilePrefix` (bash.ts:364).
    pub temp_file_prefix: &'static str,
    /// Text prepended to the command AFTER the spawn hook has run. `None` for bash. For PowerShell
    /// this is Pi's `UTF8_OUTPUT_PREFIX`, applied inside `createLocalPowerShellOperations`
    /// (powershell.ts:16,35) — i.e. downstream of `commandPrefix`, `resolveSpawnContext` and the
    /// hook, immediately before the child is built (bash.ts:451).
    pub command_preamble: Option<&'static str>,
    /// Pi's `resolveShellConfig` thunk (bash.ts:84; `() => getShellConfig(shellPath)` at
    /// bash.ts:159, the bare `getPowerShellConfig` at powershell.ts:33). Called inside `execute`,
    /// never at construction. The argument is the settings `shellPath`; the PowerShell resolver
    /// ignores it, because Pi's PowerShell surface has no `shellPath` at all.
    pub resolve_shell: fn(Option<&str>) -> Result<ShellConfig, ToolError>,
}

/// Pi's `bashToolConfig` (bash.ts:519-527).
pub static BASH_CONFIG: ShellToolConfig = ShellToolConfig {
    name: "bash",
    label: "bash",
    shell_name: "bash",
    command_description: "Bash command to execute",
    prompt_snippet: "Execute bash commands (ls, grep, find, etc.)",
    prompt_guidelines: &[
        "You can inspect CYRUP_* environment variables for current model and session details.",
    ],
    temp_file_prefix: "cyrup-bash",
    command_preamble: None,
    resolve_shell: ShellConfig::resolve,
};

/// Pi's `createShellToolDefinition` (bash.ts:338-517): ONE engine, parameterised by
/// [`ShellToolConfig`]. `bash` and `powershell` are two instantiations of this type, exactly as
/// upstream has two `ShellToolConfig` literals and one factory.
///
/// Carries NO resolved [`ShellConfig`]: the shell — and any resolution error — belongs to the CALL
/// (bash.ts:91).
pub struct ShellTool {
    config: &'static ShellToolConfig,
    proc: Arc<dyn ProcOps>,
    cwd: PathBuf,
    opts: BashOpts,
    params: serde_json::Value,
    /// `Execute a ${config.shellName} command …` (bash.ts:350) — interpolated, so it must be owned.
    description: String,
}

impl ShellTool {
    /// Pi's `createShellToolDefinition(cwd, config, options)` (bash.ts:338-345). [`BashOpts`] IS the
    /// shared options bag: upstream's factory takes `BashToolOptions` for BOTH shells, and
    /// `PowerShellToolOptions` (powershell.ts:29-30) is only the narrowed public surface on the
    /// PowerShell constructor.
    pub fn new(
        config: &'static ShellToolConfig,
        proc: Arc<dyn ProcOps>,
        cwd: PathBuf,
        opts: BashOpts,
    ) -> Self {
        // Byte-for-byte Pi's TypeBox emission (bash.ts:42-45): verbatim descriptions,
        // `type:"number"`, no `minimum`, no `additionalProperties`.
        let params = serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": { "type": "string", "description": config.command_description },
                "timeout": { "type": "number", "description": "Timeout in seconds (optional, no default timeout)" }
            }
        });
        // bash.ts:350 verbatim, with `${config.shellName}`, `${DEFAULT_MAX_LINES}` and
        // `${DEFAULT_MAX_BYTES / 1024}` interpolated. The two limits are Pi's MODULE constants, not
        // the per-call `opts` — upstream interpolates the constants even though its own truncation
        // point is configurable.
        let description = format!(
            "Execute a {} command in the current working directory. Returns stdout and stderr. \
             Output is truncated to last {} lines or {}KB (whichever is hit first). If truncated, \
             full output is saved to a temp file. Optionally provide a timeout in seconds.",
            config.shell_name,
            crate::truncate::DEFAULT_MAX_LINES,
            crate::truncate::DEFAULT_MAX_BYTES / 1024,
        );
        Self {
            config,
            proc,
            cwd,
            opts,
            params,
            description,
        }
    }

    /// Pi's `createBashToolDefinition` (bash.ts:529-534).
    pub fn bash(proc: Arc<dyn ProcOps>, cwd: PathBuf, opts: BashOpts) -> Self {
        Self::new(&BASH_CONFIG, proc, cwd, opts)
    }
}
```

**3b. The trait metadata reads the config.**

CURRENT ([bash.rs:83-107](../../../crates/cyrup-tools/src/tools/bash.rs)) — `impl Tool for BashTool`
with `fn name -> "bash"`, `fn label -> Some("bash")`, the hardcoded `fn description`, and
`fn prompt_snippet -> Some("Execute bash commands (ls, grep, find, etc.)")`.

REPLACEMENT — the doc comments on `label`, `description` and `prompt_snippet` are kept verbatim
(they carry the TOOL-045 rationale and the `bash.ts` citations); only the bodies change:

```rust
#[async_trait::async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        self.config.name
    }
    fn label(&self) -> Option<&str> {
        Some(self.config.label)
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some(self.config.prompt_snippet)
    }
```

CURRENT ([bash.rs:159-167](../../../crates/cyrup-tools/src/tools/bash.rs)) — the long
`prompt_guidelines` doc block (`PI_*` → `CYRUP_*` CYRUP-DELTA) is kept verbatim; only the body
changes:

```rust
    fn prompt_guidelines(&self) -> Vec<&str> {
        if self.opts.expose_session_environment {
            vec![
                "You can inspect CYRUP_* environment variables for current model and session details.",
            ]
        } else {
            Vec::new()
        }
    }
```

REPLACEMENT (add one sentence to the existing doc block: *"Both shell tools carry the identical
guideline — pi's `powershellToolSystemPromptContribution.guidelines` (powershell.ts:20) is the same
sentence as bash's (bash.ts:48) — so the dedup in the prompt builder emits it once when both tools
are selected."*):

```rust
    fn prompt_guidelines(&self) -> Vec<&str> {
        if self.opts.expose_session_environment {
            self.config.prompt_guidelines.to_vec()
        } else {
            Vec::new()
        }
    }
```

**3c. `execute` — three parameterised lines.**

CURRENT ([bash.rs:176-177](../../../crates/cyrup-tools/src/tools/bash.rs)):

```rust
        let input: BashInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("bash: {e}")))?;
```

REPLACEMENT:

```rust
        let input: BashInput = serde_json::from_value(params)
            .map_err(|e| error::invalid(format!("{}: {e}", self.config.name)))?;
```

CURRENT ([bash.rs:262](../../../crates/cyrup-tools/src/tools/bash.rs)):

```rust
        let mut acc = OutputAccumulator::new("cyrup-bash", max_lines, max_bytes);
```

REPLACEMENT:

```rust
        // `new OutputAccumulator({ tempFilePrefix: config.tempFilePrefix })` (bash.ts:364).
        let mut acc = OutputAccumulator::new(self.config.temp_file_prefix, max_lines, max_bytes);
```

CURRENT ([bash.rs:296-313](../../../crates/cyrup-tools/src/tools/bash.rs), post-sibling):

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

        let spec = ExecSpec {
            command: ctx.command,
            cwd: ctx.cwd,
            env: ctx.env,
            env_remove: ctx.env_remove,
            shell,
        };
```

REPLACEMENT:

```rust
        // Resolve the shell per-exec through the CONFIG's thunk — Pi's `resolveShellConfig`
        // parameter (bash.ts:84), which is `() => getShellConfig(options?.shellPath)` for bash
        // (bash.ts:159) and the bare `getPowerShellConfig` for PowerShell (powershell.ts:33). It is
        // called inside `exec`, AFTER `resolveTimeoutMs` and the abort check (bash.ts:85-91), so
        // every resolution error reaches the model as the tool result: `Custom shell path not
        // found: …` (shell.ts:73), the three-option `No bash shell found. Options: …` recipe
        // (shell.ts:100-106), `The powershell tool is only available on Windows.` (shell.ts:127),
        // and `No PowerShell executable found. …` (shell.ts:132). Pi's inner catch re-throws all of
        // them verbatim — none is an `"aborted"` nor a `"timeout:"` message, so it falls to
        // `throw err` (bash.ts:468) with NO status appended — and so does this `?`.
        let shell = (self.config.resolve_shell)(self.opts.shell_path.as_deref())?;

        // The per-shell command preamble goes on LAST, after `command_prefix`, after the session-env
        // assembly and after the spawn hook — because Pi applies it inside `operations.exec`
        // (powershell.ts:35), which is downstream of everything `resolveSpawnContext` and the hook
        // do (bash.ts:340-341,451). A hook therefore rewrites the model's command, never the UTF-8
        // preamble, and the preamble is never doubled.
        let command = match self.config.command_preamble {
            Some(preamble) => format!("{preamble}{}", ctx.command),
            None => ctx.command,
        };

        let spec = ExecSpec {
            command,
            cwd: ctx.cwd,
            env: ctx.env,
            env_remove: ctx.env_remove,
            shell,
        };
```

The module doc at [bash.rs:1-3](../../../crates/cyrup-tools/src/tools/bash.rs) gains a line saying
it now houses the shared factory, mirroring `bash.ts`.

### 4. `crates/cyrup-tools/src/tools/powershell.rs` — new file, config only

```rust
//! `powershell` — Pi's second built-in shell tool (`core/tools/powershell.ts`).
//!
//! There is no execution logic here and there must never be any. Everything except the values below
//! is [`super::bash::ShellTool`], exactly as upstream's `createPowerShellToolDefinition` is
//! `createShellToolDefinition` with a different `ShellToolConfig` (powershell.ts:49-57) and
//! `powershell.ts` imports its entire engine from `bash.ts`.

use super::bash::{ShellTool, ShellToolConfig};
use crate::config::{BashOpts, PowerShellOpts};
use crate::ops::{ProcOps, ShellConfig};
use cyrup_core::ToolError;
use std::path::PathBuf;
use std::sync::Arc;

/// `UTF8_OUTPUT_PREFIX` (powershell.ts:16) — verbatim, INCLUDING the trailing newline that
/// separates it from the model's command.
const UTF8_OUTPUT_PREFIX: &str =
    "try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}\n";

/// The `shellPath` setting names a BASH. Pi's `createLocalPowerShellOperations()` takes no options
/// at all (powershell.ts:32-33) and `PowerShellToolOptions` omits `shellPath` (powershell.ts:29-30),
/// so this resolver drops the argument rather than letting a bash path steer PowerShell.
fn resolve_powershell_ignoring_shell_path(
    _shell_path: Option<&str>,
) -> Result<ShellConfig, ToolError> {
    ShellConfig::resolve_powershell()
}

/// Pi's `powershellToolConfig` (powershell.ts:39-47).
pub static POWERSHELL_CONFIG: ShellToolConfig = ShellToolConfig {
    name: "powershell",
    label: "powershell",
    shell_name: "PowerShell",
    // v0.84.3 `bashSchema` (bash.ts:43) — the tag `powershell` exists at. See the
    // `command_description` CYRUP-DELTA on `ShellToolConfig`.
    command_description: "Shell command to execute",
    // powershell.ts:19.
    prompt_snippet: "Execute PowerShell commands",
    // powershell.ts:20, with the same `PI_*` → `CYRUP_*` divergence the bash guideline documents:
    // this sentence names the variables THIS tool injects into its own child, and cyrup injects
    // `CYRUP_*` while scrubbing `PI_*` unconditionally.
    prompt_guidelines: &[
        "You can inspect CYRUP_* environment variables for current model and session details.",
    ],
    temp_file_prefix: "cyrup-powershell",
    command_preamble: Some(UTF8_OUTPUT_PREFIX),
    resolve_shell: resolve_powershell_ignoring_shell_path,
};

impl ShellTool {
    /// Pi's `createPowerShellToolDefinition` (powershell.ts:49-57).
    pub fn powershell(proc: Arc<dyn ProcOps>, cwd: PathBuf, opts: PowerShellOpts) -> Self {
        Self::new(&POWERSHELL_CONFIG, proc, cwd, BashOpts::from(opts))
    }
}
```

### 5. `crates/cyrup-tools/src/tools/mod.rs`

CURRENT ([tools/mod.rs:1-20](../../../crates/cyrup-tools/src/tools/mod.rs)):

```rust
//! The seven built-in tools (DI-1). Each implements `cyrup_core::Tool`, including its model-facing
//! metadata (`description`/`prompt_snippet`/`prompt_guidelines`) verbatim from Pi.

pub mod bash;
pub mod edit;
pub mod edit_diff;
pub mod find;
mod globmatch;
pub mod grep;
pub mod ls;
pub mod read;
pub mod write;

pub use bash::BashTool;
pub use edit::EditTool;
pub use find::FindTool;
pub use grep::GrepTool;
pub use ls::LsTool;
pub use read::ReadTool;
pub use write::WriteTool;
```

REPLACEMENT:

```rust
//! The eight built-in tools (DI-1). Each implements `cyrup_core::Tool`, including its model-facing
//! metadata (`description`/`prompt_snippet`/`prompt_guidelines`) verbatim from Pi.
//!
//! `bash` and `powershell` are ONE type — [`bash::ShellTool`], Pi's `createShellToolDefinition`
//! (bash.ts:338-517) — instantiated from two [`bash::ShellToolConfig`] values. The engine lives in
//! [`bash`] and [`powershell`] holds only its config, mirroring upstream's own file split.

pub mod bash;
pub mod edit;
pub mod edit_diff;
pub mod find;
mod globmatch;
pub mod grep;
pub mod ls;
pub mod powershell;
pub mod read;
pub mod write;

pub use bash::{BASH_CONFIG, ShellTool, ShellToolConfig};
pub use edit::EditTool;
pub use find::FindTool;
pub use grep::GrepTool;
pub use ls::LsTool;
pub use powershell::POWERSHELL_CONFIG;
pub use read::ReadTool;
pub use write::WriteTool;
```

### 6. `crates/cyrup-tools/src/config.rs` — `PowerShellOpts` and the `ToolsOptions` slot

Add after the `BashOpts` `Debug` impl
([config.rs:269](../../../crates/cyrup-tools/src/config.rs)):

```rust
/// Pi's `PowerShellToolOptions` (powershell.ts:29-30):
/// `Pick<BashToolOptions, "operations" | "exposeSessionEnvironment" | "spawnHook">`.
///
/// Deliberately NO `shell_path` and NO `command_prefix`. `createPowerShellToolDefinition` forwards
/// only those three keys (powershell.ts:53-56) and `createLocalPowerShellOperations()` accepts no
/// options at all (powershell.ts:32-33), so the settings `shellPath` — which names a bash — can
/// never reach PowerShell. Making them unrepresentable here is what enforces that.
///
/// `bin_dir` IS present: `resolveSpawnContext` is SHARED by both shell tools (bash.ts:341,168) and
/// `getShellEnv()` prepends `getBinDir()` to `PATH` for every shell child (shell.ts:138-150), so a
/// binary cyrup manages into `<agent_dir>/bin` must be on PATH for `powershell` too.
#[derive(Clone)]
pub struct PowerShellOpts {
    pub max_lines: usize,
    pub max_bytes: usize,
    pub bin_dir: Option<PathBuf>,
    pub spawn_hook: Option<BashSpawnHook>,
    pub expose_session_environment: bool,
    pub session_env: Option<SessionEnvHandle>,
}

impl Default for PowerShellOpts {
    fn default() -> Self {
        Self {
            max_lines: DEFAULT_MAX_LINES,
            max_bytes: DEFAULT_MAX_BYTES,
            bin_dir: None,
            spawn_hook: None,
            // Pi: `options?.exposeSessionEnvironment ?? true` (bash.ts:342).
            expose_session_environment: true,
            session_env: None,
        }
    }
}

impl std::fmt::Debug for PowerShellOpts {
    // Manual for the same reason as `BashOpts`: `spawn_hook` is a boxed closure.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PowerShellOpts")
            .field("max_lines", &self.max_lines)
            .field("max_bytes", &self.max_bytes)
            .field("bin_dir", &self.bin_dir)
            .field("spawn_hook", &self.spawn_hook.as_ref().map(|_| "<hook>"))
            .field(
                "expose_session_environment",
                &self.expose_session_environment,
            )
            .field(
                "session_env",
                &self.session_env.as_ref().map(SessionEnvHandle::get),
            )
            .finish()
    }
}

/// Pi's factory takes `BashToolOptions` for BOTH shells (bash.ts:338-341); `PowerShellToolOptions`
/// is only the narrowed public surface. This widening is that fact in Rust — and it is where the
/// two omitted keys are pinned to `None` rather than merely left unset.
impl From<PowerShellOpts> for BashOpts {
    fn from(o: PowerShellOpts) -> Self {
        Self {
            max_lines: o.max_lines,
            max_bytes: o.max_bytes,
            command_prefix: None,
            shell_path: None,
            bin_dir: o.bin_dir,
            spawn_hook: o.spawn_hook,
            expose_session_environment: o.expose_session_environment,
            session_env: o.session_env,
        }
    }
}
```

CURRENT ([config.rs:316-325](../../../crates/cyrup-tools/src/config.rs)):

```rust
#[derive(Clone, Debug, Default)]
pub struct ToolsOptions {
    pub read: ReadOpts,
    pub write: WriteOpts,
    pub edit: EditOpts,
    pub bash: BashOpts,
    pub grep: GrepOpts,
    pub find: FindOpts,
    pub ls: LsOpts,
}
```

REPLACEMENT:

```rust
#[derive(Clone, Debug, Default)]
pub struct ToolsOptions {
    pub read: ReadOpts,
    pub write: WriteOpts,
    pub edit: EditOpts,
    pub bash: BashOpts,
    /// Pi `ToolsOptions.powershell` (index.ts:110).
    pub powershell: PowerShellOpts,
    pub grep: GrepOpts,
    pub find: FindOpts,
    pub ls: LsOpts,
}
```

### 7. `crates/cyrup-tools/src/registry.rs` — the eighth builtin, in pi's position

CURRENT ([registry.rs:13-20](../../../crates/cyrup-tools/src/registry.rs)):

```rust
/// The closed set of built-in tool names (DI-1), in Pi's declaration order.
///
/// Pi's `createAllToolDefinitions` returns its object literal as `read, bash, edit, write, grep,
/// find, ls` (`coding-agent/src/core/tools/index.ts:156-166`), and object-literal insertion order is
/// the order `Object.values()` / the tool registry replays. That order reaches the wire: it is the
/// order of the `tools` array in every provider request and of the tool list rendered into the
/// system prompt, both of which the model conditions on.
pub const BUILTIN_NAMES: [&str; 7] = ["read", "bash", "edit", "write", "grep", "find", "ls"];
```

REPLACEMENT:

```rust
/// The closed set of built-in tool names (DI-1), in Pi's declaration order.
///
/// Pi's `createAllToolDefinitions` returns its object literal as `read, bash, powershell, edit,
/// write, grep, find, ls` (`coding-agent/src/core/tools/index.ts:182-193`, matching `allToolNames`
/// at `:96-105`), and object-literal insertion order is the order `Object.values()` / the tool
/// registry replays. That order reaches the wire: it is the order of the `tools` array in every
/// provider request and of the tool list rendered into the system prompt, both of which the model
/// conditions on. `powershell` therefore goes THIRD, immediately after `bash` — not appended.
pub const BUILTIN_NAMES: [&str; 8] = [
    "read",
    "bash",
    "powershell",
    "edit",
    "write",
    "grep",
    "find",
    "ls",
];
```

CURRENT ([registry.rs:53-75](../../../crates/cyrup-tools/src/registry.rs), post-sibling — the
`ShellConfig::detect()` line and the `shell` argument are already gone):

```rust
    /// Build the default registry with the seven built-ins over `backend` (arch-03 §3.4).
    …
    pub fn with_builtins(cwd: PathBuf, backend: Backend, opts: ToolsOptions) -> Self {
        let mut reg = Self::new();
        let locks = Arc::new(FileMutationLocks::new());

        // Insertion order IS presentation order (see `insert`/`all`/`visible` below), and it must be
        // Pi's `createAllToolDefinitions` literal order — read, bash, edit, write, grep, find, ls
        // (`coding-agent/src/core/tools/index.ts:156-166`). It also fixes the two derived sets for
        // free: filtering this order to {read,bash,edit,write} reproduces `createCodingTools`
        // (index.ts:169-176) and to {read,grep,find,ls} reproduces `createReadOnlyToolDefinitions`
        // (index.ts:147-154).
        reg.insert(Arc::new(ReadTool::new(
            backend.fs.clone(),
            cwd.clone(),
            opts.read,
        )));
        reg.insert(Arc::new(BashTool::new(
            backend.proc.clone(),
            cwd.clone(),
            opts.bash,
        )));
```

REPLACEMENT:

```rust
    /// Build the default registry with the eight built-ins over `backend` (arch-03 §3.4).
    …
    pub fn with_builtins(cwd: PathBuf, backend: Backend, opts: ToolsOptions) -> Self {
        let mut reg = Self::new();
        let locks = Arc::new(FileMutationLocks::new());

        // Insertion order IS presentation order (see `insert`/`all`/`visible` below), and it must be
        // Pi's `createAllToolDefinitions` literal order — read, bash, powershell, edit, write, grep,
        // find, ls (`coding-agent/src/core/tools/index.ts:182-193`). It also fixes the two derived
        // sets for free: filtering this order to {read,bash,edit,write} reproduces
        // `createCodingTools` (index.ts:195-202) and to {read,grep,find,ls} reproduces
        // `createReadOnlyToolDefinitions` (index.ts:173-180). Neither derived set contains
        // `powershell`, exactly as upstream.
        reg.insert(Arc::new(ReadTool::new(
            backend.fs.clone(),
            cwd.clone(),
            opts.read,
        )));
        reg.insert(Arc::new(ShellTool::bash(
            backend.proc.clone(),
            cwd.clone(),
            opts.bash,
        )));
        // Registered on EVERY platform. Pi builds the definition unconditionally
        // (`createAllToolDefinitions`, index.ts:186) and only `getPowerShellConfig` is Windows-gated
        // (shell.ts:126-128), so the tool is always NAMEABLE and reports its own refusal as a tool
        // result. Gating registration on `cfg!(windows)` would make `--tools powershell` silently
        // select nothing off-Windows instead of saying why.
        reg.insert(Arc::new(ShellTool::powershell(
            backend.proc.clone(),
            cwd.clone(),
            opts.powershell,
        )));
```

The import at [registry.rs:7](../../../crates/cyrup-tools/src/registry.rs) becomes:

```rust
use crate::tools::{EditTool, FindTool, GrepTool, LsTool, ReadTool, ShellTool, WriteTool};
```

`coding_tools` ([registry.rs:143-151](../../../crates/cyrup-tools/src/registry.rs)) and
`read_only_tools` ([registry.rs:153-161](../../../crates/cyrup-tools/src/registry.rs)) are
**unchanged** — neither allowlist names `powershell`, matching `createCodingTools` and
`createReadOnlyTools`. The `all_tools` doc comment at
[registry.rs:163](../../../crates/cyrup-tools/src/registry.rs) reads "All seven built-in tools." →
"All eight built-in tools."

### 8. `crates/cyrup-tools/src/lib.rs` — exports

- The crate doc at [lib.rs:3-4](../../../crates/cyrup-tools/src/lib.rs) lists the default set; add
  `powershell` after `bash` and update "deliberately minimal default tool set" to note that
  `powershell` is registered but off by default.
- [lib.rs:35-38](../../../crates/cyrup-tools/src/lib.rs): add `PowerShellOpts` to the `config`
  re-export list.

### 9. `crates/cyrup-session-svc/src/builder.rs` — keep it OFF by default, and wire its options

This is the change that makes the whole tool opt-in, and it is **not** optional. `select_active_tools`'s
default arm is

```rust
            (None, None) => {
                DEFAULT_BUILTIN_TOOLS.contains(&name) || !ALL_BUILTIN_TOOLS.contains(&name)
            }
```

([builder.rs:355-357](../../../crates/cyrup-session-svc/src/builder.rs)). A name that is **not** in
`ALL_BUILTIN_TOOLS` is treated as an extension/embedder tool and stays **active**. So if
`powershell` is registered without being added to that constant, it would be enabled by default in
every session — the exact opposite of pi, whose `defaultActiveToolNames` is `read/bash/edit/write`
([sdk.ts:256](../../../tmp/pi/packages/coding-agent/src/core/sdk.ts)).

CURRENT ([builder.rs:315-322](../../../crates/cyrup-session-svc/src/builder.rs)):

```rust
/// Every tool `ToolRegistry::with_builtins` installs (`cyrup-tools/src/registry.rs:45-67`).
///
/// Needed to tell "a built-in pi does not activate by default" (`grep`/`find`/`ls`) apart from "a
/// non-built-in tool" (an extension- or embedder-supplied one), which must stay active: pi's
/// `defaultActiveToolNames` gates only its own built-ins and never suppresses a tool the host
/// registered.
const ALL_BUILTIN_TOOLS: [&str; 7] =
    ["read", "write", "edit", "bash", "grep", "find", "ls"];
```

REPLACEMENT:

```rust
/// Every tool `ToolRegistry::with_builtins` installs (`cyrup-tools/src/registry.rs:53-100`).
///
/// Needed to tell "a built-in pi does not activate by default" (`powershell`/`grep`/`find`/`ls`)
/// apart from "a non-built-in tool" (an extension- or embedder-supplied one), which must stay
/// active: pi's `defaultActiveToolNames` gates only its own built-ins and never suppresses a tool
/// the host registered.
///
/// `powershell` MUST be listed here. The default arm of `select_active_tools` keeps any name it
/// does not recognise as a built-in, so omitting it would enable PowerShell in every session —
/// while pi's default set is `read`/`bash`/`edit`/`write` (sdk.ts:256) and `powershell` is reachable
/// only through `--tools` / `defaultTools`.
const ALL_BUILTIN_TOOLS: [&str; 8] =
    ["read", "write", "edit", "bash", "powershell", "grep", "find", "ls"];
```

CURRENT ([builder.rs:898-918](../../../crates/cyrup-session-svc/src/builder.rs)):

```rust
                bash: BashOpts {
                    command_prefix: shell_command_prefix_setting.clone(),
                    shell_path: shell_path_setting.clone(),
                    session_env: Some(bash_session_env.clone()),
                    …
                    bin_dir: Some(cfg.agent_dir.join("bin")),
                    ..BashOpts::default()
                },
                ..ToolsOptions::default()
```

REPLACEMENT:

```rust
                bash: BashOpts {
                    command_prefix: shell_command_prefix_setting.clone(),
                    shell_path: shell_path_setting.clone(),
                    session_env: Some(bash_session_env.clone()),
                    …
                    bin_dir: Some(cfg.agent_dir.join("bin")),
                    ..BashOpts::default()
                },
                // The `powershell` tool shares `resolveSpawnContext` with `bash` (bash.ts:341), so
                // it gets the same live session handle and the same managed `<agent_dir>/bin` on
                // PATH. It does NOT get `shellPath` or `shellCommandPrefix`: `PowerShellOpts` has
                // no such fields, because `createLocalPowerShellOperations()` takes no options
                // (powershell.ts:32-33) and the `shellPath` setting names a bash.
                powershell: cyrup_tools::config::PowerShellOpts {
                    session_env: Some(bash_session_env.clone()),
                    bin_dir: Some(cfg.agent_dir.join("bin")),
                    ..cyrup_tools::config::PowerShellOpts::default()
                },
                ..ToolsOptions::default()
```

### 10. `crates/cyrup-session/src/prompt/builder.rs` — the three-way file-exploration branch

CURRENT ([builder.rs:88-90](../../../crates/cyrup-session/src/prompt/builder.rs)):

```rust
    guidelines_header: &'static str,
    baseline_guidelines: &'static [&'static str],
    bash_fallback_guideline: &'static str,
```

REPLACEMENT:

```rust
    guidelines_header: &'static str,
    baseline_guidelines: &'static [&'static str],
    /// Pi `system-prompt.ts:105-112` — a THREE-way branch over `hasBash`/`hasPowerShell`, not one
    /// string. Whichever shell tools are selected, the bullet names them.
    bash_fallback_guideline: &'static str,
    powershell_fallback_guideline: &'static str,
    bash_or_powershell_fallback_guideline: &'static str,
```

CURRENT ([builder.rs:110](../../../crates/cyrup-session/src/prompt/builder.rs)):

```rust
    bash_fallback_guideline: "Use bash for file operations like ls, rg, find",
```

REPLACEMENT (all three verbatim from `system-prompt.ts:107,109,111`):

```rust
    bash_fallback_guideline: "Use bash for file operations like ls, rg, find",
    powershell_fallback_guideline:
        "Use PowerShell for file operations like listing, searching, and finding files",
    bash_or_powershell_fallback_guideline:
        "Use bash or PowerShell for file operations like listing, searching, and finding files",
```

CURRENT ([builder.rs:222-226](../../../crates/cyrup-session/src/prompt/builder.rs)):

```rust
        // 3a. conditional file-exploration fallback
        let has = |n: &str| is_selected(inp.selected_tools.as_ref(), n);
        if has("bash") && !has("grep") && !has("find") && !has("ls") {
            push_guideline(out, &mut seen, t.bash_fallback_guideline);
        }
```

REPLACEMENT:

```rust
        // 3a. conditional file-exploration fallback (Pi `system-prompt.ts:97-113`). The gate is
        // `(hasBash || hasPowerShell)`, and the bullet names whichever shells are actually selected
        // — a PowerShell-only session must not be told to use `ls, rg, find`.
        let has = |n: &str| is_selected(inp.selected_tools.as_ref(), n);
        let has_bash = has("bash");
        let has_powershell = has("powershell");
        if (has_bash || has_powershell) && !has("grep") && !has("find") && !has("ls") {
            let guideline = if has_bash && has_powershell {
                t.bash_or_powershell_fallback_guideline
            } else if has_powershell {
                t.powershell_fallback_guideline
            } else {
                t.bash_fallback_guideline
            };
            push_guideline(out, &mut seen, guideline);
        }
```

`DEFAULT_SELECTED_TOOLS` ([builder.rs:324](../../../crates/cyrup-session/src/prompt/builder.rs))
stays `["read", "bash", "edit", "write"]` — pi's `system-prompt.ts:81` fallback is unchanged and
`powershell` is not in it.

### 11. `crates/cyrup-permission-system/src/sanitize/tools.rs` — the two new bullets must be droppable

The sanitizer removes a guideline bullet when its tool is no longer exposed; an unmatched bullet is
always **kept** ([tools.rs:31-52](../../../crates/cyrup-permission-system/src/sanitize/tools.rs)).
Adding two new PowerShell bullets without adding their rules would leak a guideline naming a tool
the model cannot call. Add two arms to `guideline_keep_rule` (the match key is the normalized —
trimmed, bullet-stripped, whitespace-collapsed, **lowercased** — line, per
[`normalize_guideline_text`](../../../crates/cyrup-permission-system/src/sanitize/tools.rs)):

CURRENT:

```rust
        "use bash for file operations like ls, rg, find" => Some(has("bash")),
```

REPLACEMENT:

```rust
        "use bash for file operations like ls, rg, find" => Some(has("bash")),
        "use powershell for file operations like listing, searching, and finding files" => {
            Some(has("powershell"))
        }
        "use bash or powershell for file operations like listing, searching, and finding files" => {
            Some(has("bash") && has("powershell"))
        }
```

### 12. `crates/cyrup/src/cli/help.rs` — the built-in tool name list

CURRENT ([help.rs:213-220](../../../crates/cyrup/src/cli/help.rs)):

```
Built-in Tool Names:
  read   - Read file contents
  bash   - Execute bash commands
  edit   - Edit files with find/replace
  write  - Write files (creates/overwrites)
  grep   - Search file contents (read-only, off by default)
  find   - Find files by glob pattern (read-only, off by default)
  ls     - List directory contents (read-only, off by default)
```

REPLACEMENT — `args.ts:437-445` verbatim, including the re-padding to the width of the longest
name, which is what makes the columns line up once `powershell` is present:

```
Built-in Tool Names:
  read       - Read file contents
  bash       - Execute bash commands
  powershell - Execute PowerShell commands on Windows
  edit       - Edit files with find/replace
  write      - Write files (creates/overwrites)
  grep       - Search file contents (read-only, off by default)
  find       - Find files by glob pattern (read-only, off by default)
  ls         - List directory contents (read-only, off by default)
```

The `--tools` / `--exclude-tools` entries at
[help.rs:79-82](../../../crates/cyrup/src/cli/help.rs) already say "Comma-separated
allowlist/denylist of tool names" and need no change — this block is the list they refer to. The
tagline at [help.rs:41](../../../crates/cyrup/src/cli/help.rs) ("read, bash, edit, write tools")
also stays: it names the DEFAULT set, and `powershell` is not in it.

### 13. `crates/cyrup-tui/src/transcript/` — `PS>` instead of `$`

CURRENT ([tool_render.rs:40](../../../crates/cyrup-tui/src/transcript/tool_render.rs)):

```rust
            "bash" => render_bash(run, expanded, theme, images.expand_key, &mut block),
```

REPLACEMENT — the shell prompt becomes an argument, mirroring Pi's
`formatShellCall(args, config.prompt)` (bash.ts:488) with `"$"` (bash.ts:523) and `"PS>"`
(powershell.ts:43):

```rust
            "bash" => render_bash(run, expanded, theme, images.expand_key, "$", &mut block),
            "powershell" => render_bash(run, expanded, theme, images.expand_key, "PS>", &mut block),
```

CURRENT ([tool_builtin.rs:214-234](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs)):

```rust
pub(super) fn render_bash(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    expand_key: &str,
    out: &mut Vec<Line<'static>>,
) {
    // Header: `$ command`, bold, + a muted ` (timeout Ns)` suffix (`formatBashCall`).
    let title = theme.tool_title_style();
    let mut spans = Vec::new();
    match str_arg(&run.args, &["command"]) {
        StrArg::Invalid => {
            spans.push(Span::styled("$ ".to_string(), title));
            spans.push(Span::styled("[invalid arg]".to_string(), theme.error_style()));
        }
        StrArg::Missing => {
            spans.push(Span::styled("$ ".to_string(), title));
            spans.push(Span::styled("...".to_string(), theme.tool_output_style()));
        }
        StrArg::Value(cmd) => spans.push(Span::styled(format!("$ {cmd}"), title)),
    }
```

REPLACEMENT:

```rust
pub(super) fn render_bash(
    run: &ToolRun,
    expanded: bool,
    theme: &UiTheme,
    expand_key: &str,
    prompt: &str,
    out: &mut Vec<Line<'static>>,
) {
    // Header: `<prompt> command`, bold, + a muted ` (timeout Ns)` suffix (`formatShellCall`,
    // bash.ts:238-244, called with `config.prompt` at bash.ts:488 — `$` for bash, `PS>` for
    // PowerShell).
    let title = theme.tool_title_style();
    let mut spans = Vec::new();
    match str_arg(&run.args, &["command"]) {
        StrArg::Invalid => {
            spans.push(Span::styled(format!("{prompt} "), title));
            spans.push(Span::styled("[invalid arg]".to_string(), theme.error_style()));
        }
        StrArg::Missing => {
            spans.push(Span::styled(format!("{prompt} "), title));
            spans.push(Span::styled("...".to_string(), theme.tool_output_style()));
        }
        StrArg::Value(cmd) => spans.push(Span::styled(format!("{prompt} {cmd}"), title)),
    }
```

The doc comment at
[tool_builtin.rs:212-213](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs) updates from
"`bash` — header `$ <command>`" to "`bash`/`powershell` — header `<prompt> <command>`".
[cyrup-tui/src/bash.rs:270](../../../crates/cyrup-tui/src/bash.rs) is the `/bash` slash-command
widget and stays `$`.

---

## Files changed

| file | change |
|---|---|
| [crates/cyrup-tools/src/ops/shell.rs](../../../crates/cyrup-tools/src/ops/shell.rs) | `ShellConfig.shell_name`; `find_bash_on_path` → `find_executable_on_path(exe)`; `POWERSHELL_ARGS`; `powershell_detect_from`; `ShellConfig::resolve_powershell` |
| [crates/cyrup-tools/src/ops/local/proc.rs](../../../crates/cyrup-tools/src/ops/local/proc.rs) | missing-cwd error interpolates `spec.shell.shell_name` |
| [crates/cyrup-tools/src/tools/bash.rs](../../../crates/cyrup-tools/src/tools/bash.rs) | `BashTool` → `ShellTool` + `ShellToolConfig` + `BASH_CONFIG`; owned interpolated `description`; config-driven name/label/schema/snippet/guidelines/temp-prefix; `execute` resolves through `config.resolve_shell` and applies `config.command_preamble` |
| [crates/cyrup-tools/src/tools/powershell.rs](../../../crates/cyrup-tools/src/tools/powershell.rs) | NEW — `UTF8_OUTPUT_PREFIX`, `POWERSHELL_CONFIG`, `ShellTool::powershell` |
| [crates/cyrup-tools/src/tools/mod.rs](../../../crates/cyrup-tools/src/tools/mod.rs) | declare `powershell`; export `ShellTool`/`ShellToolConfig`/`BASH_CONFIG`/`POWERSHELL_CONFIG` |
| [crates/cyrup-tools/src/config.rs](../../../crates/cyrup-tools/src/config.rs) | `PowerShellOpts` + `Default`/`Debug`/`From<PowerShellOpts> for BashOpts`; `ToolsOptions.powershell` |
| [crates/cyrup-tools/src/registry.rs](../../../crates/cyrup-tools/src/registry.rs) | `BUILTIN_NAMES` → 8 with `powershell` third; insert `ShellTool::powershell` after `ShellTool::bash`; import swap |
| [crates/cyrup-tools/src/lib.rs](../../../crates/cyrup-tools/src/lib.rs) | re-export `PowerShellOpts`; crate doc |
| [crates/cyrup-session-svc/src/builder.rs](../../../crates/cyrup-session-svc/src/builder.rs) | `ALL_BUILTIN_TOOLS` → 8 (this is what keeps it opt-in); `ToolsOptions.powershell` wiring |
| [crates/cyrup-session/src/prompt/builder.rs](../../../crates/cyrup-session/src/prompt/builder.rs) | two new template strings; three-way file-exploration branch |
| [crates/cyrup-permission-system/src/sanitize/tools.rs](../../../crates/cyrup-permission-system/src/sanitize/tools.rs) | two new `guideline_keep_rule` arms |
| [crates/cyrup/src/cli/help.rs](../../../crates/cyrup/src/cli/help.rs) | Built-in Tool Names block: add `powershell`, re-pad to width 10 |
| [crates/cyrup-tui/src/transcript/tool_render.rs](../../../crates/cyrup-tui/src/transcript/tool_render.rs) | dispatch `"powershell"` to `render_bash` with `PS>` |
| [crates/cyrup-tui/src/transcript/tool_builtin.rs](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs) | `render_bash` takes the shell prompt as a parameter |

**Ordering:** land
[LOW-on-windows-with-no-bash-installed-pi-s-actionable-no-bash-shell-found-er.md](./LOW-on-windows-with-no-bash-installed-pi-s-actionable-no-bash-shell-found-er.md)
first. Within this task, `ops/shell.rs` before `tools/bash.rs` before `tools/powershell.rs` before
`registry.rs`; `builder.rs`'s `ALL_BUILTIN_TOOLS` must land in the same change as `registry.rs`'s
`BUILTIN_NAMES`, or PowerShell is briefly on by default.

---

## Genuinely uncertain

- **Windows behaviour cannot be exercised from this workspace.** `resolve_powershell`'s live arm is
  `#[cfg(windows)]`, so only `powershell_detect_from` (hoisted for exactly this reason) is built
  here. The `pwsh.exe`-then-`powershell.exe` preference, the `where` probe and the real
  `-Command` argv can be argued from the code but not run.
- **The MCP name-collision lists are left alone.**
  [cyrup-mcp/src/registration.rs:126](../../../crates/cyrup-mcp/src/registration.rs) and
  [cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:121](../../../crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs)
  both pin `["read","bash","edit","write","grep","find","ls","mcp"]` as the names an MCP tool may
  not take. They are ports of a pi `direct-tools.ts` that is **not** in the vendored 0.84.3 tree, so
  whether upstream added `powershell` there cannot be checked. Left unchanged deliberately; the
  consequence is that an MCP server could still format a tool as `powershell` and shadow the
  built-in.
- **`defaultTools` is not read at all.** Pi's `configuredDefaultToolNames =
  settingsManager.getDefaultTools()` ([sdk.ts:257](../../../tmp/pi/packages/coding-agent/src/core/sdk.ts))
  is the documented way to make `powershell` default on a Windows box; cyrup's
  `select_active_tools` default arm uses a constant and reads no setting. That is a separate,
  pre-existing gap and is not opened here — after this task, `--tools read,powershell,edit,write`
  is the way in.
- **`ShellConfig`'s public fields gain one member.** Any out-of-workspace embedder building a
  `ShellConfig` literal breaks. None is visible in this repo, so the field is added rather than
  hidden behind a constructor.

---

## Definition of done

Observable behaviour, on a session built through the normal path:

1. A tool named `powershell` exists and is offered to the model in eighth-of-eight position,
   immediately after `bash`, whenever it is selected. `--tools read,bash,powershell,edit,write`
   yields a five-tool session in that order.
2. With no `--tools`/`--no-tools`/`--exclude-tools`, the model-visible set is exactly
   `read`, `bash`, `edit`, `write` — `powershell` is absent, and adding it changes nothing about
   which tools the other four are or the order they appear in.
3. `--exclude-tools powershell` removes it from a session that named it in `--tools`.
4. Off Windows, every `powershell` call returns an error whose text is exactly
   `The powershell tool is only available on Windows.` — no prefix, no
   `Command exited with code …` / `Command aborted` / `Command timed out …` suffix, and no output
   body. The session and the other tools keep working.
5. On Windows with neither executable on `PATH`, every `powershell` call returns exactly
   `No PowerShell executable found. Install PowerShell or add powershell.exe/pwsh.exe to PATH.`
6. On Windows with both present, `pwsh.exe` is the program that runs; with only `powershell.exe`
   present, that one runs.
7. The child process is invoked with the arguments
   `-NoProfile`, `-NonInteractive`, `-ExecutionPolicy`, `Bypass`, `-Command`, in that order, and the
   command as the single trailing argument — never over stdin.
8. That trailing argument begins with
   `try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}` followed by a newline
   and then the model's command, on every call. A registered spawn hook sees the model's command
   **without** the preamble and cannot duplicate or displace it. Non-ASCII output from a
   `powershell` call comes back as correct UTF-8.
9. A `bash` call's command is byte-identical to today's — no preamble is added to it.
10. `cyrup --help` lists `powershell - Execute PowerShell commands on Windows` between `bash` and
    `edit`, and all eight names and their descriptions are column-aligned.
11. A session whose selected tools include `powershell` but none of `grep`/`find`/`ls` shows the
    guideline `Use PowerShell for file operations like listing, searching, and finding files`; with
    `bash` also selected it shows
    `Use bash or PowerShell for file operations like listing, searching, and finding files` instead;
    with only `bash` it still shows `Use bash for file operations like ls, rg, find`. Exactly one of
    the three appears, and none appears once `grep`, `find` or `ls` is selected.
12. When the permission layer hides `powershell` from a prompt, whichever of those bullets was
    emitted is removed from that prompt too.
13. `powershell`'s "Available tools" line reads `- powershell: Execute PowerShell commands`, and its
    description reads
    `Execute a PowerShell command in the current working directory. Returns stdout and stderr. Output is truncated to last 2000 lines or 50KB (whichever is hit first). If truncated, full output is saved to a temp file. Optionally provide a timeout in seconds.`
    `bash`'s description, snippet, schema and guideline are unchanged from today.
14. A `powershell` call against a working directory that does not exist returns
    `Working directory does not exist: <path>` followed by a newline and
    `Cannot execute PowerShell commands.`; the same call through `bash` still says
    `Cannot execute bash commands.`
15. A `shellPath` setting pointing at any interpreter changes only what `bash` runs. `powershell`
    resolution is unaffected by it, and a `shellCommandPrefix` setting is not prepended to
    `powershell` commands.
16. Timeout handling, cancellation, process-tree kill, the 100 ms streaming cadence, tail
    truncation, the spill-file footers and the `CYRUP_*` / `PI_*`-scrub environment behaviour are
    identical for `powershell` and `bash`; a truncated `powershell` run spills to a file whose name
    carries the `cyrup-powershell` prefix and a truncated `bash` run to one carrying `cyrup-bash`.
17. In the transcript a `powershell` call renders as `PS> <command>` and a `bash` call as
    `$ <command>`; the timeout suffix, output tail, expand/collapse and `Took …` footer behave the
    same for both.
18. `/bash` and the `executeBash` RPC still run bash and are unchanged.
