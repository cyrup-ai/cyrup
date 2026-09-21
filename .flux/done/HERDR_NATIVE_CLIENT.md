---
stage: done
status: completed
updated: 2026-09-20
---

# The herdr native socket client — Rust to Rust

OBJECTIVE: a first-class Rust client for herdr's socket API, which the inspector/project verbs
(`INSPECTOR_AND_PROJECT_PANES.md`) and the status bridge (`HERDR_STATUS_BRIDGE.md`) both sit on.

**Source of truth: `tmp/herdr` @ `d59d060` (v0.9.1).** Read herdr's own code and docs, NOT pi's
consumption of it:
- `docs/preview/website/src/content/docs/socket-api.mdx` (959 lines) — the protocol
- `docs/preview/website/src/content/docs/plugins.mdx` — the plugin surface
- `src/api/schema.rs` (274) · `src/api/server.rs` (1 644) · `src/api/mod.rs` (101) ·
  `src/server/client_transport.rs` (2 410) — the implementation

Second and third opinions, useful but SUBORDINATE to herdr's own source:
- pi `src/inspectors/herdr/client.ts` (130 @v0.68.0) — spawns the CLI, parses the last JSON line
- `tmp/code_puppy_core_plugins/code_puppy_core_plugins/herdr/client.py` (518) — a working socket
  client with a named-pipe fallback

## What is already known and must be re-verified, not re-derived

**Discovery** — herdr injects four env vars into every pane it owns:
`HERDR_ENV=1` · `HERDR_SOCKET_PATH` · `HERDR_PANE_ID` (e.g. `w1:p1`) · `HERDR_TAB_ID` (`w1:t1`).
Absent ⇒ the client is inert: no socket, no task, no overhead.

**Framing** — newline-delimited JSON, one envelope per line:
`{"id":"req_1","method":"pane.report_agent","params":{…}}`. 32 MiB per transaction.

**Transport** — a Unix socket; on Windows herdr maps the same file-style path onto a named pipe
via the `interprocess` crate. cyrup is Rust: `tokio::net::UnixStream` is the direct fit.

**Schema** — `herdr api schema --json` emits a full JSON Schema of requests, success responses,
error responses, emitted events and subscription events. The binary is not installed in this
container, so derive the same contract from `src/api/schema.rs` and the docs, and record in the
port how a future maintainer regenerates it.

**99 methods** across `server` · `notification` · `client` · `session` · `workspace` · `worktree` ·
`tab` · `pane` (28) · `popup` · `layout` · `agent` (12) · `events` · `integration` · `plugin`.

**Error codes** — herdr's own (`agent_blocked`, `feature_disabled`, `stream_conflict`, …) plus the
normalization pi's client applies (`HERDR_UNAVAILABLE`, `PANE_GONE`, `NOT_FOUND`, `TIMEOUT`,
`VALIDATION_ERROR`). Port herdr's real set; keep pi's normalization only where a consumer needs it.

## What the augment must settle

1. **The exact request/response envelope and error shape**, from `src/api/schema.rs` and
   `src/api/server.rs` — not from the docs' examples alone. Typed Rust: one enum per method family,
   `serde` with herdr's own field names, a `Result`-shaped response.
