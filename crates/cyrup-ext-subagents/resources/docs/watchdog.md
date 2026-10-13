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
- **`WATCHDOG.md`** standing instructions, read fresh on every review: `<project>/.cyrup/WATCHDOG.md`
  first, then `WATCHDOG.md` in the agent directory, capped at 8,000 characters. Set
  `guidance.watchdogMd: false` to ignore them.

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

## Optional main-session clarification

Off by default. Turn it on alongside the watchdog:

```json
{ "subagents": { "watchdog": { "enabled": true, "clarification": true } } }
```

It applies **only to the main watchdog**, never to child watchdogs (`children.clarification` is
rejected as an unknown key). With it on:

- **The reviewer may ask.** At a boundary review it may call `watchdog_ask` with one focused question
  and the concrete evidence behind it, when missing task status or intent prevents a concrete
  judgment. The question *yields and ends* that review: it is shown as a
  `subagent_watchdog_clarification` message that steers the session into one continuation, and
  nothing waits for an answer. At most one question per user prompt, none after a warning, none
  during a cadence review or a stalemate. Questions and evidence are capped at 1,000 and 2,000
  characters. A question is **not approval, permission, or a warning**.
- **It sees delivered orchestration.** Completed turns keep a bounded tail of *paired* orchestration
  calls and their results — `subagent` dispatches and `status`/`resume`/`interrupt`/`steer`/`stop`,
  `bg_wait`, and `subagent_supervisor` `pending`/`list`/`reply` — each entry bounded to 3,000
  characters, the tail to its newest 6,000. The review input shows it as *Recent delivered
  orchestration activity*. New activity admits one extra review per user prompt even with no edit.
  Turning clarification or the watchdog off, a compaction, or a session switch clears it.
- **Your message wins.** Any message you send while a review is running — idle or mid-run — aborts
  that review, so a stale warning or question is never shown. A session model change does too, and
  so does turning clarification off or changing the reviewer's model.

With it off, none of this runs: no activity is collected, no question is offered, and your messages
leave reviews alone.

## Model selection

The watchdog picks its review model independently of the model being watched — a reviewer running on
the same model that produced the work is a weaker reviewer. `watchdog.recommend-model` reports what
it would pick and why.
