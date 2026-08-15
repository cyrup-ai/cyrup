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
| `waitTool` | bool or `{enabled}` | enabled | The `wait` tool gate |
| `missions` | object | *unset* | Durable mission store |
| `artifactConfig.cleanupDays` | number | `7` | Artifact retention; `0` disables cleanup |
| `artifactDir` | `"project"`, `"session"`, `"temp"` | `project` | Where artifact files are written |
| `authorityPolicy` | object | *unset* | Per-action authority decisions |
| `turnBudget` | object | *unset* | `{maxTurns, graceTurns}` fallback |
| `toolDescriptionMode` | string | `full` | `full`, `compact`, or a path to a custom description |

A missing file means all defaults. A malformed file warns on stderr —
`cyrup: warning: ... is not valid subagents config JSON ...; using defaults` — and cyrup carries on
with defaults. Two exceptions fail the whole load instead: an unknown key inside `missions`, and an
invalid `artifactDir` or `authorityPolicy`.

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
| `CYRUP_SUBAGENT_WAIT_TOOL_ENABLED` | Enable or disable the `wait` tool; an unrecognised value is a hard error |
| `CYRUP_SUBAGENT_BINARY`, `CYRUP_SUBAGENT_STEP_BINARY` | Override the binary used to spawn children |
| `CYRUP_SUBAGENTS_WORKTREE_DIR` | Git-worktree root for isolated runs |
| `CYRUP_SUBAGENTS_TEMP_ROOT` | Root for nested-run temporary artifacts |
| `CYRUP_HOME` | Relocate the `~/.cyrup` root this extension reads agents from |

**The `CYRUP_SUBAGENT_PARENT_*` and `_CHILD_*` variables are not knobs.** cyrup writes them into a
child's environment to carry run ids, depth, capability tokens and inbox paths across the process
boundary. Setting them yourself misdirects a child rather than configuring it.