2. **Which of the 99 methods this batch needs**, and which are deliberately unported for now (say
   so with a reason that is TRUE — "the panes batch needs `pane.split/get/run/close`, the bridge
   needs `pane.report_agent*` and `events.subscribe`" is a real scope; "large" is not).
3. **The connection model.** One shared connection, or one per request? `events.subscribe` holds a
   long-lived stream and `session.snapshot` must be called on a DIFFERENT connection to avoid the
   documented bootstrap gap (`socket-api.mdx:118-130`) — so the client needs at least: a
   request/response channel and a subscription channel. Say exactly what the API looks like.
4. **Reconnect and degradation.** herdr may not be running, may restart, may be a version older
   than the methods used (pi requires 0.7.5+; the clone is 0.9.1). What does cyrup do in each case?
   The answer must never be a silent hang or a fabricated success.
5. **The CLI fallback.** pi drives `herdr pane get/run/close/split` by spawning the binary. Keep
   that path for when `HERDR_SOCKET_PATH` is absent but `herdr` is on `PATH`, and say which
   consumers can use it and which genuinely need the socket.
6. **Where it lives.** A module in `cyrup-ext-subagents`, or its own crate? It is consumed by the
   verbs, the bridge, and potentially the TUI. Recommend, with the reason.

## Definition of done

1. A typed client that speaks herdr's protocol, with every request/response type checked against
   `tmp/herdr/src/api/schema.rs`.
2. Inert outside a herdr pane — no socket, no task — pinned by test.
3. Driven end to end by tests against a FAKE herdr socket server that speaks the real framing.
4. Reconnect, version mismatch and absent-herdr all handled explicitly and tested.
5. No fabricated success anywhere: a method that fails reports why.

---

## [AUG — client]

**Read in full for this section.** herdr @ `d59d060` (v0.9.1, `Cargo.toml:3`):
`src/api/mod.rs` (101) · `src/api/schema.rs` (274) · `src/api/schema/{common,response,session,server,events,panes,agents}.rs`
· `src/api/client.rs` (291 — **herdr's own Rust client**, which the seed did not list and which is the
single most load-bearing file in this batch) · `src/api/server.rs` (1 644) · `src/api/status.rs` (60)
· `src/api/subscriptions.rs:249-290` · `src/ipc.rs:25-80` · `src/session.rs:96-186` ·
`src/config/io.rs:30-55` · `src/cli.rs:745-802` · `src/cli/api.rs` (118) · `src/cli/pane.rs:20-53,1039-1052`
· `src/cli/protocol_guard.rs` · `src/cli/target.rs:66-191` · `src/protocol/wire.rs:20-29` ·
`src/server/client_commands.rs:1-90` · `src/server/client_transport.rs:1-60,1262-1290` ·
`docs/preview/website/src/content/docs/socket-api.mdx` (959) · **`docs/next/api/herdr-api.schema.json` (277 222 bytes)**.
pi @ `v0.68.0`: `src/inspectors/herdr/client.ts` (130) · **`src/runs/shared/herdr-connection.ts` (134)** ·
`src/integrations/herdr-status.ts:1-60,188-221`. Python: `code_puppy_core_plugins/herdr/client.py:1-43,353-419`.

### 0. Corrections to the seed — verify these before citing the seed again

1. **`herdr api schema --json` needs no binary.** The document it prints is a CHECKED-IN FILE:
   `src/cli/api.rs:1` is `include_str!("../../docs/next/api/herdr-api.schema.json")`, and
   `src/api/schema/tests.rs:182-207` (`generated_protocol_schema_artifact_is_current`) fails CI if it
   drifts from `schemars::schema_for!` over the live types. The full contract is therefore already in
   this container at `tmp/herdr/docs/next/api/herdr-api.schema.json`. The seed's "the binary is not
   installed … so derive the same contract from `src/api/schema.rs` and the docs" is wrong about the
   need. **Regeneration recipe for a maintainer: bump the pin, re-read that one file** (or, in a herdr
   checkout, `HERDR_UPDATE_API_SCHEMA=1 just test-one generated_protocol_schema_artifact_is_current`).
2. **Not 99 methods — 105.** `src/api/schema.rs:47-271` has 105 `#[serde(rename = …)]` variants plus 4
   `#[serde(skip)]` internal graphics-frame variants (109 total; `api_method_name` covers all 109 at
   `server.rs:398-510`). The published JSON Schema lists **104** (`request.oneOf`), because
   `pane.graphics.stream` is `#[schemars(skip)]` at `schema.rs:209` while still being a live wire method.
3. **The 32 MiB figure is from the wrong protocol.** 32 MiB is `MAX_GRAPHICS_FRAME_SIZE`
   (`src/protocol/wire.rs:29`) on herdr's **binary** client-shell protocol. The JSON socket API's own
   bound is **1 MiB per request line**, `MAX_INITIAL_REQUEST_BYTES` (`src/api/server.rs:32`), with a
   5 s read deadline (`:30`) and a 5 s write deadline (`:31`). `socket-api.mdx:195-196` says "client
   transport keeps each transaction within its 32 MiB wire limit" about pane graphics — not about us.
4. **`src/server/client_transport.rs` (2 410) is not part of the socket API.** Its own module doc:
   *"Blocking client socket transport for the headless server … the thin-client handshake, read loop,
   and writer loop"* (`:1-5`) — the binary attach/render endpoint, `PROTOCOL_VERSION = 22`
   (`wire.rs:20`). It is worth reading exactly once, to learn that `unsupported_method`
   (`:102`, `:1850`) is an **endpoint** code and is never produced by the JSON socket.
5. **pi is not CLI-only.** `src/runs/shared/herdr-connection.ts:70-107` is `SocketRpcClient`, a real
   NDJSON socket client with `call()` and `subscribe()` over `net.createConnection`. The seed's
   "pi spawns the CLI and parses the last JSON line" describes only `inspectors/herdr/client.ts`.
   `SocketRpcClient` is the closest thing to a reference implementation and **cyrup should mirror its
   subscribe handshake**, which is correct against herdr's source (`:100`, `result.type ===
   "subscription_started"`, and the uncorrelated `{"id":"", error.code:"invalid_request"}` case).
6. **pi's status bridge never reports semantic state.** `integrations/herdr-status.ts` calls exactly
   two shapes, both `["pane","report-metadata",paneId,…]` (`:206`, `:221`) through an injected
   `runHerdr: (args) => …` (`:45`). It never sends `pane.report_agent`. `HERDR_STATUS_BRIDGE.md`'s
   claim that pi "maps run lifecycle onto `pane.report_agent`" is false; doing so is a real
   `[CYRUP-EXCEEDS-UPSTREAM]`, not a port.
7. **`HERDR_STATUS_BRIDGE.md` also says `events.subscribe` is "not in pi's integration".** False —
   `herdr-connection.ts:103`. `session.snapshot` and `agent.view.set` genuinely are absent from pi.
8. The seed's `socket-api.mdx:118-130` bootstrap-gap anchor is **correct**. So are the four env vars,
   though herdr injects **five**: `socket-api.mdx:302-305` adds `HERDR_WORKSPACE_ID`.

### 1. The envelope, from the source

**Request** — `src/api/schema.rs:35-47`:

```rust
pub struct Request { pub id: String, #[serde(flatten)] pub method: Method }
#[serde(tag = "method", content = "params")] pub enum Method { #[serde(rename="ping")] Ping(PingParams), … }
```

Wire: `{"id":"req_1","method":"pane.report_agent","params":{…}}`. `params` is **mandatory on every
method, including `ping`** — adjacent tagging with a newtype variant. The published schema pins it:
the `ping` variant's `"required"` is `["method","params"]`. `{"id":"x","method":"ping"}` is rejected.
`EmptyParams` (`schema/common.rs:25-26`) serialises to `{}`.

**Success** — `schema/response.rs:24-28`: `{"id": String, "result": ResponseResult}` where
`ResponseResult` is internally tagged `#[serde(tag="type", rename_all="snake_case")]` (`:42-44`) over
**65 variants**, ending in `Ok {}` (`:307`). The docs' `{"id":"req_1","result":{"type":"pong"}}`
(`socket-api.mdx:670`) is **abridged** — `Pong` requires `version: String` and `protocol: u32`
(`response.rs:45-50`, produced at `server.rs:356-363`) and carries optional `capabilities`.

**Error** — `schema/response.rs:30-40`: `{"id": String, "error": {"code": String, "message": String}}`.
`code` is a bare `String` on the wire, **not an enum** — do not model it as a closed enum.

**Framing** — one JSON value per `\n`-terminated line, both directions
(`api/client.rs:185-190` write, `server.rs:781-785` write, `:539-583` read).

### 2. The connection model — one request per connection, and why that is not a choice

`handle_connection_with_stop` (`server.rs:156-317`) reads **one** line via
`read_initial_request_line` (`:168`, `:528-537`), dispatches, writes one response line (`:301`), and
returns. There is no read loop. herdr's own client agrees: `ApiClient::request_value`
(`api/client.rs:55-61`) calls `self.connect()` per request. The Python client agrees:
`_deliver_unix` (`client.py:403-408`) opens `AF_UNIX`, `sendall`, one `recv`, close. pi agrees:
`SocketRpcClient.call` (`herdr-connection.ts:77`) `net.createConnection` per call.

**Consequence.** The `socket-api.mdx:118-130` rule that `session.snapshot` must run on a *different*
connection from `events.subscribe` is not advice — it is structural. Only three methods hold a
connection open past the first response: `events.subscribe` (`server.rs:229-250` → `stream_subscriptions`
`:715-779`), `pane.graphics.stream` (`:213-228`), and the four in-band waits
(`events.wait`, `agent.prompt`, `agent.wait`, `pane.wait_for_output`, `:251-288`) which block and then
send exactly one line.

**Subscription stream shape** (`server.rs:749-778`):
line 1 is `{"id":"<your id>","result":{"type":"subscription_started"}}`; every later line is an event
with **no `id` field at all** (`subscriptions.rs:249-266` emits a bare `serde_json::Value`).
Events are polled and flushed on a **100 ms tick** — `CONNECTION_POLL_INTERVAL` (`server.rs:28`,
consumed at `:777`) — so worst-case event latency is 100 ms, and `should_stop_connection` (`:812-821`)
is the liveness check on the same tick.

**The naming trap the docs do not mention.** Two envelope types share that one stream:

| flavour | Rust type | `event` value on the wire | source |
|---|---|---|---|
| lifecycle (`pane.created`, `workspace.closed`, `layout.updated`, …) | `EventEnvelope` | **`pane_created`** — `rename_all="snake_case"` | `schema/events.rs:192-194`, `:361-365` |
| the three rich ones | `SubscriptionEventEnvelope` | **`pane.agent_status_changed`** — explicit renames | `schema/events.rs:367-381` |

Confirmed against the generated artifact: `event.$defs.EventKind.enum` is
`["workspace_created",…,"pane_agent_status_changed","layout_updated"]` (underscores) while
`subscription_event.$defs.SubscriptionEventKind.enum` is
`["pane.output_matched","pane.agent_status_changed","pane.scroll_changed"]` (dots). The **subscribe
request** uses dots for all of them (`Subscription`, `events.rs:16-85`). So: **subscribe with dots,
receive lifecycle with underscores.** `EventEnvelope.data` is itself internally tagged
(`EventData`, `events.rs:420-422`, `tag="type"`, snake_case), so the payload is doubly tagged and
`data` alone is sufficient to decode.

### 3. Errors — the real set

Produced by the **API server** itself: `invalid_request` (`server.rs:198` — *this is also what an
unknown method name yields*, `schema/tests.rs:420-424` asserts `"unknown variant"`),
`internal_error` (`:365`, `:965`), `connection_local_only` (`:373` — `client_shell.surface.set` is
**permanently unreachable** from the raw socket), `server_unavailable` (`:390`, `:892`, `:928`),
`timeout` (`:842`, asserted `:953`), `serialization_error` (`client_commands.rs:79`).

Produced by the **app**, via `encode_error` (`src/app/api/responses.rs:7`) — 57 distinct codes, of
which the ones this feature can hit: `pane_not_found` `no_active_pane` `no_active_workspace`
`stale_pane_target` `pane_split_failed` `pane_send_failed` `tab_not_found` `workspace_not_found`
`invalid_params` `invalid_metadata_source` `invalid_metadata_token` `invalid_metadata_ttl`
`invalid_agent` `invalid_agent_view` `plugin_not_found` `plugin_disabled` `query_too_large`
`ui_busy` `timeout` `stale_content`. Plus `agent_blocked` (`src/app/api/agents.rs:149`),
`feature_disabled` and `stream_conflict` (`src/app/api/pane_graphics.rs:111`),
`workspace_group_close_required` (`src/app/api/workspaces.rs:326`), `popup_not_open`.

**Not on the socket at all:** `protocol_mismatch` and `server_not_running` are **CLI-only**
(`src/cli/protocol_guard.rs:16-43`, `src/cli/server_not_running.rs:33`) — `send_request`
(`cli.rs:769-775`) synthesises them around the socket call. A native client gets **no protocol guard
for free** and must ping for itself. `unsupported_method` is endpoint-only (§0.4).

pi's `HerdrErrorCode` (`client.ts:3-9`) is a **lossy five-way normalisation of a 60-code space**
(`normalizeCode`, `:35-41`, folds everything unrecognised to `VALIDATION_ERROR`). Keep herdr's code
verbatim; keep pi's five only where cyrup must reproduce a byte-identical upstream sentence — which
is exactly one place, already in the tree: `crates/cyrup-intercom/src/project_pane.rs:30-67`.

### 4. Method scope

**Needed by the three batches (23):**

| method | who | source anchor |
|---|---|---|
| `ping` | everyone — liveness, version, protocol, capabilities | `server.rs:355-368` |
| `session.snapshot` | bridge + FleetView bootstrap | `schema/session.rs:8-23` |
| `events.subscribe` | bridge event consumer | `server.rs:715-779` |
| `pane.report_agent` · `pane.report_agent_session` · `pane.report_metadata` · `pane.release_agent` · `pane.clear_agent_authority` | bridge write half | `schema/panes.rs:447-524` |
| `pane.split` · `pane.send_input` · `pane.get` · `pane.close` · `pane.list` · `pane.current` · `pane.focus` · `pane.rename` · `pane.read` | inspector + project panes | `panes.rs:27-44,315-369`; `herdr pane run` **is** `pane.send_input{text,keys:["Enter"]}`, `cli/pane.rs:1047-1051` |
| `tab.get` · `tab.rename` | pane-owned tab label (the Python client's rule, `client.py:423-468`) | `schema/tabs.rs` |
| `agent.list` · `agent.get` | cross-pane fleet roster | `schema/agents.rs:187-226` |
| `agent.view.set` · `agent.view.clear` | sidebar projection `[CYRUP-EXCEEDS-UPSTREAM]` | `socket-api.mdx:418-495` |

**Deliberately deferred, each with a true reason:**

- `pane.graphics.*` (6) + the 4 `#[serde(skip)]` frame variants — the wire contract is a raw-byte
  side-channel after the JSON header (`socket-api.mdx:204-236`) and cyrup has no image producer.
  Not "large": **no producer exists**, so the methods would have no argument to carry.
- `plugin.*` (12) + `integration.*` (3) — require shipping a `herdr-plugin.toml` package
  (`socket-api.mdx:499-560`). That is a distinct deliverable with its own install story.
- `worktree.*` (4) — herdr worktrees create **herdr workspaces** (`response.rs:69-86`). cyrup owns
  its worktrees through `gix` in `cyrup-ext-subagents/src/spawn/worktree.rs`; adopting herdr's would
  move ownership of a feature that already works.
- Pane geometry — `swap` `move` `zoom` `resize` `neighbor` `edges` `focus_direction` `scroll`
  `copy_motion` `copy_search` `selection.read` `edit_scrollback` `clear` `input.set` `link.*`
  `process_info` `layout` + `layout.*` (3) — no consumer in any of the three batches arranges the
  user's terminal. Add on first caller; the envelope is generic so each is one enum variant.
- `server.stop` · `server.live_handoff` — destroy or restart the user's whole terminal session.
- `command.invoke` · `popup.close` · `product_announcement.dismiss` · `release_notes.dismiss` ·
  `client.window_title.*` · `notification.show` · `server.reload_config` ·
  `server.{agent_manifests,reload_agent_manifests}` · `workspace.*` (9) · `tab.{create,list,focus,move,close}` ·
  `agent.{read,explain,send_keys,rename,focus,start,prompt,wait}` · `events.wait` ·
  `pane.wait_for_output` — no caller yet. **`agent.prompt`/`agent.wait` are the highest-value of these**
  (server-owned, pane-occupant-pinned waits, `socket-api.mdx:114`) and are the obvious follow-on once
  cyrup wants to drive a sibling agent in another pane.
- `client_shell.surface.set` — **cannot be ported**: refused on the raw socket by construction
  (`server.rs:370-376`, `connection_local_only`). Model it as absent, not as deferred.

### 5. The Rust shape

**Where it lives: a new leaf crate, `crates/cyrup-herdr`.** Three reasons, each grepped:

1. ~~**Two consumer crates already exist and neither depends on the other.**~~ **DELETED — the
   grep was self-falsifying.** `grep -n cyrup-intercom crates/cyrup-ext-subagents/Cargo.toml` is
   indeed empty, but the converse is NOT:
   `grep -n cyrup-ext-subagents crates/cyrup-intercom/Cargo.toml` hits line 56,
   `cyrup-ext-subagents = { workspace = true }`, in `[dependencies]`. The two consumer layers are
   therefore **not** independent, and hosting the client inside `cyrup-ext-subagents` would have
   forced no new edge at all. (`cyrup-intercom` — `src/project_pane.rs`'s `HerdrLauncher` — is the
   one consumer today; `grep -rn 'cyrup_herdr\|cyrup-herdr' crates/cyrup-ext-subagents/` is empty.)
   The leaf stands on reason 2 alone, which is true and load-bearing. See `[FIX — round 1]`.
2. **`cyrup-ext-subagents` may not resolve for Windows.** Its `Cargo.toml` carries `nix` and `libc`
   ungated in `[dependencies]`; `.flux/todo/WINDOWS_BUILD_UNGATED_DEPS.md` is the item recording
   exactly that, quoting `cyrup-mcp`'s own warning that *"`nix` would take Windows out of the build
   entirely"*. herdr's socket is explicitly Windows-capable (`src/ipc.rs:44-51`, `GenericNamespaced`).
   Putting a Windows-capable protocol inside the one crate that today cannot target Windows buries
   the named-pipe arm as unbuildable text — the precise failure `cyrup-intercom/src/broker/listener.rs:16-25`
   documents having already happened once in this workspace.
3. The client needs only `tokio` (`net`, `io-util`, `time`, `sync`, `macros`), `serde`, `serde_json`,
   `thiserror`, `tracing`. No cyrup crate at all. It is a leaf.

```
crates/cyrup-herdr/
  Cargo.toml
  src/lib.rs          # re-exports; the crate doc carries the §2 connection-model explanation
  src/env.rs          # HerdrPane::discover(), socket-path resolution
  src/transport.rs    # LocalStream (UnixStream | NamedPipeClient), NDJSON read/write, bounds
  src/error.rs        # HerdrError, ApiError, ApiErrorCode, Unavailable
  src/schema/mod.rs   # one file per herdr schema file, same names, so a diff is line-for-line:
  src/schema/{common,request,response,events,panes,agents,session,tabs}.rs
  src/client.rs       # HerdrClient (one-shot), HerdrEvents (stream), bootstrap()
  src/reconnect.rs    # ReconnectingEvents
  src/probe.rs        # Pong / Capabilities / availability
  src/cli.rs          # HerdrCli fallback
```

**Types.** Mirror herdr's own names and field spellings exactly; `#[serde(deny_unknown_fields)]`
nowhere (`socket-api.mdx:959`: *"JSON API clients should ignore unknown fields"*).

```rust
pub struct Request { pub id: String, #[serde(flatten)] pub method: Method }
#[derive(Serialize)] #[serde(tag = "method", content = "params")]
pub enum Method { #[serde(rename = "ping")] Ping(PingParams), … }   // serialize-only: we never parse one

#[derive(Deserialize)] #[serde(untagged)]
enum WireResponse { Success(Box<SuccessResponse>), Error(ErrorResponse) }   // api/client.rs:231-236
pub struct SuccessResponse { pub id: String, pub result: ResponseResult }
pub struct ErrorResponse  { pub id: String, pub error: ErrorBody }
pub struct ErrorBody      { pub code: String, pub message: String }

#[derive(Deserialize)] #[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseResult {
    Pong { version: String, protocol: u32, #[serde(default)] capabilities: Option<ServerCapabilities> },
    SessionSnapshot { snapshot: Box<SessionSnapshot> },
    SubscriptionStarted {},
    PaneInfo { pane: PaneInfo }, PaneList { panes: Vec<PaneInfo> }, PaneCurrent { pane: PaneInfo },
    AgentList { agents: Vec<AgentInfo> }, AgentInfo { agent: AgentInfo },
    AgentView { active: bool, source: Option<String>, label: Option<String> },
    TabInfo { tab: TabInfo }, PaneRead { read: PaneReadResult }, Ok {},
    #[serde(other)] Unrecognised,   // forward compatibility: a newer herdr result is not an error
}

#[derive(Deserialize)] #[serde(untagged)]
pub enum Event { Lifecycle(EventEnvelope), Subscription(SubscriptionEventEnvelope) }
```

`#[serde(other)]` is legal on a unit variant of an internally tagged enum and is the mechanism that
makes `socket-api.mdx:959` true rather than aspirational. **A result the client does not recognise
is `Unrecognised`, never a fabricated success**: every typed accessor (`fn pane(self) -> Result<PaneInfo>`)
returns `HerdrError::UnexpectedResult { method, got }` for it — herdr's own client does the same
(`api/client.rs:119`, `ApiClientError::UnexpectedResult`).

**Errors.**

```rust
#[derive(Debug, thiserror::Error)]
pub enum HerdrError {
    #[error("{0}")]                                                    Unavailable(Unavailable),
    #[error("herdr {method} failed: {0}")]                             Api { method: &'static str, source: ApiError },
    #[error("herdr answered {method} with id {got:?}, expected {sent:?}")] IdMismatch { method: &'static str, sent: String, got: String },
    #[error("herdr closed the connection before answering {method}")]  Closed { method: &'static str },
    #[error("herdr did not answer {method} within {timeout:?}")]       Timeout { method: &'static str, timeout: Duration },
    #[error("herdr sent a line that is not valid JSON")]               Malformed { method: &'static str, #[source] source: serde_json::Error },
    #[error("herdr sent more than {limit} bytes answering {method}")]  TooLarge { method: &'static str, limit: usize },
    #[error("herdr answered {method} with {got}, which is not a {want}")] UnexpectedResult { method: &'static str, want: &'static str, got: String },
    #[error(transparent)]                                              Io(#[from] std::io::Error),
}
pub enum Unavailable {
    NotInHerdrPane,                       // HERDR_ENV != "1" or no HERDR_PANE_ID
    NoSocket { path: PathBuf, #[source] source: std::io::Error },
    BinaryMissing,                        // CLI fallback only
    ProtocolMismatch { client: u32, server: u32 },
}
pub struct ApiError { pub code: ApiErrorCode, pub message: String }
#[non_exhaustive] pub enum ApiErrorCode { PaneNotFound, NoActivePane, StalePaneTarget, AgentBlocked,
    FeatureDisabled, StreamConflict, InvalidParams, InvalidRequest, Timeout, ServerUnavailable,
    UiBusy, ConnectionLocalOnly, /* … */ Other(String) }
```

`ApiErrorCode::Other(String)` is the forward-compatible arm, and `Display` reproduces herdr's own
`message` verbatim — never a synthesised sentence (`api/client.rs:164`).

**The API surface.**

```rust
impl HerdrPane {
    /// `HERDR_ENV == "1"` && `HERDR_PANE_ID` non-empty (herdr's own gate, socket-api.mdx:302-305;
    /// pi's inspector plugin uses the same two, herdr-status.ts:130-131). `None` ⇒ inert.
    pub fn discover(env: &impl EnvSource) -> Option<Self>;
    pub fn socket_path(&self) -> &Path;   // §5.1 resolution
    pub fn pane_id(&self) -> &str; pub fn tab_id(&self) -> Option<&str>; pub fn workspace_id(&self) -> Option<&str>;
}

impl HerdrClient {
    pub fn new(socket: PathBuf) -> Self;                    // cheap, holds no connection — api/client.rs:33-44
    pub fn for_pane(pane: &HerdrPane) -> Self;
    pub async fn ping(&self) -> Result<Pong, HerdrError>;
    pub async fn call(&self, method: Method) -> Result<ResponseResult, HerdrError>;           // 15 s default
    pub async fn call_for(&self, method: Method, timeout: Duration) -> Result<ResponseResult, HerdrError>;
    pub async fn report_agent(&self, p: PaneReportAgentParams) -> Result<(), HerdrError>;     // typed sugar, ×5
    /// The ONLY way to obtain a snapshot + stream pair. Implements socket-api.mdx:118-130 in order:
    /// open the subscription connection, await `subscription_started`, buffer, call
    /// `session.snapshot` on a SECOND connection, then hand back the snapshot and the buffered-then-
    /// live stream. A caller cannot get the order wrong because there is no other constructor.
    pub async fn bootstrap(&self, subs: Vec<Subscription>) -> Result<(SessionSnapshot, HerdrEvents), HerdrError>;
    pub async fn subscribe(&self, subs: Vec<Subscription>) -> Result<HerdrEvents, HerdrError>; // stream only
}
impl HerdrEvents { pub async fn next(&mut self) -> Option<Result<Event, HerdrError>>; pub async fn close(self); }
```

`bootstrap` returning the pair, rather than pi's free-standing `subscribe`
(`herdr-connection.ts:93-106`, which leaves the ordering to its caller), is
`[CYRUP-EXCEEDS-UPSTREAM]` — **premise: herdr documents the ordering as mandatory
(`socket-api.mdx:121-125`) and pi's client does not encode it; a type that cannot be built in the
wrong order is the Rust way to hold that invariant.**

**Transport.** `transport.rs` may not reuse `cyrup-intercom`'s `BrokerStream`
(`crates/cyrup-intercom/src/transport/stream.rs:40-130`) directly — that would invert the layering —
but it **must copy its shape and its two hard-won lessons**: the boxed
`AsyncRead + AsyncWrite` eraser (`:36-40`) and the `ERROR_PIPE_BUSY` (231) retry loop (`:109-130`).
One divergence, and it is a real bug if missed: herdr names its Windows pipe with
`GenericNamespaced` over the **stringified socket path** (`src/ipc.rs:44-51`), which `interprocess`
maps to `\\.\pipe\<that whole path>` — confirmed independently by the Python client
(`client.py:410-419`, `"\\\\.\\pipe\\" + str(self._socket_path)`). cyrup must prepend that prefix;
`BrokerStream` does not, because its own paths are already pipe-form.

Bounds: refuse a response line over **4 MiB** (pi's `MAX_RPC_BYTES`, `herdr-connection.ts:9` — herdr's
own request bound is 1 MiB but it sets no response bound, and `session.snapshot` over a large session
is the one big reply), and **128 MiB** on a subscription connection's cumulative bytes before
forcing a reconnect (pi uses `MAX_RPC_BYTES * 32`, `:100`).

#### 5.1 Socket-path resolution — replicated, not shelled out

herdr's own order (`session.rs:173-181`, `api_socket_path_for` `:169-171`, `data_dir_for` `:161-167`,
`config_dir` `config/io.rs:30-35`; documented at `socket-api.mdx:686-692`):

1. explicit `--session <name>` — CLI-only, no env equivalent; cyrup skips it.
2. `HERDR_SOCKET_PATH` — verbatim.
3. `HERDR_SESSION=<name>` (rejected when it is `"default"` or fails `validate_name`, `:96-101`)
   → `<config_dir>/sessions/<name>/herdr.sock`.
4. `<config_dir>/herdr.sock`.

`config_dir` = `$XDG_CONFIG_HOME/herdr` if set, else the platform dir (`$APPDATA\herdr` on Windows,
`~/.config/herdr` on Linux — `config/io.rs:31-55`). Replicating this is `[CYRUP-EXCEEDS-UPSTREAM]`
over pi, whose `SocketRpcClient` only ever receives a path handed to it: **premise — with only
`HERDR_SOCKET_PATH`, a cyrup session started outside a herdr pane but alongside a running herdr
server could not find it, and that is the maintainer's normal state.**

#### 5.2 Version, capability and reconnect — none of which may hang or fabricate

**What actually differs between 0.7.5 and 0.9.1: not derivable here, and the port must say so.**
`git -C tmp/herdr log --oneline | wc -l` is **50** and `git tag` lists two `preview-2026-09-16-*`
tags — there is no 0.7.x in this clone, so no honest diff can be written. What *is* derivable:

- The only machine-readable version signal on the socket is `ping` →
  `{version: String, protocol: u32, capabilities}` (`response.rs:45-50`), `protocol == 22` today
  (`wire.rs:20`). `protocol` guards the **binary** attach path, not JSON method availability
  (`wire.rs:6-7` says so in its own module doc).
- The protocol's stated stability rule is per-method, not per-version:
  *"Other missing methods disable only those actions and show a client-local notice; they do not
  disconnect the UI … JSON API clients should ignore unknown fields and handle unsupported methods
  as normal errors."* (`socket-api.mdx:951-959`). An unknown method on the raw socket is
  `invalid_request` (`server.rs:177-204`).

So cyrup gates on **capability, not on a version string**: `ping` once per client construction, cache
`Pong`, and treat `invalid_request` on a specific method as "this build lacks it" — degrade that one
feature, keep the rest. pi's `supportsRawPanes` ≥ 0.7.5 string gate (`client.ts:118-120`) is kept in
exactly one place, unchanged, because it backs a byte-identical upstream sentence already in the tree:
`crates/cyrup-intercom/src/project_pane.rs:351-361`.

| state | behaviour | never |
|---|---|---|
| not in a herdr pane | `HerdrPane::discover` → `None`. No client, no task, no thread, no connect. | a background task |
| socket path absent / `ENOENT` / `ECONNREFUSED` | `Unavailable::NoSocket{path, source}` on the **first** call; consumers fall back to the CLI or report the reason | a silent success |
| herdr restarted mid-stream | `HerdrEvents::next` yields `Err(Closed)` then `None`; `ReconnectingEvents` re-runs `bootstrap` with 250 ms → 8 s capped jittered backoff, refreshing the snapshot per `socket-api.mdx:125-126` | an unbounded retry loop; a stale cache silently kept |
| request has no reply | `call_for`'s `tokio::time::timeout` → `HerdrError::Timeout{method, timeout}` (default 15 s, matching pi `herdr-connection.ts:78`, and longer than herdr's own 5 s `APP_RESPONSE_TIMEOUT`, `server.rs:29`) | a hang — there is no herdr-side deadline on a plain dispatch (`server.rs:911-913` `recv()` with `None` timeout) |
| response `id` ≠ request `id` | `HerdrError::IdMismatch` and the connection is dropped (pi does the same, `herdr-connection.ts:86`) | accepting the payload |
| method this build lacks | `ApiErrorCode::InvalidRequest` → that one capability off | refusing the whole client |

#### 5.3 The CLI fallback, and the one consumer that cannot use it

`herdr`'s CLI is itself a socket client: `send_request` (`cli.rs:769-775`) makes **two**
connections per verb — a `ping` for `ensure_server_protocol_compatible` (`:784-801`) and then the
request. Its only capability a native client lacks is herdr's own path resolution, which §5.1
replicates. So the fallback exists for one reason: **`HERDR_SOCKET_PATH` is unset and the user's
herdr config lives somewhere the replication got wrong** — plus `HERDR_BIN` honouring, already in
the tree (`crates/cyrup-intercom/src/identity.rs:56-60`).

| consumer | CLI verb | usable? |
|---|---|---|
| inspector verbs | `herdr pane split \| run \| close \| get \| list \| current \| focus \| rename` (`cli/pane.rs:20-43`) | **yes** — this is exactly pi's path |
| status-bridge write half | `herdr pane report-agent \| report-agent-session \| report-metadata \| release-agent` (`cli/pane.rs:39-42`) | **yes** |
| bootstrap snapshot | `herdr api snapshot` (`cli/api.rs:56-66`) | **yes** |
| tab label | `herdr tab get \| rename` | **yes** |
| **the event stream** | — | **NO. There is no `events` module in `src/cli/`** (`ls tmp/herdr/src/cli/` → agent, api, completion, integration, machine, notification, pane, plugin, protocol_guard, runtime, server, server_not_running, spec, status, tab, target, workspace, worktree). `events.subscribe` has no CLI wrapper at any version in this clone. |

That table is the honest scope of the fallback: every write path degrades, the **event consumer
genuinely requires the socket** and must report `Unavailable::NoSocket` rather than pretend.

### 6. What cyrup already has — do not rebuild

- `crates/cyrup-intercom/src/project_pane.rs` (870) — `HerdrLauncher` (`:389`), a working herdr CLI
  client: `run` (`:445`), `detect` (`:574`), `parse_herdr_version` (`:319`), `supports_raw_panes`
  (`:352`), `parse_last_json` (`:243`), `extract_pane_id` (`:273`), `shell_quote` (`:288`),
  `PaneErrorCode` (`:35`) with pi's five codes and its byte-identical `formatHerdrError` string
  (`:82-86`), `ENV_HERDR_BIN` (`identity.rs:60`). **`cyrup-herdr::cli` is this file's `run`/`detect`
  generalised to any argv — the error vocabulary and the sentences stay where they are.**
- `crates/cyrup-intercom/src/transport/stream.rs:36-130` — the boxed dual-transport pattern and the
  `ERROR_PIPE_BUSY` retry, to copy (§5).
- `crates/cyrup-intercom/src/broker/listener.rs` — the listener half, and the module doc explaining
  why an ungated unix type breaks the Windows build (the §5 argument-in-chief).
- `crates/cyrup-ext-subagents/src/tui/fleet.rs:1819-1840` — the `H` key already dispatches through a
  `has_inspect` seam and already renders `"Herdr inspector controls are unavailable in this context."`
  when it is false. **The seam exists; only the implementation behind it is missing.**
- `crates/cyrup-ext-subagents/src/tui/fleet_overlay.rs:276` — `"Failed to open Herdr inspector for
  async run {}."`, upstream's failure sentence, already ported.
- `crates/cyrup-ext-subagents/src/registration/authority.rs` — `inspectorOpen` / `projectOpen` are
  named at `:44` as deliberately omitted pending these verbs; the policy machinery is live.
- `crates/cyrup-ext-subagents/src/extension/tool/text.rs:384-385` — `"inspector.close"` and
  `"project.close"` already sit in the mutating-actions list.
- Integration-test precedent for a fake socket server:
  `crates/cyrup-it/tests/intercom/*.rs` and `cyrup-intercom/src/transport/client.rs:1467-1510`
  (`UnixListener::bind` in a tempdir, a task that reads a line and writes a line).
- **Not present, confirmed:** `grep -rn "HERDR_ENV\|HERDR_PANE_ID\|HERDR_SOCKET_PATH\|HERDR_TAB_ID\|
  HERDR_SESSION" --include=*.rs crates/` returns **nothing**. Env discovery is genuinely new.

### 7. Production call sites — every one of them

1. `crates/cyrup-ext-subagents/src/extension/host/native_impl.rs`, the `HostEvent::SessionStart` arm
   (around `:420-440`, in the block that already names *"after the herdr bridge"* at `:433`):
   `HerdrPane::discover(&std::env::vars())` → on `Some`, construct the client, `bootstrap`, and spawn
   the bridge task; on `None`, do nothing at all.
2. The same arm's shutdown counterpart — `pane.release_agent` on `SessionShutdown`.
3. `crates/cyrup-ext-subagents/src/extension/tool/routing.rs` — the `inspector.*` / `project.*` arms
   the VL-S6 batch adds; each calls `HerdrClient::call` with a `Method::Pane*`, falling back to
   `cyrup_herdr::cli` when `discover()` yielded no socket but `HERDR_BIN`/`herdr` resolves.
4. `crates/cyrup-ext-subagents/src/tui/fleet.rs:1819` — `has_inspect` becomes
   `herdr_client.is_some()`, and `H` dispatches through (3).
5. `crates/cyrup-intercom/src/project_pane.rs:604` `HerdrLauncher::open` — takes the socket path when
   `HerdrPane::discover` yields one, saving two process spawns per pane; keeps the CLI arm and every
   existing sentence unchanged.
6. The bridge's report path (`HERDR_STATUS_BRIDGE.md`'s batch): `pane.report_agent` on run-state
   edges, `pane.report_metadata` on label/token refresh, `agent.view.set` once at session start.

