---
title: The entire powershell built-in tool is missing from cyrup
priority: MEDIUM
tool: powershell
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# The entire `powershell` built-in tool is missing from cyrup

> **Merged finding.** Two independent lanes (bash, support) reported this. They agreed on the
> facts and disagreed on severity: one rated it **medium**, the other **low** with a specific
> argument recorded below. Filed at MEDIUM because it is the single largest unit of work in this
> backlog — a whole tool — but read the downgrade argument before prioritising it.

## What pi does

pi ships `powershell` as a first-class built-in alongside `bash`. `ToolName` is `"read" | "bash" | "powershell" | "edit" | "write" | "grep" | "find" | "ls"` (tools/index.ts:95) and `allToolNames` contains all eight (tools/index.ts:96-105). It is constructible by name (`createToolDefinition`/`createTool` cases at tools/index.ts:124-125 and 146-147) and is included in `createAllToolDefinitions` (tools/index.ts:186) and `createAllTools` (tools/index.ts:220). The tool itself is defined at tools/powershell.ts:39-67: name/label `powershell`, `shellName: "PowerShell"`, prompt `"PS>"`, `tempFilePrefix: "pi-powershell"`, snippet `"Execute PowerShell commands"` (powershell.ts:19), and its own local operations (`createLocalPowerShellOperations`, powershell.ts:32-37) which prepend `try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}\n` to every command (powershell.ts:16) and resolve `pwsh.exe`/`powershell.exe` via `getPowerShellConfig` (utils/shell.ts:125-136, Windows-only with explicit error text). It is documented as a built-in tool name in `--help` (cli/args.ts:440).

## What cyrup-tools does

cyrup has seven built-ins only. `BUILTIN_NAMES` is `["read", "bash", "edit", "write", "grep", "find", "ls"]` (crates/cyrup-tools/src/registry.rs:20) and `ToolRegistry::with_builtins` inserts exactly those seven (registry.rs:65-98). `crates/cyrup-tools/src/tools/mod.rs:4-20` declares/exports no `powershell` module. `ToolsOptions` has no `powershell` field (crates/cyrup-tools/src/config.rs:317-325). `crates/cyrup-tools/src/ops/shell.rs` contains no `pwsh`/`powershell` resolution (grep for `pwsh|powershell|PowerShell` in that file returns nothing; the only crate-wide hits are `cyrup-tui/src/theme.rs` syntax mapping and `cyrup-ext/src/caps/proc.rs`). The CLI help lists only seven built-in tool names (crates/cyrup/src/cli/help.rs:213-220).

## User-visible impact

On Windows a model has no native shell tool at all: pi exposes `powershell` (and `bash` only where a bash exists), cyrup exposes only `bash`. A caller cannot name, enable, or configure `powershell`; every PowerShell invocation, the UTF-8 console-encoding preamble, and the `PS>` call rendering are unavailable.

## Parity action

