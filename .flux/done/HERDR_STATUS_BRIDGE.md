---
stage: done
status: completed
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

---

## [AUG — bridge]

Research pass, read-only, 2026-09-20. Every citation below was opened; the seed's were re-opened
and the stale ones are corrected in §1. herdr is read at `tmp/herdr` @ `d59d060` (v0.9.1), pi at
`tmp/pi-subagents` @ `v0.68.0`, the Python client at `tmp/code_puppy_core_plugins`.

Scope boundary: this batch is the **bridge** — the state machine, the producer wiring, the privacy
rule, the event consumer and the sidebar projection. The **transport** (connect, NDJSON codec,
typed envelopes, errors, reconnect) is the sibling batch `HERDR_NATIVE_CLIENT.md`. §8 states the
exact client API this batch consumes so the two can be written against one seam.

---

### 1. Corrections to the seed — what is NOT true as written

**1a. `pane.report_agent` cannot report `done`.** The seed says the effective states are
"`idle` · `working` · `blocked` · `done` · `unknown`" *for `pane.report_agent`*. herdr has **two**
enums and only one of them is reportable:

```rust
// tmp/herdr/src/api/schema/common.rs:149-156 — what a client may REPORT
#[serde(rename_all = "snake_case")]
pub enum PaneAgentState { Idle, Working, Blocked, Unknown }

// tmp/herdr/src/api/schema/common.rs:158-166 — what herdr DERIVES and exposes/filters on
#[serde(rename_all = "snake_case")]
pub enum AgentStatus { Idle, Working, Blocked, Done, Unknown }
```

`done` is derived, never reported (`tmp/herdr/src/app/api_helpers.rs:96-107`):

```rust
pub(super) fn pane_agent_status(state: AgentState, seen: bool) -> AgentStatus {
    match (state, seen) {
        (Idle, false) => AgentStatus::Done,
        (Idle, true)  => AgentStatus::Idle,
        (Working, _)  => AgentStatus::Working,
        (Blocked, _)  => AgentStatus::Blocked,
        (Unknown, _)  => AgentStatus::Unknown,
    }
}
```

`seen` flips when the human looks at the pane's tab (`actions.rs:522-537`,
`mark_active_tab_seen`). So "`done` = idle and not yet seen" is true *of the derived status*, and
the five-value list is correct only for `agent.view.set`'s `status` filter
(`socket-api.mdx:469-470`) and for `state_labels` keys (`socket-api.mdx:780`). **cyrup's reporter
enum must have four variants.** Reporting `"state":"done"` is a `serde` deserialize failure at
herdr and comes back as `invalid_request`.

**1b. The socket is ONE REQUEST PER CONNECTION.** The seed says "request/response correlation by
`id`", which implies a multiplexed long-lived connection. herdr does not do that.
`handle_connection_with_stop` (`tmp/herdr/src/api/server.rs:156-317`) reads **one** line
(`read_initial_request_line`, `:168`), dispatches it in a single `match`, writes one response
(`:301`) and returns. Only the streaming arms keep the stream: `Method::EventsSubscribe` hands it
to `stream_subscriptions` (`:229-250`) and `Method::PaneGraphicsStream` to
`pane_graphics_stream::serve` (`:213`). The `wait`-shaped arms (`EventsWait`, `AgentPrompt`,
`AgentWait`, `PaneWaitForOutput`, `:251-288`) hold the connection only until their single response.
The second, independent client agrees: `_deliver_unix` opens a fresh `AF_UNIX` socket **inside
every send** (`tmp/code_puppy_core_plugins/.../herdr/client.py:403-408`). The `id` correlates one
response to one request on a fresh connection; it is not a multiplexing key.
Consequence for the bridge: **a report is a connect → write one line → read one line → close.**
There is no connection to keep warm and no correlation map to maintain.

**1c. pi's bridge never calls `pane.report_agent` at all.** The seed treats
`herdr-status.ts` as the reference for the state mapping. It is not — it is the reference for the
*metadata* half only. `git grep 'report-agent|report_agent' v0.68.0 -- src` over pi-subagents
returns **zero** hits. `herdr-status.ts:205-232` issues exactly one verb,
`pane report-metadata`, and raises two in-process pi events instead:
`herdr:busy` (`:250-259`) and `herdr:blocked` (`:270-279`). pi's own doc says why
(`tmp/pi-subagents` `docs/extension-api.md:465`):

> "The bridge uses Herdr's existing `herdr:blocked` sibling event when an async child needs
> attention, and emits `herdr:busy` while async work remains. Herdr versions that support the
> sibling event keep the pane's semantic state `working`; older versions ignore it safely and
> still display the metadata label **while the Pi integration remains the lifecycle authority**."

That last clause is the whole architecture: the **pi host process** is herdr's registered
lifecycle authority for the pane (`herdr:pi`/`pi` is one of the six hard-coded
`full_lifecycle_hook_authority` pairs, `tmp/herdr/src/detect/mod.rs:327-337`), and the *subagents
extension* only nudges it. cyrup has no such privileged registration upstream, so **cyrup must be
its own authority and call `pane.report_agent` directly.** That is not exceeding pi for its own
sake; it is the only way to get the same result from a non-registered source.

**1d. The privacy-rule anchor.** `herdr-status.ts:28-36` is the whole `HerdrStatusRun` interface;
the rule itself is **line 32**:

```ts
/** Explicit launch/workflow label only; raw prompts never enter pane metadata. */
taskLabel?: string;
```

Enforced at `:304-306` (`replaceRuns` strips the caller's `taskLabel` and re-derives a bounded
one) and `:72-98` (`boundedTaskLabel`/`workflowTaskLabel`). The seed's range is close enough to
find it but the citation in the port must be `:32` for the rule and `:72-98` for the enforcement.

**1e. The TTL numbers.** `herdr-status.ts:9-10` is correct as cited
(`DEFAULT_TTL_MS = 120_000`, `DEFAULT_REFRESH_MS = 45_000`) — but the seed then asks whether these
"check against herdr 0.9.1's own metadata TTL, which the Python README gives as 24h". Both halves
are misleading:

- herdr's `ttl_ms` is a **caller-supplied bound with a 24 h ceiling**, not a hold time:
  `#[schemars(range(min = 1, max = 86_400_000))]` on `PaneReportMetadataParams::ttl_ms`
  (`tmp/herdr/src/api/schema/panes.rs:503-505`), enforced at
  `normalize_metadata_ttl` (`tmp/herdr/src/app/api_helpers.rs:229-242`,
  `METADATA_TTL_MAX_MS = 86_400_000` at `:202`). **Omitting `ttl_ms` means no expiry at all** —
  `let Some(ttl_ms) = ttl_ms else { return Ok(None) }` (`:232-234`), and the docs say so:
  *"Omit `ttl_ms` for metadata that should stay until replaced, cleared, or the pane/workspace
  closes"* (`socket-api.mdx:796`). The Python client's `_METADATA_TTL_MS = 86_400_000`
  (`client.py:72-74`) is one consumer picking the ceiling, with its reason stated: *"bounds how
  long an abrupt process death can leave stale token/model numbers on the sidebar."*
- **Agent STATE has no TTL whatsoever.** `HookAuthority` (`tmp/herdr/src/terminal/state.rs:23`)
  carries `reported_at: Instant` and nothing else; every read of it
  (`:756-767`, `:2164-2172`) uses that timestamp only to order against screen observations. There
  is no expiry path. §7 turns this into a hard requirement.

**1f. The injected env set is six vars, not four.** `apply_pane_launch_env`
(`tmp/herdr/src/pane.rs:148-174`) sets `HERDR_ENV=1` (`:156`), then `apply_pane_base_env`
(`tmp/herdr/src/integration/env.rs:28-33`) sets `HERDR_SOCKET_PATH`
(`api::SOCKET_PATH_ENV_VAR`, `tmp/herdr/src/api/mod.rs:20`) and `HERDR_BIN_PATH`, then for a
`PaneLaunchIdentity::Managed` pane: `HERDR_WORKSPACE_ID`, `HERDR_TAB_ID`, `HERDR_PANE_ID`
(`pane.rs:166-168`, names at `integration/env.rs:8-10`). Note the third arm:
`PaneLaunchIdentity::OmitPane` explicitly **removes** `HERDR_PANE_ID` (`pane.rs:170-172`) while
leaving `HERDR_ENV=1` and the socket path set. So `HERDR_ENV=1` alone is **not** a sufficient
gate, which is exactly why pi's gate is a conjunction (`herdr-status.ts:130-131`):
`env.HERDR_ENV === "1" && typeof paneId === "string" && paneId.length > 0`. Port the conjunction,
not the marker.

**1g. `agent.view.set` is documented at `socket-api.mdx:418-495`, not `:698+`.** `:698` is the
"Agent state reporting" heading. The view section's own worked example is literally the ordering
the seed proposes (`:426-427,451-454`), which is a stronger citation than the seed's.

**1h. The error-code set in the seed mixes two layers.** `HERDR_UNAVAILABLE` / `PANE_GONE` /
`NOT_FOUND` / `TIMEOUT` / `VALIDATION_ERROR` are **pi's CLI-level normalization**
(`pi-intercom/project-agent.ts:10-16`, already ported at
`crates/cyrup-intercom/src/project_pane.rs:30-67`). herdr's own wire codes are lowercase
snake_case in `{"id","error":{"code","message"}}` (`socket-api.mdx:931-941`) — observed ones:
`invalid_request` (`server.rs:198`), `pane_not_found` (`server.rs:1539`), `not_found`
(`socket-api.mdx:937`), `invalid_agent`, `invalid_metadata_source`, `invalid_metadata_token`,
`invalid_metadata_ttl`, `invalid_metadata_request`, `invalid_state_label`
(`app/api/panes.rs:1623,1633,1638,1665,1654`), `invalid_agent_view`
(`app/api/agent_view.rs:13,19,51`), `timeout` (`server.rs:522`), `pane_send_failed`
(`panes.rs:1829`). Keep the two layers apart in the port: herdr's codes are data off the wire,
pi's are the launcher's normalization of a *process* failure and already exist in-tree.

---

### 2. herdr's agent-state surface, quoted

**`pane.report_agent`** — `tmp/herdr/src/api/schema/panes.rs:447-461`:

```rust
pub struct PaneReportAgentParams {
    pub pane_id: String,
    pub source: String,
    pub agent: String,
    pub state: PaneAgentState,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub agent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub agent_session_path: Option<String>,
}
```

Docs (`socket-api.mdx:717-718`): *"`state` carries semantic agent state and affects waits,
notifications, and rollups. Report display-only values separately through metadata."*

**`pane.report_agent_session`** — `panes.rs:463-476`; same head plus `session_start_source`, no
`state`. Docs `:720`: *"State-independent session reports do not affect waits, notifications, or
rollups."*

**`pane.report_metadata`** — `panes.rs:478-506`. `state_labels: HashMap<String,String>` keyed by
`idle|working|blocked|done|unknown` (`socket-api.mdx:780`, enforced by `normalize_state_labels`,
`panes.rs:1649-1658`), `tokens: HashMap<String, Option<String>>` (a **patch**: string sets,
JSON `null` clears, omitted keys unchanged — `socket-api.mdx:782`), `clear_*` booleans,
`applies_to_source`, `seq`, `ttl_ms`. Limits, all from `app/api_helpers.rs:202-208`:
≤16 token keys per request, ≤32 retained per pane, key 1–32 `[A-Za-z0-9_-]`, value ≤80 chars,
source ≤80 chars from `[A-Za-z0-9:._-]` (`socket-api.mdx:794`). Setting and clearing the same
field in one call is `invalid_metadata_request` (`panes.rs:1659-1668`), and a call that sets
nothing and clears nothing is the same error (`:1669-1682`).

**`pane.release_agent`** — `panes.rs:517-524` (`pane_id`, `source`, `agent`, `seq`) →
`AppEvent::HookAgentReleased` → `release_agent_with_mutation` for any non-official source
(`app/actions.rs:1626-1642`).

**`pane.clear_agent_authority`** — `panes.rs:508-515` (`pane_id`, optional `source`, optional
`seq`) → `AppEvent::HookAuthorityCleared` (`app/api/panes.rs:1786-1790`).

**`seq` semantics** — `socket-api.mdx:798`: *"For the same `source`, reports with a sequence
number less than or equal to the last accepted sequence are accepted by the API but ignored by the
pane state. A pane or workspace accepts sequenced token reports from at most **32 distinct
sources** during its lifetime; clearing or expiry does not release those source slots."* The
state-report side uses the same ledger (`accept_hook_report`, `state.rs:707-709`). So the
bridge uses **one** `source` string for its whole life and a monotonic `seq` — pi's
`nextMetadataReportSeq` seeds at `Date.now() * 1000` and takes `max(prev+1, Date.now()*1000)`
(`herdr-status.ts:16-21`); the Python client does the identical thing at `client.py:129,146-149`.

**`events.subscribe`** — `socket-api.mdx:800-842`, types at
`tmp/herdr/src/api/schema/events.rs:11-85`. Wire facts the docs do not spell out but the server
does:

- The ack is a normal success response, `{"id":"<your id>","result":{"type":"subscription_started"}}`
  (`ResponseResult::SubscriptionStarted{}`, `schema/response.rs:214`, written at
  `server.rs:749-754`).
- A pushed event is a `SubscriptionEventEnvelope` — `{"event": <kind>, "data": {…}}` with
  **no `id`** (`schema/events.rs:377-381`, written at `server.rs:769`). The client discriminates
  on the presence of `event` vs `result`/`error`.
- A subscription whose `pane_id` does not resolve fails the **whole** subscribe request before the
  ack (`server.rs:725-747`; the test at `server.rs:1526-1540` asserts `pane_not_found`).
- The server polls its subscriptions every `CONNECTION_POLL_INTERVAL = 100 ms`
  (`server.rs:28`, loop at `:762-778`). That is the event-delivery latency floor — design for it,
  do not fight it.
- `INITIAL_REQUEST_TIMEOUT = 5 s` (`server.rs:30`): a connection that does not write its request
  line within 5 s is dropped.
- `pane.agent_status_changed` payload: `{pane_id, agent_status}` where `agent_status` is the
  five-value **derived** `AgentStatus` (`schema/events.rs:186-189`).

**The no-gap bootstrap** — `socket-api.mdx:118-130`, verbatim protocol: open `events.subscribe` on
another connection, wait for its ack, buffer that stream while calling `session.snapshot`, install
the snapshot, apply the buffered events in order, keep streaming; re-snapshot after reconnect.
Given §1b this is not merely advisable, it is **forced**: a subscribed connection can never carry
another request.

**`agent.view.set`** — `socket-api.mdx:418-495`, types at
`tmp/herdr/src/api/schema/agents.rs:52-160`. Two constraints the docs bury and that the port must
respect:
- There is exactly **one** view override server-wide (`state.agent_view_override`, a single
  `Option`, `app/api/agent_view.rs:88-89`); *"A successful set atomically replaces the previous
  view"* (`socket-api.mdx:481-482`). Setting one is a **global UI takeover** of the human's
  sidebar.
- `agent.view.clear` with a `source` is a no-op unless that source still owns the view
  (`agent_view.rs:49-62`; *"A source mismatch leaves the active view unchanged"*,
  `socket-api.mdx:494`). Always clear **with** cyrup's source, never unconditionally.
- Non-`plugin:` sources are allowed (`socket-api.mdx:479-481`); the `plugin:` prefix triggers a
  plugin-existence check (`agent_view.rs:15-22`).

---

### 3. The authority model — the part that decides cyrup's `source` and `agent` strings

This is the highest-risk area in the batch and it is invisible from the docs. A `pane.report_agent`
lands on `AppEvent::HookStateReported` (`app/api/panes.rs:1558-1571`) and is routed at
`app/actions.rs:1536-1565`:

1. `is_reserved_native_state_source(&source, &agent_label)` (`tmp/herdr/src/agent_resume.rs:100-113`)
   — if the pair is one of `("herdr:claude","claude")`, `("herdr:codex","codex")`,
   `("herdr:copilot","copilot")`, `("herdr:devin","devin")`, `("herdr:droid","droid")`,
   `("herdr:qodercli","qodercli")`, `("herdr:qwen","qwen")`, `("herdr:cursor","cursor")`,
   `("herdr:grok","grok")` — **the state is silently dropped** and only the session reference is
   stored. **cyrup must never use a `herdr:<known-agent>` source.**
