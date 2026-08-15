# Watchdog

The watchdog is a second, independent reviewer. It reads what an agent just did — the *turn delta* —
asks a model whether the work is going wrong, and when it is, surfaces a **concern** or a **blocker**
into the transcript, optionally driving an automatic follow-up turn.

**It is off by default.** `subagents.watchdog.enabled` defaults to `false`, so a session that never
writes it into a `settings.json` and never runs `/subagents-watchdog on` installs the command and the
event subscriptions but performs no review, makes no model call, and emits no message.

## Two roles, one state machine

| Role | Where it runs | What it watches |
|---|---|---|
| main | the orchestrator session | your own turns |
| child | inside a spawned subagent | that child's turns |

Both drive the same runtime. They differ only in how the config is resolved and where a warning is
delivered.

## The four verbs

| Action | What it does |
|---|---|
| `watchdog.status` | Report the effective config and the last review |
| `watchdog.check` | Run one review now |
| `watchdog.configure` | Change the config |
| `watchdog.recommend-model` | Suggest a review model for this session |

### `watchdog.configure` and its scope default

`scope` defaults to **`session`**, which is a safety decision rather than a convenience one: an agent
that configures the watchdog changes nothing on disk unless the caller explicitly asks for `user` or
`project`. The session branch says so back to the caller — *"No settings files were changed."*

`target` selects `main`, `children`, or a single `child`.

## What it reads

- The turn delta — the messages, tool calls and file edits since the last review.
- A change signature over the edited files, so an unchanged turn does not pay for a review.
- LSP diagnostics for the edited files, where a language server is available.

An emission guard sits in front of the output so the same concern is not repeated turn after turn.

## Warnings

A **concern** is advisory: it is rendered into the transcript and the turn continues. A **blocker**
is stronger — it can drive an automatic follow-up turn asking the agent to address it before going
on.

## Configuration

`subagents.watchdog` in `settings.json`:

```json
{
  "subagents": {
    "watchdog": {
      "enabled": true,
      "main": { "enabled": true },
      "children": { "enabled": false }
    }
  }
}
```

`/subagents-watchdog` shows the current state and toggles it for this session.

## Model selection

The watchdog picks its review model independently of the model being watched — a reviewer running on
the same model that produced the work is a weaker reviewer. `watchdog.recommend-model` reports what
it would pick and why.
