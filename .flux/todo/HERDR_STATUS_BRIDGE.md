---
stage: new
status: pending
updated: 2026-09-20
---

# herdr — a native Rust client, and cyrup's fleet in your sidebar

OBJECTIVE: cyrup speaks herdr's socket protocol natively, so that a cyrup session running in a
herdr pane publishes its subagent fleet state authoritatively — **who is working, who is blocked
waiting on you, who is done** — and reacts to herdr's own event stream.

Source of truth: **`tmp/herdr` @ `d59d060` (v0.9.1)**, cloned 2026-09-20. Its own docs are
`docs/preview/website/src/content/docs/socket-api.mdx` (959 lines) and `plugins.mdx`. Read THOSE,
not a consumer's inference. pi's `integrations/herdr-status.ts` (400) and
`inspectors/herdr/client.ts` (130) are one consumer; `tmp/code_puppy_core_plugins/.../herdr/`
(1 728 lines of Python) is a second. Where they disagree with herdr's source, herdr's source wins.

## The finding that reframes this whole area

**pi's herdr integration is a thin, lossy subset.** pi spawns the `herdr` binary and parses the
last JSON line of stdout (`client.ts:43-55`, `HERDR_BIN ?? "herdr"`), driving four CLI verbs:
`pane get`, `pane run`, `pane close`, `pane split`.

herdr v0.9.1 exposes **99 socket methods**, including a first-class agent protocol pi never
touches:

| area | methods |
|---|---|
| Agent | `agent.list` `agent.get` `agent.read` `agent.explain` `agent.send_keys` `agent.prompt` `agent.wait` `agent.rename` `agent.focus` `agent.start` `agent.view.set` `agent.view.clear` |
| Agent state | `pane.report_agent` `pane.report_agent_session` `pane.report_metadata` `pane.release_agent` `pane.clear_agent_authority` |
| Events | `events.subscribe` `events.wait` |
| Session | `session.snapshot` |
| Pane | 28 methods incl. `split` `read` `send_text` `wait_for_output` `graphics.*` |
| Workspace / Tab / Worktree / Layout / Plugin / Integration | the rest |

**cyrup is Rust. herdr is Rust** (its socket maps onto a named pipe on Windows via the
`interprocess` crate). A newline-delimited JSON client over `tokio::net::UnixStream` with serde is
an afternoon's work, and `herdr api schema --json` emits a full JSON Schema of every request,
success response, error response, emitted event and subscription event — a machine-checkable
contract. There is no reason for cyrup to shell out per report the way TypeScript must.

**This is the standing rule in its strongest form: upstream's structure is the contract; upstream's
transport is not.**

## The protocol, from herdr's own docs

**Discovery.** herdr injects four env vars into every pane it owns:
`HERDR_ENV=1` · `HERDR_SOCKET_PATH` · `HERDR_PANE_ID` (e.g. `w1:p1`) · `HERDR_TAB_ID` (`w1:t1`).
Absent ⇒ the bridge does **nothing**: no socket, no task, no overhead, no behaviour change.

**Framing.** Newline-delimited JSON, one envelope per line:
`{"id":"req_1","method":"pane.report_agent","params":{…}}`. 32 MiB wire limit per transaction.

**Agent state reporting** (`socket-api.mdx:698-716`) — the core of this batch:

```json
{"id":"req_1","method":"pane.report_agent","params":{
  "pane_id":"w1:p1","source":"custom:docs","agent":"docs-bot",
  "state":"working","message":"building docs"}}
```

- Effective states: **`idle` · `working` · `blocked` · `done` · `unknown`**; `done` means *idle and
  not yet seen*.
- *"`state` carries semantic agent state and affects waits, notifications, and rollups. Report
  display-only values separately through metadata."* — so cyrup's run labels go through
  `pane.report_metadata`, and only the semantic state through `pane.report_agent`.
- `pane.report_agent_session` carries a native session reference
  (`{pane_id, source, agent, agent_session_id}`); it is state-INDEPENDENT and *"does not affect
  waits, notifications, or rollups"*.

**Event subscriptions** (`:800-845`):

```json
{"id":"sub_1","method":"events.subscribe","params":{"subscriptions":[
  {"type":"pane.agent_status_changed","pane_id":"w1:p1","agent_status":"blocked"}]}}
```

First response ACKs; later lines are pushed events. *"Lifecycle subscriptions start when the
request is accepted and do not replay events retained before that point."* Pane events include
`pane.created` `updated` `closed` `focused` `moved` `exited` `agent_detected` `output_matched`
`agent_status_changed` `scroll_changed`.