2. Otherwise `set_hook_authority_at` (`terminal/state.rs:640-754`) runs a gauntlet:
   - `session_identity_only_integration` (`detect/mod.rs:339-347`) → `return None` (hermes, qwen,
     letta, antigravity).
   - a non-`full_lifecycle` source is ignored while `recent_agent_process_exit` matches its agent
     label (`:653-659`);
   - `known_agent_label_conflicts_with_detected_agent` (`:672-674`, defined `:1629-1635`) →
     `parse_agent_label(label)` that resolves to a *different* agent than the pane's detected one
     drops the report;
   - `current_session_owner_conflicts` (`:675-684`) drops it unless a foreground takeover is
     confirmed;
   - `accept_hook_report(&source, seq)` (`:707-709`) applies the `seq` ledger.

`parse_agent_label` (`detect/mod.rs:188-228`) is a closed table of 26 known agent names.
**`"cyrup"` is not in it**, so `parse_agent_label("cyrup") == None`, which makes the
label-conflict rung (`:1633-1634`, `is_some_and`) unreachable and the exit rung (`:655`) too.
`normalize_reported_agent_label` (`app/api_helpers.rs:191-200`) passes an unknown label through
verbatim after trimming. This is the same trick the Python client uses with `"codepuppy"`
(`client.py:60-61`) — except that comment claims the label *must* match
`agent_label(Agent::CodePuppy)`, and there is no `CodePuppy` variant in
`detect/mod.rs:200-227`; the label works precisely *because* it is unknown.

**Therefore: `source = "cyrup:subagents"`, `agent = "cyrup"`.** Rationale, in the port's doc
comment: the `herdr:` prefix is reserved by convention for herdr's own registered integrations
(`socket-api.mdx:479`, `detect/mod.rs:327-347`, `agent_resume.rs:100-113`) and every `herdr:`
pair herdr knows is a *drop* or a *privilege* rung cyrup is not on. A `cyrup:`-prefixed source is
inside the character set (`socket-api.mdx:794`), is under 80 chars, is stable for the whole
process (the `seq` ledger and the 32-source cap both key on it), and is what
`agent.view.clear`'s ownership check will be given.

**And the consequence that shapes §7:** `hook_authority_is_effective`
(`terminal/state.rs:1812-1817`) is

```rust
!full_lifecycle_hook_authority(&authority.source, &authority.agent_label)
    || parse_agent_label(&authority.agent_label)
        .is_none_or(|agent| self.detected_agent == Some(agent)
                         && self.recent_agent_process_exit.is_none())
```

For a `cyrup:` source the first disjunct is `true` **unconditionally**. cyrup's reported state
therefore wins over screen detection forever, with no TTL (§1e) and no process-exit override — the
exit path only records `recent_agent_process_exit` when it can name a known agent
(`state.rs:401-407`, `if let Some(agent) = agent`), and cyrup is not one. **A cyrup that dies
without releasing strands its pane at `working` or `blocked` permanently.** Release is not
hygiene here; it is the correctness requirement.

---

### 4. What cyrup already has — do not rebuild these

