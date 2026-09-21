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
| `children.list` | management | List this session's settled workflow children, newest first, with whether each can be resumed |
| `create` | management | Write a new agent file |
| `update` | management | Edit an agent file |
| `delete` | management | Remove an agent file |
| `eject` | management | Copy a builtin persona into your user directory |
| `disable` | management | Disable an agent via settings overrides |
| `enable` | management | Re-enable a disabled agent |
| `reset` | management | Drop an agent's settings overrides |
| `status` | control | List active runs; `view` selects fleet or transcript |
| `debug.run` | control | Run-lifecycle diagnostic dump for one async run (`id`/`runId`/`dir`): status file, session, state, runner pid liveness, active-capacity slot, workflow steps; refuses `view` |
| `grant-spawn-budget` | management | Add launches to an exhausted per-session cap |
| `interrupt` | control | Interrupt a run |
| `resume` | control | Deliver a follow-up, or revive a terminal run from its transcript |
| `steer` | control | Queue non-terminal guidance for a live child |
| `stop` | control | Stop a run |
| `dismiss` | control | Clear a recovered workflow with no live runner from the display |
| `append-step` | control | Add a step to a running chain |
| `inspect` | control | Read one run's (or one child's) transcript tail and final output |
| `doctor` | management | Discovery diagnostics |
| `guide` | management | Read this packaged documentation |
| `mission.create` | management | Open a mission |
| `mission.list` | management | List missions in scope |
| `mission.show` | management | Show one mission and its attached runs |
| `mission.update` | management | Change a mission's fields |
| `mission.resolve-decision` | management | Resolve one open mission decision |
| `mission.attach-run` | management | Bind a run to a mission |
| `mission.close` | management | Close a mission |
| `worktree.discard` | management | Remove the worktrees a fan-out preserved. **Destructive; confirmed by default.** |
| `worktree.cleanup` | management | Plan which fan-out worktrees are safe to remove. **Plan-only: nothing is removed.** |
| `lane.status` | management | Show one handoff manifest's cleanup eligibility. Read-only |
| `lane.recordMerge` | management | Attest that a lane merged, with evidence |
| `lane.recordSupersession` | management | Attest that another lane superseded this one |
| `refine` | management | Propose and write a bounded, evidence-cited refinement overlay for one agent |
| `refine.show` | management | Show one agent's refinement overlay, its revision history and base-prompt drift. Read-only |
| `refine.rollback` | management | Undo the last overlay revision by appending a rollback revision. **Destructive and NOT confirmed** — it rewrites the overlay immediately, and that overlay is folded into every later spawn of the agent. |
| `inspector.open` | management | Open an inspector pane for one async run (or one child of it) in a supported host — Herdr first, then Ghostty. With no host available it refuses with `No inspector plugin is available. Start a supported inspector host, or use inspector.command for a standalone command.` **Confirmed only if `authorityPolicy.inspectorOpen` says so; the default is `auto`.** |
| `inspector.command` | management | Print the standalone command that would launch the inspector for a run, without opening anything. **Needs no inspector backend at all** — it answers on a bare Linux box with no Herdr and no Ghostty, and is the fallback `inspector.open`'s refusal points at. Read-only |
| `inspector.status` | management | Report the inspector pane bound to a run. Answers from the binding file, with no call to any host — so `No inspector plugin owns this binding for async run <id>.` is a NORMAL answer and not an error. Read-only |
| `inspector.close` | management | Close the inspector pane bound to a run and drop its binding. Like `inspector.status`, "no inspector is open" is a normal answer, not an error. |
| `project.open` | management | Open (or focus, if one is already open) a Herdr project pane running an agent session for a project root. **Destructive-adjacent and confirmed by default** — `authorityPolicy.projectOpen` defaults to `confirm`, because this starts a new agent session in a pane of your workspace. Needs Herdr. |
| `project.status` | management | Report the project pane bound to a project root — open, stale, or absent. **Answers with Herdr not installed**: it reads the binding file and makes no Herdr call when there is no binding, and `No Herdr project pane binding exists for <root>.` is a normal answer, not an error. Read-only |
| `project.close` | management | Close a project's Herdr pane and remove its binding. Like `project.status`, it answers with no binding and no Herdr installed, and "no binding exists" is a normal answer rather than an error. |
| `watchdog.status` | management | Report the effective watchdog config |
| `watchdog.check` | management | Run one watchdog review now |
| `watchdog.configure` | management | Change the watchdog config |
| `watchdog.recommend-model` | management | Suggest a watchdog review model |
| `validate` | management | Structurally check a `workflowScript` without running it |
| `schedule.create` | management | Create a durable schedule that runs a `workflowScript` |
| `schedule.list` | management | List this project's schedules, session-only ones marked |
| `schedule.show` | management | Show one schedule |
| `schedule.history` | management | List one schedule's recorded runs |
| `schedule.pause` | management | Stop a schedule firing, keeping it |
| `schedule.resume` | management | Let a paused schedule fire again |
| `schedule.run` | management | Fire one schedule now, without consuming its next slot |
| `schedule.run-due` | management | Fire every schedule that is due right now |
| `schedule.delete` | management | Remove a schedule and its history |