Add a `PowerShellTool` in `crates/cyrup-tools/src/tools/powershell.rs` over the existing bash execution engine parameterised by shell name/prompt/temp-prefix (pi's `ShellToolConfig`, bash.ts:328-336), resolving `pwsh.exe`/`powershell.exe` on Windows with pi's error strings, prefixing the UTF-8 OutputEncoding line to each command; add `"powershell"` to `BUILTIN_NAMES` in pi's literal position (after `bash`), add `PowerShellOpts { operations, expose_session_environment, spawn_hook }` to `ToolsOptions`, and list it in `crates/cyrup/src/cli/help.rs`.

## Why this gap is real

> Genuinely absent from the Rust. crates/cyrup-tools/src/tools/mod.rs declares only bash/edit/edit_diff/find/globmatch/grep/ls/read/write; registry.rs:20 BUILTIN_NAMES is 7 names and with_builtins (registry.rs:56-98) inserts exactly those 7; ToolsOptions (config.rs:317-325) has no powershell field. ops/shell.rs ShellConfig::try_detect resolves ONLY bash (/bin/bash -> which bash -> sh -c on unix; Git Bash candidates -> where bash.exe -> hard error "No bash shell found" on Windows) with no pwsh.exe/powershell.exe probe and no -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command arg set. BashTool is monomorphic (fn name -> "bash", hardcoded "Bash command to execute" schema at bash.rs:69,85) and cyrup has no analogue of pi's generic ShellToolConfig/createShellToolDefinition factory (grep for ShellToolConfig|ShellTool|shell_tool across all crates: zero hits), so no parameterized shell tool exists that a PowerShell variant could be instantiated from. Repo-wide case-insensitive rg for powershell|pwsh over crates/ yields exactly one hit, cyrup-tui/src/theme.rs ("ps1" syntax mapping). I also checked the sanitized-identifier angle (pi's tool surfaces as "il" in the tmp/pi docs corpus): no IlTool, no "il" tool name, and no UTF-8 [Console]::OutputEncoding preamble anywhere in cyrup. Severity lowered to medium because the claimed impact is overstated: cyrup DOES provide a shell tool on Windows (Git Bash detection, where bash.exe, plus the shellPath setting honored by ShellConfig::resolve, which a user can point at pwsh.exe, and the model can run powershell.exe -Command ... from bash). Pi's powershell tool is Windows-only and opt-in via defaultTools, not on by default. Nothing is silently wrong - the tool is merely unnameable. The real blocker is narrow: a Windows machine with no bash at all, where pi lets the user select powershell and cyrup hard-errors at session construction.

## The severity-downgrade argument (second adversary)

> Confirmed absent after an exhaustive search. `rg -in "powershell|pwsh"` over all of /home/user/cyrup/crates yields only two irrelevant hits (cyrup-tui/src/theme.rs:1608 syntax-highlight map "ps1" => "powershell", and a doc comment about $env: interpolation at cyrup-ext/src/caps/proc.rs:178). crates/cyrup-tools/src/tools/mod.rs has no powershell module; registry.rs:20 pins BUILTIN_NAMES to 7 and with_builtins (registry.rs:54-96) registers only those; ops/shell.rs is bash-only (is_legacy_wsl_bash_path, get_bash_shell_config, find_bash_on_path with `which bash`/`where bash.exe`, windows_detect_from ending in the `No bash shell found` throw) with no pwsh.exe/powershell.exe probe and no POWERSHELL_ARGS; cyrup/src/cli/help.rs:213-221 lists seven names. There is also no generic shell-tool factory to refute with: BashTool (tools/bash.rs) is a concrete tool, not a parameterized ShellToolConfig factory like pi's createShellToolDefinition. The only near-miss is the `shellPath` setting (config.rs:213-215, used at tools/bash.rs:297-302 and cyrup-session-svc/src/session/bash.rs:88-93 via ShellConfig::resolve), which accepts an arbitrary interpreter path with `-c` argv transport and would in fact run `pwsh -c "…"` — but that REPLACES bash rather than adding a second tool, still presents as the `bash` tool to the model, applies no -NoProfile/-NonInteractive/-ExecutionPolicy Bypass args and no UTF-8 [Console]::OutputEncoding preamble, and is not Windows-gated. A workaround, not the capability. Severity corrected down to low: (1) pi does NOT enable powershell by default — agent-session.ts:2751-2754 builds all definitions but the default enabled selection is read/bash/edit/write (core/sdk.ts:66-70), so powershell is opt-in via --tools/defaultTools; (2) it is Windows-only by construction (pi utils/shell.ts:127 throws off-Windows) and on Windows the model can still reach PowerShell through the bash tool (`powershell.exe -Command …` under Git Bash) or via shellPath; (3) nothing is silently wrong — `--tools powershell` fails loudly as an unknown tool name. An opt-in, single-platform, loudly-failing gap with a working execution path around it.

## Definition of done

1. A `powershell` tool exists and is registered as an eighth builtin.
2. Off-Windows it fails with pi's exact message; on Windows it resolves `pwsh.exe` then `powershell.exe`.
3. The `-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command` arg set and the UTF-8
   `[Console]::OutputEncoding` preamble are both applied.
4. `--tools`/`--exclude-tools` help lists it; the system prompt gains the PowerShell guideline branch.
5. It is opt-in, matching pi's default tool selection (read/bash/edit/write).