No `#[allow(dead_code)]`, no `todo!()`, and no method enum variant lands without a caller in this
list or in a sibling batch's list.

### 8. Reachability tests — each with the mutation that must turn it RED

All against a **fake herdr server**: a `tokio::net::UnixListener` in a tempdir that speaks the real
framing — read one `\n`-terminated line, parse as `Request`, write one `\n`-terminated line, close
(the exact `server.rs:156-317` behaviour). Unit tests in `crates/cyrup-herdr/src/`, integration tests
in `crates/cyrup-it/tests/subagents/herdr_client_integration.rs`.

1. **`ping_round_trips_and_exposes_version_protocol_capabilities`** — the fake asserts the received
   line is exactly `{"id":"…","method":"ping","params":{}}` and replies with a real `pong`.
   *Gut it:* drop `params` from the serialised request → the assertion on the received bytes fails,
   and a real herdr would answer `invalid_request` (`server.rs:198`). RED.
2. **`an_unknown_result_type_is_not_a_success`** — fake replies `{"id":"…","result":{"type":"martian"}}`.
   Must be `HerdrError::UnexpectedResult`, not `Ok`.
   *Gut it:* make the typed accessor return `Ok(Default::default())` for `Unrecognised`. RED.
3. **`an_error_envelope_carries_herdrs_own_code_and_message`** — fake replies
   `{"id":"…","error":{"code":"pane_not_found","message":"pane w1:p9 not found"}}`.
   Asserts `ApiErrorCode::PaneNotFound` **and** the message byte-for-byte.
   *Gut it:* fold the code into a generic `VALIDATION_ERROR` the way pi does. RED.
4. **`an_unknown_error_code_survives_as_other`** — `{"code":"brand_new_code"}` →
   `ApiErrorCode::Other("brand_new_code")` with the message intact.
   *Gut it:* map unknown codes to `Other("unknown")`. RED.
5. **`a_mismatched_response_id_is_refused`** — fake replies with `"id":"someone_else"`.
   *Gut it:* delete the `id` comparison. RED — the test asserts `IdMismatch`, and without the check
   the call returns the wrong pane's data as success.
6. **`bootstrap_opens_the_subscription_before_the_snapshot`** — the fake records the arrival order of
   its two connections and the method on each. Asserts connection #1 carried `events.subscribe`,
   that `subscription_started` was written before connection #2 was accepted, and that connection #2
   carried `session.snapshot`.
   *Gut it:* swap the two awaits in `bootstrap`. RED.
7. **`bootstrap_replays_events_that_arrived_during_the_snapshot`** — the fake pushes two events on
   the subscription connection *while* holding the snapshot reply, then answers. Assert the caller
   sees the snapshot first and then both events, in arrival order, none dropped.
   *Gut it:* drop the buffer and start reading only after the snapshot returns. RED — events lost.
8. **`a_lifecycle_event_uses_underscores_and_a_subscription_event_uses_dots`** — the fake pushes
   `{"event":"pane_created","data":{"type":"pane_created","pane":{…}}}` and
   `{"event":"pane.agent_status_changed","data":{…}}` on one connection; both must decode.
   *Gut it:* give `EventKind` explicit dot renames (the plausible mistake). RED on the first line.
9. **`the_subscription_ack_must_be_subscription_started`** — fake replies
   `{"id":"…","result":{"type":"pong"}}` to `events.subscribe`. Must be an error, not a live stream.
   *Gut it:* accept any success as the ack. RED. (pi pins the same thing, `herdr-connection.ts:100`.)
10. **`an_uncorrelated_invalid_request_fails_the_handshake`** — fake replies
    `{"id":"","error":{"code":"invalid_request","message":"…"}}`, the shape `server.rs:186-192`
    produces when it cannot recover an id. Must fail, not wait forever.
    *Gut it:* require `record.id == sent_id` before treating an error as fatal → the handshake hangs
    to its timeout. RED (the test has a 2 s bound).
11. **`a_server_that_never_answers_times_out`** — the fake accepts and reads but never writes.
    Asserts `HerdrError::Timeout` inside 2 s.
    *Gut it:* remove the `tokio::time::timeout`. RED by test-harness timeout.
12. **`a_reconnect_refetches_the_snapshot`** — the fake drops the subscription connection; assert
    `ReconnectingEvents` opens a **new** subscription *and* issues a second `session.snapshot`.
    *Gut it:* reconnect the stream without re-snapshotting. RED — the assertion counts
    `session.snapshot` arrivals.
13. **`reconnect_backoff_is_bounded_and_gives_up_loudly`** — a listener that accepts and immediately
    closes; assert attempts are spaced by the backoff schedule and that the consumer receives a
    terminal `Err`, not an infinite silent loop.
    *Gut it:* make backoff constant-zero. RED on the elapsed-time assertion.
14. **`report_agent_sends_exactly_herdrs_field_names`** — the fake asserts the received `params`
    object's keys are exactly `{pane_id, source, agent, state, message}` and that `state` is
    `"working"` (snake_case, `schema/common.rs:149-156`).
    *Gut it:* rename `state` → `status`, or serialise `PaneAgentState::Working` as `"Working"`. RED.
15. **`the_socket_path_resolution_order_is_herdrs_own`** — table test over the four rungs of §5.1,
    including `HERDR_SESSION=default` being ignored (`session.rs:99`).
    *Gut it:* drop the `!= "default"` filter. RED.
16. **`the_snapshot_request_is_accepted_over_a_million_byte_line`** / `…refused_over_four_mib` —
    pins both bounds.
    *Gut it:* remove the response bound. RED on the second.
17. **Production reachability, in `cyrup-it`:** `session_start_inside_a_fake_herdr_pane_reports_working` —
    drive `NativeExtension::on_event(SessionStart)` with `HERDR_ENV=1`, `HERDR_PANE_ID=w1:p1` and
    `HERDR_SOCKET_PATH` pointing at the fake; assert a `pane.report_agent` line arrives on the real
    socket. **This is the test that fails if the whole implementation is replaced by a stub**, because
    nothing is mocked between the host event and the socket.

### 9. Not installed, and not in a pane — the honest path

Three distinguishable states, and the port must keep them distinguishable:

| state | detection | what happens | test |
|---|---|---|---|
| **not in a herdr pane** (this container; CI; any plain terminal) | `HERDR_ENV != "1"` or `HERDR_PANE_ID` empty — `HerdrPane::discover` → `None` | **Nothing runs.** No `HerdrClient`, no connect, no tokio task, no thread, no log line above `trace`. `has_inspect` is `false` and `H` keeps rendering the existing sentence (`fleet.rs:1825`). | `no_herdr_env_means_no_client_and_no_task` — construct the extension with a scrubbed env, assert `discover()` is `None`, assert `tokio::runtime::Handle::metrics().num_alive_tasks()` is unchanged across `SessionStart`. *Gut it:* make `discover` fall through to the default socket path → a connect is attempted and the task count moves. RED. |
| **in a pane, herdr server gone** (herdr crashed; socket stale) | first call → `ECONNREFUSED`/`ENOENT` | `Unavailable::NoSocket{path, source}` surfaced to the caller with the path in the message. Bridge logs once at `warn` and stops; verbs report it; nothing retries in a tight loop. | `a_dead_socket_is_unavailable_not_a_hang` — bind then unlink the socket. *Gut it:* swallow the io error and return `Ok(())`. RED. |
| **herdr not installed at all** (no binary, no socket — this container) | `discover()` `None` **and** `HERDR_BIN`/`herdr` not on `PATH` | The CLI fallback answers `Unavailable::BinaryMissing` with pi's existing install hint, already in the tree: `"Herdr is not installed or is not on PATH. Install Herdr 0.7.5+ or set HERDR_BIN."` (`client.ts:55`, ported at `project_pane.rs:423`). | `an_absent_binary_is_unavailable_with_the_install_hint` **already exists and passes** — `project_pane.rs:795`. Extend it to the new crate. |

**The whole test suite runs green with neither herdr nor ghostty installed**, because every test drives
a fake `UnixListener` this workspace creates. No test may require the `herdr` binary; the one that
needs a binary's *absence* (`BinaryMissing`) sets `HERDR_BIN` to a path that does not exist, exactly as
`project_pane.rs:795` does today.

### 10. Files touched

New: `crates/cyrup-herdr/Cargo.toml`, `crates/cyrup-herdr/src/{lib,env,transport,error,client,reconnect,probe,cli}.rs`,
`crates/cyrup-herdr/src/schema/{mod,common,request,response,events,panes,agents,session,tabs}.rs`,
`crates/cyrup-herdr/src/tests/fake_server.rs`,
`crates/cyrup-it/tests/subagents/herdr_client_integration.rs`.
Edited: `Cargo.toml` (workspace members + the `cyrup-herdr` workspace dependency entry),
`crates/cyrup-ext-subagents/Cargo.toml`, `crates/cyrup-intercom/Cargo.toml`,
`crates/cyrup-ext-subagents/src/extension/host/native_impl.rs` (the `SessionStart`/`SessionShutdown` arms),
`crates/cyrup-ext-subagents/src/tui/fleet.rs` (`has_inspect`, and delta 2 in its module doc at `:58-60`
becomes false and must be **deleted**, not softened),
`crates/cyrup-intercom/src/project_pane.rs` (`HerdrLauncher` gains the socket route),
`crates/cyrup-it/tests/subagents/main.rs` (register the new module),
`docs/gap-analysis/00-residual-ledger.md` + `PARITY-GAPS.md` (the herdr rows).

### 11. Sizing, and the order to land it

**Honest size: large — roughly 2 600–3 200 lines of new Rust, of which ~900 are the schema types and
~1 100 are tests.** Not hard; wide. The protocol is fully specified, herdr's own client is 291 lines,
and this workspace already owns every hard part (dual transport, pipe-busy retry, fake-socket test
harness, the CLI client, the error vocabulary). What makes it big is the honest count of
request/response types, not any unsolved problem.

Land it in five commits, in this order, each independently green:

1. **The crate skeleton + transport + `ping`.** `Cargo.toml`, `env.rs` (§5.1 resolution, the discovery
   gate), `transport.rs`, `error.rs`, the `Request`/`SuccessResponse`/`ErrorResponse`/`ErrorBody`
   envelope, `Method::Ping`, `probe.rs`. Tests 1, 3, 4, 5, 11, 15, and the not-in-a-pane test.
   ~600 lines. **Everything else depends on this and nothing depends on the rest.**
2. **The write half.** `pane.report_agent` / `report_agent_session` / `report_metadata` /
   `release_agent` / `clear_agent_authority`, `tab.get` / `tab.rename`, plus `cli.rs`. Tests 2, 14, 16.
   ~450 lines. Unblocks `HERDR_STATUS_BRIDGE.md` entirely.
3. **The read half.** `session.snapshot` and its whole `SessionSnapshot` / `PaneInfo` / `AgentInfo` /
   `TabInfo` / `WorkspaceInfo` / `PaneLayoutSnapshot` type closure, `pane.get/list/current`,
   `agent.list/get`. ~800 lines, mostly `#[derive]`. Unblocks FleetView's herdr roster.
4. **The stream.** `events.subscribe`, `Event`, `bootstrap`, `reconnect.rs`. Tests 6, 7, 8, 9, 10, 12, 13.
   ~550 lines. **This is the one with the subtle semantics** (§2's two naming conventions, the
   buffered bootstrap, the 100 ms tick) and it must not be merged into step 3.
5. **The production wiring + `agent.view.set`.** The `native_impl.rs` arms, `fleet.rs`'s `has_inspect`,
   `project_pane.rs`'s socket route, the `cyrup-it` reachability test 17, and the doc deletions.
   ~400 lines. **Nothing before this commit is reachable from production, so nothing before it is done.**

`INSPECTOR_AND_PROJECT_PANES.md` (VL-S6) needs steps 1–3 and can start against step 2's `cli.rs`
immediately. `HERDR_STATUS_BRIDGE.md` needs 1, 2 and 4. Neither can close without 5.

**One thing neither this batch nor its two siblings covers, recorded so it is not lost:**
pi's `runs/shared/herdr-{connection,machine,placed-run,external-adapters}.ts` (845 lines) place
subagent runs in herdr panes on **remote machines** over SSH StreamLocal forwarding
(`herdr-connection.ts:119-134`). That is a real upstream feature with no cyrup row. It is **big, not
out** — it sits directly on top of this crate's `HerdrClient` (pi's own version is
`new SocketRpcClient(forwardedSocketPath)`), so landing it later costs a forwarder and an adapter,
not a second client.

---

## [EXEC — foundation]

**PR1 step 1 of 4: the `cyrup-herdr` crate, the transport, discovery, errors, the probe, and the
fake server.** AUG §11 commit 1. Green: `10771 tests run: 10771 passed, 9 skipped`
(baseline 10748 / 9 — the delta is exactly the 23 new tests, no regressions).

### What landed

New leaf crate `crates/cyrup-herdr` (16 files, no `cyrup-*` dependency — `tokio`, `serde`,
`serde_json`, `thiserror`, `tracing` only), registered in the root `Cargo.toml` as a workspace
member, a `default-members` entry and a `workspace.dependencies` entry.

```
crates/cyrup-herdr/
  Cargo.toml                   # the §5 leaf argument, stated on the [dependencies] table
  src/lib.rs                   # the protocol in one place + the three-states table
  src/env.rs                   # EnvSource/ProcessEnv, HerdrPane::{discover,require}, resolve_socket_path
  src/error.rs                 # HerdrError (9 arms), ApiError, ApiErrorCode (25 + Other), Unavailable
  src/transport.rs             # LocalStream (UnixStream | named pipe), NDJSON framing, bounds, request()
  src/probe.rs                 # Pong, Capability, ping / ping_for / ping_current_pane
  src/schema/{mod,common,request,response,server}.rs
  src/tests/{mod,fake_server,discovery,socket_path,probe_round_trip,transport_bounds}.rs
```

