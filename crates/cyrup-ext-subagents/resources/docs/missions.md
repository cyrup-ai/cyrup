# Missions

A mission is a durable record of a goal that outlives any single run. Runs attach to it, its status
survives a restart, and the orchestrator can pick the thread back up in a later session.

## The seven verbs

| Action | What it does |
|---|---|
| `mission.create` | Open a mission with a title and goal |
| `mission.list` | List missions in scope |
| `mission.show` | Show one mission with its attached runs |
| `mission.update` | Change title, goal, status or notes; record decisions, artifacts and receipts |
| `mission.resolve-decision` | Resolve one open decision by its `id`, with the resolution in `summary` |
| `mission.attach-run` | Bind an existing subagent run to a mission |
| `mission.close` | Close a mission |

`mission.list` and `mission.show` are read-only. The other five are mutating, and a child-safe
fanout tool refuses exactly those five with the child-safe refusal text.

## Parameters

`missionId` addresses an existing mission. `mission` carries the creation payload, `missionUpdate`
the update payload, `missionStatus` the target status, and `missionScope` the scope to list within.
`mission.resolve-decision` takes `missionId`, the decision `id` (as `mission.show` renders it), and
the resolution text in `summary`; an empty summary, an unknown id, or an already-resolved decision
is refused rather than silently ignored.

## Decisions

`mission.update` with `decisions: [{ title, prompt?, options?, recommendation? }]` records open
decisions. Adding one to an `active` mission gates it as `needs_decision`; a `planned` or `waiting`
mission keeps its lifecycle status while the decision stays visible. While any decision is open, an
`active` or `completed` status is held at `needs_decision` — so a mission cannot be closed as
completed over an unresolved decision. `mission.resolve-decision` closes one; once the last open
decision is resolved, a `needs_decision` mission returns to `active`. The goal driver's next ready
action names the first open decision (its recommendation, or `Resolve decision: <title>`), so
resolving it is what lets a goal mission move on to its next action.

## The store

Missions are persisted per scope. `missions` in the extension `config.json` configures the store;
`{"enabled": false}` stops automatic mission creation without removing the verbs. **An unknown key
inside the `missions` block rejects the whole config file** rather than being ignored — this is the
one config block that fails closed.

## Lifecycle

A mission moves through open → active → closed. Runs attached to a mission report back into it as
they complete, so `mission.show` is the one place that answers "what has actually been done toward
this goal" without replaying a transcript.

The goal driver reads a mission's goal to decide whether attached work has satisfied it; the
workflow-state record is what lets a recovered workflow rejoin its mission after a restart.