An unknown action is answered with a did-you-mean suggestion drawn from this list, except that a
destructive candidate (`delete`, `eject`, `reset`, `stop`, `interrupt`, …) is only suggested under a
deliberately stricter rule, so a loose typo is never nudged toward a destructive verb.

### Parameters

| Parameter | Applies to | Meaning |
|---|---|---|
| `agent` | single, management | Agent name, or the management target |
| `task` | single | The task text; optional for self-contained agents |
| `workflowScript` | workflow | Inline JavaScript statement body run as a workflow |
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
| `view`, `lines` | `status`, `inspect` | Fleet or transcript view, and transcript/message line cap |
| `childId` | `stop`, `inspect` | Address one child by its workflow key, child run id, or `step:<n>` |
| `message` | `steer`, `resume` | Guidance or follow-up text |
| `mode` | `steer`, `worktree.cleanup` | `steer`, `follow_up` or `auto`; `worktree.cleanup` requires `plan` and refuses `apply` |
| `repo` | `worktree.cleanup` | Repository to plan for; omitted, the request `cwd` |
| `handoffPath` | `worktree.*`, `lane.*` | An existing parallel-handoff manifest. Required by every verb except `worktree.cleanup` |
| `planId` | `worktree.cleanup` | Reserved; a value is refused because apply does not exist |
| `laneId` | `lane.*` | The manifest's exact `runId`; a mismatch is refused |
| `merge` | `lane.recordMerge` | `{prNumber, reviewedHead, mergeCommit, treeEquivalent, postMergeChecks, attestedBy, attestedAt}` |
| `supersession` | `lane.recordSupersession` | `{supersededBy, attestedBy, attestedAt}` |
| `lane` | parallel, workflow | Launch-declared lane metadata: `{version, key, mode, sourceRef, claims, outputPaths}` |
| `additional` | `grant-spawn-budget` | Positive launches to add |
| `scope`, `target`, `thinking` | `watchdog.configure` | Watchdog scope and target |
| `missionId`, `mission`, `missionUpdate`, `missionStatus`, `missionScope` | `mission.*` | Mission payloads |
| `runMode`, `runStatus`, `summary` | `mission.attach-run` | Run binding fields |
| `id`, `summary` | `mission.resolve-decision` | Decision id and its resolution text |
| `name` | `schedule.create` | Display name; omitted, it is derived from the script target |
| `at`, `every` | `schedule.create` | Exactly one: a `+10m` delay or zoned ISO stamp, or `30m`/`6h`/`2d`/`2w` |
| `sessionOnly` | `schedule.create` | Fire only while the creating session is alive |
| `quiet` | `schedule.create`, `schedule.run` | Deliver the completion without waking a turn |
| `overlap`, `catchUp` | `schedule.create` | `skip` only; `none` or `latest` (default `latest`) |
| `on`, `timezone` | `schedule.create` | Reserved for calendar schedules, refused today |
| `baseRef` | `schedule.create` | Reserved; refused rather than run against the wrong tree |
| `args` | workflow, `schedule.create` | Arguments object the `workflowScript` runs with |
| `config` | management | Extension config fragment for the call |

### Lanes: recording convergence

A `worktree: true` fan-out publishes a **parallel-handoff manifest** and reports its path on the
group's output (and on an async run's status, as `parallelHandoff.path`). That path is the
`handoffPath` every verb below takes.

```
{ action: "lane.status", laneId: "<run id>", handoffPath: "<manifest>" }
```

`lane.status` renders whether removing that fan-out's worktrees is safe, and why not when it is
not. It is **read-only and stays available to a child-safe fanout tool** — a delegated child can
read its own lane graph. Every other verb in this feature — `lane.recordMerge`,
`lane.recordSupersession`, `worktree.cleanup` and `worktree.discard` — is refused there.

## Inspector and project panes

Seven verbs put a run, or a project, in front of you in a real terminal pane.

### Inspector panes — `inspector.*`

```
{ action: "inspector.open", id: "<run id or prefix>", index?: 0, focus?: true }
```

`inspector.open` opens a live dashboard for one async run — or for one child of it, when `index`
names one — in whichever inspector host is available. The backends are consulted in a fixed order,
**Herdr first, then Ghostty**, and the first one whose host is actually present wins. Herdr is
present when this process is itself running inside a Herdr pane; Ghostty is present on macOS when
`TERM_PROGRAM` is `ghostty`.

