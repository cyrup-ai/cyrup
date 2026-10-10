# Configuration

Three stores shape the extension, and they are separate on purpose.

1. **`config.json`** — the extension's own per-installation knobs.
2. **`settings.json`** — the `subagents` block, layered user ◁ project.
3. **Environment variables** — per-process overrides.

## `config.json`

`~/.cyrup/agent/subagents/config.json`, or `<project>/.cyrup/subagents/config.json`:

```json
{
  "maxSubagentDepth": 3,
  "globalConcurrencyLimit": 8,
  "parallel": { "maxTasks": 6, "concurrency": 3 },
  "fleetViewPlacement": "aboveEditor"
}
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `asyncByDefault` | bool | `false` | Runs go to the background unless told otherwise |
| `forceTopLevelAsync` | bool | `false` | Force every top-level run async |
| `globalConcurrencyLimit` | number | `20` | Cap on concurrently running children |
| `maxSubagentSpawnsPerSession` | number | `40` | Cap on total spawns in one session |
| `maxSubagentDepth` | number | `2` | Recursion ceiling for new top-level runs |
| `parallel.maxTasks` | number | `8` | Cap on tasks in one parallel fan-out |
| `parallel.concurrency` | number | `4` | How many of those run at once |
| `chain.dynamicFanout.maxItems` | number | *unset* | Cap on a dynamic fan-out's expansion |
| `control` | object | *unset* | Live-control notice thresholds |
| `proactiveSkillSubagents` | object or `false` | *unset* | Proactive skill-subagent suggestions |
| `defaultSessionDir` | path | *unset* | Where new child session files are written |
| `singleRunOutputBaseDir` | path | *unset* | Base directory for single-run output artifacts |
| `worktreeBaseDir` | path | *unset* | Where isolated git worktrees are created |
| `worktreeSetupHook` | path | *unset* | Script run once per worktree group |
| `worktreeSetupHookTimeoutMs` | number | `30000` | Timeout for that hook |
| `fleetView` | bool | `true` | The persistent fleet widget; only explicit `false` disables |
| `fleetViewPlacement` | string | below | `"aboveEditor"`; anything else is below the editor |
| `waitTool` | bool or `{enabled}` | enabled | The `bg_wait` tool gate |
| `missions` | object | *unset* | Durable mission store |
| `artifactConfig.cleanupDays` | number | `7` | Artifact retention; `0` disables cleanup |
| `artifactDir` | `"project"`, `"session"`, `"temp"` | `project` | Where artifact files are written |
| `authorityPolicy` | object | *unset* | Per-action authority decisions |
| `turnBudget` | object | *unset* | `{maxTurns, graceTurns}` fallback |
| `toolDescriptionMode` | string | `full` | `full`, `compact`, or a path to a custom description |
| `runnerLaunchers` | object | *unset* | Named argv prefixes that wrap background runners; see below |

A missing file means all defaults. A malformed file warns on stderr —
`cyrup: warning: ... is not valid subagents config JSON ...; using defaults` — and cyrup carries on
with defaults. Two exceptions fail the whole load instead: an unknown key inside `missions`, and an
invalid `artifactDir` or `authorityPolicy`. A file that declares any fail-closed key
(`runnerLaunchers` among them) is refused as a whole when any part of it is invalid, rather than
replaced by the defaults.

## `runnerLaunchers`

```json
{ "runnerLaunchers": { "net": ["env", "SUBAGENT_SANDBOX=1"] } }
```

Each entry names an argv prefix. An agent selects one with `launcher: net` frontmatter, and its
background runner is then started as that argv followed by the runner command, passed to the
operating system as an argument list, never through a shell. The key is read only from this user
config file; settings, agent overrides, agent management and tool calls cannot define or set a
launcher. An agent file, including a project agent from a cloned repository, can only name a
launcher this file already defines, but a project agent with the same name as one of your agents
still shadows it, so your own agent files and their names are the trust boundary. Names start with
a letter or digit and use only letters, digits, `.`, `_` and `-` (at most 128 characters); every
argv is a non-empty list of non-blank strings without NUL characters. An invalid value refuses the
whole file.

A launcher wraps the background runner only, so a launcher agent always runs in the background, and
an explicit `async: false`, a `machine`, an external `runner`, or a chain mixing launchers is refused.
An agent naming a launcher this file does not define fails before anything launches. `status.json`
records `launcher: {name, argv}`, and a resumed run reuses the launcher it was started with, reading
its argv from this file again. Wrappers that `exec` the runner (`env`, most sandbox tools) or keep it
as a child are supported; cyrup has no runner identity handshake, so a broker that starts the runner
outside the wrapper's process tree is not.

A launcher command must:

- run the runner command it receives, either by replacing itself with it (`exec`) or by staying
  attached until it exits;
- give the runner read and write access, at the same absolute paths, to the subagent temp root
  (`async-subagent-runs/`, `async-subagent-results/` and `supervisor-channels/`; the root is
  `CYRUP_SUBAGENTS_TEMP_ROOT` when set, else `cyrup-subagents` under the OS temp directory), the
  child session and artifact directories, the agent directory (`~/.cyrup/agent`, which holds
  auth), and the working directory, plus read access to the cyrup install;
- allow network access to your model provider;
- if it filters the environment, pass through at least `HOME`, `PATH`, `TMPDIR`, every `CYRUP_*`
  variable, and the API keys your provider needs.

Steering, stop and supervisor requests and replies travel through files in those directories, not
through signals, so they work as long as the paths are shared. A sandbox such as `bwrap` therefore
needs a `--bind` for each of those directories; a read-only root with no writable binds, or a
network namespace with no route to the provider, starts a runner that fails on its first write or
model call.

## `authorityPolicy`

Six actions can be gated: `discardWorktree`, `destructiveCleanup`, `spawnBudgetGrant`,
`scheduleCreate`, `stopRun`, `steerRun`. Each maps to `auto`, `confirm` or `forbid`; three default
to `confirm`. An unknown action key or a bad decision value fails config load with a typed error
rather than being ignored.

```json
{ "authorityPolicy": { "stopRun": "forbid", "steerRun": "confirm" } }
```

## `settings.json`

Per-scope subagent settings live at `~/.cyrup/agents/settings.json` and
`<project>/.cyrup/agents/settings.json`, under a `subagents` object. **Project beats user** on every
scalar and on every per-agent override name — a project `disableBuiltins: false` re-enables what a
user `true` disabled.

| Key | Meaning |
|---|---|
| `defaultModel` | Fallback model when nothing else supplies one |
| `defaultThinking` | Fallback thinking level |
| `defaultExtensions` | Extensions handed to every child |
| `disableBuiltins` | Exclude the bundled personas entirely |
| `disableThinking` | Force extended thinking off |
| `overrides.<agent>` | Per-agent override delta |
| `modelScope` | The model allowlist policy |
| `projectRootResolution` | `nearest` or `git-root` |

**A malformed `settings.json` aborts discovery** rather than degrading, so if every agent vanishes at
once, look there first.

This is the extension's only settings store. It is **not** `~/.cyrup/agent/settings.json` — the
binary's own layered settings document — which this extension never reads.

## Profiles

A profile is a named `subagents` block saved under `~/.cyrup/subagents/profiles`.
`/subagents-load-profile <name>` replaces the `subagents` key of the user settings file with it and
reports the profile's worker-tier model. `/subagents-check-profile` verifies its models are still
resolvable, and `/subagents-generate-profiles <provider>` writes a profile set from a provider's
catalog.

## Environment variables

These are the ones you set:

| Variable | Meaning |
|---|---|
| `CYRUP_SUBAGENTS` | Turn the extension on (`1`, `true`, `on`, `yes`) |
| `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` | Extra read-only agent directories, path-list separated |
| `CYRUP_SUBAGENT_BUILTIN_AGENTS_DIR` | Relocate the bundled personas |
| `CYRUP_SUBAGENT_MAX_DEPTH` | Recursion ceiling; overrides `maxSubagentDepth` |
| `CYRUP_SUBAGENT_MAX_SPAWNS_PER_SESSION` | Per-session spawn cap |
| `CYRUP_SUBAGENT_TOOL_BUDGET` | Tool budget handed to children, as JSON |
| `CYRUP_SUBAGENT_WAIT_TOOL_ENABLED` | Enable or disable the `bg_wait` tool; an unrecognised value is a hard error |
| `CYRUP_SUBAGENT_BINARY`, `CYRUP_SUBAGENT_STEP_BINARY` | Override the binary used to spawn children |
| `CYRUP_SUBAGENTS_WORKTREE_DIR` | Git-worktree root for isolated runs |
| `CYRUP_SUBAGENTS_TEMP_ROOT` | Root for nested-run temporary artifacts |
| `CYRUP_HOME` | Relocate the `~/.cyrup` root this extension reads agents from |

**The `CYRUP_SUBAGENT_PARENT_*` and `_CHILD_*` variables are not knobs.** cyrup writes them into a
child's environment to carry run ids, depth, capability tokens and inbox paths across the process
boundary. Setting them yourself misdirects a child rather than configuring it.