Per the AUG's mirroring rule, each `schema/*.rs` carries herdr's own file name: `common.rs` ←
`schema/common.rs`, `server.rs` ← `schema/server.rs` (`PingParams` **and** `ServerCapabilities` both
live there in herdr, not in `common`), `response.rs` ← `schema/response.rs`, and `request.rs` ← the
top level of `schema.rs`, where herdr keeps `Request`/`Method`.

### Anchors re-verified against `tmp/herdr` @ `d59d060`, not taken from the AUG

| claim | anchor | verdict |
|---|---|---|
| socket ladder | `session.rs:173-181` → `:169-171` → `:161-167` → `config/io.rs:30-35` | **confirmed** — `data_dir_for` is at `:161`, `api_socket_path_for` at `:169`, `active_api_socket_path` at `:173` |
| 1 MiB request bound | `api/server.rs:32` `MAX_INITIAL_REQUEST_BYTES` | **confirmed**, with `:30` `INITIAL_REQUEST_TIMEOUT` = 5 s and `:31` `STREAM_WRITE_TIMEOUT` = 5 s |
| 32 MiB is the other protocol | `protocol/wire.rs:29` `MAX_GRAPHICS_FRAME_SIZE`, `PROTOCOL_VERSION = 22` at `:20` | **confirmed** — `wire.rs:1-8`'s own module doc scopes it to the binary endpoint |
| one request per connection | `api/server.rs:156-317`, no read loop; `api/client.rs:55-61` connects per request | **confirmed** |
| `params` mandatory on `ping` | `schema.rs:35-47` adjacent tagging; `PingParams` at `schema/server.rs:3-4` | **confirmed** |
| `error.code` is a bare `String` | `schema/response.rs:35-39` | **confirmed** |
| the Windows pipe name | `ipc.rs:44-51` `GenericNamespaced` over the stringified path; `client.py:410-419` spells the `\\.\pipe\` prefix | **confirmed** |
| the five injected env vars | `pane.rs:156,166-168`, `integration/env.rs:8-10,29`, `socket-api.mdx:302-305` | **confirmed** |

**One anchor the AUG did not have, and it changes the gate.** `tmp/herdr/src/pane.rs:170-172`:
a `PaneLaunchIdentity::OmitPane` launch **removes** `HERDR_PANE_ID` while leaving `HERDR_ENV=1` and
`HERDR_SOCKET_PATH` in place. So a process can sit inside herdr, see a live socket, and own no pane.
The AUG's two-part gate is therefore not belt-and-braces, it is load-bearing, and
`herdr_env_without_a_pane_id_is_not_a_pane` pins it.

**A second correction, made in the port rather than inherited.** herdr's `app_dir_name`
(`config/io.rs:22-28`) answers `"herdr-dev"` under `cfg!(debug_assertions)` — but that reads
*herdr's* build profile, and this code compiles into *cyrup*. Deriving it here would make a debug
cyrup look in `~/.config/herdr-dev` while the user's released herdr listens on `~/.config/herdr`:
a client that works in release and silently finds nothing in debug. `HERDR_APP_DIR` is the literal
`"herdr"`, and `the_config_directory_is_herdrs_release_name_whatever_cyrups_build_profile_is` runs
under `cargo test` — i.e. with `debug_assertions` on — so it is exactly the configuration that would
break.

### The pin: not-in-a-pane means no client, no connect, no task

`no_herdr_env_means_no_client_no_connect_and_no_task` builds a **hostile** environment — a fake
herdr server is listening and its path is in `HERDR_SOCKET_PATH` — with `HERDR_ENV` absent, which is
the state of this container, of CI, and of any plain terminal. Three assertions:

1. `HerdrPane::discover` → `None`.
2. **`fake.received()` is empty** — no connection was opened. This is the assertion that fails if
   discovery falls through to a default socket path, and mutation **M7** (delete the `HERDR_ENV`
   gate) turns it RED.
3. `Handle::current().metrics().num_alive_tasks()` is unchanged across the sequence.

`resolve_socket_path` is kept *separate* from `discover` for this reason: the ladder is a deliberate
escape hatch for a caller that was asked for herdr by name, never an ambient fallthrough.
`HerdrPane::require` is the asked-for entry and produces `Unavailable::NotInHerdrPane`; `discover`
is the ambient one and produces `None`.

### Degradation, as implemented

| state | behaviour | pinned by |
|---|---|---|
| not in a pane | `discover` → `None`; nothing runs | `no_herdr_env_means_no_client_no_connect_and_no_task` |
| socket absent / `ENOENT` / `ECONNREFUSED` | `Unavailable::NoSocket{path, source}`, path in the sentence | `a_dead_socket_is_unavailable_not_a_hang` |
| no reply | `HerdrError::Timeout{method,timeout}` (default 15 s; there is **no** herdr-side deadline, `server.rs:911-913`) | `a_server_that_never_answers_times_out` |
| success `id` ≠ request `id` | `HerdrError::IdMismatch`, connection dropped | `a_mismatched_response_id_is_refused` |
| error envelope, **any** `id` including `""` | fatal at once, never a wait | `an_uncorrelated_invalid_request_is_fatal_not_a_wait` |
| method this build lacks | `invalid_request` → `HerdrError::is_unsupported_method()` → that one feature off | `invalid_request_is_the_one_code_that_means_unsupported_method` |
| unknown `result.type` | `ResponseResult::Unrecognised` decodes, accessor returns `UnexpectedResult` | `an_unknown_result_type_is_not_a_success` |
| unknown error code | `ApiErrorCode::Other("<exact spelling>")`, message verbatim | `an_unknown_error_code_survives_as_other` |

**Gated on capability, never on a version string.** `probe.rs`'s module doc states both traps:
`protocol` is `PROTOCOL_VERSION` and guards the **binary** endpoint (`wire.rs:6-8`), so this client
records it and does not gate on it; and `version` is not a capability list either, because herdr's
stability rule is per method (`socket-api.mdx:951-959`). herdr's *CLI* does refuse any protocol
difference (`cli.rs:786-801` → `cli/protocol_guard.rs:16-43`), but that is a herdr build talking to
a herdr build; reproducing it here would disable working JSON calls for a reason that does not apply.
`Pong::supports(Capability)` answers `false` for an absent capability block — never `true`
(`an_absent_capability_block_declares_nothing`).

### Deliberately NOT shipped in this step, each with a true reason

- **`Unavailable::BinaryMissing` and `Unavailable::ProtocolMismatch`.** Both belong to the enum;
  neither has a constructor until `cli.rs` lands (step 2), and this crate ships nothing it cannot
  reach. `protocol_mismatch` is CLI-only — `api/server.rs` never produces it — so a socket client
  only meets it as text on a shelled-out herdr's stderr. `#[non_exhaustive]` makes adding them
  additive. The reason is written into `error.rs` beside the enum.
- **`ResponseResult::Ok {}`** and every method variant but `Ping`. The AUG's rule is that no variant
  lands without a caller; steps 2–4 add theirs.
- **The named-pipe arm is compiled but not exercised.** `tokio::net::UnixListener` has no Windows
  twin, so `src/tests/fake_server.rs` is `#[cfg(unix)]`. The `#[cfg(windows)]` `connect_local` is
  type-checked on Windows targets only. The prefix bug the AUG warned about is guarded by a comment
  citing both independent sources (`ipc.rs:44-51`, `client.py:410-419`) and by *not* reusing
  `cyrup-intercom`'s `BrokerStream`, whose paths are already pipe-form.
- **Production reachability across crates.** Per AUG §11, nothing before commit 5 is reachable from
  a cyrup production caller; this crate's own public API (`ping`, `ping_current_pane`,
  `HerdrPane::discover`/`require`, `resolve_socket_path`, `transport::request`) is driven end to end
  by the tests against a real socket, with no mock between the entry point and the wire.

### Mutations run, each RED, each restored byte-for-byte

22 gutting mutations, one per behaviour, covering all 23 tests. Every one compiled alone (the
restore touches every file — `cp -a` preserves the pristine mtimes, which are older than the last
build's output, and cargo would otherwise skip the rebuild and measure the next mutation against a
binary still carrying the previous one). `sha256sum -c` over all 16 files after the sweep: 0
mismatches.

| # | gut | test that went RED |
|---|---|---|
| M1 | `Method`: `content = "params"` dropped | `ping_round_trips_and_exposes_version_protocol_capabilities` |
| M2 | `pong()` returns a defaulted `Pong` for `Unrecognised` | `an_unknown_result_type_is_not_a_success` |
| M3a | `from_wire` loses the `pane_not_found` arm | `an_error_envelope_carries_herdrs_own_code_and_message` |
| M3b | unknown codes → `Other("unknown")` | `an_unknown_error_code_survives_as_other` |
| M4 | the response-`id` comparison deleted | `a_mismatched_response_id_is_refused` |
| M5 | an error envelope must correlate before it is fatal | `an_uncorrelated_invalid_request_is_fatal_not_a_wait` |
| M6 | the caller's deadline ignored | `a_server_that_never_answers_times_out` |
| M7 | the `HERDR_ENV` gate deleted | `no_herdr_env_means_no_client_no_connect_and_no_task`, `a_pane_id_without_herdr_env_is_not_a_pane` |
| M8 | empty `HERDR_PANE_ID` accepted | `herdr_env_without_a_pane_id_is_not_a_pane` |
| M9 | the `!= "default"` filter dropped | `the_socket_path_resolution_order_is_herdrs_own` |
| M10 | the `validate_name` filter dropped | `the_socket_path_resolution_order_is_herdrs_own` |
| M11 | `HERDR_APP_DIR` derived from `cfg!(debug_assertions)` | `the_config_directory_is_herdrs_release_name_…`, `the_socket_path_resolution_order_is_herdrs_own` |
| M12 | rung 2 (`HERDR_SOCKET_PATH`) removed | `the_socket_path_resolution_order_is_herdrs_own`, `a_herdr_pane_exposes_every_injected_identifier`, `inside_a_pane_the_probe_reaches_the_socket` |
| M13 | the 1 MiB request bound removed | `a_request_line_over_one_mib_is_refused_before_it_is_written` |
| M14 | the response bound set to the request bound | `a_response_line_over_four_mib_is_refused`, `a_large_but_bounded_response_still_round_trips` |
| M15 | EOF returns an empty line instead of `Closed` | `a_server_that_closes_without_answering_is_closed_not_timeout` |
| M16 | an absent capability block reads as `true` | `an_absent_capability_block_declares_nothing` |
| M17 | `is_unsupported_method` true for every api error | `invalid_request_is_the_one_code_that_means_unsupported_method` |
| M18 | a connect failure flattened to `Io`, losing the path | `a_dead_socket_is_unavailable_not_a_hang` |
| M19 | `require` returns a generic io error | `require_names_the_reason_outside_a_pane` |
| M20 | an undecodable line → `Ok(Unrecognised)` | `a_line_that_is_not_a_response_is_malformed` |
| M21 | `tab_id` always `None` | `a_herdr_pane_exposes_every_injected_identifier` |

M14 is a value mutation rather than a deletion on purpose: the test feeds a newline-less stream, so
deleting the bound outright is an unbounded allocation on the test host, not an observation.

### Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` — clean.
- `cargo nextest run --workspace --features test-fixtures` — **`Summary [111.435s] 10771 tests run:
  10771 passed, 9 skipped`**.

### Next

Step 2 (the write half + `cli.rs`) can start against `transport::request`; it adds
`ResponseResult::Ok`, the five `pane.report_*` / `release_agent` / `clear_agent_authority` variants,
`tab.get`/`tab.rename`, and the two deferred `Unavailable` arms. Steps 3 and 4 add the read half and
the stream; the `cyrup-intercom` `HerdrLauncher` migration and the `cyrup-ext-subagents` production
arms land with them, so that this batch leaves **one** herdr client in the workspace, not two.

---

## [EXEC — typed]

**PR1 step 2 of 4: the typed method surface — schema, one-shot client, write half and read half.**
AUG §11 commits 2 and 3, landed together because the read half's `ResponseResult` variants and the
write half's `Method` variants are the same two enums. Green:
`Summary [99.271s] 10807 tests run: 10807 passed, 9 skipped` (step 1 left 10771 / 9 — the delta is
exactly the 36 new tests, no regressions).

### What landed

```
crates/cyrup-herdr/src/
  client.rs                    # HerdrClient: 21 methods, id correlation, 15 s default, WAIT_GRACE
  schema/common.rs             # + PaneTarget TabTarget AgentTarget SplitDirection ReadSource
                               #   ReadFormat PaneAgentState AgentStatus
  schema/panes.rs              # NEW — 18 types
  schema/agents.rs             # NEW — AgentInfo, AgentSessionInfo, AgentSessionRefKind
  schema/tabs.rs               # NEW — TabRenameParams, TabInfo
  schema/session.rs            # NEW — SessionSnapshot
  schema/workspaces.rs         # NEW — WorkspaceInfo, WorkspaceWorktreeInfo
  schema/events.rs             # NEW — PaneWaitForOutputParams, OutputMatch
  schema/request.rs            # Method: 1 variant → 21, in herdr's own declaration order
  schema/response.rs           # ResponseResult: 2 variants → 13, + 11 typed accessors
  error.rs                     # ApiErrorCode: 25 named → 31
  tests/{method_names,write_half,read_half,client_contract}.rs   # NEW — 36 tests
```

`workspaces.rs` is not in the AUG §5 file list, and it is here anyway: `SessionSnapshot.workspaces`
is `Vec<WorkspaceInfo>` (`tmp/herdr/src/api/schema/session.rs:18`), and herdr keeps that record in
`schema/workspaces.rs`. Folding it into `session.rs` would break the one rule that makes this
directory worth its shape — **same file names, so a diff against a future pin is line-for-line**.
None of `workspace.*`'s nine methods is ported; only the record.

### Types mirrored, each against the herdr line it came from

Checked twice: against `tmp/herdr/src/api/schema/*.rs` at `d59d060`, and against the checked-in
`docs/next/api/herdr-api.schema.json` (`request.$defs` / `success_response.$defs`), which is the
artifact herdr's own CI fails on if it drifts (`src/api/schema/tests.rs:182-207`).

| this crate | herdr |
|---|---|
| `common::{PaneTarget,TabTarget,AgentTarget}` | `schema/common.rs:33-36`, `:49-52`, `:54-57` |
| `common::{SplitDirection,ReadSource,ReadFormat}` | `:70-75`, `:77-84`, `:93-101` |
| `common::PaneAgentState` (4) · `common::AgentStatus` (5) | `:149-156` · `:158-166` |
| `panes::{PaneRightClickTarget,PaneSplitParams}` | `schema/panes.rs:16-24`, `:26-43` |
| `panes::{PaneProcessInfoParams,PaneListParams,PaneCurrentParams}` | `:141-146`, `:314-319`, `:320-326` |
| `panes::{PaneSendInputParams,PaneReadParams}` | `:345-354`, `:355-369` |
| `panes::PaneReportAgentParams` · `PaneReportAgentSessionParams` | `:447-462` · `:464-477` |
| `panes::PaneReportMetadataParams` · `PaneClearAgentAuthorityParams` · `PaneReleaseAgentParams` | `:479-507` · `:509-516` · `:518-525` |
| `panes::{PaneInfo,PaneScrollInfo}` | `:527-561`, `:563-568` |
| `panes::{PaneProcessInfo,PaneProcessInfoProcess}` | `:570-581`, `:583-595` |
| `panes::{PaneLayoutSnapshot,PaneLayoutRect,PaneLayoutPane,PaneLayoutSplit}` | `:669-678`, `:680-686`, `:688-693`, `:695-701` |
| `panes::PaneReadResult` | `:755-765` |
| `agents::AgentInfo` · `AgentSessionInfo` | `schema/agents.rs:186-226` · `:228-234` |
| `agents::AgentSessionRefKind` | **`src/agent_resume.rs:14-19`** — outside `api/schema/`, pulled in by a fully qualified path at `agents.rs:232` |
| `tabs::TabRenameParams` · `TabInfo` | `schema/tabs.rs:27-31` · `:39-48` |
| `session::SessionSnapshot` | `schema/session.rs:8-23` |
| `workspaces::WorkspaceInfo` · `WorkspaceWorktreeInfo` | `schema/workspaces.rs:61-76` · `:78-85` |
| `events::PaneWaitForOutputParams` · `OutputMatch` | `schema/events.rs:94-105` · `:107-112` |
| `request::Method` (21 of 105) | `schema.rs:47-271`, same order |
| `response::ResponseResult` (12 of 65, + `Unrecognised`) | `schema/response.rs:41-308`, same order |

**Two deliberate divergences, both stated in the source:**

1. **`BTreeMap` where herdr writes `HashMap`** (`env`, `tokens`, `state_labels`). Not a field
   spelling and not a wire shape — a JSON object is a JSON object — but it makes the bytes this
   client *writes* deterministic, so a request can be asserted byte-for-byte and read in a log
   without its keys shuffling per process. herdr sorts the one map whose order it could have cared
   about (`normalize_launch_env`, `src/app/api/env.rs:30`) and reads every other one into an
   unordered map, so nothing upstream observes it. Stated in `schema/mod.rs`.
2. **`PaneReadParams` has five fields, not six.** herdr's sixth, `intent: ReadIntent`, carries
   `#[serde(skip)]` and `#[schemars(skip)]` (`panes.rs:366-368`): it is how herdr's in-process
   `pane.wait_for_output` loop marks its own reads passive, and it never reaches the wire.

### Anchors the AUG did not have, found by reading herdr rather than the AUG

1. **`pane.split` answers `PaneInfo` — the NEW pane** (`src/app/api/panes.rs:124-132`,
   `encode_success(id, ResponseResult::PaneInfo { pane })` after `self.pane_info(ws_idx,
   new_pane.pane_id)`). Not a `PaneSplit`-shaped result; there is none.
