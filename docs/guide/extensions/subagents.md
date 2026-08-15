# Subagents

Subagents delegate work to child `cyrup` processes, each with its own persona, model, tool set and
depth budget. This page covers turning the feature on, writing an agent, running one, and every
setting that shapes how they behave.

Subagents is a [native extension](overview.md) and is off by default.

## Turning it on

```sh
CYRUP_SUBAGENTS=1 cyrup
```

Set the variable in your shell profile to keep it on. Alternatively, create either config file and
the extension arms itself without any environment variable:

- `~/.cyrup/agent/subagents/config.json` — for every project
- `<project>/.cyrup/subagents/config.json` — for one repository

An empty `{}` in either file is enough. The keys are listed under [Configuration](#configuration).

## What you get

A running subagent is a real OS process: cyrup re-executes its own binary as a child, hands it a
persona and a task, and streams the result back into your session. Because it is a separate process
with a separate context window, the child's exploration does not fill yours.

Children can run in the foreground (you wait) or in the background (you keep working and collect the
result later). They can be strung into a chain where each step feeds the next, or fanned out in
parallel across a batch of tasks. A run can be isolated in its own git worktree so a child editing
files does not collide with your working tree.

Each child has a depth budget, so a subagent that spawns its own subagent eventually runs out of
room. The default ceiling is two levels.

## Defining a subagent

An agent is one Markdown file. The frontmatter configures the child; the body is its system prompt.
This is the `reviewer` persona that ships with cyrup:

```markdown
---
name: reviewer
description: Versatile review specialist for code diffs, plans, proposed solutions, codebase health, and PR/issue validation
tools: read, grep, find, ls, intercom
thinking: high
systemPromptMode: replace
inheritProjectContext: true
inheritSkills: false
---

You are a disciplined review subagent. Your job is to inspect, evaluate, and report findings with
evidence. You do not guess; you verify from the code, tests, docs, or requirements.

## Working rules
- Read the relevant files first.
- Do not use shell commands or write files.
- Do not invent issues. Only report problems you can justify from evidence.
```

`name` and `description` are required. **A file missing either is skipped silently** — no warning,
no diagnostic, the agent simply never appears. If an agent you wrote is not listed by
`/subagents-doctor`, check those two keys first.

The frontmatter parser is a YAML subset: line-oriented, with one level of block nesting. It does not
understand two-level nesting.

### Frontmatter keys

| Key | Meaning |
|---|---|
| `name` | The agent's name; how you select it. Required |
| `description` | One-line summary shown in listings and to the orchestrating model. Required |
| `package` | Package namespace; makes the runtime name `<package>.<name>` |
| `alias`, `aliases` | Alternate names the agent also answers to |
| `tools` | Tool allowlist. Omit for all built-ins; an empty list means no tools |
| `model` | Model this agent runs on |
| `fallbackModels` | Models to try when the primary is unavailable |
| `thinking` | Thinking level for the child, for example `high` or `off` |
| `systemPromptMode` | `replace` (the body is the whole prompt) or `append` |
| `inheritProjectContext` | Whether the child inherits your project context |
| `inheritSkills` | Whether the child sees your skills |
| `defaultContext` | `fresh` (no inherited conversation) or `fork` (branch your session) |
| `skill`, `skills` | Skill names injected into the child's prompt at spawn |
| `extensions` | Extension allowlist for the child |
| `subagentOnlyExtensions` | Extensions visible to the child but not to you |
| `output` | Default output path and mode (`inline`, `fileAndInline`, `fileOnly`) |
| `defaultReads` | Files handed to the child when the call names none |
| `defaultProgress` | Whether progress is visible by default |
| `interactive` | Parsed and round-tripped; not enforced |
| `maxSubagentDepth` | Per-agent recursion ceiling |
| `completionGuard` | `false` disables the completion-mutation guard for this agent |
| `toolBudget` | Tool-call budget enforced inside the child |
| `turnBudget` | Turn budget applied when the call site omits one |
| `memory` | Persistent-memory scope folded into the child's prompt |
| `async` | Run in the background by default when the call site does not say |
| `timeoutMs` | Default run timeout when the call site does not say |

**Unrecognised keys are preserved, not dropped.** They round-trip through cyrup's own agent editing
intact. That is how a `permission:` block rides along in an agent file and gets enforced by
[the permission system](permissions.md) as its own policy layer.

## Where agent files live

Agents are collected from several directories and merged. In precedence order, lowest first:

| Tier | Directory |
|---|---|
| Builtin | the personas bundled in the binary |
| Package | the `agents` directories contributed by installed packages |
| Extra dirs | every path in `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` |
| User | `~/.cyrup/agents`, plus legacy `~/.agents` if it exists |
| Project | `<project>/.agents` if it exists, then `<project>/.cyrup/agents` |

Chains live alongside them in `~/.cyrup/chains` and `<project>/.cyrup/chains`.

**Watch the path.** Agent files go in `~/.cyrup/agents` — the `~/.cyrup` root, *not* the
`~/.cyrup/agent` agent directory that holds `settings.json` and the subagents `config.json`. The two
paths differ by one directory segment and by an `s`. An agent file in `~/.cyrup/agent/agents/` is
not read by subagents at all — that path belongs to the permission system.

Set `CYRUP_HOME` to relocate the `~/.cyrup` root that subagents uses. Note that it does not move
`settings.json`; see [Environment variables](../reference/environment.md).

The project root is the nearest ancestor directory containing `.cyrup/` or `.agents/`. Anything
under a path segment named `skills` is excluded from agent discovery.

**Discovery re-runs on every call.** Drop a new agent file in and the next `/run` picks it up — no
restart, no reload command.

Per-scope subagent settings live at `~/.cyrup/agents/settings.json` and
`<project>/.cyrup/agents/settings.json`, under a `subagents` object. A malformed one aborts
discovery rather than degrading, so if every agent vanishes at once, look there.

## Running one

```sh
/run reviewer review the changes on this branch
```

Three surfaces reach the same machinery.

The **`subagent` tool** is what the model calls on its own. In single mode it takes an `agent` name
and an optional `task`; in parallel mode it takes an array of `{agent, task}` pairs. Chain,
template and management modes exist on the same tool.

The **`wait` tool** blocks on a background run until it finishes.

The **slash commands** are for you:

| Command | Usage |
|---|---|
| `/run` | `/run <agent>[key=value,...] [task] [--bg] [--fork]` |
| `/chain` | `/chain agent1 "task1" -> agent2 "task2" [--bg] [--fork]` |
| `/parallel` | `/parallel agent1 "task1" -> agent2 "task2" [--bg] [--fork]` |
| `/run-chain` | `/run-chain <chainName> -- <task> [--bg] [--fork]` |
| `/subagents-fleet` | Open the live fleet inspector |
| `/subagents-stop` | `/subagents-stop [run-id]` — stop a background run in this session |
| `/subagent-cost` | Parent and child usage cost for this session |
| `/subagents-doctor` | Diagnostics: what was discovered, and what failed |
| `/subagents-models` | `/subagents-models [agent]` — the models the builtin agents resolve to |
| `/subagents-profiles` | List saved subagent profiles |
| `/subagents-load-profile` | `/subagents-load-profile <name>` |
| `/subagents-check-profile` | `/subagents-check-profile <name>` — are its models still usable |
| `/subagents-generate-profiles` | `/subagents-generate-profiles <provider>` |
| `/subagents-refresh-provider-models` | `/subagents-refresh-provider-models <provider> [--force]` |
| `/prompt-workflow` | `/prompt-workflow <name> [args] [--fork\|--fresh] [--worktree] [--bg] [--subagent <agent>]` |
| `/chain-prompts` | `/chain-prompts prompt-a -> prompt-b -- args` |
| `/subagents-watchdog` | Show or toggle the default-off subagent watchdog |

`--bg` runs in the background, `--fork` branches your current session into the child instead of
starting it fresh.

## The fleet view

While subagents are running, a status widget sits permanently above or below the editor showing what
is in flight. `fleetView` turns it off — only an explicit `false` does — and `fleetViewPlacement`
moves it: the exact string `"aboveEditor"` puts it above the editor, anything else leaves it below.

`/subagents-fleet` opens the full inspector as a navigable overlay, where you can look into
individual runs rather than the one-line summary. `/subagents-stop` ends a run you no longer want.

## Configuration

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
| `forceTopLevelAsync` | bool | `false` | Force every top-level run async, overriding per-call choice |
| `globalConcurrencyLimit` | number | `20` | Cap on concurrently running children |
| `maxSubagentSpawnsPerSession` | number | `40` | Cap on total spawns in one session |
| `maxSubagentDepth` | number | `2` | Recursion ceiling for new top-level runs |
| `parallel.maxTasks` | number | `8` | Cap on tasks in one parallel fan-out |
| `parallel.concurrency` | number | `4` | How many of those run at once |
| `chain.dynamicFanout.maxItems` | number | *unset* | Cap on a dynamic fan-out's expansion |
| `control` | object | *unset* | Live-control notice thresholds |
| `proactiveSkillSubagents` | object or `false` | *unset* | Proactive skill-subagent suggestions; `false` disables |
| `defaultSessionDir` | path | *unset* | Where new child session files are written |
| `singleRunOutputBaseDir` | path | *unset* | Base directory for single-run output artifacts |
| `worktreeBaseDir` | path | *unset* | Where isolated git worktrees are created |
| `worktreeSetupHook` | path | *unset* | Script run once per worktree group before any child starts |
| `worktreeSetupHookTimeoutMs` | number | `30000` | Timeout for that hook |
| `fleetView` | bool | `true` | The persistent fleet widget; only explicit `false` disables |
| `fleetViewPlacement` | string | below | `"aboveEditor"`; anything else is below the editor |
| `waitTool` | bool or `{enabled}` | enabled | The `wait` tool gate |
| `missions` | object | *unset* | Durable mission store; `{"enabled": false}` stops automatic mission creation |
| `artifactConfig.cleanupDays` | number | `7` | Artifact retention; `0` disables cleanup |

A missing file means all defaults. A malformed file warns on stderr —
`cyrup: warning: ... is not valid subagents config JSON ...; using defaults` — and cyrup carries on
with defaults. One exception: an unknown key inside the `missions` block rejects the whole file
rather than being ignored.

Some behaviour is configured from `settings.json` instead, under a `subagents` object —
`defaultModel`, `defaultThinking`, `defaultExtensions`, `disableBuiltins`, `disableThinking`, and
per-agent overrides. See [settings.json](../reference/settings.md).

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
boundary. You will see them in a child's environment and in `/subagents-doctor` output. Setting them
yourself misdirects a child rather than configuring it.

## Turning it off

Unset `CYRUP_SUBAGENTS` and remove both `config.json` files — either one is enough to keep the
extension armed. `cyrup --no-extensions` disables it for a single run along with everything else.
