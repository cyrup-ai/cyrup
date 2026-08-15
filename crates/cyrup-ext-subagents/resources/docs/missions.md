# Missions

A mission is a durable record of a goal that outlives any single run. Runs attach to it, its status
survives a restart, and the orchestrator can pick the thread back up in a later session.

## The six verbs

| Action | What it does |
|---|---|
| `mission.create` | Open a mission with a title and goal |
| `mission.list` | List missions in scope |
| `mission.show` | Show one mission with its attached runs |
| `mission.update` | Change title, goal, status or notes |
| `mission.attach-run` | Bind an existing subagent run to a mission |
| `mission.close` | Close a mission |

`mission.list` and `mission.show` are read-only. The other four are mutating, and a child-safe
fanout tool refuses exactly those four with the child-safe refusal text.

## Parameters

`missionId` addresses an existing mission. `mission` carries the creation payload, `missionUpdate`
the update payload, `missionStatus` the target status, and `missionScope` the scope to list within.

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