2. **`tab.rename` answers `TabInfo`, not `Ok`** (`src/app/api/tabs.rs:169-171`). So the
   pane-owned-tab-label pattern the Python client implements (`client.py:423-468`) is two calls,
   not three. Pinned by `tab_get_and_tab_rename_both_answer_a_tab_record`, and the other direction —
   an `ok` must **not** be upgraded into a fabricated record — by
   `an_acknowledgement_is_not_a_tab_record`.
3. **`PaneAgentState` has four values and `AgentStatus` has five.** The AUG's step brief named
   `AgentStatus = Idle|Working|Blocked|Done|Unknown` for `pane.report_agent`; that is the wrong
   type. `pane.report_agent.state` is `PaneAgentState` (`schema/panes.rs:451`), which is
   `Idle|Working|Blocked|Unknown` (`common.rs:149-156`) — **no `Done`**. `Done` exists only on the
   *observed* status herdr publishes (`common.rs:158-166`). Sending `"done"` would be
   `invalid_request`. Both directions pinned by
   `the_reported_state_has_four_values_and_the_published_status_has_five`.
4. **`pane.current`'s param is `caller_pane_id`, not `pane_id`** (`panes.rs:320-326`). It means
   *the pane this process is in* — which is what `HERDR_PANE_ID` holds. Unset, herdr answers
   whichever pane the **user** focused (`src/app/api/panes.rs:145-149`): a different question, and
   the wrong one for a background bridge reporting on itself.
5. **`pane.wait_for_output` with no `timeout_ms` waits forever.** The deadline is built with
   `params.timeout_ms.map(…)` (`src/api/wait.rs:30-32`) and every later check is
   `deadline.is_some_and(…)` (`:113`). `None` is not "herdr's default"; it is no deadline at all,
   on top of the no-deadline-on-dispatch this crate already documents.
6. **`revision` is hard-coded `0` on EVERY pane-read-derived payload**, not only `pane.read`.
   The literal is `src/app/api/panes.rs:1540`, in the one `PaneRead` handler, and every other
   producer routes through it: `pane.wait_for_output` copies the field it just read
   (`let revision = read.revision;`, `src/api/wait.rs:98`), the `pane.output_matched` subscription
   builds its event from the same helper (`src/api/subscriptions.rs:295-320` → `:493-515`), and
   the headless server's frozen-alt-screen override touches only `text` and `truncated`
   (`src/server/headless.rs:3141-3147`). The earlier claim that `output_matched` carried "a real
   revision" was FALSE in both directions and is deleted: a consumer that took `matched.revision`
   as an `EventMatch` floor or a cache key would floor at `0` and never invalidate — the exact
   silent no-op, mis-attributed to the other call. See `[FIX — round 1]`.
7. **Six error codes this batch newly made reachable**, each from the handler that produces it:
   `invalid_key` (`panes.rs:1852` — and **nothing is written**, herdr encodes the whole input
   first), `invalid_env` (`api/env.rs:3-31`), `confirmation_required` (`panes.rs:1880-1886` — the
   pane is **still open**), `invalid_regex` (`wait.rs:38-48` — refused before the first read),
   `agent_not_found` and `agent_target_ambiguous` (`app/agents.rs:298-322`). Named variants rather
   than `Other`, because a consumer branches on them.

### `[CYRUP-EXCEEDS-UPSTREAM]` — the answer envelope is not decoded untagged

*Premise, grepped:* herdr's own client decodes the answer with `#[serde(untagged)]`
(`src/api/client.rs:231-236`), and serde discards each arm's real error before reporting an
untagged failure. Every decode failure therefore reads `"data did not match any variant of untagged
enum WireResponse"` at `line: 0, column: 0` with no `source` — identically for a truncated line, a
corrupt line, a line from another protocol, and **a perfectly good response carrying one field this
client has not caught up with**. That last case is the one this crate exists to not have: a herdr
upgrade would look exactly like corruption.

`WireResponse::decode` dispatches on the key that actually discriminates — herdr writes `error` or
`result`, never both (`api/server.rs:180-201` versus `:301`) — and then decodes one arm, so serde's
own message survives and names the field: ``missing field `label` at line 1 column 84``. Pinned by
`a_missing_field_is_named_in_the_error_not_swallowed`, whose gutting mutation (**N30**) is the
untagged decoder restored.

### Deliberately NOT ported in this step, each with a true reason

Written into the crate doc as a table, not just here. `size` is not among them.

- **`events.subscribe` · `events.wait` · `agent.prompt` · `agent.wait`** — these hold a connection
  open past the first answer (`api/server.rs:229-288`, `:715-779`), so they are the *stream* half's
  shape, not this one's. `events.subscribe` lands with the bridge's event consumer (step 4); the
  other three land when cyrup drives a **sibling** agent in another pane, which no batch does yet.
- **`agent.view.set` / `agent.view.clear`** — the sidebar projection. Real, and above pi — but it
  projects the fleet roster, and the roster is what `agent.list` in *this* step first makes
  available. Step 5.
- **`pane.graphics.*` (4 — `info`, `set`, `clear`, `stream`) + the 4 `#[serde(skip)]` frame variants** — the wire contract is a
  raw-byte side channel after the JSON header (`socket-api.mdx:204-236`) and **cyrup has no image
  producer**, so the methods would have no argument to carry.
- **`plugin.*` (11) · `integration.*` (3)** — require shipping a `herdr-plugin.toml` package
  (`socket-api.mdx:499-560`): a distinct deliverable with its own install story.
- **`worktree.*` (4)** — herdr worktrees create herdr *workspaces*. cyrup owns its worktrees through
  `gix` (`crates/cyrup-ext-subagents/src/spawn/worktree.rs`); adopting herdr's would move ownership
  of a feature that already works. The **record** is decoded anyway (`WorkspaceWorktreeInfo`),
  because a snapshot carries it and reconciling a fleet view against the user's terminal wants it.
- **Pane geometry** — `swap` `move` `zoom` `resize` `neighbor` `edges` `focus_direction` `scroll`
  `copy_motion` `copy_search` `selection.read` `edit_scrollback` `clear` `input.set` `link.*`
  `layout` + `layout.*` (3): **no consumer arranges the user's terminal.** Every batch in flight
  splits, reads, writes and closes; none moves panes around. `PaneLayoutSnapshot` *is* mirrored,
  because `session.snapshot` carries one per tab.
- **`server.stop` · `server.live_handoff`** — destroy or restart the user's whole terminal session.
- **`workspace.*` (9) · `tab.{create,list,focus,move,close}` ·
  `agent.{read,explain,send_keys,rename,focus,start}` · `command.invoke` · `popup.close` ·
  `notification.show` · `client.window_title.*` · `product_announcement.dismiss` ·
  `release_notes.dismiss` · `server.reload_config` · `server.{agent_manifests,reload_agent_manifests}`**
  — no caller yet.
- **`client_shell.surface.set`** — **cannot be ported**: refused on the raw socket by construction
  (`api/server.rs:370-376`, `connection_local_only`). Absent, not deferred.
- **`Unavailable::{BinaryMissing,ProtocolMismatch}`** — still unreachable; `cli.rs` is the AUG's
  step-2 companion and did not land in this step. The reason stays written beside the enum in
  `error.rs`, and `#[non_exhaustive]` keeps adding them additive.

### One design decision recorded rather than left implicit

**`AgentStatus` is closed, unlike `ResponseResult`.** A sixth status from a newer herdr fails the
line instead of folding into `Unknown`. `#[serde(other)]` is only available on an internally or
adjacently tagged enum, so a bare string enum has no forward-compatible arm to offer; and herdr's
own type is closed, with the published artifact pinning exactly five
(`success_response.$defs.AgentStatus.enum`) and its CI failing on drift. So a sixth status is a
protocol change herdr flags at its own gate, and the remedy here is the documented one: bump the
pin and re-read that file. A loud failure is honest; reporting a new status as `Unknown` would be a
fabricated reading. Stated on the type.

### Mutations run, each RED, each restored byte-for-byte

38 gutting mutations over 36 new tests. Each was applied to a pristine copy, compiled, run, and
restored from that copy; `sha256sum -c` over all 27 files after the sweep: **0 mismatches**.

| # | gut | test that went RED |
|---|---|---|
| N1 | `#[serde(rename="pane.report_agent")]` → `"pane.reportAgent"` | `every_method_serialises_under_herdrs_own_name` |
| N2 | `Method::name` drifts from the rename on `pane.wait_for_output` | `every_method_serialises_under_herdrs_own_name` |
| N3 | `PaneAgentState` loses `rename_all = "snake_case"` | `the_reported_state_has_four_values…`, `report_agent_sends_exactly_herdrs_field_names` |
| N4 | `pane.report_agent`'s `state` renamed to `status` | `report_agent_sends_exactly_herdrs_field_names` |
| N5 | `message` nulled instead of omitted | `report_agent_sends_exactly_herdrs_field_names` |
| N6 | `report_metadata` drops the `None` token entries | `report_metadata_sends_the_token_patch_with_null_for_a_clear` |
| N7 | `clear_agent_authority` sends `source: null` | `release_names_its_agent_and_a_bare_clear_omits_its_source` |
| N8 | `ok()` accepts `Unrecognised` | `only_an_ok_is_a_successful_write` |
| N9 | `report_agent` swallows herdr's error and reports success | `a_refused_write_carries_herdrs_own_code_and_message` |
| N10 | `tab()` fabricates a record from an acknowledgement | `an_acknowledgement_is_not_a_tab_record` |
| N11 | `PaneInfo.agent_session` renamed to `session` | `the_whole_snapshot_record_closure_decodes` |
| N12 | `AgentSessionRefKind` loses `rename_all` | `the_whole_snapshot_record_closure_decodes` |
| N13 | `WorkspaceInfo.active_tab_id` renamed to `tab_id` | `the_whole_snapshot_record_closure_decodes` |
| N14 | `PaneLayoutSplit.direction` renamed and defaulted | `the_whole_snapshot_record_closure_decodes` |
| N15 | `pane.split` stops sending `ratio` | `pane_split_sends_env_and_ratio_and_answers_the_new_pane` |
| N16 | `pane.split` stops sending `env` | `pane_split_sends_env_and_ratio_and_answers_the_new_pane` |
| N17 | `pane()` accepts a `pane_current` | `a_pane_current_is_not_a_pane_info_and_the_reverse` |
| N18 | `pane.current` sends `pane_id` instead of `caller_pane_id` | `the_three_pane_targeting_shapes_each_send_their_own_key` |
| N19 | `AgentTarget.target` renamed to `agent_id` | `agent_list_and_agent_get_keep_herdrs_ambiguity_message_intact` |
| N20 | `agent_target_ambiguous` folded onto a generic code | that test + `the_codes_this_batch_made_reachable_are_named_not_other` |
| N21 | `PaneSendInputParams::run` stops appending `Enter` | `pane_send_input_is_herdr_pane_run_and_close_acknowledges` |
| N22 | `send_input` sends `"keys":[]` | `pane_send_input_is_herdr_pane_run_and_close_acknowledges` |
| N23 | `pane.read` omits `strip_ansi=true`, leaving the default to the server | `pane_read_answers_herdrs_own_zero_revision` |
| N24 | `ReadSource::RecentUnwrapped` gets a camelCase rename | `the_read_vocabulary_round_trips_under_snake_case`, `pane_read_answers_herdrs_own_zero_revision` |
| N25 | `PaneProcessInfo.foreground_processes` loses `#[serde(default)]` | `pane_process_info_decodes_both_a_full_and_an_empty_answer` |
| N26 | `pane.wait_for_output` inherits the per-call default deadline | `a_wait_deadline_is_the_herdr_wait_plus_the_poll_grace` |
| N27 | a wait with no herdr deadline gets a fixed 30 s client deadline | `a_wait_with_no_herdr_deadline_still_ends` |
| N28 | the wait's `match` renamed to `match_event` | `a_wait_deadline_is_the_herdr_wait_plus_the_poll_grace` |
| N29 | `OutputMatch` loses its internal tag | `a_wait_deadline_is_the_herdr_wait_plus_the_poll_grace` |
| N30 | the answer envelope goes back to an untagged decode | `a_missing_field_is_named_in_the_error_not_swallowed` |
| N31 | `DEFAULT_TIMEOUT` drops to 5 s | `the_default_request_deadline_is_fifteen_seconds` |
| N32 | `with_timeout` is a no-op | that test + `a_client_deadline_bounds_every_call` |
| N33 | `for_pane` resolves the ambient socket ladder | `for_pane_talks_to_the_panes_own_socket` |
| N34 | every request reuses one correlation id | `every_call_carries_its_own_correlation_id`, `a_typed_call_refuses_a_mismatched_id` |
| N35 | `new()` warms the connection with a background `ping` | `constructing_a_client_opens_nothing` |
| N36 | `TabInfo` gains `deny_unknown_fields` | `an_unknown_field_on_a_record_is_ignored_not_an_error` |
| N37 | `AgentStatus` switches to PascalCase | `the_reported_state_has_four_values…`, `the_whole_snapshot_record_closure_decodes` |
| N38 | `Method` drops `content = "params"` | `an_empty_params_method_still_sends_an_empty_object`, `every_method_serialises_under_herdrs_own_name` |
| N39 | the response-`id` comparison deleted | `a_typed_call_refuses_a_mismatched_id` |
| N40 | `ResponseResult` loses its `#[serde(other)]` arm | `call_hands_back_the_raw_result_including_an_unrecognised_one` |
| N41 | `HerdrClient::ping` probes the ambient ladder, not the client's socket | `ping_through_the_client_uses_the_clients_socket_and_deadline` |
| N42 | `invalid_key` folded onto `Other` | `the_codes_this_batch_made_reachable_are_named_not_other` |
| N43 | `agent_session_path` renamed to `session_path` | `report_agent_carries_the_session_reference_and_the_ordering_token` |
| N44 | `session_start_source` renamed to `start_source` | `report_agent_session_carries_no_state_and_names_its_start_source` |
| N45 | `tab_rename` dispatches `tab.get` | `tab_get_and_tab_rename_both_answer_a_tab_record` |
| N46 | `call_for` ignores the deadline it was given | `call_for_overrides_the_clients_deadline_for_one_call` |
| N47 | `HerdrClient::ping` ignores the client's deadline | `ping_through_the_client_uses_the_clients_socket_and_deadline` |
| N48 | a connect failure flattened to a bare `Io`, losing the path | `a_dead_socket_names_the_path_on_a_typed_call`, `a_dead_socket_is_unavailable_not_a_hang` |

Two mutations were written, observed to **survive**, and replaced rather than reported: an earlier
N14 that only added a `default` to a field the fixture supplies (a default never consulted is not a
mutation), and an earlier N23 whose `skip_serializing_if` predicate happened to be false for the
value under test. An earlier N38 silently patched the doc comment that *quotes* the attribute
instead of the attribute; the anchor was tightened to the `#[derive]`-adjacent form. A third,
N41-as-a-cache, survived because a single-test process never reaches the second call — the test was
**strengthened** instead (it now also pins the deadline against a silent server) and two mutations
replaced the one.

### Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` — clean.
- `cargo nextest run --workspace --features test-fixtures` — **`Summary [99.271s] 10807 tests run:
  10807 passed, 9 skipped`**.

### Next

Nothing in this crate is reachable from a cyrup production caller yet, and per AUG §11 nothing is
until commit 5 — the `native_impl.rs` `SessionStart`/`SessionShutdown` arms, `fleet.rs`'s
`has_inspect`, `project_pane.rs`'s socket route and the `cyrup-it` reachability test. Step 3 is the
stream (`events.subscribe`, `Event`, `bootstrap`, `reconnect.rs`) plus `cli.rs` and its two
`Unavailable` arms; step 4 is the production wiring, `agent.view.set`, and the `cyrup-intercom`
`HerdrLauncher` migration that leaves **one** herdr client in this workspace rather than two.

---

## [EXEC — stream]

**PR1 step 3 of 4: the event stream, the no-gap bootstrap, and reconnect.** AUG §11 commit 4.
Green: `Summary [96.913s] 10829 tests run: 10829 passed, 9 skipped` (step 2 left 10807 / 9 — the
delta is exactly the 22 new tests, no regressions).

### What landed

```
crates/cyrup-herdr/src/
  stream.rs                    # NEW — HerdrEvents, MAX_STREAM_BYTES, the cancel-safe line reader
  reconnect.rs                 # NEW — ReconnectingEvents, Backoff, StreamItem
  client.rs                    # + subscribe(), bootstrap()
  schema/events.rs             # + EventsSubscribeParams, Subscription (27), EventKind (26),
                               #   EventEnvelope, EventData (26), SubscriptionEvent{Kind,Envelope,Data},
                               #   the three rich payloads, Event + Event::decode
  schema/worktrees.rs          # NEW — WorktreeInfo (the record only; none of worktree.*'s 4 verbs)
  schema/request.rs            # Method: 21 variants → 22 (`events.subscribe`)
  schema/response.rs           # ResponseResult: + SubscriptionStarted {} + its accessor
  schema/common.rs             # AgentStatus gains Serialize (a Subscription carries one)
  tests/fake_server.rs         # + Reply::Script/StreamHandle — a connection that writes MANY lines
  tests/{stream,bootstrap,reconnect}.rs   # NEW — 22 tests
```

`worktrees.rs` is here for the same reason `workspaces.rs` was in step 2: three lifecycle events
carry a `WorktreeInfo` (`tmp/herdr/src/api/schema/events.rs:455-471`), so a subscriber that asked
for `worktree.*` events needs the record even though **no `worktree.*` verb is ported** and none
will be while cyrup owns its worktrees through `gix`.

### The whole of `Subscription` is mirrored, and that is not a scope slip

Every other enum in this crate carries only what has a caller. `Subscription` carries all 27
because it is not a method list — it is the *vocabulary of one method*. A `Method` variant this
crate omits is a call a consumer makes later by adding one line; a `Subscription` variant it omits
is a subscription herdr accepts and this client **cannot express at all**. The asymmetry is stated
on the type. The consequence follows through `EventData`: all 26 payloads decode, because an event
kind the union omitted would fail a stream the consumer explicitly subscribed to — the one outcome
worse than not offering the subscription.