| already in tree | where | use it for |
|---|---|---|
| A complete herdr **CLI** client: bin resolution, spawn, timeout+cancel race, ENOENT→not-installed, JSON/text output, error normalization | `crates/cyrup-intercom/src/project_pane.rs:378-520` (`HerdrLauncher::run`), types `:30-143` | the fallback path and the not-installed message; **do not write a second one** |
| `HERDR_BIN` env constant, with the "no `CYRUP_` prefix for vendor vars" rule already argued | `crates/cyrup-intercom/src/identity.rs:56-60` | `ENV_HERDR_BIN`; add the herdr-owned five beside it |
| The `HERDR_UNAVAILABLE`/`PANE_GONE`/… normalized code set, as a Rust enum with pi's wire spellings | `crates/cyrup-intercom/src/project_pane.rs:30-67` | the CLI layer's errors; the socket layer gets herdr's own codes (§1h) |
| **The authoritative "awaiting the human" counter**, process-wide, already ref-counted | `crates/cyrup-ext/src/native.rs:88-123` (`HumanWaitGate { waiting: AtomicUsize }`, `is_waiting`, `begin`, `Drop`) | §6 — this IS the `blocked` seam |
| The one session-scoped human-prompt lock every dialog takes | `crates/cyrup-ext/src/host/services.rs:196-232` (`HumanInteractionLock` / `HumanInteractionGuard`) | the second, coarser blocked signal; see §6 |
| Inter-extension bus: declare in `init`, receive in `on_bus_event` | `crates/cyrup-ext/src/native.rs:466` (`InitApi::subscribe_bus`), `:523` (`NativeExtension::on_bus_event`), fanout `crates/cyrup-ext/src/facade.rs:2785` | the consumer half; already used at `crates/cyrup-ext-subagents/src/extension/host/native_impl.rs:261` |
| Two live bus topics with real producers | `crates/cyrup-ext-subagents/src/background/watch/observer.rs:88` (`subagent:async-complete`, emitted `:149`) and `:178` (`subagent:process-terminal`, PR #147) | the run-ended edges |
| `HostServices::emit_event(topic, payload)` | `crates/cyrup-ext/src/host/services.rs:468` | the producer half of any new topic |
| The **exact** `getRuns` projection pi's bridge wants | `crates/cyrup-ext-subagents/src/tui/fleet_state.rs:460` (`FleetState`), built by `SubagentExecutor::fleet_state(cwd, …)`, already called on host edges at `crates/cyrup-ext-subagents/src/extension/host/slash.rs:302-309` | the TTL-refresh resync and the `SessionStart` restore |
| `AsyncRunView` with `is_active()`, `updated_at()`, `status.telemetry` | `tui/fleet_state.rs:~392-437` | run enumeration |
| `ActivityState::{ActiveLongRunning, NeedsAttention}` on live telemetry | `crates/cyrup-ext-subagents/src/background/telemetry.rs:25-32`, carried on `NestedStepView::activity_state` / `NestedRunView::activity_state` (`tui/fleet_state.rs:62,95`) | the per-run attention flag |
| The whole needs-attention state machine and its notice pipeline | `crates/cyrup-ext-subagents/src/exec/control.rs:1205-1236` (`emit_needs_attention`, returns `true` only on a genuine transition), sink at `crates/cyrup-ext-subagents/src/extension/executor/notices.rs:343-389`, pump `:283-340` | the foreground `blocked` edge — subscribe to this sink, do not re-derive |
| **The privacy-safe label source**, already persisted | `RunStatus.telemetry.workflow_graph: Option<WorkflowGraphSnapshot>` (`background/telemetry.rs:173`), written every step by `refresh_workflow_graph` (`background/runner_main/status.rs:44-47`); `WorkflowGraphSnapshot { nodes, current_node_id }` (`background/workflow_graph.rs:154-166`); `WorkflowGraphNode { label: String, status, .. }` (`:97-136`) | the direct port of pi's `workflowTaskLabel` — §9 |
| The raw-prompt field that must NEVER leave the machine | `RunHistoryEntry.task` (`background/run_history.rs:24`), `SingleStepSpec.task` (`registration/slash_commands.rs:1261,1328,1426,1620`) | the negative half of the privacy test |
| `has_ui` on every host ctx | `crates/cyrup-ext/src/native.rs:131` | pi's `sessionStarted({ hasUI })` root-session gate (`herdr-status.ts:376-380`) |
| Host lifecycle edges already handled in this crate | `extension/host/native_impl.rs:315` `SessionStart`, `:442` `AgentEnd`, `:529` `SessionShutdown`, `:590` `TurnEnd` | arm / refresh / release |
| `cyrup-it` already drives real Unix sockets with `tokio::net::{UnixListener,UnixStream}` and a hostile-listener pattern | `crates/cyrup-it/tests/intercom/protocol_explicit_null_rejection.rs:53,406`; `crates/cyrup-it/Cargo.toml:127` already has tokio `net` | the fake-herdr test server — copy this pattern |

Two things are **not** there and the greps prove it:

- `git grep -n 'subagent:async-started' crates/` → no hits. Only `async-complete` and
  `process-terminal` exist (`observer.rs:88,178`). There is **no launch-edge bus topic**.
- `git grep -nE 'signal::unix|SignalKind|ctrl_c' crates/ | grep -v tests` → no hits. cyrup installs
  **no** termination-signal handler anywhere. Given §3 that is a real gap for this feature.
- Workspace `tokio` has no `net` feature (`Cargo.toml:146`); `cyrup-intercom` adds it on its own
  edge (`crates/cyrup-intercom/Cargo.toml:56`). Whichever crate hosts the socket client must do
  the same. Not a blocker, a one-line manifest change.

---

### 5. The state mapping, and how depth is counted with fleets of children

Adopt the Python client's shape (`reporter.py:20-28,95-101`) — it is the only one of the three
sources that actually reports semantic state, and its ordering is right:

```
blocked  if attention_depth > 0        // parked on the human — highest priority
working  elif run_depth > 0            // some work is in flight
idle     otherwise                     // control is the human's
```

`unknown` is never reported: cyrup always knows which of the three it is. `done` cannot be reported
(§1a); herdr derives it from `idle` + unseen, which is precisely the "finished while you were
looking away" badge, and it arrives for free.

**Counting `run_depth` for a fleet.** cyrup is not code-puppy: it has a root turn, foreground
sub-runs, and detached background runners that outlive the parent. Three sources, one counter,
each with its own rule:

1. **The root turn** — `+1` on `HostEvent::AgentStart`, `−1` on `HostEvent::AgentEnd`
   (`crates/cyrup-ext/src/event.rs:329,330`). This is the edge pi's subagents bridge does not have
   and code-puppy does (`register_callbacks.py:29`). It is what makes the pane say `working` while
   *cyrup itself* is thinking, not only while a subagent is out.
2. **Foreground child runs** — the foreground control registry already keeps one entry per
   in-flight run (`extension/executor/foreground_control.rs`, read at
   `extension/executor/notices.rs:348-369`). Count **entries**, not children: a PARALLEL run with
   nine children is one run in flight, and nine `+1/−1` pairs would make the counter's value
   meaningless without changing its sign. The pane only needs the sign.
3. **Background runs** — derived, not counted. `FleetState`'s async runs already carry
   `AsyncRunView::is_active()` (`tui/fleet_state.rs:~184-189`: `Queued | Running`). Take
   `state.async_runs.iter().filter(|r| r.is_active()).count()` at each resync. This is a **level**,
   not an edge, which is correct for detached runners: a background run that was started by a
   previous process and is still going must count on `SessionStart`, and nothing will ever deliver
   its "started" edge to this process.

So the counter is `AtomicUsize` for (1)+(2) — genuine edges, saturating-subtracting on the way
down exactly as the Python client does (`reporter.py:193`, `max(0, depth-1)`) and as
`HumanWaitGuard::drop` already does in-tree — plus a *recomputed level* for (3) folded in at every
resync. `run_depth > 0` is then
`edge_depth.load() > 0 || active_background_runs > 0`.

**Why the saturating subtract matters and is not paranoia.** `AgentEnd` fires without a matching
`AgentStart` on at least one path in this crate: `native_impl.rs:442-470` handles `AgentEnd`
unconditionally, including for a session that was resumed mid-turn. An unsigned wrap would pin the
pane at `working` forever (§3: forever means *forever*). `HumanWaitGate` already models the right
discipline with `fetch_add`/`fetch_sub` around an RAII guard (`native.rs:103-122`); use a guard
here too so a panic unwind cannot leak a count.

**The transition filter.** Only *changed* `(state, message)` pairs go on the wire
(`reporter.py:126-134`). A cyrup fleet produces far more edges than code-puppy, and every report is
a socket connect (§1b). Dedup is not an optimization here, it is the thing that keeps the reporter
from opening a socket per tool call.

---

### 6. Where `blocked` comes from — the load-bearing seam

**The primary seam, with file:line:**
`crates/cyrup-ext/src/native.rs:88-123` —

```rust
pub struct HumanWaitGate {
    /// Number of live [`HumanWaitGuard`]s (a human wait is in progress while `> 0`). …
    waiting: AtomicUsize,
}
impl HumanWaitGate {
    pub fn is_waiting(&self) -> bool { self.waiting.load(Ordering::Acquire) > 0 }
    fn begin(self: &Arc<Self>) -> HumanWaitGuard { self.waiting.fetch_add(1, Ordering::AcqRel); … }
}
impl Drop for HumanWaitGuard { fn drop(&mut self) { self.gate.waiting.fetch_sub(1, Ordering::AcqRel); } }
```

Reached only through `HostCtx::begin_human_wait` (`native.rs:256`), which is the sanctioned wrapper
every blocking-on-a-human handler takes. Its current holders, all verified:
`crates/cyrup-permission-system/src/extension/prompt.rs:184` (the permission dialog),
`crates/cyrup-mcp/src/owner.rs:665` (MCP's `select`/`input`/`confirm` owner path).
`crates/cyrup-ext/src/native.rs:1035` states the contract: *"Only natives that actually block on a
human"* take it. This is cyrup's exact analogue of code-puppy's
`command_runner.set_awaiting_user_input` (`register_callbacks.py:17-22`) — **one process-wide
choke-point, already ref-counted, already `AtomicUsize`.**

The companion lock is `crates/cyrup-ext/src/host/services.rs:196-225`
(`HumanInteractionLock`, one `Semaphore` permit, reached via
`HostServices::human_interaction_lock()`); its doc at `:180-191` names both callers and says
*"a permission approval and a subagent clarify can never prompt the same human simultaneously."*
Use the **gate**, not the lock: the lock's holder set is the same but a semaphore has no
observable count, and the gate is already the thing the dispatcher polls.

**The gate is level-only today — it has no edge.** `is_waiting()` is a poll. The bridge needs a
notification, and polling a `100 ms`-latency sidebar from a `100 ms` timer is the wrong trade. The
minimal, honest change is an **optional observer on the gate**, in `cyrup-ext`:

```rust
// crates/cyrup-ext/src/native.rs, beside HumanWaitGate
pub trait HumanWaitObserver: Send + Sync {
    /// Called on every 0→n and n→0 transition of the gate's count. Never called with the
    /// gate's lock held (the gate has none); must not block.
    fn human_wait_changed(&self, waiting: bool);
}
impl HumanWaitGate {
    /// Install the one observer. Idempotent-by-replacement; `None` removes it.
    pub fn set_observer(&self, observer: Option<Arc<dyn HumanWaitObserver>>) { … }
}
```

`begin`/`Drop` then compare the pre/post count against zero and fire on the edge only. This keeps
`is_waiting()` byte-identical for the budget watchdog, adds no cost when no observer is installed
(one `Option<Arc<…>>` load), and gives the bridge a real edge. It is 25 lines.

**The secondary seam — a subagent that needs the human.** `ActivityState::NeedsAttention`
(`background/telemetry.rs:29-31`) is raised by
`crates/cyrup-ext-subagents/src/exec/control.rs:1205-1236` (`emit_needs_attention`, which
*returns `true` only on a genuine transition*, `:1236`) and again by the completion-mutation guard
at `:1420-1428`. It reaches the parent two ways, both already built:
- **foreground**: the `ControlEventSink` at
  `extension/executor/notices.rs:343-389`, whose pump builds a `ControlNotice` with
  `kind: ControlNoticeKind::NeedsAttention` (`:301-326`) and a `LiveRunView { needs_attention }`
  (`:327-331`). Attach there — the pump is already ordered and already has the run id.
- **background**: on the fleet projection, as
  `NestedStepView::activity_state == Some(NeedsAttention)` / `NestedRunView::activity_state`
  (`tui/fleet_state.rs:62,95`), folded into `FleetState` by `fleet_state()`. Picked up at every
  resync — no new plumbing.

This is pi's own `blocked` source (`herdr-status.ts:117-126,363-368`) and it is the one the seed
names. It is the *secondary* seam because it is a heuristic (idle past
`needs_attention_after_ms`, `exec/control.rs:76-77,465-479`) whereas the gate is a fact.

**The combined rule.** `attention_depth > 0` where the depth is
`human_wait_edges + |{runs with needs_attention not yet acknowledged}|`, with pi's acknowledgement
semantics ported verbatim: a raise is suppressed for a run already in the map
(`herdr-status.ts:290`), `agentStarted()` acknowledges everything currently raised
(`:372-375`) so the next turn does not re-block on a stale notice, and a run leaving the active set
drops its acknowledgement (`:313-315`). Map `agentStarted()` onto `HostEvent::AgentStart`.

**Report the two with different `message` text** — `message` is free-form on
`PaneReportAgentParams` (`panes.rs:453-454`) and the Python client already uses it decoratively
(`reporter.py:55-56,103-109`). `"awaiting approval"` for the gate, the run's own notice reason for
the subagent case (`exec::control::control_event_reason_wire`, `notices.rs:313-318`). The
*state* is `blocked` either way; only the message differs. Never put the notice **body** there —
`format_control_notice_message` (`notices.rs:322-325`) renders the full text and that text can
quote the child's output. §9.

---

### 7. The producer, event by event

pi's bridge is fed by three pi events (`herdr-status.ts:343-368`): `SUBAGENT_ASYNC_STARTED_EVENT`,
`SUBAGENT_ASYNC_COMPLETE_EVENT`, `SUBAGENT_CONTROL_EVENT`. cyrup has one of the three on the bus.
The mapping, with what is missing marked:

| edge | cyrup source | exists? | bridge action |
|---|---|---|---|
| session arms | `HostEvent::SessionStart` at `extension/host/native_impl.rs:315`, with `ctx.has_ui` | **yes** | `bridge.session_started(has_ui, fleet_state)` — pi `:376-380`. Non-UI ⇒ stay inert forever. |
| root turn begins | `HostEvent::AgentStart` (`cyrup-ext/src/event.rs:329`) | **yes**, not yet handled in this crate | `run_depth += 1`; acknowledge outstanding attention (pi `agentStarted`, `:372-375`) |
| root turn ends | `HostEvent::AgentEnd` at `native_impl.rs:442` | **yes** | `run_depth -= 1` (saturating) |
| turn boundary repaint | `HostEvent::TurnEnd` at `native_impl.rs:590` | **yes** | resync + metadata refresh (the Python client's canonical refresh point, `reporter.py:220-229`) |
| a background run is launched | — | **NO** | add `subagent:async-started` beside the two existing topics, emitted from the launch path, owned by its emitter exactly as `observer.rs:81-88` argues |
| a background run finished | `subagent:async-complete` (`observer.rs:88`, emitted `:149`) | **yes** | drop the run from the active set |
| a runner's process really ended | `subagent:process-terminal` (`observer.rs:178`, PR #147) | **yes** | reconcile the active set against reality; this is the topic that catches a crashed runner whose result file never appeared |
| a run needs the human | foreground `ControlEventSink` (`notices.rs:343`); background via `FleetState` | **yes** | raise/clear attention (§6) |
| a human dialog opens/closes | `HumanWaitGate` + the new observer (§6) | gate **yes**, edge **no** | raise/clear attention |
| session ends cleanly | `HostEvent::SessionShutdown` at `native_impl.rs:529` | **yes** | `pane.release_agent`, then `pane.report_metadata{clear_*}` — pi `dispose()` (`:388-398`) |
| session dies | — | **NO** | §3 makes this mandatory. See below. |

**The `subagent:async-started` topic.** Rather than inventing a payload, mirror
`BusAnnouncingCompletionObserver`'s discipline (`observer.rs:109-123`): publish the launch's own
record, verbatim, and nothing derived. The constant lives with its emitter. Note that
`subscribe_bus` is `InitApi`-only (`cyrup-ext/src/native.rs:466`), so the bridge declares all three
topics during `init`, beside the existing `api.subscribe_bus(SUBAGENT_RPC_REQUEST_EVENT)` at
`native_impl.rs:261`.

**The death case, and why it is in scope.** §3 proved that herdr never reclaims state from a
`cyrup:` source: no TTL, no process-exit override. `SessionShutdown` covers a clean exit. It does
not cover SIGTERM, SIGHUP, a closed pane, or a panic — and cyrup installs no signal handler at all
(the grep in §4). The Python client treats this as first-class and says why
(`register_callbacks.py:36-40`):

> "Callbacks alone are not enough to guarantee the pane is released. They fire from a `finally:` in
> `cli_runner`, which the interpreter only reaches on a graceful exit. `_install_exit_guards` adds
> an `atexit` hook and a SIGTERM/SIGHUP handler so a closed pane, a plain `kill`, or a logout also
> release pane authority instead of stranding a dead agent in herdr's sidebar."

Port that: a **release guard** armed only when the bridge is active, installing a
`tokio::signal::unix` handler for `SIGTERM` and `SIGHUP` that performs one bounded, blocking
`pane.release_agent` and then re-raises the default disposition. `signal` is already in the
workspace tokio features (`Cargo.toml:146`); only `net` is missing. This is the single most
user-visible piece of the batch after `blocked` itself: a stale `⏳ working` row that never clears
is worse than no row at all.

---

### 8. The Rust shape

New module tree, in `crates/cyrup-ext-subagents/src/herdr/`:

```
herdr/mod.rs          // the gate, the public entry points, the module doc carrying §3's rationale
herdr/state.rs        // ReportedState, StateModel  (pure; no I/O, no clock, no socket)
herdr/label.rs        // the privacy rule: bounded_task_label, workflow_task_label, summary, title_suffix
herdr/reporter.rs     // the tokio task: two-lane mailbox, dedup, seq, TTL refresh, release
herdr/consumer.rs     // events.subscribe + session.snapshot bootstrap, the no-gap ordering
herdr/view.rs         // agent.view.set / .clear
herdr/release_guard.rs// SessionShutdown + SIGTERM/SIGHUP
```

Types:

```rust
/// The four states herdr accepts on `pane.report_agent`
/// (tmp/herdr/src/api/schema/common.rs:149-156). `Done` is DERIVED by herdr from
/// `Idle` + unseen (api_helpers.rs:96-107) and is deliberately absent here.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneAgentState { Idle, Working, Blocked, Unknown }

/// The pure state machine. No clock, no I/O, no socket — every input is a method call
/// and the only output is `Option<StateReport>`, which is `Some` exactly on a change.
pub struct StateModel {
    edge_depth: u32,                       // root turn + foreground runs (saturating)
    active_background_runs: usize,         // recomputed level, not an edge
    human_waits: u32,                      // HumanWaitGate edges
    attention: BTreeMap<RunId, String>,    // run -> notice reason (insertion-ordered like pi's Map)
    acknowledged: BTreeSet<RunId>,
    last_reported: Option<(PaneAgentState, Option<String>)>,
}

pub struct StateReport { pub state: PaneAgentState, pub message: Option<String> }

/// What the pane's presentation shows. Separate type, separate verb, separate lane.
pub struct MetadataReport {
    pub state_labels: BTreeMap<&'static str, String>, // idle/done/working, pi herdr-status.ts:224-226
    pub tokens: BTreeMap<&'static str, Option<String>>, // summary, title-suffix
    pub ttl_ms: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum HerdrBridgeError {
    #[error("herdr reported {code}: {message}")] Rejected { code: String, message: String },
    #[error("herdr pane {pane_id} is gone")]     PaneGone { pane_id: String },
    #[error("herdr transport: {0}")]             Transport(#[from] HerdrClientError), // sibling batch
}
```

**The client seam this batch consumes** (owned by `HERDR_NATIVE_CLIENT.md`, named here so both
batches agree):

```rust
#[async_trait]
pub trait HerdrPaneReporter: Send + Sync {
    async fn report_agent(&self, p: ReportAgentParams)      -> Result<(), HerdrClientError>;
    async fn report_metadata(&self, p: ReportMetadataParams)-> Result<(), HerdrClientError>;
    async fn release_agent(&self, p: ReleaseAgentParams)    -> Result<(), HerdrClientError>;
    async fn set_agent_view(&self, p: AgentViewSetParams)   -> Result<(), HerdrClientError>;
    async fn clear_agent_view(&self, source: &str)          -> Result<(), HerdrClientError>;
    async fn session_snapshot(&self)                        -> Result<SessionSnapshot, HerdrClientError>;
    /// Opens a DEDICATED connection (§1b) and yields the ack, then the pushed events.
    async fn subscribe(&self, subs: Vec<Subscription>)      -> Result<EventStream, HerdrClientError>;
}
```

A trait, not a struct, for one reason that is about *this* batch: the reachability tests in §11
drive the production bridge against a recording double and against a real
`UnixListener`-backed fake herdr, and both need the same seam. The production impl is the sibling
batch's socket client; the CLI fallback wraps the existing
`crates/cyrup-intercom/src/project_pane.rs:389` `HerdrLauncher`.

**Gate**, ported from `herdr-status.ts:129-131` with §1f's correction:

```rust
pub struct HerdrEnv { pub pane_id: String, pub tab_id: Option<String>, pub socket_path: Option<PathBuf> }
impl HerdrEnv {
    /// `HERDR_ENV == "1"` AND a non-empty `HERDR_PANE_ID`. The conjunction is load-bearing:
    /// `PaneLaunchIdentity::OmitPane` (tmp/herdr/src/pane.rs:170-172) leaves `HERDR_ENV=1` and
    /// the socket path set while REMOVING the pane id.
    pub fn detect(env: impl Fn(&str) -> Option<String>) -> Option<Self>;
}
```

**Threading.** One `tokio` task, fed by a coalescing two-lane mailbox
(`tokio::sync::watch` per lane is the idiomatic latest-wins slot; the Python client hand-rolls the
same thing at `client.py:104-120` and explains the lanes at `:21-34`): a **critical** lane for
state edges and the release, a **decorative** lane for message-only refreshes and metadata, with
the decorative lane discarded once a release is queued. `seq` is assigned at wire time, not at
enqueue time, so a critical report that overtakes a decorative one still carries the higher
sequence (`client.py:353-356` states exactly this and it is not obvious).

---

### 9. The privacy rule, ported and pinned

The rule, verbatim from `herdr-status.ts:32`: *"Explicit launch/workflow label only; raw prompts
never enter pane metadata."*

**What may be sent:**
- `WorkflowGraphNode::label` for the active node, selected exactly as pi's `workflowTaskLabel`
  does (`herdr-status.ts:79-98`): prefer `current_node_id`'s node, else the first `Running`, else
  the first `Pending`; walk `children` to depth 8 over at most 128 nodes (`:13-14,85-94`).
  In cyrup the graph is already on disk at
  `RunStatus.telemetry.workflow_graph` (`background/telemetry.rs:173`) with
  `current_node_id` (`workflow_graph.rs:165`) and per-node `status`
  (`WorkflowNodeStatus`, `:111`), so this is a pure function over a struct cyrup already has —
  no new capture, no new persistence.
- agent **names** (`NestedRunView::agent`/`agents`, `fleet_state.rs:20-22`), counts, pane counts.
- the run's own control-notice **reason** token (`control_event_reason_wire`, an enum, not text).

**What may not, ever:** `RunHistoryEntry.task` (`background/run_history.rs:24` — pi's 200-char
prompt capture), `SingleStepSpec.task` (`registration/slash_commands.rs:1261`), any child
transcript or tool output, and `format_control_notice_message`'s rendered body
(`extension/executor/notices.rs:322-325`), which interpolates the child's own text.

**Bounds, from herdr and from pi both:** pi caps its label at 80 chars
(`MAX_TASK_LABEL_CHARS`, `herdr-status.ts:11`) and the title suffix at 42
(`MAX_TITLE_TASK_CHARS`, `:12`); herdr independently caps every presentation field and token value
at 80 and strips control characters before storage (`socket-api.mdx:792`). Apply pi's caps
client-side so the *cyrup* side of the contract is testable without a herdr, and route every string
through `cyrup_tui::ansi::sanitize_display_text` (`crates/cyrup-tui/src/ansi.rs:29`) — the in-tree
equivalent of pi's `sanitizeDisplayText` (`herdr-status.ts:6`).

**And the rule that is not about prompts:** the semantic state goes through `pane.report_agent`
and the labels through `pane.report_metadata`, never the reverse
(`socket-api.mdx:717-718,754`). A `message` on `report_agent` is decorative and short; the
presentation belongs in metadata. Pin that too — it is the seam a careless refactor collapses.

---

### 10. Production call sites — exact

1. **`crates/cyrup-ext-subagents/src/extension/host/native_impl.rs`, `init`** (beside
   `api.subscribe_bus(SUBAGENT_RPC_REQUEST_EVENT)` at `:261`): declare
   `subagent:async-started`, `subagent:async-complete`, `subagent:process-terminal`.
2. **`native_impl.rs:315`, `HostEvent::SessionStart`**, ordered **before**
   `self.emit_rpc_ready()` at `:437` and after `refresh_fleet_status_widget` at `:429` — pi's own
   order is "herdr bridge, then `rpcBridge.emitReady`", which the comment at `:432-434` already
   records (*"the tail of its own `session_start` handler, after the herdr bridge and before
   `supervisorChannel.start()`"*). Call
   `self.herdr.session_started(ctx.has_ui, &self.executor.fleet_state(&ctx.cwd, …).await)`.
3. **`native_impl.rs:442`, `HostEvent::AgentEnd`** — `run_depth -= 1` and resync, after
   `self.watchdog.handle_agent_end` (registration order is already argued at `:443-455`; the
   bridge is a notifier and goes last).
4. **`native_impl.rs`, a new `HostEvent::AgentStart` arm** — `run_depth += 1` plus
   `acknowledge_attention()`.
5. **`native_impl.rs:590`, `HostEvent::TurnEnd`** — the metadata/TTL refresh point.
6. **`native_impl.rs:529`, `HostEvent::SessionShutdown`** — `release()`, awaited, bounded.
7. **`native_impl.rs:701`, `on_bus_event`** — the three topics.
8. **`crates/cyrup-ext-subagents/src/extension/executor/notices.rs:343-389`**, inside the existing
   `ControlEventSink` closure, after the channel gate at `:375-380` and the
   `ActiveLongRunning` early return at `:383-385`: raise/clear attention for foreground runs.
9. **`crates/cyrup-ext/src/native.rs`** — the `HumanWaitObserver` hook (§6); the bridge installs
   itself as the observer at `session_started` and removes itself at `release`.
10. **The background-launch path** — emit `subagent:async-started`, placed by the same rule
    `observer.rs:81-88` uses (the topic constant lives with its emitter).
11. **`herdr/view.rs`** — `agent.view.set` at `session_started` **only when configured on**, and
    `agent.view.clear{source}` at `release`. §12 argues why it is opt-in.

Note on `getProjectPaneCount` (`herdr-status.ts:44,162`): it is fed by the project-pane manager,
which is the `INSPECTOR_AND_PROJECT_PANES.md` batch. Until that lands, the label simply omits the
pane clause — pi's own code does the same when the callback is absent
(`:162`, `?? 0`, and `:163` renders nothing at 0). No stub, no `todo!()`, no coupling.

---

### 11. Reachability tests, each with the gutting that makes it go red

All in `crates/cyrup-it/tests/subagents/`, registered in that suite's `main.rs`, following the
`UnixListener` fake-server pattern already used at
`crates/cyrup-it/tests/intercom/protocol_explicit_null_rejection.rs:406`.

1. **`herdr_status_bridge_integration.rs` — `a_blocked_subagent_reaches_the_pane`.** Bind a fake
   herdr `UnixListener`, set `HERDR_ENV=1`/`HERDR_PANE_ID=w1:p1`/`HERDR_SOCKET_PATH`, drive a real
   `SessionStart` with `has_ui: true`, start a run, raise `needs_attention` through the **real**
   `ControlEventSink`, and assert the fake server received a `pane.report_agent` line whose
   `params.state == "blocked"` and `params.source == "cyrup:subagents"`.
   *Gutted by*: making the sink's raise a no-op, replacing `StateModel::recompute` with
   `PaneAgentState::Idle`, dropping the `blocked`-before-`working` precedence, or removing the
   `session_started` call site — each yields zero `blocked` lines and the assertion fails on an
   empty capture.
2. **`the_human_wait_gate_blocks_the_pane`.** Same harness; take a real
   `HostCtx::begin_human_wait()` guard and assert a `blocked` report lands; drop the guard and
   assert the next report is `working` (a run still in flight) or `idle`.
   *Gutted by*: removing the `HumanWaitObserver` install, firing the observer on every change
   rather than on the 0↔n edge (the second guard would emit a duplicate the dedup at
   `StateModel::last_reported` must swallow — assert exactly **one** `blocked`), or ordering
   `run_depth` above `human_waits` in `recompute` (the report becomes `working`).
3. **`a_finishing_child_does_not_flip_the_pane_idle`.** Start two children under one run, end one,
   assert **no** `idle` report was written and the last state is still `working`.
   *Gutted by*: counting children instead of runs, or by making the background level an edge —
   both flip the sign at the wrong moment. This is the exact defect
   `reporter.py:26-28` exists to prevent.
4. **`raw_prompts_never_enter_pane_metadata`.** Launch with a task string containing a unique
   sentinel, run to a labelled workflow node, assert every byte the fake server received
   (`report_agent` *and* `report_metadata`, both verbs) contains the node's `label` and **does not
   contain the sentinel**. Assert it over the raw received lines, not over a struct.
   *Gutted by*: passing `spec.task` into `MetadataReport::tokens`, dropping the
   `workflow_task_label` selection in favour of the run description, or putting
   `format_control_notice_message`'s body into `report_agent.message` — each makes the sentinel
   appear on the wire.
5. **`state_and_presentation_use_different_verbs`.** Assert the capture contains no
   `pane.report_agent` line with a `state_labels`/`tokens`/`title` key, and no
   `pane.report_metadata` line with a `state` key.
   *Gutted by*: folding the two reports into one envelope. This pins
   `socket-api.mdx:717-718`.
6. **`seq_is_monotonic_and_the_source_is_stable`.** Assert `seq` strictly increases across a mixed
   critical/decorative burst and that every line carries the same `source`.
   *Gutted by*: assigning `seq` at enqueue instead of at wire time (`client.py:353-356`), or by
   deriving the source per-report — the 32-source cap (`socket-api.mdx:798`) would be burned.
7. **`session_shutdown_releases_the_pane`.** Drive `SessionShutdown`, assert a
   `pane.release_agent` with cyrup's source/agent, and that it is the **last** line.
   *Gutted by*: removing the release, or by letting a decorative report race past it (the Python
   client drops the decorative lane once a release is scheduled, `client.py:26-28`).
8. **`a_rejected_report_never_disturbs_the_agent`.** Make the fake server answer
   `{"error":{"code":"pane_not_found",…}}`, then `{"error":{"code":"invalid_agent",…}}`, then
   close the socket mid-write. Assert the driving turn completes normally and the bridge keeps
   reporting afterwards.
   *Gutted by*: `?`-propagating `HerdrBridgeError` out of the sink, or panicking on a close.
   pi's own contract: *"Herdr integration is best effort"* (`herdr-status.ts:188-191`).
9. **`the_event_consumer_installs_the_snapshot_before_the_buffered_events`.** Fake server that,
   after acking `events.subscribe`, pushes an event *before* answering `session.snapshot`; assert
   the resulting local cache has the snapshot applied first and the buffered event second.
   *Gutted by*: calling `session.snapshot` before the ack, reusing the subscribe connection for
   the snapshot (which cannot work at all, §1b — the test must show the failure is *observable*,
   not merely theoretical), or dropping the buffer.
10. **`subscribe_and_request_use_different_connections`.** Count `accept()`s on the fake listener:
    assert ≥2 and that the subscribed one never carries a second request line.
    *Gutted by*: a multiplexing client — the fake server (which mirrors `server.rs:156-317` and
    reads exactly one line on a non-streaming connection) simply never answers.
11. **`herdr_status_bridge.rs` unit tests (in `src/herdr/`)** — `StateModel` transition table over
    all orderings of the three inputs; `workflow_task_label`'s depth-8/128-node bounds
    (`herdr-status.ts:13-14`) proven by a 200-node, depth-12 graph; the 80/42-char caps; the
    `HerdrEnv::detect` conjunction including the `OmitPane` shape (`HERDR_ENV=1`, socket set,
    **no** pane id ⇒ `None`).

---

### 12. Not installed, not in a pane, and everything in between

Four distinct runtime states. They are not the same and the batch must not collapse them:

| state | detection | behaviour |
|---|---|---|
| not in a herdr pane | `HerdrEnv::detect` → `None` (`HERDR_ENV != "1"` or no `HERDR_PANE_ID`) | **Total inertness**: no task spawned, no socket opened, no observer installed on `HumanWaitGate`, no bus subscription consumed, no signal handler, no allocation beyond the `Option<...>: None`. pi's `enabled` flag gates its whole registration the same way (`herdr-status.ts:131,342`). |
| in a pane, headless (`cyrup -p`) | `HerdrEnv` present, `ctx.has_ui == false` | Detected but never armed: `session_started` returns before setting `root_session` (pi `:377`, `if (hasUI !== true) return`). A headless child must not fight its parent for the pane's lifecycle authority — pi's doc comment at `:55-60` is the rationale and should be ported verbatim. |
| in a pane, herdr's server gone | `HerdrEnv` present, every connect returns `ECONNREFUSED`/`ENOENT` | Best-effort: log at `debug`, keep the desired state, retry on the next edge and on the TTL refresh. Never surface to the user, never fail a turn. The Python client's own framing: *"reporting agent state must never be able to disturb the agent itself"* (`client.py:17-19`). |
| the `herdr` binary absent | only reached on the CLI fallback path | The existing message, unchanged: *"Herdr is not installed or is not on PATH. Install Herdr 0.7.5+ or set HERDR_BIN."* (`crates/cyrup-intercom/src/project_pane.rs:423-429`). |

**How inertness is tested — the part that usually rots.** Assertions on *absence* are weak unless
they can fail. Three that can:

- `no_herdr_env_means_no_socket`: point `HERDR_SOCKET_PATH` at a `UnixListener` that
  `panic!`s on `accept()`, and *do not* set `HERDR_ENV`. Drive a full session with a run. The test
  fails loudly if anything connects. *Gutted by*: gating on `HERDR_SOCKET_PATH` instead of the
  `HERDR_ENV`+`HERDR_PANE_ID` conjunction.
- `no_herdr_env_means_no_observer`: after `SessionStart` with no herdr env, assert
  `HumanWaitGate::observer()` is `None`, then take a real `begin_human_wait()` guard and assert
  nothing was enqueued anywhere. *Gutted by*: installing the observer unconditionally.
- `omit_pane_shape_is_inert`: `HERDR_ENV=1` + `HERDR_SOCKET_PATH` set + **no** `HERDR_PANE_ID`
  (herdr's real `OmitPane` pane, `tmp/herdr/src/pane.rs:170-172`), same panicking listener.
  *Gutted by*: the single-condition gate. This is the one a reviewer would not think to write and
  it is a shape herdr actually produces.

**Neither herdr nor ghostty is installed in this container.** Every contract above is derived from
herdr's source, and every test above runs against a `UnixListener` this workspace writes, so the
whole batch is verifiable here. Nothing in it needs the binary. The CLI fallback's *spawn* path is
already covered by the existing `HerdrLauncher` tests; this batch adds no new binary dependency.

**Why `agent.view.set` is opt-in.** It is a global, single-slot takeover of the human's sidebar
(§2), and the maintainer's sidebar is a shared surface across every agent in the fleet — not
cyrup's to seize on startup. Ship it behind a config flag, default off, with `agent.view.clear`
scoped to cyrup's source on release so an enabled-then-crashed cyrup cannot leave a filtered
sidebar behind (the view also dies with herdr's server, `socket-api.mdx:482-483`).

---

### 13. Files touched

**New**
- `crates/cyrup-ext-subagents/src/herdr/{mod,state,label,reporter,consumer,view,release_guard}.rs`
- `crates/cyrup-it/tests/subagents/herdr_status_bridge_integration.rs`
- `crates/cyrup-it/tests/subagents/support/fake_herdr.rs` (the `UnixListener` server + capture)

**Modified**
- `crates/cyrup-ext-subagents/src/lib.rs` — `pub mod herdr;`
- `crates/cyrup-ext-subagents/src/extension/host/native_impl.rs` — `:261` (subscribe), `:315`
  (`SessionStart`), a new `AgentStart` arm, `:442` (`AgentEnd`), `:529` (`SessionShutdown`),
  `:590` (`TurnEnd`), `:701` (`on_bus_event`)
- `crates/cyrup-ext-subagents/src/extension/host/mod.rs` — the bridge handle beside
  `fleet_status`/`fleet_inspector_open`
- `crates/cyrup-ext-subagents/src/extension/executor/notices.rs:343-389` — attention raise/clear
- `crates/cyrup-ext-subagents/src/background/watch/observer.rs` — the new
  `SUBAGENT_ASYNC_STARTED_EVENT` constant, beside `:88`/`:178`
- the background-launch path — emit it
- `crates/cyrup-ext/src/native.rs:88-123` — `HumanWaitObserver` + `set_observer`
- `crates/cyrup-ext-subagents/Cargo.toml` — tokio `net` + `signal` on this crate's edge
  (the workspace table at root `Cargo.toml:146` has `signal` but **not** `net`)
- `crates/cyrup-it/tests/subagents/main.rs` — register the new file + its module doc's file count
- `crates/cyrup-ext-subagents/src/extension/rpc/ping.rs` — advertise `events.asyncStarted` only
  once it is emitted (the rule `observer.rs:96-100` already states)
- `docs/gap-analysis/09-cyrup-ext-subagents.md` + `docs/PARITY-PLAN.md:1388` — the `VL-S6` row
  gains its herdr half

---

### 14. Sizing, and the order to land it in

**This is a medium-large batch with one hard external dependency and one small cross-crate
change.** Honest count: ~900–1,200 lines of implementation across seven new files, ~700 of tests,
plus ~25 lines in `cyrup-ext` and eight call sites in `native_impl.rs`. The state machine itself is
small and pure — perhaps 200 lines — and it is the only part anyone will argue about. The bulk is
the reporter's two-lane mailbox, the no-gap consumer, and the tests, and none of that is subtle
once §1b (one request per connection) and §3 (state never expires, never reverts) are internalized.
Nothing here is a research risk any more: every contract in §2 and §3 was read out of herdr's own
source this pass.

**The dependency is real and it is the sibling batch.** `HERDR_NATIVE_CLIENT.md` owns the socket
client. This batch cannot land before it, and it should not be worked around: a private NDJSON
client inside `herdr/reporter.rs` would be the second one in the tree and the wrong one to keep.

**Land in this order.**

1. **`HERDR_NATIVE_CLIENT.md`** — the transport, with §1b as a stated requirement of its API (one
   connection per request, a dedicated one per subscription) and §1a as a stated requirement of its
   types (four reportable states).
2. **`herdr/state.rs` + `herdr/label.rs` + their unit tests** (test 11). Pure, no I/O, no socket —
   landable and reviewable on their own, and they are where the privacy rule and the depth rule
   live. If only this much ever landed, the hard thinking would be done and checked in.
3. **`HumanWaitObserver` in `cyrup-ext`.** Twenty-five lines, one crate, zero behaviour change when
   no observer is installed. Small and independently reviewable; it unblocks the `blocked` half.
4. **`herdr/reporter.rs` + `herdr/mod.rs` + the `native_impl.rs` call sites + the release path**,
   with tests 1–8 and the three inertness tests. **This is the shippable increment**: at the end of
   it the maintainer's sidebar says which pane is blocked on them, which is the whole point of the
   feature, and a clean or signalled exit clears it.
5. **`subagent:async-started`** and its emitter, folding background launches into the model as an
   edge rather than only a level. Improves latency; the level already makes the feature correct
   without it, which is why it is fifth and not second.
6. **`herdr/consumer.rs`** — `events.subscribe` + `session.snapshot`, tests 9–10. This is the first
   piece that is *not* required for the headline outcome. It earns its place by letting cyrup's
   FleetView show the rest of the herdr fleet, and by catching a pane closed from herdr's side.
7. **`herdr/view.rs`** — `agent.view.set`, opt-in, last. It is the smallest piece and the one most
   likely to need the maintainer's taste rather than a spec.

Steps 1–4 are the feature. Steps 5–7 are the part where cyrup goes past pi, and each can land on
its own week without the others.

**Not a residual, but a coupling worth naming:** `getProjectPaneCount` belongs to
`INSPECTOR_AND_PROJECT_PANES.md`. This batch does not need it, does not stub it, and renders
correctly without it (§10). If that batch lands first the clause appears; if it lands second,
nothing here changes.

---

## [WRITE — bridge-live]

Written, not built (writing phase; a VERIFY agent compiles, tests and runs the mutation sweep).
`cargo check -p cyrup-ext-subagents` was run once at the end: **every error in `src/herdr/` and
`src/extension/host/native_impl.rs` is cleared.** The crate as a whole still fails on a
CONCURRENT SIBLING's file — `src/inspectors/actions.rs:486`, `INSPECTOR_SUBCOMMAND` not yet in
`super::runner` — which blocks `--all-targets`, so the new `#[cfg(test)]` modules are
**type-unchecked**. That is the first thing VERIFY should compile.

### What landed

| file | what it owns |
|---|---|
| `src/herdr/reporter.rs` (new, ~480) | the tokio task: two `watch` lanes (critical `pane.report_agent`, decorative `pane.report_metadata`), `biased` arbitration, wire-time `seq`, the 45 s refresh / 120 s TTL, the metadata clear, the release |
| `src/herdr/consumer.rs` (new, ~260) | `events.subscribe` + `session.snapshot` through `cyrup_herdr::ReconnectingEvents`; `pane.closed`/`pane.exited` ⇒ stop; `pane.agent_status_changed` ⇒ re-assert a contradicted state |
| `src/herdr/runtime.rs` (new, ~700) | the four-state gate, the one process-wide `static BRIDGE`, `arm`/`arm_in`/`bridge`/`shutdown`, `sync_fleet`, the human-wait watcher |
| `src/herdr/mod.rs` | the module map and the re-exports |
| `src/extension/host/native_impl.rs` | `init` (bus + `AgentStart` subscriptions), `SessionStart` arm + restore, a new `AgentStart` arm, `AgentEnd`, `TurnEnd` resync, `SessionShutdown` release, `on_bus_event` for the two run-ended topics |
| `crates/cyrup-it/tests/subagents/herdr_status_bridge_integration.rs` (new, ~620) | eight tests against a real `UnixListener` fake herdr |

`Cargo.toml` needed **no change**: `signal` is already on this crate's tokio edge (`:94`), and
`net` is not needed here — every socket byte goes through `cyrup-herdr`, which carries `net`
itself (`crates/cyrup-herdr/Cargo.toml:37`), and features are additive across the graph.
`cyrup-herdr` was already a dependency (`:66-69`).

### The AUG's biggest premise is FALSE — §7's release guard would have been a REGRESSION

> `git grep -nE 'signal::unix|SignalKind|ctrl_c' crates/ | grep -v tests` → no hits. cyrup installs
> **no** termination-signal handler anywhere. Given §3 that is a real gap for this feature.
> — AUG §4

Re-grepped this pass with a pattern that does not drop the hits (`tokio::signal`, no `-- tests`
filter that also swallows `src/`):

```
crates/cyrup/src/signals.rs:185-200          wait_for_signal(): SIGTERM, SIGHUP, ctrl_c
crates/cyrup/src/signals.rs:317-330          the handler body
crates/cyrup-ext-subagents/src/spawn/signal.rs:619
crates/cyrup-ext-subagents/src/background/runner_main/control_watcher.rs:146
```

`crates/cyrup/src/signals.rs` is a full port of pi's three `registerSignalHandlers` sites and has
been there all along. On the **interactive** host — the only host this bridge ever arms on, §12
row 2 — `first_delivery_exit_code` returns `None` (`signals.rs:224-230`), so the handler does
**not** exit: it aborts the session, fires the cancel token, the run loop breaks, and `main`
disposes the runtime, which fans `session_shutdown{quit}` out to every extension
(`signals.rs:60,88`, tested at `crates/cyrup/src/tests/dispatch.rs:274`). That lands on this
crate's `HostEvent::SessionShutdown` arm, which calls `crate::herdr::shutdown()` and releases the
pane.

So **`kill` was already covered**, and `src/herdr/release_guard.rs` — written, then deleted in this
same pass — would have been cyrup's SECOND SIGTERM listener, racing `std::process::exit(143)`
against that teardown. `signals.rs:86-90` records that exact race being removed once already
("truncating the very shutdown ACP-005/ACP-023 exist to guarantee"). The file is gone, no handler
is installed by this batch, and the reasoning is in `src/herdr/mod.rs`'s module doc so it cannot be
re-derived the wrong way.

**The honest residual**, now stated where it belongs (`reporter::METADATA_TTL_MS`): a `SIGKILL`,
and a SECOND signal delivery (which hard-exits, `signals.rs:300-305`), reach no handler in any
language. The presentation half is bounded by the 120 s metadata TTL; the **state** half is not.
Closing it needs something on the herdr side — agent state has no `ttl_ms` field to send
(`tmp/herdr/src/api/schema/panes.rs:447-461`) — so it is not a cyrup-side omission.

### Corrections to the AUG, recorded rather than silently applied

1. **§7's shutdown ordering is wrong and §11's test 7 is right.** §7 says
   "`pane.release_agent`, then `pane.report_metadata{clear_*}` — pi `dispose()`". pi's `dispose()`
   (read this pass, `herdr-status.ts:388-398`) **never calls `release_agent` at all** — it clears
   attention, clears runs and `publish()`es the clear, which is consistent with §1c (pi never
   reports agent state). And releasing before clearing would publish presentation onto a record
   whose agent had just been retired. Implemented as **clear, then release**, with the release the
   last line, which is what §11 test 7 actually asserts.
2. **§8's `HerdrPaneReporter` trait is not needed and was not written.** It was specified so the
   bridge and the transport could be written against one seam; the transport landed first and
   `cyrup_herdr::HerdrClient` IS that seam. A trait over it would be a second surface with one
   implementor, and the tests drive a real `UnixListener` rather than a double, so the trait buys
   nothing. `HerdrBridgeError` likewise: nothing in the bridge returns a `Result` to a caller —
   reporting is best effort by contract (`client.py:17-19`, `herdr-status.ts:188-191`), so the
   error is logged at `debug` at the one place it can occur.
3. **§7's release guard was written and then DELETED** — see the section above. Not a descope:
   the behaviour it was meant to produce already happens, through a handler cyrup has had all
   along, and a second one would have broken it.
4. **§6's `HumanWaitObserver` could not be written** — `crates/cyrup-ext/src/native.rs` is not this
   writer's file. See `contractGaps`. `HerdrBridge::set_human_waiting` is the edge sink an observer
   would feed and is `pub` for that reason; it is fed today by `watch_human_wait`, a 100 ms sampler
   over `HumanWaitGate::is_waiting`. 100 ms is herdr's **own** delivery floor
   (`CONNECTION_POLL_INTERVAL`, `tmp/herdr/src/api/server.rs:28`), so the sampler costs the sidebar
   nothing it could have shown; the sample is one `AtomicUsize::load`, and the task exists only
   while the bridge is armed.
5. **§5's `human_waits: u32` is a level, not a second refcount** — already recorded on
   `StateModel::human_waiting` at 9ac1b84, and the sampler confirms it: the gate counts its own
   guards, so counting again here would be two ledgers for one fact.
6. **`agent.view.set` is NOT shipped**, and it is not a descope decision — `cyrup_herdr::Method`
   is `#[non_exhaustive]` and carries no `AgentViewSet`/`AgentViewClear` variant, and
   `schema/agents.rs` has no `AgentViewSetParams`. `crates/cyrup-herdr/` is not this writer's
   file. See `contractGaps`; it is the orchestrator's call, and it is §14's step 7 — the last and
   smallest piece.

### What still needs wiring by someone who owns the file

- `src/extension/executor/notices.rs:343-389` — the FOREGROUND attention raise. Without it
  `HerdrBridge::raise_attention`/`clear_attention` have only the background feeder
  (`sync_fleet`'s `activity_state` read, which IS live). The exact edit is in `needsWiring`.
- `crates/cyrup-ext/src/native.rs` — `HumanWaitObserver`, to replace the sampler.
- `crates/cyrup-it/tests/subagents/main.rs` — `mod herdr_status_bridge_integration;` plus the
  file-count in the module doc.
- `crates/cyrup-herdr/` — `agent.view.set` / `agent.view.clear`.
- The foreground-control registry's own start/finish edges, for
  `HerdrBridge::foreground_run_started`/`_finished`. The root turn (`AgentStart`/`AgentEnd`) and
  the background level are wired; the foreground refcount's two methods are driven by tests and by
  no production caller yet.

### Premises re-grepped this pass

- `grep -rnE 'signal::unix|SignalKind|tokio::signal' crates/ --include=*.rs` → **FOUR hits.** The
  AUG's "cyrup installs no termination-signal handler anywhere" is FALSE; `crates/cyrup/src/signals.rs`
  has taken SIGTERM/SIGHUP all along. The guard was deleted rather than shipped — see the section
  above. This bullet replaces the AUG's, which is the premise rule applied to the AUG itself.
- `grep -rn "HumanWaitObserver\|set_observer\|human_wait_changed" crates/` → **zero hits.** §6's
  observer does not exist.
- `git grep -n 'subagent:async-started' crates/` → still no hits. There is no launch-edge bus
  topic; the background level (`sync_fleet`) is what makes the feature correct without one, which
  is why §14 puts the topic at step 5.
- `crates/cyrup/src/tests/dispatch.rs:274` — the existing test that `session_shutdown{reason:"quit"}`
  is what a signal-driven teardown emits. That is the edge this batch's release hangs off.
- `cyrup-ext-subagents/Cargo.toml:94` already carries tokio `signal`; `crates/cyrup-herdr/Cargo.toml:37`
  already carries `net`. §13's "tokio `net` + `signal` on this crate's edge" was a true
  requirement at AUG time and is now satisfied by the graph — no manifest change.


---

## [VERIFY]

**The first agent on this feature permitted to build.** Everything below was compiled, run and, for
every test, proven by gutting the line it is about and watching it go RED.

### 1. Compile

`cargo check -p cyrup-ext-subagents -p cyrup --all-targets --features test-fixtures` was **already
clean** on arrival — the WIRE agent's pass had closed every break the writers flagged
(`INSPECTOR_SUBCOMMAND`, the `lib.rs`/`main.rs` arms, `HerdrProjectPaneSnapshot`'s path, the
`AgentStatus` clones in `consumer.rs`, the two unregistered `cyrup-it` files). **No compile error
was found or fixed.** The `#[cfg(test)]` modules bridge-live listed as type-unchecked type-check.

### 2. Tests, before any mutation

Five unit failures and five integration failures. Every one was diagnosed to its cause; none was
"fixed" by weakening an assertion.

| what failed | cause | fix |
|---|---|---|
| `inspectors::actions::…::the_launch_carries_the_base64_session_roots` | the test indexed `launch.args[1]` for the async dir; `launch_for` prepends `spawn_command.base_args` and then the subcommand token, so the dir is at the flag's index + 1 | the test now finds `--async-dir` and reads the next token — correct whatever the base args are |
| `inspectors::runner::…::the_control_refusals_are_upstreams` | the row asserted `steer   ` refuses with *"steer requires a message."*; upstream trims the line FIRST (`inspector-runner.ts:100`), so that refusal is **unreachable upstream** and the bare word falls through to `:116` and is sent to every running child as guidance | **`[CYRUP-EXCEEDS-UPSTREAM]`**: the bare `steer` now matches the steer arm, in upstream's own position and with upstream's own sentence. The dead branch becomes live; nothing else moves |
| `herdr::runtime::…::the_human_wait_watcher_follows_the_gate` | the helper polled `StateModel::last_reported`, which a failed send is REQUIRED to clear (`reporter.rs`'s `invalidate_last_report`); with no socket in these tests every send fails, so the helper raced the reporter | poll `StateModel::desired()` — the model's own answer, which only the watcher moves |
| `registration::authority::…::validation_refuses_unknown_actions_and_bad_decisions_with_pis_text` | the expected literal predated `inspectorOpen`/`projectOpen`. Upstream has had both since `policy/authority.ts:8-9` @v0.68.0 | the literal now matches upstream's `AUTHORITY_ACTIONS.join(", ")` byte for byte |
| `registration::guide::…::every_authority_gated_verb_that_the_table_lists_says_so` | the new `project.open` row said *"CONFIRMED by default"*, and the served reference now has **two** tables keyed on the verb, so a per-ROW predicate demanded the refusal table repeat the gate | the doc row matches `worktree.discard`'s lowercase convention, and the predicate is per-VERB: a gated verb must be described as gated somewhere |
| IT `fleet_inspector_integration::…renders_the_interactive_inspector_frame` | the footer literal still read `H Herdr`; the live footer is `Enter/H Inspect`, which is upstream's own `${bindingLabel(…,"inspect")} Inspect` (`fleet.ts:1368`) | the IT literal now matches upstream |
| IT `herdr_status_bridge_integration` ×4 | the `until(…)` predicates called `last_state(fake)`, whose `.expect("at least one pane.report_agent")` fires on the FIRST poll — before the bridge's first report has crossed the socket | a `last_state_word` accessor that answers `None` while none has arrived; `last_state` stays strict for direct assertions |

### 3. Two production defects the sweep and the reading found

**(a) `inspector.open` could never have worked inside a real herdr.** `inspector_ready_marker`
(`inspectors/herdr/actions.rs`) spelled the `pane wait-output --match` string by hand as
`subagents inspector for {runId}`. The runner's dashboard header is
`{INSPECTOR_HEADER_PREFIX}{runId}` = `cyrup-inspector for {runId}`. **The marker is not a substring
of the header**, so every `inspector.open` would have waited `INSPECTOR_READY_TIMEOUT_MS`, closed the
pane it had just opened and written no binding — the headline verb, failing silently, on every
invocation. No unit test could see it: they all script the `wait-output` answer. The marker now
DERIVES from `INSPECTOR_HEADER_PREFIX`, the false doc premise is replaced with the true one, and
`the_ready_marker_is_the_runners_own_dashboard_header` pins the two together (mutation `V1` — RED).
The other half — that the header really reaches the pane's stdout — is already asserted against the
real binary by `inspector_runner_subcommand_integration.rs:231`.

**(b) `INSPECTOR_PANE_RATIO` was inverted, and its doc premise was false.** The doc read *"the
fraction of the current pane the inspector takes"*. herdr's `split_at` puts the ORIGINAL pane in
`first` and the new one in `second` (`tmp/herdr/src/layout.rs:598-603` @`d59d060`), and the geometry
gives `first` the ratio (`:694`, `:702`). So `ratio` is the session's own share: `0.4` gave the
read-only mirror **60 %** of the user's terminal. Now `0.6`, with the orientation cited rather than
assumed.

A third, smaller one: `INSPECTOR_SESSION_ROOTS_ENV` was documented as *"the belt to that pair of
braces"*. Nothing reads it — `parse_args` takes the roots from the argv flag alone, and a shell that
split the payload would be refused by that parser's pairwise scan before any fallback could run. The
`--env` is kept (a human can see what a pane was launched with); the premise is corrected.

### 4. The mutation sweep — one batched scripted pass

One script, one table, one run: apply the gut to a pristine copy, run **only** that test
(`-p <crate> -E 'test(<name>)'`), record the verdict, restore the whole pristine set byte-for-byte,
verify the restore against a `sha256sum` manifest, move on. It carries a wall-clock budget so it
always stops BETWEEN mutations and can never be killed mid-gut.

**160 mutations over 154 distinct tests. 160 RED. 160 restored. Final `sha256sum -c` over the
29-file pristine manifest: clean.**

Eight mutations came back GREEN on the first pass. Every one was a real hole, and every one is
now RED:

| test | why it survived its own gut | what it now asserts |
|---|---|---|
| `the_bare_executable_set_is_upstreams_exactly` | five examples from the ACCEPTED side only — a WIDENED set (`%` added to the `matches!`) left all five green | both directions over the whole of ASCII, i.e. `/^[\w./@:-]+$/` as a predicate |
| `a_symlink_never_escapes_the_trusted_root` | the symlink pointed OUT of the root, so the realpath leg refused it and `:36`'s symlink check was redundant | a second symlink whose target is INSIDE the root — passes both `pathWithin` calls, and only the `:36` refusal stops it |
| `the_plugin_receives_the_launch_and_the_runs_own_cwd` | the fixture gave the run the SAME cwd as the dispatcher, so `deps.cwd` and `status.cwd` were indistinguishable | the run's `status.json` is rewritten with a distinct cwd, and the dispatcher's is asserted ABSENT |
| `status_and_close_route_through_the_owning_plugin` | both recorders answered the same bytes, so "the owner answered" could not be told from "somebody answered" | each recorder's reply names its author; the OWNER's tag is asserted |
| `project_open_with_no_herdr_carries_upstreams_install_sentence` | an unavailable seam answers EVERY call with `HERDR_UNAVAILABLE`, so deleting `detect_herdr` produced the same sentence off the first `pane get` | `--version` must be the FIRST argv — which is what makes the refusal instant and is upstream's order |
| `an_inspect_action_reaches_inspector_open_and_keeps_upstreams_fallback` | all three assertions were satisfied by the fallback string the gut returns | the dispatcher's OWN resolver sentence (*"… is outside trusted run roots."*) must come back — a string no fixed fallback in that file can produce |
| `an_invalidated_report_is_sent_again` | it went through `resend()`, which clears `last_reported` on its own way in, so the invalidate did nothing | through the DE-DUPLICATOR: an unchanged edge after a failed send is sent again, and once it lands the dedup is back in force |
| IT `a_rejected_report_never_disturbs_the_agent` | the fake keyed its refusals on the CONNECTION ordinal (which counted `events.subscribe` and `session.snapshot`), and the three edges each CHANGED state — so every `working` line it saw would have appeared with or without the retry | the fake keys on the METHOD; three edges that all settle on `working` reach the wire only because the first two were refused and invalidated |

Four of my own guts were malformed or aimed at the wrong line and were corrected before being
counted: `D7` (`let longest = 2` — ambiguous integer), `M3`/`Q5` (`PaneAgentState` has no `Done`
variant — the contract's enum holds only the four REPORTABLE states, so `Unknown` is the gut),
`O1` (`Subscription::pane_agent_status_changed` takes `AgentStatus`, not `PaneAgentState`), `L3`
(the test is about the steer draft; the gut had to move the inspect arm ABOVE it, not drop `Enter`
from it), `R4` (`cyrup --help` is `cli/help.rs`'s hand-written text, not clap's `about`).

### 5. Two tests VERIFY added

* `inspectors::herdr::actions::tests::the_ready_marker_is_the_runners_own_dashboard_header` — the
  cross-module pin defect (a) needed. Mutation `V1`: RED.
* IT `herdr_status_bridge_integration::a_session_shutdown_releases_the_bridge_in_the_global_slot` —
  **the teardown the product actually runs.** Every other test in that file drives a `HerdrBridge`
  it holds by hand; production parks it in a process-global slot at `SessionStart` and
  `HostEvent::SessionShutdown` calls `crate::herdr::shutdown()`, which TAKES that slot
  (`native_impl.rs:480,630`). `arm_in`/`bridge`/`shutdown` had **no test at all**. This arms through
  the real gate, drives an edge, calls `shutdown()`, and asserts one `pane.release_agent` on the
  wire, an emptied slot, and idempotence on a second call. Mutation `V2`: RED.

### 6. The gates

- `cargo fmt --all --check` — **clean**.
- `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` —
  **`Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 1m 43s`**, no diagnostics.
  Five findings were fixed in the code, not allowed away: `manual_ascii_case_comparison`
  (ghostty/plugin.rs), `needless_option_as_deref` and `useless_vec` (project_panes.rs),
  `manual_find`/`filter().next_back()` and `large_enum_variant` (fleet_status.rs — `FleetStatusEntry`
  is carried BY VALUE in `FleetTreeRow::Owner`, so the new `project_pane` snapshot is boxed). Two
  test modules (`herdr/consumer.rs`, `herdr/reporter.rs`) were missing
  `clippy::indexing_slicing` from the allow-list every other test module in the crate carries.
  `FakeHerdrClient::calls` in `herdr/client.rs` was dead; it now has a real consumer asserting that
  `detect_herdr` issues exactly one `--version`.
- `cargo nextest run --workspace --features test-fixtures` —
  **`Summary [ 105.081s] 11014 tests run: 11014 passed, 9 skipped`**.
- `cargo nextest run -p cyrup-it --features it` (`CYRUP_IT_BIN_DIR` set, AWS vars unset) —
  **`Summary [ 323.125s] 588 tests run: 588 passed, 0 skipped`** (587 before; the delta is §5's
  second test).

### 7. Does the feature work?

| journey | proven by |
|---|---|
| **the sidebar says which pane is blocked on you** | IT `a_human_wait_puts_the_pane_at_blocked` — a real `UnixListener` speaking herdr's framing, the production `HerdrBridge::start`, and the gate raised through `HostCtx::begin_human_wait`, which is the ONLY door into it (the permission dialog and MCP's dialog owner take the same one). Asserts `blocked` under cyrup's own source and agent, and that dropping the guard returns to `working`, not `idle`, while the turn runs |
| **H opens an inspector** | three hops, each pinned, no single end-to-end test: `every_key_upstream_binds_crosses_the_seam` (host key → `FleetKey`), `h_and_enter_both_route_to_one_inspect_action` (key → `FleetPendingAction::Inspect`), and `an_inspect_action_reaches_inspector_open_and_keeps_upstreams_fallback` (pending action → `SubagentExecutor::inspector_open`, now asserting the dispatcher's own sentence). The hosted overlay itself is driven by `fleet_inspector_integration` |
| **every verb advertised AND dispatched** | all seven are in the advertised action list (`extension/tool/schema.rs:1006-1012`, in upstream's position) and all seven have a live dispatch arm behind one shared authority consult (`extension/tool/routing.rs:1538,1595`). `inspector.command`, `inspector.status`, `inspector.close`, `project.status` and `project.close` answer on a box with **no herdr and no ghostty**, and their tests assert `is_ok()` explicitly rather than merely the text |
| **SIGTERM releases the row** | the release itself: IT `a_clean_shutdown_releases_the_pane_last`. The production path into it: §5's new IT, over the process-global slot the `SessionShutdown` arm uses. The signal→`session_shutdown{quit}` leg is cyrup's own handler (`crates/cyrup/src/signals.rs:317-330`), already covered by `crates/cyrup/src/tests/dispatch.rs:274`. **What is still not executed end to end is a real signal delivered to a real `cyrup` binary**; that is a `crates/cyrup`-level test and is named in the residuals below |

### 8. Residuals

1. **No single test drives a real SIGTERM through a real `cyrup` process to a `pane.release_agent`.**
   Every link is covered (signal → `session_shutdown{quit}`; that event → `herdr::shutdown()`;
   `shutdown()` → the wire) but the chain is not walked in one process. It needs a `crates/cyrup`
   integration test that spawns the binary against a fake herdr socket and sends it SIGTERM.
2. **`inspector.open` has never been run against a real herdr**, because herdr is not installed in
   this container. Defect (a) is exactly the class of bug that leaves — the argv contract is pinned
   by `open_performs_split_run_verify_report_write` and the CLI-to-typed-call translation by
   `inspectors/herdr/client.rs`'s dispatch table, but neither has met the real server.
3. **`path_within` is a fourth private copy** (`inspectors/actions.rs`, plus `background/fleet_view.rs`,
   `tui/fleet_transcript.rs`, `background/scheduled_runs/store.rs`). verbs-core filed it as a
   contract gap; it is still open, and it is a one-line promotion plus three deletions.
4. **`HerdrClient::run` still carries a bare code, not herdr's own message** (verbs-herdr's MAJOR gap).
   `client::message_for` reconstructs a sentence from the code plus the argv, so every message the
   tests assert is right — but herdr's SPECIFIC explanation for a `pane_split_failed` never reaches
   the user, and the socket already delivered it.

### The mutation table

| # | test | gutted | verdict | restored |
|---|---|---|---|---|
| `A1` | `encoding_is_standard_base64_of_the_json_array` | drop the base64 hop | **RED** | restored |
| `A2` | `an_empty_root_list_encodes_to_the_empty_json_array` | empty slice encodes to nothing | **RED** | restored |
| `A3` | `roots_round_trip_through_the_encoding` | drop the base64 decode | **RED** | restored |
| `A4` | `the_encoded_payload_has_no_quote_backslash_or_space` | drop the base64 hop | **RED** | restored |
| `A5` | `every_malformed_payload_reports_one_stable_sentence` | not-an-array becomes Ok(empty) | **RED** | restored |
| `B1` | `a_bare_unix_executable_is_invoked_directly_with_quoted_args` | is_bare_executable always false | **RED** | restored |
| `B2` | `a_unix_executable_needing_quotes_is_invoked_through_sh_c` | is_bare_executable always true | **RED** | restored |
| `B3` | `a_unix_argument_with_a_single_quote_is_closed_escaped_and_reopened` | backslash escape instead of close/escape/reopen | **RED** | restored |
| `B4` | `the_win32_branch_uses_the_call_operator_and_double_quotes` | drop the PowerShell call operator | **RED** | restored |
| `B5` | `a_win32_argument_with_a_double_quote_is_backslash_escaped` | drop the double-quote escape | **RED** | restored |
| `B6` | `the_bare_executable_set_is_upstreams_exactly` | widen the bare set by one char | **RED** | restored |
| `B7` | `an_empty_argv_renders_the_executable_alone` | is_bare_executable always false | **RED** | restored |
| `B8` | `the_host_platform_matches_the_build_target` | flip the cfg arms | **RED** | restored |
| `C1` | `resolve_target_refuses_with_each_of_upstreams_sentences` | fuse one of the seven refusals into a generic sentence | **RED** | restored |
| `C10` | `the_child_index_flag_is_emitted_only_when_the_target_names_a_child` | emit --index unconditionally | **RED** | restored |
| `C11` | `a_mission_bound_run_carries_its_record_path_into_the_launch` | mission_for's binding rung returns None | **RED** | restored |
| `C12` | `inspector_open_refuses_with_no_backend_and_writes_nothing` | one character of the refusal const | **RED** | restored |
| `C13` | `inspector_open_takes_the_first_available_plugin_without_consulting_owns` | gate open on owns() as well as available() | **RED** | restored |
| `C14` | `the_plugin_receives_the_launch_and_the_runs_own_cwd` | build_context always uses deps.cwd | **RED** | restored |
| `C15` | `status_and_close_with_no_owner_are_answers_not_errors` | the no-owner answer becomes an error | **RED** | restored |
| `C16` | `status_and_close_route_through_the_owning_plugin` | status/close take the first plugin instead of the owning one | **RED** | restored |
| `C17` | `an_owner_without_the_method_reports_upstreams_unsupported_sentence` | the unsupported-status arm stops being an error | **RED** | restored |
| `C18` | `an_unresolvable_target_is_an_error_for_every_verb` | inspector.command stops erroring on an unresolvable target | **RED** | restored |
| `C2` | `a_run_whose_status_vanishes_reports_the_lifecycle_sentence` | collapse the second status read onto the first sentence | **RED** | restored |
| `C3` | `a_valid_child_index_resolves_onto_the_target` | drop child_index from the resolved target | **RED** | restored |
| `C4` | `trusted_dir_honours_both_rungs` | delete the live-job rung | **RED** | restored |
| `C5` | `a_symlink_never_escapes_the_trusted_root` | delete the symlink refusal | **RED** | restored |
| `C6` | `inspector_command_answers_with_no_backend_installed` | move the Command early return below the plugin loop | **RED** | restored |
| `C7` | `the_launch_carries_the_base64_session_roots` | raw JSON instead of the base64 payload | **RED** | restored |
| `C8` | `the_runs_own_session_root_joins_the_launch` | drop the live-job leg of the roots chain | **RED** | restored |
| `C9` | `the_launch_flags_follow_the_authority_policy` | hard-code both authority flags to true | **RED** | restored |
| `D1` | `the_runner_argv_parser_matches_parse_args_exactly` | accept a bare value as a key | **RED** | restored |
| `D10` | `a_refused_control_line_becomes_a_notice_not_an_exit` | a refused control line ends the process | **RED** | restored |
| `D11` | `a_missing_status_renders_upstreams_unavailable_screen` | the degraded screen is never written | **RED** | restored |
| `D12` | `the_refresh_predicate_is_upstreams_not_run_state_is_terminal` | route the refresh predicate through RunState::is_terminal | **RED** | restored |
| `D13` | `the_subcommand_token_is_the_reserved_one` | one character of the reserved token | **RED** | restored |
| `D2` | `the_dashboard_header_and_controls_line_are_upstreams` | one fixed controls line for every case | **RED** | restored |
| `D2b` | `the_dashboard_header_and_controls_line_are_upstreams` | drop INSPECTOR_HEADER_PREFIX from the header | **RED** | restored |
| `D3` | `steer_and_stop_from_stdin_land_real_requests_on_the_control_channel` | drop the real steer request, keep the receipt | **RED** | restored |
| `D4` | `an_aggregate_steer_targets_every_running_child` | aggregate steer narrows to child 0 | **RED** | restored |
| `D5` | `an_aggregate_steer_with_no_running_child_is_refused` | delete the empty-fan-out guard | **RED** | restored |
| `D6` | `the_control_refusals_are_upstreams` | route the inspector predicate through RunState::is_terminal | **RED** | restored |
| `D7` | `a_steering_receipt_fences_wider_than_the_message_it_quotes` | fixed fence width | **RED** | restored |
| `D8` | `a_steering_preview_is_redacted_before_it_is_truncated` | truncate without redacting | **RED** | restored |
| `D9` | `run_inspector_renders_then_lands_a_steer_from_its_injected_stdin` | the Ok reply never becomes the next screen's notice | **RED** | restored |
| `E1` | `the_two_independent_literals_agree` | one character of the binary's own literal | **RED** | restored |
| `E2` | `only_an_exact_argv_1_selects_the_inspector` | membership-anywhere instead of an exact argv[1] | **RED** | restored |
| `E3` | `the_inspector_subcommand_is_not_a_user_facing_subcommand` | advertise the token in SUBCOMMANDS | **RED** | restored |
| `F1` | `the_version_parser_finds_the_first_triple_anywhere_in_the_text` | whole-string parse instead of first-match-anywhere | **RED** | restored |
| `F2` | `the_version_floor_is_pis_three_term_disjunction` | collapse the three-term disjunction | **RED** | restored |
| `F3` | `detect_herdr_carries_upstreams_two_refusal_sentences` | delete the unsupported-version refusal | **RED** | restored |
| `F4` | `an_unavailable_herdr_carries_upstreams_install_sentence` | the install sentence becomes the generic one | **RED** | restored |
| `F5` | `the_deadline_table_is_pis_per_call_one` | one deadline row | **RED** | restored |
| `F6` | `the_argv_helpers_read_repeated_flags` | flag_values returns only the first match | **RED** | restored |
| `F7` | `herdr_error_codes_normalise_the_way_pi_normalises_them` | every API code collapses to VALIDATION_ERROR | **RED** | restored |
| `F8` | `no_client_is_constructed_outside_a_herdr_pane` | discover always yields a client | **RED** | restored |
| `G1` | `focus_uses_one_pane_focus_call` | pi's two-hop pane get -> tab focus | **RED** | restored |
| `G2` | `an_answer_with_no_pane_id_is_an_invalid_pane_response` | delete the invalid-pane-response guard | **RED** | restored |
| `G3` | `a_pre_0_9_herdr_with_no_tab_or_workspace_is_pane_focus_unsupported` | the pre-0.9 classification never fires | **RED** | restored |
| `G4` | `a_missing_pane_is_not_relabelled_as_a_focus_support_problem` | delete the NotFound/Unavailable short-circuit | **RED** | restored |
| `G5` | `the_pane_record_probe_accepts_both_of_upstreams_shapes` | drop the nested {pane:{..}} rung | **RED** | restored |
| `H1` | `the_binding_path_is_upstreams` | every child shares one binding file | **RED** | restored |
| `H10` | `a_binding_belongs_to_exactly_one_target` | drop the run-id staleness check | **RED** | restored |
| `H11` | `an_unknown_key_written_by_pi_survives_a_round_trip` | deny_unknown_fields on the binding | **RED** | restored |
| `H12` | `the_session_roots_payload_is_lifted_off_the_launch_argv` | the --session-roots payload is never lifted | **RED** | restored |
| `H13` | `close_tolerates_a_pane_that_is_already_gone` | close leaves the binding on disk | **RED** | restored |
| `H2` | `open_performs_split_run_verify_report_write` | one flag spelling in the split vector | **RED** | restored |
| `H3` | `a_pane_run_failure_closes_the_pane_and_writes_no_binding` | a failed pane run leaves the pane open | **RED** | restored |
| `H4` | `a_pane_that_never_starts_the_inspector_is_closed_and_leaves_no_binding` | delete the wait-output verification | **RED** | restored |
| `H5` | `an_already_open_pane_is_focused_not_reopened` | an open pane is re-split | **RED** | restored |
| `H5b` | `an_already_open_pane_is_focused_not_reopened` | focus: true no longer calls pane focus | **RED** | restored |
| `H6` | `a_split_with_no_pane_id_is_upstreams_pane_gone_literal` | delete the PANE_GONE guard | **RED** | restored |
| `H7` | `status_and_close_round_trip_the_binding` | drop the Run state line | **RED** | restored |
| `H8` | `close_tolerates_a_pane_that_is_already_gone` | drop the NotFound/PaneGone tolerance | **RED** | restored |
| `H9` | `status_and_close_with_no_binding_are_not_errors` | the no-binding status answer becomes an error | **RED** | restored |
| `I1` | `available_is_pis_two_environment_variables_and_nothing_else` | available() always true | **RED** | restored |
| `I2` | `available_never_touches_the_seam` | available() probes the seam | **RED** | restored |
| `I3` | `owns_is_the_binding_file_and_never_the_host` | owns() always true | **RED** | restored |
| `I4` | `status_and_close_are_supplied_not_absent` | status() is absent | **RED** | restored |
| `I5` | `open_with_no_seam_carries_upstreams_install_sentence` | the no-seam refusal loses upstream's sentence | **RED** | restored |
| `J1` | `project_status_and_close_answer_with_no_herdr_at_all` | close consults detect_herdr first | **RED** | restored |
| `J10` | `a_pane_herdr_no_longer_has_is_stale_in_both_modes` | status stops reporting a gone pane as stale | **RED** | restored |
| `J11` | `project_open_focus_on_an_open_pane_focuses_it` | project.open --focus on an open pane never focuses | **RED** | restored |
| `J12` | `the_session_start_restore_reads_every_bound_root` | restore inserts nothing | **RED** | restored |
| `J13` | `the_root_index_is_a_sorted_set_that_deletes_itself_when_empty` | the root index stops de-duplicating | **RED** | restored |
| `J14` | `a_project_root_must_be_a_directory_that_exists` | a file is accepted as a project root | **RED** | restored |
| `J15` | `an_unknown_key_survives_the_project_pane_binding_round_trip` | deny_unknown_fields on the project-pane binding | **RED** | restored |
| `J16` | `the_strict_reader_refuses_a_blank_required_field` | the strict reader accepts a blank field | **RED** | restored |
| `J2` | `project_open_with_no_herdr_carries_upstreams_install_sentence` | open no longer detects herdr | **RED** | restored |
| `J3` | `project_open_status_close_round_trip` | close no longer requires an idle pane | **RED** | restored |
| `J3b` | `project_open_status_close_round_trip` | open no longer writes the root index | **RED** | restored |
| `J4` | `ownership_mismatch_refuses_close_and_leaves_the_binding` | close drops the ownership gate | **RED** | restored |
| `J5` | `project_close_with_no_binding_still_prunes_the_root_index` | the absent path stops pruning the root index | **RED** | restored |
| `J6` | `a_project_pane_summary_comes_from_state_labels_first` | delete the three cyrup summary rungs | **RED** | restored |
| `J7` | `ownership_verifies_on_either_cwd_and_mismatches_only_when_both_disagree` | drop the foreground_cwd ownership candidate | **RED** | restored |
| `J8` | `the_two_compatibility_modes_disagree_exactly_where_upstream_does` | legacy mode stops tolerating a malformed binding | **RED** | restored |
| `J9` | `an_invalid_pane_response_is_tolerated_only_in_legacy_mode` | status drops the legacy invalid-pane tolerance | **RED** | restored |
| `K1` | `ghostty_opens_through_an_injected_runner_on_any_platform` | drop the -- separator from the osascript argv | **RED** | restored |
| `K10` | `ghostty_owns_nothing_and_supplies_neither_optional_method` | ghostty claims every binding | **RED** | restored |
| `K11` | `open_runs_the_injected_seam` | seam() ignores the injected runner | **RED** | restored |
| `K2` | `focus_is_a_strict_true_comparison` | focus becomes a loose comparison | **RED** | restored |
| `K3` | `an_empty_terminal_id_and_a_runner_error_both_carry_the_hint` | the empty-id failure loses the hint | **RED** | restored |
| `K4` | `a_non_zero_exit_is_a_failure_carrying_osascripts_own_stderr` | a non-zero exit is reported as success | **RED** | restored |
| `K5` | `the_applescript_is_upstreams_byte_for_byte` | one line of the AppleScript | **RED** | restored |
| `K6` | `captured_output_is_capped_at_upstreams_buffer` | the buffer cap never applies | **RED** | restored |
| `K7` | `the_launch_directory_is_the_trusted_dir` | the launch cwd becomes the async dir | **RED** | restored |
| `K8` | `available_is_darwin_plus_a_case_folded_term_program` | drop the darwin conjunct | **RED** | restored |
| `K9` | `available_never_touches_the_runner` | available() probes the runner | **RED** | restored |
| `L1` | `the_unavailable_message_is_upstreams_v0_68_0_wording` | restore the pre-v0.68.0 wording | **RED** | restored |
| `L10` | `the_collapsed_line_counts_panes_separately_and_flags_attention` | the collapsed line counts panes as agents | **RED** | restored |
| `L11` | `project_panes_render_in_their_own_section_and_not_in_the_agent_tree` | project panes join the agent tree | **RED** | restored |
| `L12` | `needing_attention_is_a_substring_test_over_status_and_summary` | attention becomes an equality test | **RED** | restored |
| `L13` | `a_project_name_is_the_last_segment_of_either_separator` | project_name uses Path::file_name | **RED** | restored |
| `L14` | `the_iso8601_parser_inverts_the_writers_format` | one field offset in the ISO-8601 parser | **RED** | restored |
| `L15` | `a_pane_row_joins_the_roster_so_enter_can_select_it` | pane rows never join the roster | **RED** | restored |
| `L2` | `h_and_enter_both_route_to_one_inspect_action` | drop Enter from the inspect binding | **RED** | restored |
| `L3` | `enter_still_sends_a_steer_draft_before_it_inspects` | the inspect arm runs BEFORE the steer draft | **RED** | restored |
| `L4` | `the_inspect_target_resolver_is_upstreams_not_the_async_one` | inspect uses the async resolver | **RED** | restored |
| `L5` | `a_settled_async_child_is_refused_by_the_inspect_resolver` | the inspect resolver drops the actionable-state test | **RED** | restored |
| `L6` | `inspect_with_nothing_selected_says_so` | the nothing-selected sentence changes | **RED** | restored |
| `L7` | `an_inspect_action_reaches_inspector_open_and_keeps_upstreams_fallback` | inspect never reaches the executor | **RED** | restored |
| `L8` | `a_restored_project_pane_becomes_a_roster_row_and_a_stale_one_does_not` | stale panes join the roster | **RED** | restored |
| `L9` | `an_empty_status_and_an_unparseable_stamp_take_upstreams_fallbacks` | drop the refreshed_at fallback | **RED** | restored |
| `M1` | `a_finishing_child_does_not_flip_the_pane_idle` | a finishing child zeroes the refcount | **RED** | restored |
| `M2` | `blocked_outranks_working_outranks_idle` | working outranks blocked | **RED** | restored |
| `M3` | `unknown_and_done_are_never_produced` | idle is reported as done | **RED** | restored |
| `M4` | `background_runs_count_as_a_level` | background runs stop counting | **RED** | restored |
| `M5` | `an_invalidated_report_is_sent_again` | a failed report is believed to have landed | **RED** | restored |
| `N1` | `state_and_presentation_are_built_by_different_verbs` | the metadata verb stops carrying its own state_labels | **RED** | restored |
| `N2` | `a_publish_sets_three_state_labels_and_two_tokens` | blocked joins the state labels | **RED** | restored |
| `N3` | `a_publish_without_a_suffix_clears_the_previous_one` | an absent suffix is omitted rather than nulled | **RED** | restored |
| `N4` | `the_clear_path_clears_labels_and_nulls_both_tokens` | the Clear arm also populates state_labels | **RED** | restored |
| `N5` | `the_sequence_is_strictly_increasing_and_clock_seeded` | the sequence counter is not clock-seeded | **RED** | restored |
| `N6` | `the_token_keys_and_the_source_are_inside_herdrs_charsets` | a token key outside herdr's charset | **RED** | restored |
| `O1` | `the_subscription_set_is_the_three_this_bridge_can_act_on` | narrow the status subscription | **RED** | restored |
| `P1` | `the_human_wait_watcher_follows_the_gate` | delete the human-wait watcher spawn | **RED** | restored |
| `P10` | `the_omit_pane_shape_is_inert` | the non-empty pane-id half of the gate | **RED** | restored |
| `P2` | `raw_prompts_never_reach_the_published_label` | the raw description becomes the launch label | **RED** | restored |
| `P3` | `a_resync_counts_each_active_run_once` | delete the de-duplication | **RED** | restored |
| `P4` | `a_background_run_needing_attention_blocks_the_pane` | the activity_state read is hard-coded false | **RED** | restored |
| `P5` | `the_project_pane_clause_appears_only_when_there_are_panes` | the pane count never reaches the summary | **RED** | restored |
| `P6` | `a_released_bridge_produces_nothing_further` | delete the released guard on edge | **RED** | restored |
| `P7` | `no_herdr_env_means_no_bridge` | the HERDR_ENV half of the gate | **RED** | restored |
| `P8` | `a_headless_session_in_a_pane_is_detected_but_never_armed` | delete the headless gate | **RED** | restored |
| `P9` | `an_armed_bridge_survives_a_socket_that_is_not_there` | release stops releasing | **RED** | restored |
| `Q1` | `a_human_wait_puts_the_pane_at_blocked` | move the is_running branch above human_waiting | **RED** | restored |
| `Q2` | `no_herdr_env_means_no_socket` | gate on the socket path alone | **RED** | restored |
| `Q3` | `a_headless_session_in_a_pane_never_arms` | delete the headless gate | **RED** | restored |
| `Q4` | `a_finishing_child_does_not_flip_the_pane_idle` | a finishing child zeroes the refcount | **RED** | restored |
| `Q5` | `the_wire_never_carries_a_state_herdr_cannot_accept` | idle is reported as done | **RED** | restored |
| `Q6` | `seq_is_monotonic_and_the_source_is_stable` | seq is no longer taken at wire time | **RED** | restored |
| `Q7` | `a_clean_shutdown_releases_the_pane_last` | delete the release at the end of drain | **RED** | restored |
| `Q8` | `a_rejected_report_never_disturbs_the_agent` | a failed report is believed to have landed | **RED** | restored |
| `Q9` | `subscribe_and_request_use_different_connections` | snapshot before subscribe | **RED** | restored |
| `R1` | `cyrup_subagent_inspector_renders_a_real_run_and_a_steer_line_lands` | delete the classification arm | **RED** | restored |
| `R2` | `cyrup_subagent_inspector_renders_a_real_run_and_a_steer_line_lands` | delete the main.rs dispatch arm | **RED** | restored |
| `R3` | `a_bad_inspector_argv_is_refused_by_the_runners_own_parser` | delete the classification arm | **RED** | restored |
| `R4` | `the_inspector_subcommand_is_undiscoverable` | advertise the token in the help text the binary prints | **RED** | restored |
| `V1` | `the_ready_marker_is_the_runners_own_dashboard_header` | the marker is spelled by hand again | **RED** | restored |
| `V2` | `a_session_shutdown_releases_the_bridge_in_the_global_slot` | shutdown() no longer takes the global slot | **RED** | restored |

---

## [CONSOLIDATE C4-C8]

Resumed at C4 after the container restart that interrupted the previous consolidation pass
(`f623737`). C1-C3 were re-verified in the tree before anything else was touched, because the
fixer never got to report.

### C1-C3 — verified, not assumed

| row | what was checked | evidence |
|---|---|---|
| C1 | `watch_human_wait` polls the SHARED session lock, not this extension's own gate | `herdr/runtime.rs:483` takes `Arc<HumanInteractionLock>` and calls `lock.is_held()`; production hands it `self.executor.host_services().and_then(|s| s.human_interaction_lock())` (`extension/host/native_impl.rs:490-494`); the permission dialog acquires the SAME instance off the same backend (`cyrup-permission-system/src/extension/prompt.rs:171-178`); `HumanInteractionLock::is_held` is a non-blocking permit read (`cyrup-ext/src/host/services.rs:249`). The IT drives both ends through `HostServices::human_interaction_lock()` (`cyrup-it/tests/subagents/herdr_status_bridge_integration.rs:229-247`). |
| C2 | the privacy rule is asserted on the PRODUCTION producer | `raw_prompts_never_reach_the_published_label` (`herdr/runtime.rs:758`) calls `sync_fleet` and reads the lane back with `Reporter::queued_metadata` (`:730-731`). `metadata_for` is gone; `Reporter::metadata`'s only production caller is `sync_fleet` (`runtime.rs:430-433`). |
| C3 | the wire-string→enum seam is crossed for all seven verbs | `extension/tool/inspector_actions_dispatch_tests.rs` — advertised in both lists, `as_str`↔`from_wire` bijection, and all seven driven through the real `cyrup_core::Tool::execute` with the unknown-action fallback asserted against. In-crate, so it gates `cargo test --workspace`. |

### C4 — the project-pane map (QA major, `routing.rs:1584`)

The THREAD was already in the tree at `f623737` (the restart landed between the edit and the
report): `routing.rs:1597` passes `Some(self.executor.herdr_project_pane_map())`,
`ProjectPaneDeps::panes` is a `&Mutex<..>` so the handler can write through it across its
`await`s, and `remember`/the `Close` arm write into it. What was missing is the PIN, and it is
here now — two halves, because the two verbs reach the map by different routes:

* **the remove half, through the REAL tool** — `project_close_through_the_tool_forgets_the_pane`
  (`extension/tool/inspector_actions_dispatch_tests.rs`). `SessionStart`'s own
  `restore_herdr_project_panes` fills the map, the binding is removed (another process closed the
  pane), and `subagent({action:"project.close"})` is dispatched through `cyrup_core::Tool::execute`.
  Both readers must then see nothing: `herdr_project_pane_snapshots()` (the roster) and
  `open_herdr_project_pane_count()` (the pane label's `" · N panes"`). **This is the one
  project-pane path that reaches `panes` with no herdr call at all**, which is what lets it pin
  `routing.rs`'s own arm on a box with no herdr — `panes: None` there is RED.
* **the insert half** — `project_status_remembers_into_the_executors_own_map`, driving
  `handle_herdr_project_pane_action` with the executor's OWN map (the exact expression `routing.rs`
  passes) and a `LivePaneHerdr` fake, because `project.status` on a root that HAS a binding does
  talk to herdr.

There is one `ProjectPaneDeps` construction site in `routing.rs` serving all three verbs, so
"either verb goes back to `None`" is one edit and the close test is RED for it.

### C5 — the shutdown budget (QA major, `native_impl.rs:628`)

`crate::herdr::shutdown()` is bounded by `reporter::RELEASE_TIMEOUT` = **5 s**; the handler runs
under `cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET` = **5 s**; and the dispatcher enforces that
budget by DROPPING the handler future (`Dispatcher::invoke_contained`'s `tokio::time::timeout`).
A herdr that accepted the connection and then stopped answering therefore spent the whole budget
on the arm's FIRST statement and everything below it — the supervisor channel, the watchdog, the
wait subscriptions, the scheduled-run manager, `teardown_session`, both widget slots — was
silently dropped. Nothing logged and nothing failed.

**The fix**: the arm SPAWNS the release, runs the in-process teardown, and joins at the end under
`HERDR_RELEASE_JOIN_BUDGET` (2 s). The release still goes out first and is still awaited; the
waiting now happens after the work that cannot hang. A join that times out does not abort the
task — it keeps draining until `RELEASE_TIMEOUT` ends it.

**The coupling is a compile error.** `cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET` is now `pub`
(it has to be: a handler cannot budget against a number it cannot see) and `native_impl.rs`
carries `const _: () = assert!(HERDR_RELEASE_JOIN_BUDGET.as_millis() * 2 < DEFAULT_INVOKE_BUDGET.as_millis())`.
Shrinking the dispatch budget breaks the build rather than quietly re-truncating the teardown.

**The pin**: `tests/herdr_shutdown_budget_integration.rs` —
`a_hung_herdr_cannot_starve_the_rest_of_the_session_shutdown`. A real `UnixListener` that accepts
and answers nothing; `crate::herdr::arm_in` parking a real bridge in the process-global slot; a
REAL `cyrup_ext::Dispatcher` at its real budget holding the extension through a real
`NativeHandle`. The observable is the registry `teardown_session()` clears. With the old shape the
dispatch is truncated and that registry still holds its agent.

### C6 — false premises, each one re-grepped

| where | the premise that was there | what the grep actually says | action |
|---|---|---|---|
| `inspectors/herdr/client.rs:30` | "pi drives exactly four herdr invocations plus `herdr --version`" | **SIX** CLI verbs: `pane get` (`herdr/actions.ts:85`, `focus.ts:33`, `project-panes.ts:402`), `pane split`, `pane run`, `pane close`, **`tab focus`** (`focus.ts:40`) and **`workspace focus`** (`focus.ts:44`) — two of them at lines the old claim's own citation list named | rewritten as a verb-by-verb table: four rows are pi's, `pane focus` REPLACES pi's two focus verbs with one call, and four rows are cyrup's |
| same table | "the six beyond pi's four each have a reason recorded on the arm" | false for `pane read`: the arm had no reason, no table row, and — `grep -rn '"pane", "read"' crates/` — **no production caller anywhere** | the `pane read` arm and `pane_read_json` are **DELETED**, with a comment at the match saying where to add one back with its first caller |
| `inspectors/herdr/client.rs:613` | "`--token summary=` … because nothing was ever writing it" | pi's OWN status bridge writes `--token summary=${text}` on every publish (`integrations/herdr-status.ts:221-229`) | rewritten to agree with the careful version in the sibling file (`herdr/project_panes.rs:48-53`): only `tokens.summary` can hit, and only if something sent `pane.report_metadata --token summary=…`. The delta is WHERE cyrup sends it from — `inspectors/`, which pi's `project-panes.ts` only ever READS |
| `inspectors/herdr/client.rs` version floor | "`pane.split`/`send_input`/`get`/`close` all predate 0.7.5 and are unchanged at 0.9.1"; "nothing in herdr calls a pane 'raw'" | `tmp/herdr` is ONE commit with 50 commits of history and no release tags — "predates 0.7.5" has no grep behind it. And `git grep -in "raw pane" d59d060` is NOT empty: three test strings use "raw pane id" for herdr's internal pane id | replaced with what IS greppable: `raw_panes`/`rawPanes` is empty (no such capability), the three "raw pane" hits are a different sense, all four methods exist at the pin, and **when they appeared is explicitly not asserted** |
| `inspectors/herdr/focus.rs:13` | "one of the **17** that are live but absent from the published method table" | the table at `socket-api.mdx:96-113` lists **89** of the 105 in `schema.rs:47-271`; the difference is **16** | corrected to 16, and the other fifteen are named so the number is checkable |
| `cyrup-herdr/src/lib.rs:70-84` | "`pane.graphics.*` (6)", "`plugin.*` (12)", and a not-ported table whose families did not add up | `pane.graphics.*` is **4**, `plugin.*` is **11**, and `pane.rename` / `pane.send_text` / `pane.send_keys` were in **no row at all** | the table now carries a count column, states 24 ported + 81 absent = 105, and the three orphans are placed |
| `crate::herdr::AGENT` doc | "`HookAuthority` holds `reported_at` and nothing else" | it holds six fields (`source`, `agent_label`, `state`, `message`, `reported_at`, `session_ref`) | restated as "a stamp of WHEN, and no expiry to compare it against" — which is the true form of the no-TTL claim |
| citations | `state.rs:23`, `panes.rs:124-132`, `cli/runtime.rs:109-111`, `panes.rs:132`, `socket-api.mdx:715-716` | each pointed one to ten lines off the thing being cited | retargeted: `state.rs:18-25` (the whole struct), `panes.rs:126-134` (through the `encode_success`), `cli/runtime.rs:9-14` (`print_method_response` itself), `socket-api.mdx:717-718` |

Verified and left alone (each re-opened): `is_reserved_native_state_source`'s **nine** pairs
(`agent_resume.rs:100-112`); `full_lifecycle_hook_authority` registering `("herdr:pi","pi")`
(`detect/mod.rs:327-337`); `pane.focus` crossing tabs AND workspaces — the citation now names
`focus_pane_in_workspace`'s `switch_workspace_tab` (`app/actions.rs:295-322`) and herdr's own
`api_pane_focus_focuses_direct_target_across_tabs_and_workspaces` (`panes.rs:4152`); "105 renamed
methods" (counted); `socket-api.mdx:114/118-130/750-752/780/782/794/798/959` (each line read).

**Reported, not edited:** `.flux/mandate/STANDING_BAR.md`'s worked example says *"pi drives 4
herdr CLI verbs; herdr v0.9.1 exposes 99 socket methods"*. Both numbers are wrong by the greps
above — **six** verbs and **105** methods. The mandate is the orchestrator's document and this
batch does not edit it; it is flagged here so the next reader does not inherit it.

### C7 — the five orchestrator-owned contract gaps, closed

**1. `InspectorContext` has no `env`.** It has one now (`inspectors/types.rs`), filled by
`build_context` from `InspectorDispatcherDeps::env`, which production fills with
`inspectors::actions::process_env()` — pi's `env: deps.env ?? process.env` (`actions.ts:126`).
Both backends read it and NOTHING else: herdr's gate is `HerdrPane::discover(&ctx.env)`, ghostty's
is `ctx.env["TERM_PROGRAM"]` folded case-insensitively. `HerdrInspectorPlugin::with_env` and
`GhosttyInspectorPlugin::with_env` are **gone**; `with_client`, `with_runner` and `with_macos`
stay, because those are upstream's own `deps` (`herdr/plugin.ts:5-7`, `ghostty/plugin.ts:4-7`).

Two behaviours changed with it, both toward upstream:

* `HerdrInspectorPlugin::available` no longer reads `self.client.is_some() || …`. That disjunct
  made any host supplying a seam "available" outside a herdr pane, which `herdr/plugin.ts:13-14`
  does not do. Pinned by `an_injected_client_does_not_make_a_non_pane_available`.
* the not-installed path is now drivable THROUGH the dispatcher:
  `the_builtin_backends_gate_on_the_contexts_env_through_the_dispatcher` runs
  `handle_inspector_action(Open, ..)` with the real `builtin_inspector_plugins()` under three
  environments — empty (`NO_INSPECTOR_PLUGIN_AVAILABLE`, nothing written), herdr's two variables
  (the herdr arm runs and answers herdr's own sentence), and `TERM_PROGRAM=Ghostty` on a non-darwin
  host (still unavailable, which is the row that catches a dropped platform conjunct).

**2. `INSPECTOR_SUBCOMMAND` / `INSPECTOR_HEADER_PREFIX` in an implementation module.** Both are in
`inspectors/types.rs` now. The header half got more than a move: `types::inspector_header_line`
is ONE function, called by `runner::format_inspector_dashboard` to PRINT the line and by
`herdr::actions::inspector_ready_marker` to MATCH it with `pane wait-output --match`. A drift
between those two halves times out every `inspector.open`, closes the pane it just opened, writes
no binding, and **no unit test in this subtree notices** because each one scripts the
`wait-output` answer — which is exactly what a hand-written literal did on this branch. Now a half
that stops calling the function is a compile error at its own call site. The coupling is stated on
the function and in `inspectors/mod.rs`'s map.

**3. `HerdrProjectPaneSnapshot` in an implementation module.** Moved to `inspectors/types.rs`
beside `HerdrInspectorBinding`, with `ProjectPaneSnapshots`. Four modules outside
`herdr/project_panes.rs` hold them — `extension/executor` (the live map), `tui/fleet_state`,
`tui/fleet_status`, and `extension/tool/routing` through the deps. `herdr/mod.rs` deliberately
does NOT re-export them: two public paths to one type would put that module back in the middle of
a seam it does not own.

**4. `path_within`, four private copies.** One `pub(crate)` in `crate::paths` — the crate's single
port of pi's `shared/utils.ts` path helpers — and the four copies deleted
(`background/fleet_view.rs`, `tui/fleet_transcript.rs`, `background/scheduled_runs/store.rs`,
`inspectors/actions.rs`). It is a containment check every caller uses to REFUSE an escaping path,
so four copies were four places for a security-relevant predicate to drift. Behaviour is
unchanged: the three absolutizing copies were identical, and `scheduled_runs::store` canonicalizes
both inputs before calling, so `std::path::absolute` on them is a no-op. Its doc states what it is
NOT: `absolute` resolves neither `..` nor symlinks, so a caller that must survive a planted
symlink canonicalizes first — which is what `trusted_dir`'s second leg does.

**5. `agent.view.set` / `agent.view.clear` could not be sent. DECIDED: ADDED, opt-in.**
The spec asserted the capability (`HERDR_STATUS_BRIDGE.md` "What cyrup builds" §5) and the code
could not do it, and the blocker `cyrup-herdr/src/lib.rs` recorded — *"it needs the fleet roster to
exist first"* — is satisfied: this feature IS the fleet roster. So:

* `cyrup-herdr/src/schema/agents.rs` gains the eleven types mirroring
  `tmp/herdr/src/api/schema/agents.rs:52-162` — `AgentViewSetParams`, `AgentViewClearParams`,
  `AgentViewFilter`, `AgentViewField`, `AgentViewBuiltinField`, `AgentViewValue`,
  `AgentViewContext`, `AgentViewSort`, `AgentViewSortField`, `AgentViewBuiltinSortField`,
  `AgentViewSortOrder` — plus `AgentView`, the answer both verbs give;
* `Method::AgentViewSet` / `AgentViewClear` (`schema.rs:128,130`), `ResponseResult::AgentView`
  (`response.rs:110-116`) with an `agent_view` accessor, and
  `HerdrClient::{set_agent_view,clear_agent_view}`;
* **the production caller**, which is what the `Method` enum's own rule requires: the status
  bridge's drain installs the projection before its first report and clears it after the release
  (`herdr/reporter.rs`), gated by `herdr::view::enabled(env)` off the same `EnvSource` the bridge's
  own gate reads.

**It is a SORT with no filter** — `attention` desc, then `state_change_seq` desc, which is herdr's
own attention rank with the most recent transition breaking ties, i.e. "whoever needs me, most
recent first". A filter is refused on purpose: there is exactly ONE view server-wide
(`app/api/agent_view.rs:88-89`) and it governs the sidebar, the mobile list, mouse targets, indexed
focus and next/previous navigation (`socket-api.mdx:421-424`), so a cyrup filter would HIDE the
user's other agents — including agents cyrup has nothing to do with — from their own terminal.

**And it is OPT-IN** (`CYRUP_HERDR_AGENT_VIEW=1`), for the same reason one step further: even a
re-ordering is a global change to someone else's UI that they did not ask for, and a set
atomically replaces whatever view another program owns. The clear always names `cyrup:subagents`,
so a session whose view was already replaced leaves the replacement alone (`socket-api.mdx:492`).

`cyrup-herdr/src/lib.rs`'s not-ported table loses the row and the count goes 21 → 24 ported /
81 absent; `method_names.rs` gains both verbs.

### C8 — gates and the gutting pass

`cargo fmt --all --check` clean. `cargo clippy --workspace --all-targets --features test-fixtures
-- -D warnings` clean.

Every mutation was applied, run against **only its target test** (`-p <crate> -E 'test(<name>)'` —
never a workspace run), and restored byte-for-byte in one scripted pass.

### New and changed tests

| test | crate | what it pins |
|---|---|---|
| `project_close_through_the_tool_forgets_the_pane` | `cyrup-ext-subagents` | C4 — `routing.rs` passes the LIVE map, through the real `Tool::execute`, with no herdr |
| `project_status_remembers_into_the_executors_own_map` | `cyrup-ext-subagents` | C4 — the insert half reaches both readers of the executor's own map |
| `a_hung_herdr_cannot_starve_the_rest_of_the_session_shutdown` | `cyrup-ext-subagents` | C5 — a black-hole socket, the real `Dispatcher` at its real budget, and the teardown below the release still runs |
| `the_builtin_backends_gate_on_the_contexts_env_through_the_dispatcher` | `cyrup-ext-subagents` | C7.1 — the not-installed path, the herdr path and the darwin conjunct, all through `handle_inspector_action` with the REAL backends |
| `an_injected_client_does_not_make_a_non_pane_available` | `cyrup-ext-subagents` | C7.1 — a seam does not widen `available()` past `herdr/plugin.ts:13-14` |
| `the_dashboard_...` header assertion (extended) | `cyrup-ext-subagents` | C7.2 — the rendered dashboard's first line IS `herdr::actions::inspector_ready_marker`, asserted against the other half's own function |
| `the_projection_is_opt_in_on_exactly_one` | `cyrup-ext-subagents` | C7.5 — the gate is `== "1"`, off the injected env |
| `the_set_payload_is_herdrs_own_shape_and_carries_no_filter` | `cyrup-ext-subagents` | C7.5 — the WIRE bytes: bare `"attention"`, `snake_case`, and no `filter` key |
| `the_clear_names_this_source` | `cyrup-ext-subagents` | C7.5 — the clear is ownership-scoped |
| `the_source_and_label_are_inside_herdrs_bounds` | `cyrup-ext-subagents` | C7.5 — `normalize_source`/`normalize_label` would not refuse the whole call |
| `the_opt_in_sidebar_projection_is_installed_and_cleared` | `cyrup-it` | C7.5 — both verbs over a REAL socket, through `HerdrBridge::start` |
| `without_the_opt_in_no_agent_view_verb_is_ever_sent` | `cyrup-it` | C7.5 — the default really is inert, asserted after a report that DID land |
| `every_method_serialises_under_herdrs_own_name` (extended) | `cyrup-herdr` | C7.5 — both new `Method` variants under herdr's own `#[serde(rename)]` |

### What this batch did NOT close, and why that is a report rather than a decision

* **Two contract gaps remain open and are still reported as gaps**, because neither is in C7's
  list and both are the orchestrator's call, not a local redefinition:
  `InspectorContext` carries no run-state snapshot where pi's does (`inspectors/types.ts:19-23`),
  so `herdr::actions::run_state` re-reads the run's own `status.json`; and `HerdrClient::run`
  returns a bare `HerdrErrorCode` with no message, so `client::message_for` reconstructs herdr's
  sentence from the code and the argv instead of forwarding herdr's own
  (`inspectors/herdr/client.rs:88-123`, `inspectors/ghostty/actions.rs:18-35`).
* **`agent.prompt` / `agent.wait` / `events.wait` stay unported.** The reason is unchanged and is
  a missing CALLER, not size: all three need cyrup to drive a *sibling* agent in another pane, and
  no surface does. `cyrup-herdr/src/lib.rs`'s table states it.
* **The mandate's own worked example is wrong** — see C6's last row. Flagged, not edited.

**The gutting table.** Two scripted passes, 24 mutations; each applied, run against ONLY its
target test, and restored byte-for-byte with the restore verified by re-reading the file.

| id | mutation | test | result | tree |
|---|---|---|---|---|
| `M-C4a` | `routing.rs`'s `project.*` deps go back to `panes: None` | `project_close_through_the_tool_forgets_the_pane` | **RED** | restored |
| `M-C4b` | the `Close` arm stops removing from the map | `project_close_through_the_tool_forgets_the_pane` | **RED** | restored |
| `M-C4c` | `remember`'s `None` early return becomes unconditional | `project_status_remembers_into_the_executors_own_map` | **RED** | restored |
| `M-C5a` | `crate::herdr::shutdown().await` back as the arm's FIRST statement | `a_hung_herdr_cannot_starve_the_rest_of_the_session_shutdown` | **RED** | restored |
| `M-C5b` | the join budget raised past the dispatch budget (30 s) | `a_hung_herdr_cannot_starve_the_rest_of_the_session_shutdown` | **RED** | restored |
| `M-C71a` | `build_context` passes an empty `env` instead of `deps.env` | `the_builtin_backends_gate_on_the_contexts_env_through_the_dispatcher` | **RED** | restored |
| `M-C71b` | herdr `available()` regains its `self.client.is_some() ||` disjunct | `an_injected_client_does_not_make_a_non_pane_available` | **RED** | restored |
| `M-C71c` | ghostty `available()` drops the `darwin` conjunct | `the_builtin_backends_gate_on_the_contexts_env_through_the_dispatcher` | **RED** | restored |
| `M-C72` | `inspector_ready_marker` spelled by hand (`subagents inspector for …`) | `the_ready_marker_is_the_runners_own_dashboard_header` | **RED** | restored |
| `M-C74` | `path_within` drops its `starts_with` leg | `a_symlink_never_escapes_the_trusted_root` | **RED** | restored |
| `M-C74b` | `path_within` returns `true` | `path_outside_trusted_roots_is_refused` | **RED** | restored |
| `M-C74c` | `path_within` compares rendered STRINGS instead of components | `path_within_is_component_wise_containment_over_absolute_paths` | **RED** | restored |
| `M-C75a` | `view::enabled` accepts any value, not `"1"` | `the_projection_is_opt_in_on_exactly_one` | **RED** | restored |
| `M-C75b` | `AgentViewSortField` loses `#[serde(untagged)]` | `the_set_payload_is_herdrs_own_shape_and_carries_no_filter` | **RED** | restored |
| `M-C75c` | `clear_params` becomes an UNCONDITIONAL clear | `the_clear_names_this_source` | **RED** | restored |
| `M-C75d` | `HerdrBridge::start` hard-codes `agent_view: false` | `the_opt_in_sidebar_projection_is_installed_and_cleared` | **RED** | restored |
| `M-C75e2` | the drain's `agent.view.clear` gate is inverted | `the_opt_in_sidebar_projection_is_installed_and_cleared` | **RED** | restored |
| `M-C75f` | `Method::name` misspells `agent.view.set` | `every_method_serialises_under_herdrs_own_name` | **RED** | restored |
| `M-C75g` | `AgentViewField` loses `#[serde(untagged)]` | `the_documented_agent_view_set_request_round_trips_byte_for_byte` | **RED** | restored |
| `M-C75h` | `AgentViewValue` loses `#[serde(untagged)]` | `the_documented_agent_view_set_request_round_trips_byte_for_byte` | **RED** | restored |
| `M-C75i` | `AgentViewFilter`'s tag becomes `kind` instead of `op` | `the_documented_agent_view_set_request_round_trips_byte_for_byte` | **RED** | restored |
| `M-C75j` | `AgentViewBuiltinField` loses `rename_all = "snake_case"` | `the_documented_agent_view_set_request_round_trips_byte_for_byte` | **RED** | restored |
| `M-C75k` | `ResponseResult::agent_view` defaults instead of refusing an unexpected type | `the_agent_view_answer_decodes_both_outcomes` | **RED** | restored |

**Gate results.**

* `cargo fmt --all --check` — clean.
* `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` — clean, zero
  warnings.
* `cargo nextest run --workspace --features test-fixtures` — **`11032 tests run: 11032 passed,
  9 skipped`** (baseline 11 014 / 9 skipped; +18, every one of them named in the table above).
* `cargo nextest run -p cyrup-it --features it` (`CYRUP_IT_BIN_DIR` on the existing `it-bins`, the
  `AWS_*` variables unset) — **`590 tests run: 590 passed, 0 skipped`** (baseline 588; +2, the two
  `agent.view` socket tests).

**The tree is restored.** Both mutation scripts write the original bytes back in a `finally` and
assert the file re-reads identical; after the passes, every mutated anchor was re-grepped by hand
— `panes: Some(self.executor.herdr_project_pane_map())`, `tokio::spawn(crate::herdr::shutdown())`,
`candidate == base || candidate.starts_with(&base)`, `"agent.view.set"`,
`AgentViewClearParams::owned_by(super::SOURCE)`, `super::view::enabled(env)`, all four
`#[serde(untagged)]`, `tag = "op"`, `if drain.agent_view`, and
`other => other.unexpected(method, "agent_view")` — and `cargo fmt --check`, clippy and both
suites were run on the restored tree, in that order.
