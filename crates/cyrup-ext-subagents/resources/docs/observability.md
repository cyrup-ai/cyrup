# Observability

## The fleet view

While subagents are running, a status widget sits permanently above or below the editor showing what
is in flight. `fleetView` turns it off — only an explicit `false` does — and `fleetViewPlacement`
moves it: the exact string `"aboveEditor"` puts it above the editor, anything else leaves it below.

`/subagents-fleet` opens the full inspector as a navigable overlay, where you can look into
individual runs rather than the one-line summary, including each child's live transcript.

## Status

`subagent({ action: "status" })` lists the active runs in this session. Two views narrow it:

- `view: "fleet"` — the read-only foreground/async fleet surface.
- `view: "transcript"` with `id`/`dir` (and optional `index`) — tail a run transcript. `lines`
  caps how many lines come back, defaulting to 80 and capped at 500.

A run that has gone idle or is blocked on a decision is reported as needing attention, and that
state also ends an outstanding `wait`.

## Waiting

The `wait` tool blocks until a background run finishes. It is scoped to the current session and to
the current working directory's run root, so two cyrup sessions in one repository do not block on
each other's runs. It wakes as soon as this process observes a completion, with a one-second poll
underneath as reconciliation, and times out after 30 minutes by default.

A background run that finishes while no `wait` is outstanding fires its completion notice once. The
run's own `status.json` and `result.json` survive, so `{action:"status", view:"transcript"}` and a
direct read of the run directory both still answer — but the notice itself is not replayed into a
later turn.

## Doctor

`/subagents-doctor` reports what discovery actually found and what failed: the directories scanned,
the agents resolved at each tier, the agent files skipped and why, the active model-scope policy, and
the resolved binary used to spawn children. It is the first thing to run when an agent you wrote is
not being found.

## Cost

`/subagent-cost` reports parent and child usage cost for this session, broken down per run.

## Run artifacts

Each run writes into a run directory:

| File | Contents |
|---|---|
| `status.json` | The authoritative run record — state, steps, pid, timings |
| `events.jsonl` | The append-only event log, capped at 50 MB |
| `result.json` | The terminal result, written once |
| `control/` | The parent-to-runner control inbox (interrupt, stop, steer) |

`artifactDir` chooses the root: `project` (the default, `<cwd>/.cyrup-subagents`), `session`, or
`temp`. `artifactConfig.cleanupDays` (default 7) ages old run directories out; `0` disables cleanup.

## Notices

Completion notices are delivered into your session as custom messages rather than printed, one per
completed run.

## Live control

`control` in the extension `config.json` sets the live-control notice thresholds — how long a child
may be silent before the run is flagged, and how often progress is reported.