### Anchors re-verified against `tmp/herdr` @ `d59d060`

| claim | anchor | verdict |
|---|---|---|
| the subscription stream's shape: one ack, then bare events | `api/server.rs:715-779`; ack at `:749-760`; poll/flush loop `:762-778` | **confirmed** |
| events carry **no `id`** | `api/subscriptions.rs:249-266` — `poll` returns a bare `serde_json::Value` | **confirmed** |
| 100 ms event latency | `CONNECTION_POLL_INTERVAL` `server.rs:28`, consumed at `:777`; liveness on the same tick, `should_stop_connection` `:812-821` | **confirmed** |
| lifecycle spells snake_case, the rich three spell dots | `schema/events.rs:192-194` vs `:367-375`; artifact `event.$defs.EventKind.enum` vs `subscription_event.$defs.SubscriptionEventKind.enum` | **confirmed** |
| the **request** spells all 27 with dots | `schema/events.rs:16-83` | **confirmed** |
| the bootstrap order | `socket-api.mdx:118-130`, `:125-126` | **confirmed** |
| `events.subscribe` is `schema.rs:237` | — | the AUG did not name the line |
| **27** subscriptions, not 28 | `schema/events.rs:16-83`, 27 `#[serde(rename = …)]` | the AUG's `events.rs:16-85` overshoots the enum by two lines |

**The anchor that turns the ordering from advice into a law.** `socket-api.mdx:817-819` says
*"Lifecycle subscriptions start when the request is accepted and do not replay events retained
before that point"*, and `tmp/herdr/src/api/server.rs:723` is why: `stream_subscriptions` takes
`event_hub.current_sequence()` as its floor **before** it builds a single subscription. So the
window between a snapshot and a later subscribe is not a latency problem to be smoothed over —
those events are never sent, there is no correction, and the cache is wrong for as long as it
lives. `bootstrap` exists so the window cannot be opened.

**A second anchor the AUG did not have, and it changes what "buffer that stream" means.**
`handle_connection_with_stop` sets the connection's send timeout to `STREAM_WRITE_TIMEOUT` = 5 s
(`server.rs:164-166`, `:31`) and `stream_subscriptions` writes every event with a **blocking**
`write_json_line` inside that loop (`:762-778`). A client that waits for its snapshot before
reading a single event therefore does not merely fall behind: herdr's write blocks, times out,
and `stream_subscriptions` returns — **the subscription is dropped**, and nothing on the wire says
so. The docs' "buffer that stream while calling `session.snapshot`" is not a convenience; reading
concurrently is the only way the snapshot arrives at all. `bootstrap` therefore polls the snapshot
call and the stream read together, and
`bootstrap_drains_the_stream_while_the_snapshot_is_held` reproduces the dependency exactly: the
fake answers `session.snapshot` only once every one of its 64 pushed events has been written, so a
client that reads afterwards deadlocks.

### `[CYRUP-EXCEEDS-UPSTREAM]` — `bootstrap` cannot be misordered

*Premise, grepped:* pi's `SocketRpcClient.subscribe` is a free-standing method beside `call`
(`src/runs/shared/herdr-connection.ts:93-106` @v0.68.0) with nothing tying the two together, and
its consumers order the snapshot themselves. herdr's own Rust client has no subscribe at all
(`src/api/client.rs`, 291 lines, `request_value` per call). So every pi consumer can silently lose
the events that fire while its snapshot is being built, and the loss is invisible: the cache is
merely wrong.

`HerdrClient::bootstrap(subscriptions) -> (SessionSnapshot, HerdrEvents)` writes the correct order
down once, and the buffering is inside it, so there is nothing to forget. It does **not** make the
losing order unreachable, and no longer claims to: `session_snapshot()` and `subscribe()` are both
public and compose into exactly it. What is guaranteed is that this method is right, not that the
pair cannot be built another way. See `[FIX — round 1]`.
`HerdrClient::subscribe` remains for a consumer that keeps **no cache** — a pane waiting on one
status change has nothing to bootstrap — and says so on itself.

### `[CYRUP-EXCEEDS-UPSTREAM]` — a 27th event kind does not sever a live subscription

*Premise:* `EventKind` is a bare string enum, so `#[serde(other)]` is unavailable (serde allows it
only on an internally or adjacently tagged enum) and the type is closed, exactly as herdr's is.
A plain `#[derive(Deserialize)]` on `EventEnvelope` would therefore fail the line — and, because
every error on this stream is terminal, would end a working subscription on the day herdr adds an
event nobody subscribed to caring about.

`Event::decode` dispatches on the `event` **name** before it decodes anything, so:

| the line says | result |
|---|---|
| one of the 3 dotted names | `Event::Subscription`, decoding **one** payload struct |
| one of the 26 snake_case names | `Event::Lifecycle` |
| a name this client does not know | `Event::Unrecognised(value)` — the raw line, verbatim; **not an error, and the stream continues** |
| a name it knows, a payload it cannot read | `HerdrError::Malformed`, with serde's own ``missing field `pane` `` |

The last two rows are the pair that matters and both are pinned. Folding the fourth into
`Unrecognised` would report a *changed* event as a readable one, which is the fabricated-reading
failure this crate exists to not have. It is also why `SubscriptionEventData` is **not** decoded
`#[serde(untagged)]` as herdr declares it (`schema/events.rs:383-389`) — the same argument as step
2's `WireResponse`, and the `event` field beside it already names the arm.

### Degradation, as implemented

| state | behaviour | pinned by |
|---|---|---|
| ack is not `subscription_started` | `UnexpectedResult`, no stream handed back | `the_subscription_ack_must_be_subscription_started` |
| ack is an error envelope, **any** `id` including `""` | fatal at once | `an_uncorrelated_invalid_request_fails_the_handshake` (2 s bound), `a_refused_subscription_carries_herdrs_own_code_and_message` |
| ack carries another request's `id` | `IdMismatch` | `a_mismatched_ack_id_is_refused` |
| herdr restarts mid-stream | `next()` → `Err(Closed)` then `None`; `ReconnectingEvents` re-bootstraps | `a_closed_stream_yields_closed_then_none`, `a_reconnect_refetches_the_snapshot` |
| an error envelope after the ack | terminal `Err(Api)` with herdr's code and message | `an_error_envelope_after_the_ack_ends_the_stream` |
| a line over 4 MiB | terminal `TooLarge` | `a_stream_line_over_four_mib_is_refused` |
| 128 MiB carried on one connection | terminal `TooLarge`; `ReconnectingEvents` recycles it **with a fresh snapshot** | `the_stream_recycles_itself_once_it_has_carried_its_byte_budget` |
| every reconnect attempt failed | the **last** failure, handed over as a terminal `Err`, then `None` | `reconnect_backoff_is_bounded_and_gives_up_loudly` |
| herdr was never there | the first bootstrap is **not** retried — `Unavailable::NoSocket` with the path, at once | `the_first_bootstrap_is_not_retried`, `a_dead_socket_names_the_path_on_a_bootstrap` |

**Every error on a `HerdrEvents` is terminal**, and that is a decision rather than an accident: a
malformed line, an unexpected envelope, a bound or a closed socket all mean this connection's
state is no longer known, and reading the next line would be guessing. pi ends its stream on the
same three (`herdr-connection.ts:100`). Recovery is a new subscription **plus a fresh snapshot**,
which is what `ReconnectingEvents` is.

### The reconnect, and why it hands back a snapshot rather than resuming

herdr says *"Call `session.snapshot` again after reconnecting"* (`socket-api.mdx:125-126`) and its
source says why — a new subscription's floor is `current_sequence()` at accept time
(`server.rs:723`), so nothing that happened during the outage is replayed. A consumer that
reconnected the stream alone would hold a cache describing a session that has moved, with no event
ever arriving to correct it.

So `ReconnectingEvents::next` yields `StreamItem::Snapshot(..)` **in band**, before the events of
the new stream. There is no way to keep reading across a reconnect without being told to replace
the cache. The first snapshot is returned by `connect` as a separate value instead, because a
consumer has nothing to replace yet and a `StreamItem::Snapshot` it had to wait for would let it
apply events to an empty cache.

`Backoff` is 250 ms doubling to a 8 s ceiling, jittered into `[delay/2, delay]`, giving up after
**6** consecutive failures (≈ 15.75 s nominal). Three properties, each pinned:

- **bounded** — `reconnect_backoff_is_bounded_and_gives_up_loudly` asserts the number of retries is
  exactly `attempts`, and wraps the whole thing in a 10 s deadline, because an unbounded loop never
  reaches the assertion at all;
- **spaced** — the same test asserts elapsed ≥ the schedule's half-jitter floor;
- **jittered** — `the_reconnect_schedule_is_a_quarter_second_doubling_to_eight_capped_and_jittered`
  samples 256 draws per attempt and requires the band not to collapse to a point. Half-jitter
  rather than full: a delay that can be near zero re-converges the herd it exists to spread. The
  jitter source is the wall clock's nanosecond field mixed with a per-process counter — this crate
  has five dependencies and a reconnect delay is not a reason for a sixth.

### `MAX_STREAM_BYTES` is a recycle threshold, and it is crossed for real in a test

The memory bound is the **per-line** 4 MiB; nothing accumulates across lines, because each is
decoded and handed over. 128 MiB is the circuit breaker, and it is pi's figure applied to pi's own
per-line bound for the same reason (`MAX_RPC_BYTES * 32`, `herdr-connection.ts:100`). The
difference is what happens next: pi's consumer learns through `onDisconnect`, here it is a terminal
`TooLarge` that `ReconnectingEvents` answers with a re-bootstrap — a recycle, not a loss.

`the_stream_recycles_itself_once_it_has_carried_its_byte_budget` pushes the whole 128 MiB through a
real socket (≈ 1.0 s) rather than asserting the constant. A threshold no test has ever crossed is a
threshold nobody has proven is reachable, and this project has shipped one of those before.

### Deliberately NOT in this step, each with a true reason

- **`cli.rs` and `Unavailable::{BinaryMissing,ProtocolMismatch}`.** The AUG pairs `cli.rs` with the
  write half; it did not land in step 2 and this step's brief is the stream. Both arms stay
  described beside the enum in `error.rs`, still unreachable, still additive behind
  `#[non_exhaustive]`. **There is no CLI fallback for the event stream at any version in this
  clone** — `ls tmp/herdr/src/cli/` has no `events` module — so nothing in this step could have had
  one.
- **`events.wait`, `agent.prompt`, `agent.wait`.** The other three methods that hold a connection
  open (`api/server.rs:251-288`), and the only three left after this step. Unlike
  `events.subscribe` they answer **once** and close, so they need no stream — they need a
  sibling-agent caller, which no batch has yet.
- **`EventsWaitParams` / `EventMatch`** (`schema/events.rs:87-91`, `:114-190`) — `events.wait`'s
  vocabulary. Unlike `Subscription`, nothing decodes one as a side effect of a subscription, so it
  lands with its method.
- **`EventKind::PaneOutputChanged`** is mirrored but has **no `Subscription`** — herdr declares the
  kind (`events.rs:216`) and no way to ask for it, and excludes it from its own hook list as
  high-volume (`:286-310`). The kind is mirrored because a snapshot-shaped consumer can still meet
  it; the absent request variant is herdr's, not this crate's.
- **Production reachability across crates.** Per AUG §11 nothing in this crate is reachable from a
  cyrup production caller until commit 5. This crate's own public API — `subscribe`, `bootstrap`,
  `HerdrEvents::next`, `ReconnectingEvents::{connect_with,next}` — is driven end to end by these
  tests against a real `UnixListener`, with no mock between the entry point and the wire.

### Mutations run, each RED, each restored byte-for-byte

23 gutting mutations over the 22 new tests. Each was applied to a pristine copy, compiled, run, and
restored from that copy; `sha256sum` over all 33 files after the sweep: **0 mismatches**.

Three tests were **strengthened** mid-sweep rather than accepting a slow RED: the byte-budget test,
`reconnect_backoff_is_bounded_and_gives_up_loudly` and `the_first_bootstrap_is_not_retried` each
originally went RED by the harness's 180 s slow-timeout under S11/S19/S22. A test that fails by
running out of patience does not say which failure it saw, so each grew an explicit
`tokio::time::timeout` naming the property — and for the backoff that upper bound is a second real
assertion, since an unbounded retry loop never reaches the "gives up" line at all.

| # | gut | test that went RED |
|---|---|---|
| S1 | `Subscription::PaneAgentStatusChanged`'s rename camel-cased | `subscribe_sends_herdrs_own_subscription_vocabulary_with_dots`, `every_subscription_this_client_offers_is_one_herdr_accepts` |
| S2 | `Subscription` loses `tag = "type"` | both of the above |
| S3 | `EventKind::PaneCreated` gets a dotted rename (the plausible mistake — the *request* uses dots) | `a_lifecycle_event_uses_underscores_…`, `a_known_event_whose_payload_changed_…`, `an_event_name_this_client_does_not_know_…`, `bootstrap_delivers_an_event_…` |
| S4 | `Event::decode` drops the unknown-name pre-check | `an_event_name_this_client_does_not_know_is_unrecognised_not_a_dead_stream` |
| S5 | a failed lifecycle decode folds into `Unrecognised` | `a_known_event_whose_payload_changed_is_malformed_not_unrecognised` |
| S6 | `pane.agent_status_changed` routed through the lifecycle arm (the naming trap in reverse) | `a_lifecycle_event_uses_underscores_and_a_subscription_event_uses_dots` |
| S7 | any success accepted as the subscription ack | `the_subscription_ack_must_be_subscription_started` |
| S8 | an error envelope must correlate before it is fatal | `an_uncorrelated_invalid_request_fails_the_handshake` |
| S9 | the ack's `id` comparison deleted | `a_mismatched_ack_id_is_refused` |
| S10 | the 4 MiB per-line bound removed | `a_stream_line_over_four_mib_is_refused` |
| S11 | the 128 MiB stream budget removed | `the_stream_recycles_itself_once_it_has_carried_its_byte_budget` |
| S12 | EOF returns `None` instead of `Closed` | `a_closed_stream_yields_closed_then_none` |
| S13 | a post-ack error envelope decoded as an event | `an_error_envelope_after_the_ack_ends_the_stream` |
| S14 | `bootstrap` snapshots first and subscribes second | `bootstrap_opens_the_subscription_and_acks_it_before_the_snapshot`, `bootstrap_drains_the_stream_…` |
| S15 | what is read during the snapshot is discarded ("not streaming yet") | `bootstrap_delivers_an_event_that_arrived_between_the_ack_and_the_snapshot`, `bootstrap_drains_the_stream_…` |
| S16 | `bootstrap` awaits the snapshot to completion, then reads | `bootstrap_drains_the_stream_while_the_snapshot_is_held` — **deadlock**, which is what a real herdr's blocking write produces |
| S17 | a reconnect re-subscribes without re-snapshotting | `a_reconnect_refetches_the_snapshot`, `reconnect_backoff_is_bounded_and_gives_up_loudly` |
| S18 | `Backoff::delay` returns zero | `reconnect_backoff_…`, `the_reconnect_schedule_…` |
| S19 | the attempt budget ignored — an unbounded retry loop | `reconnect_backoff_is_bounded_and_gives_up_loudly` |
| S20 | `Backoff::delay` drops the `max` cap | `the_reconnect_schedule_is_a_quarter_second_doubling_to_eight_capped_and_jittered` |
| S21 | the jitter removed, leaving a constant delay | that test |
| S22 | the **first** bootstrap put behind the retry schedule | `the_first_bootstrap_is_not_retried` |
| S23 | `HerdrEvents::next` reads live before draining what bootstrap buffered | `bootstrap_delivers_an_event_…`, `bootstrap_drains_the_stream_…` |

### Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` — clean.
- `cargo nextest run --workspace --features test-fixtures` — **`Summary [96.913s] 10829 tests run:
  10829 passed, 9 skipped`**.

### Next

Step 4 is the production wiring and the last of PR1: the `native_impl.rs`
`SessionStart`/`SessionShutdown` arms, `fleet.rs`'s `has_inspect` (and the delta its module doc
carries, which becomes false and must be deleted), `agent.view.set`/`clear`, `cli.rs` with its two
`Unavailable` arms, the `cyrup-it` reachability test, and the `cyrup-intercom` `HerdrLauncher`
migration that leaves **one** herdr client in this workspace rather than two. Nothing in this crate
is reachable from a cyrup production caller until it lands.

## [EXEC — cli-and-intercom]

**PR1 step 4 of 4: the CLI fallback, and the reconciliation.** AUG §5.3 and §6. Green:
`Summary [99.593s] 10843 tests run: 10843 passed, 9 skipped` — step 3 left 10829 / 9, and the
delta is exactly the 14 new tests (12 in `cyrup-herdr`, 2 in `cyrup-intercom`), no regressions. The
brief's stated baseline of 10748 / 9 was stale by one batch.

**The migration's own proof:** `cargo nextest run -p cyrup-intercom` is
`Summary [10.630s] 330 tests run: 330 passed, 0 skipped` — 328 of which are the crate's
**pre-existing tests, passing UNMODIFIED**. Not one line of an existing `cyrup-intercom` test was
edited; the two additions are new end-to-end tests, listed below.

### What landed

```
crates/cyrup-herdr/src/
  cli.rs                       # NEW (534) — HerdrCli, CliOutput, CliError, HERDR_BIN(_DEFAULT),
                               #   parse_last_json, json_to_string, extract_pane_id, shell_quote,
                               #   parse_herdr_version
  error.rs                     # + Unavailable::BinaryMissing { bin } — now with a constructor
  lib.rs                       # + pub mod cli, the three re-exports, the fourth-state paragraph
  Cargo.toml                   # tokio gains `process`
  tests/cli.rs                 # NEW (388) — 12 tests
  tests/mod.rs                 # the "no test spawns a binary" premise was made false; corrected

crates/cyrup-intercom/
  Cargo.toml                   # + cyrup-herdr
  src/project_pane.rs          # HerdrLauncher MIGRATED: -253 / +246 (110 lines of helpers
                               #   deleted, 2 end-to-end tests added)
```