With NO host available the verb does not guess and does not half-succeed. It answers:

```
No inspector plugin is available. Start a supported inspector host, or use inspector.command for a
standalone command.
```

That sentence names the way out, and the way out always works:

```
{ action: "inspector.command", id: "<run id or prefix>" }
```

`inspector.command` **needs no backend whatsoever**. It returns the single, platform-quoted shell
command that launches the inspector for that run, for you to paste into any terminal you like. It
is the verb to reach for on a bare Linux box, over SSH, or inside CI.

```
{ action: "inspector.status", id: "<run id or prefix>" }
{ action: "inspector.close",  id: "<run id or prefix>" }
```

Both read the binding file this run's inspector wrote, and neither needs a host to be running.
**"No inspector is open" is a normal answer, not an error** — `No inspector plugin owns this
binding for async run <id>.` comes back as an ordinary reply, so a model can ask the question
freely without having to guard the call.

The directory an inspector may be launched against is not taken on trust: it must either be a run
this process is actively tracking, or live inside the configured async root, checked both
literally and through its real path so a planted symlink cannot escape. The inspector's in-pane
steer and stop controls are enabled only when the authority policy's `steerRun` / `stopRun` are
`auto` — a `confirm` or `forbid` policy produces a pane whose controls are genuinely disabled
rather than one that prompts where there is nothing to prompt through.

### Project panes — `project.*`

```
{ action: "project.open",   cwd?: "<project root>", message?: "<first message>", focus?: true }
{ action: "project.status", cwd?: "<project root>" }
{ action: "project.close",  cwd?: "<project root>" }
```

A project pane is a Herdr pane running an agent session for a project root, bound to that root by
a binding file so the same project reopens the same pane instead of accumulating panes.
`project.open` focuses an existing live pane rather than duplicating it.

`project.open` is **confirmed by default**: `authorityPolicy.projectOpen` defaults to `confirm`
because opening one starts a new agent session inside your workspace. `inspectorOpen` defaults to
`auto`, because an inspector is a read-only view of a run you already named.

**What works with no Herdr installed.** `project.status` and `project.close` answer from the
binding file and make no Herdr call at all when there is no binding, so both are usable anywhere:

| verb | no binding on disk | binding on disk, no Herdr | binding on disk, Herdr live |
|---|---|---|---|
| `project.status` | `No Herdr project pane binding exists for <root>.` — a normal answer | `Herdr project pane error (HERDR_UNAVAILABLE): …` | the pane's state, agent status and summary |
| `project.close` | `No Herdr project pane binding exists for <root>.` — a normal answer | `Herdr project pane error (HERDR_UNAVAILABLE): …` | the pane is closed and the binding removed |
| `project.open` | needs Herdr: `Herdr project pane error (HERDR_UNAVAILABLE): Herdr is not installed or is not on PATH. Install Herdr 0.7.5+ or set HERDR_BIN.` | same | the pane is opened, or the live one focused |

Neither read verb is an error when there is nothing there. That distinction is deliberate and is
pinned by tests: a model asking "is a pane open for this project?" should get an answer, not a
failure it has to interpret.

### Child-safe fanout

`inspector.open`, `inspector.close`, `project.open` and `project.close` are all refused from a
child-safe fanout tool with `Action '<verb>' is not available from child-safe subagent fanout
mode.` The three reads — `inspector.command`, `inspector.status` and `project.status` — stay
available, so a fanout child can still discover and report, just not act.

## Refinement overlays

A refinement overlay is a project-local file at
`.cyrup-subagents/refinements/<agent>.md` holding accumulated, evidence-cited guidance for ONE
agent. Its `current` block is folded into that agent's system prompt on **every subsequent spawn**,
inside a `<pi-subagents-refinement>` region that explicitly does not override tool, developer,
task, output, acceptance or safety instructions.

```
{ action: "refine", agent: "reviewer" }
```

`refine` collects a bounded packet of recent, this-project evidence for that agent — at most 8
items, at most 14 days old, at most 2 KiB per item and 16 KiB in total — and, only if the packet is
non-empty, launches a read-only proposal child constrained by a JSON schema and a
`{hard: 1, block: ["write","edit","bash"]}` tool budget. Every proposed edit must carry a title, a
rationale and at least one evidence id **from that packet**, and guidance that tries to widen its
own scope (`all agents`, `global`), to disable acceptance/safety/tool/policy instructions, or to
name the base persona file, `settings.json` or the agent directory is REFUSED. On any refusal —
no evidence, a failed child, an invalid proposal, or zero edits — **no overlay is written**, and
the reply says so.

`refine.show` prints the overlay's path, revision, base source and whether the base prompt has
changed since the overlay was written, plus the current guidance and the last five revisions. It
is **read-only and stays available to a child-safe fanout tool**; `refine` and `refine.rollback`
are refused there.