**Bootstrap without a gap** (`:118-130`) — herdr documents the exact race protocol, port it
verbatim: open `events.subscribe` on ANOTHER connection and wait for its ack; buffer that stream
while calling `session.snapshot`; install the snapshot; apply the buffered events in order;
continue streaming. Re-snapshot after reconnect.

**`agent.wait`** is server-owned and event-driven, and *"pins the resolved pane occupant so a
replacement cannot satisfy the wait."* `agent.prompt` takes an optional `{wait:{until,timeout_ms}}`
so submit-and-wait is one request with no race; if the agent is already `blocked` it returns
`agent_blocked` without sending input.

**`agent.view.set`** (`:698+`) installs a declarative projection over herdr's Agents sidebar, with
a filter/sort DSL (`all|any|not|eq|in|exists` over `status`, `workspace_id`, `tab_id`, `pane_id`,
`agent`, `seen`, `state_change_seq`, plus `{"token":"name"}` for plugin-reported metadata).
`source` identifies the owner; a non-`plugin:` source is allowed for other callers.

## What cyrup builds

1. **`herdr/client.rs` — the native socket client.** Env discovery, `UnixStream` (named pipe on
   Windows), NDJSON codec, request/response correlation by `id`, typed errors. Generate or
   hand-write the types against `herdr api schema --json` / `tmp/herdr/src/api/schema.rs` (274
   lines). Error codes from herdr plus pi's normalization set (`HERDR_UNAVAILABLE`, `PANE_GONE`,
   `NOT_FOUND`, `TIMEOUT`, `VALIDATION_ERROR`, `agent_blocked`, `feature_disabled`).
2. **The CLI fallback.** The seven inspector verbs (VL-S6) drive `pane get/run/close/split`; keep
   a CLI path for when `HERDR_SOCKET_PATH` is absent but `herdr` is on PATH, exactly as the Python
   client falls back to a named pipe.
3. **The status bridge.** Subscribe to cyrup's own bus — PB-8 landed this in #142
   (`extension/rpc/`, `subscribe_bus`, `on_bus_event`), so the producer side already exists — and
   map run lifecycle onto `pane.report_agent`:
   - a run in flight ⇒ `working` (refcounted, so a finishing child does not flip the pane idle
     mid-turn — the Python client's rule, and it is correct)
   - awaiting the human ⇒ `blocked`, which is **the signal that tells the sidebar which pane needs
     you**, and the reason this feature exists
   - otherwise `idle`; herdr derives `done` itself from idle-and-unseen
   - run labels, model, context and token counts ⇒ `pane.report_metadata`, never `report_agent`
   - **raw prompts never enter pane metadata** — pi states this rule at `herdr-status.ts:28-36`
     ("Explicit launch/workflow label only"); port it and pin it with a test
   - `pane.release_agent` on clean exit
4. **The event consumer.** `events.subscribe` on `pane.agent_status_changed` and the pane
   lifecycle events cyrup's FleetView can use, with the documented snapshot-race protocol.
5. **`agent.view.set`** projecting cyrup's fleet into the sidebar — sorted by attention then
   `state_change_seq`, which is exactly "show me who needs me first".

## cyrup seams

- Bus events: PB-8's `subscribe_bus` / `on_bus_event` (`extension/rpc/`, #142).
- The active-run projection the FleetView already computes feeds `getRuns`.
- `getProjectPaneCount` comes from the project-pane manager in the VL-S6 batch — this batch and
  VL-S6 share a seam and should land together.
- A `tokio` task for the reporter with TTL refresh (pi uses 120 s TTL / 45 s refresh —
  `herdr-status.ts:9-10`; check those against herdr 0.9.1's own metadata TTL, which the Python
  README gives as 24 h for pane metadata).

## Definition of done

1. Inside a herdr pane, cyrup reports `working`/`blocked`/`idle` authoritatively and the sidebar
   shows which pane needs the human. Driven end to end by a test against a fake socket server.
2. Outside a herdr pane: zero socket, zero task, zero overhead — pinned by test.
3. Raw prompts never reach pane metadata — pinned by test.
4. Socket transport when `HERDR_SOCKET_PATH` is set; CLI fallback otherwise; both exercised.
5. `events.subscribe` consumed with the documented no-gap bootstrap ordering.
6. Every request/response type checked against `tmp/herdr/src/api/schema.rs`, not against pi.
7. VL-S6 can cite this as ported — it is one of the two upstream paths that row names.

## [CYRUP-EXCEEDS-UPSTREAM] — recorded deliberately

`agent.view.set`, `events.subscribe`, `session.snapshot` and the native socket transport are
**not** in pi's integration. They are in herdr's protocol, they serve the exact workflow cyrup
subagents exist for, and cyrup is in the right language to use them. Porting only pi's CLI subset
would be choosing a TypeScript limitation on purpose.