### The reconciliation, exactly

`HerdrLauncher` keeps its LAUNCHER semantics and its provenance — it is a port of pi-INTERCOM's
`project-agent.ts` (`v0.12.0`), a different upstream from `cyrup-herdr`'s, and its module doc says
so. What moved is the *herdr mechanics*:

| moved to `cyrup_herdr::cli` | stayed in `project_pane.rs` |
|---|---|
| spawning, `kill_on_drop`, `CREATE_NO_WINDOW`, the deadline/cancel race | the 6-variant `PaneErrorCode` and its wire spellings |
| the `HERDR_BIN ?? "herdr"` ladder | `normalize_code` — pi's lossy 5-way fold |
| `parseLastJson`, `String(value)`, `extractPaneId`, `shellQuote`, `parseHerdrVersion` | `formatHerdrError`'s `"{backend} project pane error ({code}): {message}"` |
| the error envelope outranking the exit code, `envelope.result ?? parsed`, the first-non-blank stderr line | **every sentence**, byte for byte, in `HerdrLauncher::rendered` |
| the `--version` probe | `supports_raw_panes` — the ≥0.7.5 gate, in ONE place, unchanged |

`HerdrLauncher::run` is now four lines over `HerdrCli::run_cancellable` plus `rendered`, the
`CliError → PaneLaunchError` map. `CliError` carries **structure, never a rendered sentence** —
`SpawnFailed{source}`, `TimedOut{command,timeout}`, `Api{code,message: Option<String>}`,
`Exit{status,stderr_line}` — because exactly one place in this workspace owes a byte-identical
upstream string and it is not the herdr crate.

**`CliError` is deliberately NOT `#[non_exhaustive]`,** unlike `Unavailable` and `ApiErrorCode`.
Those two are open because *herdr's* vocabulary is open and an unknown code must be a value rather
than a parse failure. `CliError` is this crate's own account of running a subprocess, it is consumed
inside one workspace, and every consumer renders each arm as a sentence a user reads — so a new arm
must be a compile error at every consumer, not a value that folds into someone's catch-all and
reports a spawn failure as a validation error.

**The compensating `pane close` lost its fresh `CancelToken`.** Upstream passes no `AbortSignal` to
that cleanup on purpose (`:248`); the port reproduced it with a token that was never fired.
`HerdrCli::run` has no cancel arm at all, so the same guarantee is now structural — there is no
token to reuse by accident.

**One state was already in the tree and is now shared:** the `HerdrLauncher.bin` field keeps its
name and type (its test asserts `l.bin`), and `HerdrCli::new(self.bin.clone())` is built per verb.
That is correct rather than lazy: a `HerdrCli` *is* that `String` — it opens nothing, looks nothing
up and spawns nothing until a verb runs — so a stored field would be a second copy to drift.

### Anchors verified against `tmp/herdr` @ `d59d060`

| claim | anchor | verdict |
|---|---|---|
| herdr's CLI prints the WHOLE response envelope: `{"id":…,"result":…}` on stdout / exit 0, `{"id":…,"error":…}` on **stderr** / exit 1 | `src/cli.rs:745-752` (`print_response`) | **confirmed** — so the JSON on a successful line is the same `ResponseResult` the socket produces, one frame out |
| herdr's CLI is itself a socket client, two connections per verb | `src/cli.rs:769-775` + `ensure_server_protocol_compatible` `:784-801` | **confirmed** |
| `protocol_mismatch` is CLI-only | `src/cli/protocol_guard.rs:16-41`, code literal at `:38` | **confirmed** — and it reaches a cyrup process ONLY down this path, as `ApiErrorCode::Other("protocol_mismatch")` carrying herdr's own message. Pinned by `a_cli_only_code_survives_as_other_from_stderr` |
| there is no `events` module in `src/cli/` | `ls src/cli/` → agent, api, completion, integration, machine, notification, pane, plugin, protocol_guard, runtime, server, server_not_running, spec, status, tab, target, workspace, worktree | **confirmed** — the event consumer genuinely requires the socket |
| `herdr pane run` is `pane.send_input{text, keys:["Enter"]}` | `src/cli/pane.rs:1039-1052` — `args[1..].join(" ")` typed into the pane | **confirmed**, and it is why `shellQuote` exists: the pane's shell parses the text |
| a `pane.split` answer nests the id as `pane.pane_id` | `src/api/schema/panes.rs:527-528` | **confirmed** — upstream's `pane_id`/`paneId`/`id` ladder fires on its FIRST rung against a real herdr; the other two are tolerance |

The AUG's §5.3 claim that the fallback's only reason is herdr's own path resolution is restated on
`cli.rs`'s module doc with that anchor, not repeated as an assertion.

### `Unavailable::BinaryMissing` landed WITHOUT pi's install hint, on purpose

`error.rs` had reserved the arm and named pi's sentence for it. The arm is now here with a real
constructor — and its `Display` is `"no herdr binary at {bin}"`, not
`"Herdr is not installed or is not on PATH. Install Herdr 0.7.5+ or set HERDR_BIN."`. That hint is
a byte-identical upstream string with exactly one home (`project_pane.rs:362-368`,
`HerdrLauncher::not_installed`), where
`HerdrLauncher::rendered` maps this arm onto it; a second copy in the herdr crate would be a second
thing to keep in sync and no consumer would be better off. What the arm owes is the fact and the
path that was tried — which is what `cyrup-ext-subagents` will need in its own words. The comment
that reserved the arm is rewritten, not deleted: `ProtocolMismatch` still has no constructor and the
comment now says where that code does surface instead.

### Tests — 12 new in `cyrup-herdr` (81 → 93), 2 new in `cyrup-intercom` (328 → 330)

Every CLI test drives either the **absent-binary** path or a short `#!/bin/sh` script in a
per-test tempdir that prints herdr's own bytes. No test needs the `herdr` binary, and the one that
needs its *absence* names a path inside its own tempdir — the suite stays green in this container.

| test | mutation | RED |
|---|---|---|
| `the_binary_ladder_reads_herdr_bin_and_falls_through_a_blank_one` | drop `.filter(|v| !v.is_empty())` | ✔ |
| `json_to_string_renders_a_non_string_as_its_json` | render a non-string as `"[object Object]"` (the JS behaviour the delta departs from) | ✔ |
| `an_absent_binary_is_binary_missing_and_names_what_was_tried` | delete the `NotFound` arm so every spawn error is `SpawnFailed` | ✔ |
| `a_successful_verb_yields_herdrs_result_frame_unwrapped` | return `parsed` without unwrapping `result` → `extract_pane_id` finds the request id | ✔ |
| `an_error_envelope_outranks_a_zero_exit` | require `!succeeded` before reading the envelope | ✔ |
| `a_cli_only_code_survives_as_other_from_stderr` | `ApiErrorCode::from_wire("unknown")` — pi's fold | ✔ |
| `a_json_line_on_stderr_is_ignored_when_the_verb_succeeded` | read stderr whatever the exit code | ✔ |
| `a_bare_non_zero_exit_carries_the_first_non_blank_stderr_line_or_the_status` | take the first line instead of the first non-blank | ✔ |
| `a_verb_that_never_finishes_times_out_and_its_child_is_killed` | (a) drop `.kill_on_drop(true)` → the killed child finishes its work; (b) `sleep(timeout * 100)` | ✔ ✔ |
| `a_cancelled_run_is_cancelled_rather_than_timed_out` | report a withdrawn caller as `TimedOut` | ✔ |
| `version_text_reads_the_plain_line_and_a_json_payload_alike` | return `probe.text` and ignore a JSON payload | ✔ |
| `every_named_error_code_round_trips_its_wire_spelling` | (a) misspell one `as_str` arm; (b) make one named code decode to `Other` | ✔ ✔ |

`every_named_error_code_round_trips_its_wire_spelling` is there because **this step created the
dependency**: `HerdrLauncher::rendered` hands pi's `normalizeCode` the spelling it gets back from
`ApiErrorCode::as_str`, where before the migration it read the raw JSON string itself.
`normalizeCode` matches substrings (`timeout`, `gone`, `not_found`, `not-found`, `no_such_pane`),
so a spelling that did not survive `from_wire` → `as_str` would silently change which of upstream's
five codes a user is shown, with nothing else in the tree to notice it. All 31 named codes are
covered; `Other` round-trips by construction and was already pinned.

The migration itself is pinned by `cyrup-intercom`'s **unmodified** tests plus two new end-to-end
ones that drive `HerdrLauncher::open` against a fake herdr on disk:

| test | mutation | RED |
|---|---|---|
| `an_absent_binary_is_unavailable_with_the_install_hint` (pre-existing) | let `BinaryMissing` fall through to the generic `Unavailable` arm | ✔ — the install hint is gone |
| `an_already_cancelled_token_aborts_before_the_deadline` (pre-existing) | map `Cancelled` to `VALIDATION_ERROR` | ✔ |
| `the_herdr_binary_honours_its_vendor_env_var_and_defaults_to_the_bare_name` (pre-existing) | resolve the bin as a constant instead of through `HerdrCli::with_env` | ✔ |
| `a_herdr_error_envelope_reaches_the_caller_as_upstreams_sentence` (NEW) | fold every `Api` code to `VALIDATION_ERROR`, as pi's `normalizeCode` does for anything it does not know | ✔ |
| `a_herdr_older_than_zero_seven_five_is_refused_before_a_pane_is_split` (NEW) | `patch >= 4` | ✔ |

The last two are the reconciliation's real proof: a herdr error envelope now crosses a crate
boundary that did not exist before and still arrives as
`Herdr project pane error (NOT_FOUND): pane w1:p9 not found`, and the 0.7.5 gate still refuses
before anything is split.

### What this step did NOT do, and where it belongs

- **The socket route into `HerdrLauncher::open`** (AUG §7 call site 5 — take the socket path when
  `HerdrPane::discover` yields one, saving two process spawns per pane). That is AUG §11's commit 5
  together with the `native_impl.rs` arms, `fleet.rs`'s `has_inspect` and the `cyrup-it`
  reachability test; this step's scope was the CLI fallback and moving intercom's transport onto the
  one client. The seam is ready: `HerdrLauncher::cli()` is the one place a second route would branch.
- `cyrup-ext-subagents` does not consume `cyrup-herdr` yet — no verb of its own exists to consume
  it, and `INSPECTOR_AND_PROJECT_PANES.md` (VL-S6) is the batch that adds them. It can now start
  against `cyrup_herdr::cli` immediately, as AUG §11 says.

---

## [FIX — round 1]

14 defects, 2 fake tests, 19 false premises. **All 14 fixed**, both fake tests made
discriminating, every false premise corrected or deleted. One SUB-claim is refuted with a citation
— the `ReadSource` half of the `AgentStatus` defect, see F9 — and the defect it sits inside is
fixed regardless. Nothing deferred. Every claim below was re-grepped against `tmp/herdr` @
`d59d060` before it was written.

### F1 — the leaf-crate rationale (major; `lib.rs:78`, `cyrup-herdr/Cargo.toml:16-19`, spec §5.1)

Both clauses were false and are **deleted, not softened**:

```
$ grep -rn 'cyrup_herdr\|cyrup-herdr' crates/cyrup-ext-subagents/     # (empty)
$ grep -n cyrup-ext-subagents crates/cyrup-intercom/Cargo.toml
56:cyrup-ext-subagents = { workspace = true }
```

So `cyrup-ext-subagents` is not a consumer today, and the two consumer layers are **not**
independent — hosting the client inside `cyrup-ext-subagents` would have forced no new edge. The
architectural conclusion survives on reason 2 alone (`nix`/`libc` ungated vs. herdr's
Windows-capable socket, `tmp/herdr/src/ipc.rs:44-51`), which I re-verified. Corrected at **four**
sites, one of which the QA pass had not found: `crates/cyrup-herdr/src/lib.rs`,
`crates/cyrup-herdr/Cargo.toml`, `Cargo.toml:139-145` (the workspace dependency comment, which
carried the same "either consumer can take it without taking the other") and spec §5.1.

### F2 — the migration tests were ~1-in-12 flaky under `cargo test` (major; `project_pane.rs:772`)

Reproduced on the pristine tree: **11 pass / 1 fail in 12 runs** of
`cargo test -p cyrup-intercom --lib project_pane`. `fake_herdr` writes and chmods an executable;
`cargo test` runs a crate's tests as threads in ONE process, so a sibling test's `fork` inherits
the write fd this thread just closed and the kernel answers `ETXTBSY` to the `exec`.

Fixed with an exec-readiness probe, `wait_until_executable`, at both helper sites
(`crates/cyrup-intercom/src/project_pane.rs` and `crates/cyrup-herdr/src/tests/cli.rs`, which had
the identical latent shape with 8 spawning tests in one binary). It spawns the file once and kills
the child immediately: a successful `exec` **proves** no write fd exists any more, and none can
appear again, because nothing writes that path afterwards and `fork` only copies fds that already
exist. Bounded at 500 × 10 ms; any non-`ETXTBSY` error is left to the spawning test to report.

After: **12/12** on the filtered run, **5/5** on the full `-p cyrup-intercom --lib`, **10/10** on
`-p cyrup-herdr --lib`.

### F3 — `a_bare_non_zero_exit_…` did not pin "first" (`tests/cli.rs:259`)

The fixture had one non-blank stderr line, so `find` and `rev().find()` were the same string. It
now prints two (`error: unexpected argument`, then a blank, then a usage summary) and asserts the
first. **Mutation M2**: `.find(…)` → `.rev().find(…)` at `cli.rs:362-365` → RED.

### F4 — the two `PaneErrorCode::Timeout` sentences were interchangeable (`project_pane.rs:444`)

`CliError::Cancelled` and `CliError::TimedOut` share a code but render two different byte-identical
upstream sentences, and no test asserted either. Both are now pinned **on the production path**:
`an_already_cancelled_token_aborts_before_the_deadline` asserts `…was aborted.`, and a new
`a_deadline_that_expires_renders_upstreams_timeout_sentence` drives `detect` against a fake herdr
that never answers and asserts `…timed out after 3000ms.` **Mutations M7/M8**: swapping the two
arms' sentences → one RED each, no overlap.

### F5 — `revision` is 0 on every PANE-READ-DERIVED payload, not just on `pane.read` (major; 6 sites + the AUG)

herdr's only `PaneReadResult` producer on the `PaneRead` dispatch hard-codes the literal:

- `tmp/herdr/src/app/api/panes.rs:1540` — `revision: 0`, and `params.intent` is never consulted
- `tmp/herdr/src/api/wait.rs:98` — `let revision = read.revision;` over that same read
- `tmp/herdr/src/api/subscriptions.rs:295-320` → `:493-515` — the subscription dispatches
  `Method::PaneRead` to the same handler
- `tmp/herdr/src/server/headless.rs:3141-3147` — the frozen-alt-screen override touches only
  `text` and `truncated`

So `OutputMatched.revision`, `OutputMatched.read.revision` and `PaneOutputMatchedEvent.read.revision`
are **all 0** at this pin. Corrected at `schema/response.rs`, `schema/events.rs`, `schema/panes.rs`,
`client.rs`, `tests/read_half.rs` (both sites) and spec finding 6. The cited literal is `:1540`,
not `:1539` — fixed everywhere (`grep -rn 1539 crates/cyrup-herdr/` is now empty).

**The fake test.** `read_half.rs:588`'s assertion is kept — it is a real decode check — but its
framing is rewritten: `91` is now documented as a value **no herdr at this pin ever sends**, chosen
precisely so the assertion proves the number is read off the wire rather than defaulted or
hard-coded client-side. A fixture of `0` would pass against a client that dropped the field.
**Mutation M6**: `ResponseResult::output_matched` returning `0` instead of the decoded `revision`
→ RED.

### F6 — two wrong counts in the `Subscription` doc (`schema/events.rs:157,162`)

herdr declares **27**, not 28 (`tmp/herdr/src/api/schema/events.rs:16-83`; 8 workspace + 3 worktree
+ 5 tab + 7 pane + 3 rich + `layout.updated`). Re-verified mechanically, and the mirror matches:

```
$ diff <(… mirror rename list …) <(… herdr rename list …) && echo IDENTICAL
IDENTICAL      # 27 vs 27, same order
```

And 24 of the 27 answer in `EventEnvelope`, not 23 — the three that do not are the three that carry
parameters. The prose is corrected; the code was already right.

### F7 — the request bound was one byte stricter than herdr's (`transport.rs:205`)

