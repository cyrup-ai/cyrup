# Tool reference

The extension registers two tools: `subagent` and `wait`.

## `subagent`

With no `action`, the tool is in **execution** mode and the shape of the call selects the workflow:
`agent` + `task` for single, `tasks[]` for parallel, `chain[]` for a chain, `chainName` for a named
chain.

With an `action`, the tool is in **management** or **control** mode.

### Actions

| Action | Mode | Purpose |
|---|---|---|
| `list` | management | List discoverable agents |
| `get` | management | Show one agent |
| `models` | management | Report the model each agent resolves to |
| `create` | management | Write a new agent file |
| `update` | management | Edit an agent file |
| `delete` | management | Remove an agent file |
| `eject` | management | Copy a builtin persona into your user directory |
| `disable` | management | Disable an agent via settings overrides |
| `enable` | management | Re-enable a disabled agent |
| `reset` | management | Drop an agent's settings overrides |
| `status` | control | List active runs; `view` selects fleet or transcript |
| `grant-spawn-budget` | management | Add launches to an exhausted per-session cap |
| `interrupt` | control | Interrupt a run |
| `resume` | control | Deliver a follow-up, or revive a terminal run from its transcript |
| `steer` | control | Queue non-terminal guidance for a live child |
| `stop` | control | Stop a run |
| `dismiss` | control | Clear a recovered workflow with no live runner from the display |
| `append-step` | control | Add a step to a running chain |
| `doctor` | management | Discovery diagnostics |
| `guide` | management | Read this packaged documentation |
| `mission.create` | management | Open a mission |
| `mission.list` | management | List missions in scope |
| `mission.show` | management | Show one mission and its attached runs |
| `mission.update` | management | Change a mission's fields |
| `mission.attach-run` | management | Bind a run to a mission |
| `mission.close` | management | Close a mission |
| `watchdog.status` | management | Report the effective watchdog config |
| `watchdog.check` | management | Run one watchdog review now |
| `watchdog.configure` | management | Change the watchdog config |
| `watchdog.recommend-model` | management | Suggest a watchdog review model |

An unknown action is answered with a did-you-mean suggestion drawn from this list, except that a
destructive candidate (`delete`, `eject`, `reset`, `stop`, `interrupt`, …) is only suggested under a
deliberately stricter rule, so a loose typo is never nudged toward a destructive verb.

### Parameters

| Parameter | Applies to | Meaning |
|---|---|---|
| `agent` | single, management | Agent name, or the management target |
| `task` | single | The task text; optional for self-contained agents |
| `action` | management, control | See the table above; omit for execution mode |
| `tasks` | parallel | Array of `{agent, task, …}` |
| `chain` | chain | Array of ordered steps |
| `chainName` | chain | A named chain from `~/.cyrup/chains` or `<project>/.cyrup/chains` |
| `concurrency` | parallel | How many children run at once |
| `async` | all | Detach the run |
| `timeoutMs` | all | Wall-clock timeout for the run |
| `maxRuntimeMs` | all | Absolute deadline across a composite run |
| `cwd` | all | Working directory for the children |
| `worktree` | all | Isolate the run in a git worktree |
| `context` | all | `fresh` or `fork` |
| `sessionDir` | all | Where child session files are written |
| `chainDir` | chain | Artifact directory for this chain run |
| `artifacts` | all | Artifact behaviour for this run |
| `output`, `outputMode` | single, chain | Output path and `inline`/`fileAndInline`/`fileOnly` |
| `outputSchema` | single | JSON Schema the child's structured output must satisfy |
| `includeProgress`, `share` | all | Progress visibility |
| `clarify` | single | Ask the child to clarify before working |
| `control` | all | Per-run live-control thresholds |
| `skill` | single | Skill injected into the child's prompt |
| `model` | single | Model override |
| `turnBudget` | all | `{maxTurns, graceTurns}` |
| `toolBudget` | all | Tool-call budget enforced in the child |
| `usageBudget` | all | Token/cost budget checked when the run settles |
| `acceptance` | single, chain | Acceptance criteria and verify commands |
| `agentScope` | management | Which discovery scopes to read or write |
| `id`, `runId`, `dir` | control | Address a run by id or by directory |
| `index` | control | Zero-based child index within a run |
| `view`, `lines` | `status` | Fleet or transcript view, and transcript line cap |
| `message` | `steer`, `resume` | Guidance or follow-up text |
| `mode` | `steer` | `steer`, `follow_up` or `auto` |
| `additional` | `grant-spawn-budget` | Positive launches to add |
| `scope`, `target`, `thinking` | `watchdog.configure` | Watchdog scope and target |
| `missionId`, `mission`, `missionUpdate`, `missionStatus`, `missionScope` | `mission.*` | Mission payloads |
| `runMode`, `runStatus`, `summary` | `mission.attach-run` | Run binding fields |
| `config` | management | Extension config fragment for the call |

### Structured output

`outputSchema` on a single run requires the child to call its `structured_output` tool with a value
matching the schema; the run does not settle until it does. A child that never calls it fails with
the missing-structured-output error rather than returning free text.

### Acceptance

`acceptance` attaches criteria and verify commands to a run. Verify commands are memoized per run, so
re-evaluating acceptance does not re-run a passing command, and evaluation can be cancelled.

## `wait`

`wait` blocks until background runs finish. `id` waits for one run, `all` waits for every run that
was in flight when the wait began, and `timeoutMs` bounds it (30 minutes by default). It is gated by
`waitTool` in `config.json` and by `CYRUP_SUBAGENT_WAIT_TOOL_ENABLED`; an unrecognised value for that
variable is a hard configuration error rather than a silent default.

## Child-safe mode

A child that the parent authorized to fan out gets a restricted `subagent` tool: the mutating
management verbs are refused with the child-safe refusal text, and the tool description is the
compact form.