`refine.rollback` restores the previous guidance by **appending** a new `rollback` revision rather
than popping history, so a second consecutive rollback returns to where it started.

```
{ action: "lane.recordMerge", laneId, handoffPath,
  merge: { prNumber: 42, reviewedHead: "<40 hex>", mergeCommit: "<40 hex>",
           treeEquivalent: true, postMergeChecks: "recorded",
           attestedBy: "reviewer", attestedAt: "2026-09-18T00:00:00Z" } }
```

The attestation is **digest-bound**: the recorder stamps `manifestDigest` itself from the
manifest's own facts, so evidence cannot be carried over to a manifest it was not made for, and a
stale digest blocks cleanup rather than passing silently. `treeEquivalent` must be `true` and
`postMergeChecks` must be `recorded` for the lane to become eligible; anything else is reported as
a named blocker. Recording merge evidence for a lane that still has a non-terminal child is
refused. `lane.recordSupersession` is the same shape with `{supersededBy, attestedBy, attestedAt}`,
and a lane cannot supersede itself.

A stored eligibility read back off disk is **never believed on its own** — it is re-derived from
the evidence, and a disagreement collapses to `unknown`, which reads as "removal is not safe".

### Worktree discard

`worktree.discard` removes the worktrees and temporary branches a fan-out deliberately preserved.

```
{ action: "worktree.discard", handoffPath: "<manifest>" }
```

**It deletes things, and it asks first.** The `discardWorktree` authority action defaults to
`confirm`, so a default install prompts; `"authorityPolicy": {"discardWorktree": "forbid"}` refuses
outright, and a `confirm` in a session with no interactive UI is a refusal, never an implicit yes.
Declining the prompt is not an error — nothing is changed and the call succeeds saying so.

A worktree that still holds uncommitted work, untracked files, or commits the run's base does not
have is checked a second time: under a `confirm` policy it is removed only when a human actually
confirmed, and otherwise it is preserved with the reason recorded. So is one the manifest can no
longer inspect — if `git status` cannot answer, the worktree is kept, never removed on the
assumption that it was empty. Anything left behind is reported with the exact
`git worktree remove --force` / `git branch -D` commands to finish by hand.

### Worktree cleanup

`worktree.cleanup` answers one question: **of the git worktrees a `worktree: true` fan-out left
behind, which is it safe to remove?** It cross-checks `git worktree list --porcelain` against the
parallel-handoff manifests under `.cyrup-subagents/artifacts/`, gives every worktree a state and a
decision, and writes the result to `.cyrup-subagents/cleanup-plans/<planId>.json`.

```
{ action: "worktree.cleanup", mode: "plan" }
```

**It removes nothing.** `mode` must be `plan`; `apply` and `planId` are both refused, and the
rendered plan ends with `Plan-only mode: no worktrees or branches were removed.` Removal is a
separate, not-yet-built phase, and the plan is the evidence that phase would check.

A worktree is only ever proposed for removal when every one of these holds: it is a strict child of
this build's managed worktree base directory and carries the `cyrup-worktree-` prefix; it is not the
repository root, not a symlink, and not inside the extensions directory; a manifest claims it, marks
it `preserved`, and records its group's cleanup as `partial`; its branch is not checked out anywhere
else; its owning run is provably finished, with every child settled; `git status --porcelain=v1
--untracked-files=all` is empty (the flag is explicit so a `status.showUntrackedFiles` setting cannot
silence it); any committed divergence from the base commit is either already
merged into the local `HEAD` or preserved by a `.patch` that still validates against the worktree;
and none of its own durable evidence — the manifest, the output, the transcript, the patch — lives
inside it. Anything unreadable or ambiguous is reported as `unknown` and kept. Absence of proof is
never treated as proof of safety.

### Schedules

A schedule is a durable instruction to run a `workflowScript` in a project directory, once at a
time or on a fixed interval. It is stored under the project, not the session, so it outlives the
terminal that created it and fires in whichever session is live when it comes due — unless it was
created with `sessionOnly: true`, which binds it to its creating session and to no other.

```
{ action: "schedule.create", every: "6h", name: "nightly sweep",
  workflowScript: "return runs.run('main', { agent: 'reviewer', task: 'sweep' })" }
```

A fired schedule produces a real run: it charges this session's spawn budget, is addressable by
`action: "interrupt"`, writes a status, a receipt and a result, and delivers a completion that names
the schedule it came from. `overlap: "skip"` is enforced with an exclusive lock file, so two cyrup
instances sharing one project cannot double-launch the same occurrence. `catchUp: "latest"` (the
default) fires ONCE at the most recent missed slot after a long sleep rather than replaying a
backlog; `catchUp: "none"` records the missed occurrences instead of firing them.

Due schedules fire on their own — `schedule.run-due` only forces the same pass early.

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