`read_initial_request_line_with_limits` (`tmp/herdr/src/api/server.rs:556-568`) pushes a byte and
**then** tests `bytes.len() > max_bytes`, and the `b'\n'` branch above it breaks out before any
length check — so herdr **accepts** a JSON line of exactly 1048576 bytes. The client refused it.
`write_line` now tests `line.len() > MAX_REQUEST_BYTES`, and the const's doc says in as many words
that the newline is not counted on either side. `transport_bounds.rs` no longer locks the
divergence in: it pins all three of `MAX-1` (written), `MAX` (**written** — herdr's own ceiling)
and `MAX+1` (refused), each on its own connection, because the fake reads one line and closes
exactly as herdr does. **Mutation M1/M1b**: restoring `line.len() + 1 > MAX` → RED.

### F8 — `bootstrap` claimed a misuse-proof surface it does not have (`client.rs:466`, `lib.rs:33`)

`session_snapshot()` and `subscribe()` are both `pub` and compose into exactly the losing order —
`subscribe`'s own doc said so, so the two docs contradicted each other. Narrowed to what is true at
all three sites (`client.rs`, `lib.rs`, spec §"a snapshot paired with a stream"): `bootstrap` writes
the correct order down **once**, and the guarantee is that this method is right, not that the pair
cannot be built another way.

### F9 — `AgentStatus` is now open; `ReadSource` is **refuted** as safely closed (major)

`agent_status` is required and un-defaulted on `PaneInfo`, `TabInfo`, `AgentInfo` and
`WorkspaceInfo`, and `SessionSnapshot` carries `Vec<PaneInfo>`, so one pane in a sixth status failed
the whole `session.snapshot` line — and did not degrade, because `rebootstrap` burns its budget
re-fetching the same undecodable snapshot and then ends the stream for ever. The justification for
closing it was false: variant-level `#[serde(untagged)]` has given exactly this catch-all since
serde **1.0.181**, and `Cargo.lock:6963-6964` pins **1.0.228**. `AgentStatus` now carries
`Unrecognised(String)` behind `#[serde(untagged)]` — loud rather than folded into `Unknown`
("herdr cannot tell" is a status herdr publishes; reporting a new one as that would be a fabricated
reading), and round-tripping herdr's own spelling. It loses `Copy`; nothing needed it.

**`ReadSource` is refuted, with a citation.** It cannot receive a value this client did not send:
herdr's `pane.read` handler writes `source: params.source` straight out of the request
(`tmp/herdr/src/app/api/panes.rs:1533-1543`), and the one place herdr substitutes a different value,
`output_match_read_source` (`tmp/herdr/src/api/subscriptions.rs:11-18`), maps `Recent` to
`RecentUnwrapped` and is the **identity on everything else**. A sixth variant therefore cannot reach
`PaneReadResult::source`. Left closed, with that reason written on the type. The same
re-examination was applied to `EventKind` (`events.rs:334`), which carried the same false clause:
it is safely closed for a different and true reason — `Event::decode` dispatches on the `event`
**string** first, so an unknown name is already `Event::Unrecognised`.

New test `an_unknown_agent_status_decodes_instead_of_failing_the_whole_snapshot` drives
`session_snapshot()` against a snapshot carrying `"waiting"` on a workspace, a tab, an agent and one
of two panes, and asserts the **other** pane is still readable. `the_reported_state_has_four_values_…`
was rewritten from "an unknown status must fail the line" to the new, true contract.
**Mutations M5/M9b**: dropping `#[serde(untagged)]` → RED in both tests.

### F10 — `is_unsupported_method`'s "and nothing worse" (`error.rs:371`)

`tmp/herdr/src/api/server.rs:177-201` emits `invalid_request` from the `Err` branch of
`serde_json::from_str::<Request>(line)`, and `Method` is adjacently tagged with `content = "params"`,
so params deserialisation happens inside that same call: an unknown method tag, a missing required
field, a wrong JSON type and an unknown enum value all produce the identical code. The doc now says
so, states the honest reading (*turn this **request shape** off*, not *this method does not exist*)
and points a caller that must distinguish them at `ApiError::message`, where herdr's own test pins
the `"unknown variant"` wording (`tmp/herdr/src/api/schema/tests.rs:420-424`). The behaviour is
unchanged — herdr gives no other signal — and the `#[non_exhaustive]` enum-level doc was already
accurate.

### F11 — the "fourth state" paragraph described unshipped behaviour (`lib.rs:44`)

Two corrections. The trigger "no socket path resolves" cannot occur — `env::resolve_socket_path`
(`env.rs:207-216`) always answers a `PathBuf`, and `env.rs:156-158` says so in the same crate; the
condition that exists is *the resolved path does not connect*. And "Every verb degrades that way
except one" asserted an implemented socket-to-CLI fallback: there is none. `HerdrClient` never
constructs a `HerdrCli`; every socket verb ends at `transport::request`. The paragraph now says the
two routes are offered side by side, that `cyrup-intercom`'s `HerdrLauncher` takes the CLI route
outright, and that the automatic hop is AUG §11's step 5 — a step-5 claim is no longer written in
step-4 tense.

### F12 — "which of the four states this is" (`error.rs:286`)

`Unavailable` has three arms (`:40`, `:45`, `:63`); `ProtocolMismatch` is deliberately absent with
its reason at `:69-80`. Now reads "which of its three arms this is", which also stops it
contradicting `lib.rs:36`.

### F13 — `const GENEROUS` broke the Windows build under `-D warnings` (`tests/cli.rs:17`)

Moved inside the `#[cfg(unix)] mod spawned` that is its only consumer. Proven both ways with
`mod spawned` gated off as a proxy for a non-unix target:

- old placement → `error: constant `GENEROUS` is never used` under
  `cargo clippy -p cyrup-herdr --all-targets -- -D warnings`
- new placement → `Finished`, clean

### F14 — `rebootstrap` seeded a failure that never happened (`reconnect.rs:234`)

`ReconnectingEvents::next` now **carries the reason the stream ended** into `rebootstrap` instead of
letting it invent one: a `Some(Err(reason))` passes that error, and a clean end passes
`HerdrError::Closed{events.subscribe}` — which is then a description of what happened, not a
placeholder. With `attempts: 0` the consumer is told why the stream stopped rather than being told
about a connection close that was never attempted.

Both halves of the fake test are fixed. `reconnect_backoff_is_bounded_and_gives_up_loudly`'s retries
now answer a `server_unavailable` envelope, so the last failure is **different** from the seed and
the assertion labelled "the LAST failure is the one reported" has a way to be wrong. A new
`a_zero_attempt_budget_reports_why_the_stream_ended_not_a_close_that_never_happened` ends the stream
on an undecodable line and asserts `HerdrError::Malformed`. **Mutations M3** (deleting
`last = reason`) and **M4** (restoring the fabricated seed) → one RED each.

### F15 — two trivia, listed for completeness

- `env.rs:194` cited `tmp/herdr/src/session.rs:174`, which **reads** `EXPLICIT_SESSION_REQUESTED`;
  `apply_explicit_name` **sets** it at `:482`. Corrected, with the note that only herdr's own
  `--session` flag reaches that setter (`:76-91`), which is what makes `HERDR_SOCKET_PATH`-first
  correct for an env-only client.
- The `herdr` vs `herdr-dev` app-dir divergence was re-checked and is **not** a false premise: it is
  documented with its reason at `env.rs:86-95` and pinned by `tests/socket_path.rs:89-103`.

### Mutations — 9 run, 9 RED, 9 restored byte-for-byte (sha256-verified)

| # | mutation | test turned RED |
|---|---|---|
| M1/M1b | `write_line` counts the newline again | `a_request_line_over_one_mib_is_refused_before_it_is_written` |
| M2 | stderr takes the LAST non-blank line | `a_bare_non_zero_exit_carries_the_first_non_blank_stderr_line_or_the_status` |
| M3 | `rebootstrap` stops keeping the last failure | `reconnect_backoff_is_bounded_and_gives_up_loudly` |
| M4 | `rebootstrap` re-seeds the fabricated `Closed` | `a_zero_attempt_budget_reports_why_the_stream_ended_not_a_close_that_never_happened` |
| M5 | `AgentStatus` loses `#[serde(untagged)]` | `an_unknown_agent_status_decodes_instead_of_failing_the_whole_snapshot` |
| M6 | `output_matched` drops the wire `revision` | `a_wait_deadline_is_the_herdr_wait_plus_the_poll_grace` |
| M7 | the `Cancelled` arm renders the TIMED-OUT sentence | `an_already_cancelled_token_aborts_before_the_deadline` |
| M8 | the `TimedOut` arm renders the ABORTED sentence | `a_deadline_that_expires_renders_upstreams_timeout_sentence` |
| M9b | `AgentStatus` loses `#[serde(untagged)]` | `the_reported_state_has_four_values_and_the_published_status_has_five` |

Plus the W1 proxy above, which is a platform check rather than a gutting mutation.

### Gates

```
cargo fmt --all --check                                  # clean
cargo clippy --workspace --all-targets -- -D warnings    # Finished `dev` profile in 2m 09s
cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2 -- -D warnings   # Finished in 14.45s
cargo clippy -p cyrup-it --features it --all-targets -- -D warnings   # Finished in 8m 45s
cargo nextest run -p cyrup-herdr
    Summary [   5.258s] 95 tests run: 95 passed, 0 skipped
cargo nextest run -p cyrup-intercom
    Summary [  10.733s] 331 tests run: 331 passed, 0 skipped
cargo nextest run --workspace --features test-fixtures
    Summary [ 106.710s] 10846 tests run: 10846 passed, 9 skipped
```

`cyrup-intercom`'s public behaviour is unchanged and its byte-identical upstream sentences are
intact. Two that no test asserted at all — `…was aborted.` and `…timed out after {ms}ms.` — are now
asserted on the production path, one each.

## [FIX — round 2]

One defect, filed three times over the same lines: **the round-1 F5 fix over-corrected.** It
replaced a false narrow claim ("`output_matched`'s `revision` is a real revision") with a false
*wide* one ("there is no herdr payload at this pin whose `revision` is meaningful"), which this
crate's own `PaneInfo::revision` doc contradicted 129 lines higher in the same file. Both fake
tests and the whole false-premise list had already been closed in round 1; each was re-greped here
and is recorded below with the grep that settles it.

### R2-1 — narrow the `revision` claim to the payloads the evidence covers (major; 6 sites)

**What is true, and how it is known.** herdr has *two different* fields spelled `revision`:

| field | herdr source | value |
|---|---|---|
| `PaneReadResult.revision` — `pane.read`, `OutputMatched.read`, `OutputMatched.revision`, `PaneOutputMatchedEvent.read.revision` | `tmp/herdr/src/app/api/panes.rs:1540` (`revision: 0`), copied by `tmp/herdr/src/api/wait.rs:98`, reached by the subscription through `tmp/herdr/src/api/subscriptions.rs:295-320` → `:493-515`; `tmp/herdr/src/server/headless.rs:3141-3147` overrides only `text`/`truncated` | **hard `0`** |
| `PaneInfo.revision` — `pane.get`, `pane.list`, `pane.current`, `pane.split`, `pane.focus`, and every pane inside `session.snapshot` | filled from the terminal's own counter at `tmp/herdr/src/app/creation.rs:354` (`revision: terminal.revision`); initialised `0` at `tmp/herdr/src/terminal/state.rs:180`; advanced at `tmp/herdr/src/terminal/state.rs:235` (`wrapping_add(1)`), `tmp/herdr/src/app/actions.rs:428` and `tmp/herdr/src/app/api/panes.rs:1749` (both `saturating_add(1)`) | **a live ordering token** |

So "0 everywhere" was never provable, and this crate already decodes the live one — and already
asserts it off the wire as non-zero at `src/tests/read_half.rs:129` and `:194` (`17`) and
`src/tests/stream.rs:256` (`7`). The wide claim was refuted by tests the same crate ships.

**The six sites, narrowed.** Every one now scopes the `0` to *pane-read-derived* payloads and
points at `PaneInfo::revision` as the field a cache or an event floor actually keys on:

- `src/schema/panes.rs` — `PaneReadResult`'s type doc. "**This type's `revision` is always `0`**
  … on every payload that carries a `PaneReadResult`", then names the three dead fields explicitly
  (`Self::revision`, `OutputMatched::revision`, `PaneOutputMatchedEvent::read`), then: "**That `0`
  is a fact about the read, not about the protocol.**"
- `src/schema/panes.rs` — `PaneInfo::revision`, expanded from one line to the four herdr citations
  above, and cross-linked to `PaneReadResult::revision` so the two can no longer be read as rivals.
- `src/client.rs` — `pane_read`'s doc: the last sentence now forwards to `PaneInfo::revision` and
  names the three methods that carry it, instead of denying that any live revision exists.
- `src/schema/response.rs` — `ResponseResult::OutputMatched::revision` and the lifted
  `OutputMatched::revision`, both scoped to "pane-read-derived".
- `src/schema/events.rs` — `PaneOutputMatchedEvent::read`, same scoping, plus a forward link.
- `src/tests/read_half.rs:439-443` — the `pane_read_answers_herdrs_own_zero_revision` doc said
  "and so is every other `revision` at this pin"; now "every other `PaneReadResult`", with the
  parenthetical that `PaneInfo::revision` is a different field and a live counter.

**Also corrected:** `Event::PaneOutputChanged::revision` (`src/schema/events.rs`) had a bare "The
new revision." It is the *terminal* counter, not the dead `0` — and herdr constructs that variant
nowhere outside its own tests at this pin (`tmp/herdr/src/app/api/plugins/mod.rs:3515` sits inside
`#[test] fn output_changed_event_hooks_do_not_run_even_if_event_is_emitted`), which is the other
half of why `Subscription` offers no way to ask for it (`tmp/herdr/src/api/schema/events.rs:16-83`
has no `pane.output_changed` rename). The field doc now says both.

**Spec heading.** `### F5 — revision is 0 EVERYWHERE` was the over-general form the code docs were
derived from; the body under it was already correctly scoped. Renamed to
`### F5 — revision is 0 on every PANE-READ-DERIVED payload, not just on pane.read`.

``grep -rn 'every other `revision`\|no herdr payload\|revision` is meaningful\|0 EVERYWHERE' crates/``
is now empty. (It still hits *this section*, which quotes the four removed strings in order to say
they were removed; scoping the grep to `crates/` is what makes the claim checkable rather than
self-falsifying.)

### The two "fake tests" — both already discriminating; re-proven by mutation, not re-edited

- **`src/tests/read_half.rs`, `a_wait_deadline_is_the_herdr_wait_plus_the_poll_grace`.** Round 1
  had already rewritten the doc (:544-551) and the assertion message (:594) to say the fixture's
  `91` is a value **no herdr at this pin sends**, and that the assertion pins the decode rather
  than a herdr behaviour. **Mutation R2-M1**: in `ResponseResult::output_matched`
  (`src/schema/response.rs:357-373`) replace the lifted `revision` with a hard `0` →
  `FAIL [0.351s] (1/1) … a_wait_deadline_is_the_herdr_wait_plus_the_poll_grace`. Restored
  byte-for-byte.
- **`src/tests/reconnect.rs`, `reconnect_backoff_is_bounded_and_gives_up_loudly`.** Round 1 had
  already made the retries answer an `server_unavailable` *error envelope* (`src/tests/reconnect.rs:132`),
  which is `HerdrError::Api` and therefore **not** the `HerdrError::Closed{method:
  "events.subscribe"}` that `rebootstrap` seeds `last` with. **Mutation R2-M2**: delete
  `last = reason;` (`src/reconnect.rs:266`) → `FAIL [0.264s] (1/1) …
  reconnect_backoff_is_bounded_and_gives_up_loudly`. Restored byte-for-byte.

### The false-premise list — all previously closed, each re-greped

| premise | state | the grep |
|---|---|---|
| `lib.rs:78` "both consume it" | fixed in r1 | `lib.rs:94-98` now says the one consumer is `cyrup-intercom` and that `grep -rn 'cyrup_herdr\|cyrup-herdr' crates/cyrup-ext-subagents/` is empty |
| `lib.rs:78` / `Cargo.toml:16-19` "neither depends on the other" | fixed in r1 | both now cite `crates/cyrup-intercom/Cargo.toml:56` and say the graph argument is not made |
| spec `:273` self-falsifying grep | fixed in r1 | reason 1 struck through, converse stated |
| `response.rs:139-140` "a real revision here" | fixed in r1, **re-narrowed here** | R2-1 |
| `events.rs:515-517` "a **real** revision here" | fixed in r1, **re-narrowed here** | R2-1 |
| `panes.rs:602-605` "real only inside `output_matched`" | fixed in r1, **re-narrowed here** | R2-1 |
| `client.rs:401-403` "meaningful only on an `OutputMatched`" | fixed in r1, **re-narrowed here** | R2-1 |
| spec `:940-943` AUG finding 6 | fixed in r1 | now "hard-coded `0` on EVERY pane-read-derived payload" — correctly scoped |
| `events.rs:157` "All 28" | fixed in r1 | reads "All 27" |
| `events.rs:162` "23 lifecycle kinds" | fixed in r1 | reads "the 24 that answer in the lifecycle envelope" |
| `client.rs:466` "no constructor in the losing order" | fixed in r1 | `bootstrap`'s doc now says both are `pub` and compose into exactly the losing order |
| `transport.rs:41-45` off-by-one bound | fixed in r1 | doc now derives the exact parity from `server.rs:556-568` |
| `client.rs:402` / `panes.rs:603` cite `:1539` | fixed in r1 | `grep -rn 1539 crates/cyrup-herdr/` empty |
| `env.rs:194` cites `session.rs:174` as the setter | fixed in r1 | now `apply_explicit_name` `:475-484`, `store(true)` on `:482` |
| `common.rs:138-139` `#[serde(other)]` conclusion | fixed in r1 | `common.rs:159-161` concedes the conclusion does not follow; the enum carries a variant-level `#[serde(untagged)]` arm at `:199-200` |
| `lib.rs:44-45` unreachable fourth state | fixed in r1 | now "the resolved socket path does not connect", with the `env.rs:156-158` infallibility note |
| `lib.rs:47-48` "every verb degrades that way" | fixed in r1 | now "**This crate does not make that hop for you.**" |
| `error.rs:286` "four states" | fixed in r1 | "which of its three arms this is" |
| `error.rs:370-371` `is_unsupported_method` | fixed in r1 | now "herdr refused to deserialise this request — which includes, but is not limited to" |

### Gates

```
cargo fmt --all -- --check                                  clean
cargo clippy -p cyrup-herdr --all-targets --all-features -D warnings    Finished, 0 warnings
cargo clippy -p cyrup-intercom --all-targets --all-features -D warnings Finished, 0 warnings
cargo doc -p cyrup-herdr --no-deps        Generated target/doc/cyrup_herdr/index.html (rustdoc links are DENIED; the six new intra-doc links resolve)
cargo nextest run -p cyrup-herdr          Summary [5.251s] 95 tests run: 95 passed, 0 skipped
cargo nextest run -p cyrup-intercom       Summary [10.692s] 331 tests run: 331 passed, 0 skipped
```

Both mutations restored byte-for-byte; `git status --porcelain crates/cyrup-herdr` shows only the
untracked crate directory, and the two mutated files diff clean against their pre-mutation copies.
