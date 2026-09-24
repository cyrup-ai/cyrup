# 16 — cyrup-herdr: client conformance against herdr's socket API

**This is a client-conformance area, not a port.** cyrup does not port herdr. `crates/cyrup-herdr`
is a **client** for herdr's newline-delimited JSON socket API (plus a thin wrapper around the
`herdr` CLI and `herdr machine list --json` / `herdr status server --json`). This file checks whether
that client and its consumers speak herdr's protocol as herdr actually defines it. It does not ask
whether cyrup reproduces herdr. There is no "unported herdr feature" category here. A herdr method
cyrup has no reason to call is not a gap.

Item kinds used by this area. They extend README's taxonomy for this area only:
`client-bug` (the client or a consumer disagrees with herdr **at the pinned tag**), `protocol-drift`
(herdr changed the API after the pin and the client does not follow), and `not-used` (an unused
herdr API worth recording because a known cyrup need would use it). One `tooling` row uses README's
own kind. Ids are `HERDR-NNN`. **Next free id: `HERDR-004`.**

> ### RE-MEASURE — 2026-09-24 — first measurement of this area
>
> **Pins:** cyrup **`ea23ca2`**. herdr cloned fresh into `tmp/herdr` from
> `https://github.com/herdrdev/herdr.git` (clone HEAD `9c96f7dd`, 2026-09-24). Newest **release**
> tag is **`v0.9.1`** (`065ef9d6`, 2026-09-16). Newest tag of any kind is
> **`preview-2026-09-21-0ff0f27e2226`** (`0ff0f27e`). `preview-2026-09-21-0ff0f27e2226..HEAD` is
> 21 untagged commits, read as post-tag leads. Upstream was read only as
> `git -C tmp/herdr show <tag>:<path>` / `git -C tmp/herdr diff <tag>..<tag>`. The one exception is
> the crate's own pin, `d59d0603`, an untagged commit, which was diffed against both tags because
> the crate names it (see `HERDR-002`). No cargo was run.
>
> **What this pass read, in full.**
> 1. **herdr `v0.9.1`, the API surface:** `src/api/schema.rs` (all 104 methods) and every
>    `src/api/schema/*.rs` a cyrup method touches (`common`, `server`, `session`, `tabs`,
>    `workspaces`, `worktrees`, `agents`, `panes`, `events`, `response`). Also
>    `src/api/server.rs` (`handle_connection_with_stop`, `stream_subscriptions`),
>    `src/api/subscriptions.rs`, `src/api/event_hub.rs`, `src/api/wait.rs` (`wait_for_output`,
>    the `agent.prompt` wait), `src/api/mod.rs`, `src/session.rs` (socket-path ladder) and
>    `src/config/io.rs` (`config_dir`). Then the handlers each cyrup method lands on:
>    `src/app/api/{panes,tabs,agents,agent_view,workspaces,session}.rs`, `src/app/agents.rs`
>    (`agent.start`, `valid_agent_name`), `src/app/api_helpers.rs` (metadata normalisers),
>    `src/terminal/state.rs::accept_hook_report`, `src/agent_resume.rs`, `src/cli/machine.rs`,
>    `src/cli/status.rs`, `src/cli/pane.rs` (the `pane split` argument parser),
>    `src/protocol/wire.rs` (`PROTOCOL_VERSION`), and the `socket-api.mdx` compatibility and
>    bootstrap prose.
> 2. **The two post-release windows, on every API path** (`src/api`, `src/app/api*`, `src/cli.rs`,
>    `src/cli/{machine,status}.rs`, `docs/next/api/herdr-api.schema.json`):
>    `v0.9.1..d59d0603`, `d59d0603..preview-2026-09-21-0ff0f27e2226` and
>    `preview-2026-09-21-0ff0f27e2226..HEAD`.
> 3. **cyrup, in full:** every non-test file of `crates/cyrup-herdr/src/`: `lib.rs`, `client.rs`,
>    `transport.rs`, `stream.rs`, `reconnect.rs`, `error.rs`, `env.rs`, `probe.rs`, `machine.rs`,
>    `schema/*.rs`, and `remote.rs` up to the ssh runner. **Consumers:** `cyrup-ext-subagents`
>    `src/herdr/{consumer,reporter,view}.rs` (the status bridge), `src/placement/{native,external}.rs`
>    at every herdr call site, `src/inspectors/herdr/client.rs::dispatch` and the `pane split`
>    argv builders in `project_panes.rs` / `actions.rs`, and `cyrup-intercom`
>    `src/project_pane.rs` (`HerdrLauncher`, the CLI route).
>
> **Filed:** `HERDR-001` (medium), `HERDR-002` (low), `HERDR-003` (low). **Closed:** none (new
> area). **Leads:** none left unresolved (see `## Conformance record` and `## Post-tag leads`).
>
> **Still unread, stated exactly.** (a) `crates/cyrup-herdr/src/relay.rs` and the ssh-runner half
> of `remote.rs` were not compared. They carry cyrup's own relay frame protocol over ssh and the
> OpenSSH StreamLocal forward, not herdr's API. The only herdr surface in them is
> `herdr status server --json`, which was read (`HERDR-003`). (b) `crates/cyrup-herdr/src/cli.rs`
> was read for its verbs and error mapping only, not line by line. (c) herdr's non-API changes in
> the post-release windows (agent screen detection, TUI, Windows input, worktree internals) were
> not read, because no cyrup call depends on them. (d) The `v0.9.1..d59d0603` whole-tree diff is
> 192 files, +21 319 / −1 600, because `v0.9.1` sits on a release side-branch. Only its API paths
> were read.
> (e) **`cyrup-tui` has no herdr surface to check:** `grep -rni herdr crates/cyrup-tui` is empty at
> `ea23ca2`. The "pane status bridge" is `cyrup-ext-subagents/src/herdr/`, and that was read.

## Provenance and pins

| side | pin | how obtained |
|---|---|---|
| cyrup | **`ea23ca2`** | `git log -1` |
| herdr, baseline | **`v0.9.1`** (`065ef9d6`, 2026-09-16), the newest release tag. **It is not an ancestor of `main`:** it is a release commit on a side branch off `2c29fb29` (= `preview-2026-09-16-2c29fb29e302`) | `git -C tmp/herdr tag --sort=v:refname`; `git merge-base --is-ancestor v0.9.1 HEAD` → false |
| herdr, newest tag | **`preview-2026-09-21-0ff0f27e2226`** (`0ff0f27e`, 2026-09-21) | `git -C tmp/herdr tag` (preview tags are named tags) |
| herdr, crate's own pin | `d59d0603` (2026-09-20). This is **untagged**: `git describe` = `preview-2026-09-16-2c29fb29e302-25-gd59d0603`. It is an ancestor of the newest preview tag, and `v0.9.1` is **not** its ancestor | `git -C tmp/herdr describe --tags d59d0603` |
| herdr clone HEAD | `9c96f7dd` (2026-09-24), 21 commits past the newest tag | `git -C tmp/herdr rev-list --count preview-2026-09-21-0ff0f27e2226..HEAD` |

`PROTOCOL_VERSION` (`src/protocol/wire.rs:20`) is **22** at `v0.9.1`, at `d59d0603`, at the newest
preview tag and at HEAD. The binary client-shell protocol has not moved across the whole window.

## Census

| what | count | source |
|---|---|---|
| herdr socket methods (`#[serde(rename)]` on `Method`) | `v0.9.1` **104** · `d59d0603` 105 (+`pane.clear`) · preview-2026-09-21 105 · HEAD 106 (+`server.ssh_agent.register`) | `git show <rev>:src/api/schema.rs \| grep -c '#\[serde(rename = "'` |
| methods `cyrup-herdr` sends (`schema::Method` variants) | 28 | `crates/cyrup-herdr/src/schema/request.rs` |
| lifecycle event kinds decoded / subscription event kinds decoded | 26 / 3, matching `v0.9.1`'s `EventKind` / `SubscriptionEventKind` exactly | `schema/events.rs` vs `git show v0.9.1:src/api/schema/events.rs` |
| `ResponseResult` variants decoded | 18 named + `Unrecognised` (`#[serde(other)]`) | `schema/response.rs` |
| cyrup-herdr non-test source | 8 110 lines | `find crates/cyrup-herdr/src -name '*.rs' -not -path '*/tests/*' \| xargs cat \| wc -l` |
| consumers | `cyrup-ext-subagents` (status bridge, placement, inspectors) and `cyrup-intercom` (`HerdrLauncher`, CLI route). **Not** `cyrup-tui` | `grep -rln cyrup_herdr crates --include=*.rs` |

## Methodology

1. For each of the 28 methods: the `v0.9.1` param struct was compared field by field with cyrup's
   `Serialize` mirror (name, optionality, `skip_serializing_if`, defaults). Then the `v0.9.1`
   handler's `ResponseResult` arm was compared with the accessor cyrup's client calls
   (`ok`/`pane`/`tab`/…).
2. For each event: `v0.9.1`'s `EventKind` wire spelling was compared with cyrup's decode path.
   Lifecycle events are snake_case (`"pane_closed"`); subscription events are dotted
   (`"pane.agent_status_changed"`), with untagged `data`. So were `EventData`'s tagging and every
   variant's fields.
3. Error handling, forward compatibility, the subscribe handshake, reconnect and the socket-path
   ladder were each read against the herdr function that defines them.
4. Every consumer call site was read for the params it builds, against herdr's validators
   (`valid_agent_name`, `normalize_metadata_source`, `normalize_state_labels`, token/TTL bounds,
   `accept_hook_report`).
5. The three post-release windows were diffed on API paths. Anything that moved was checked against
   the client.

## Open items

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| HERDR-001 | medium | client-bug | S | The inspector's socket adapter turns `pane split --current` into "split the focused pane of the active workspace". herdr's `--current` means the caller's own pane (`HERDR_PANE_ID`) |
| HERDR-002 | low | tooling | S | The crate's pin "herdr v0.9.1 (`d59d060`)" names an untagged `main` commit that is not `v0.9.1`, and every `tmp/herdr/...:N` citation in the crate is at that commit |
| HERDR-003 | low | client-bug | S | A stopped remote herdr is reported as "returned incomplete identity" rather than "stopped or incompatible", because `version`/`protocol` are `null` in herdr's `not_running` status JSON and are checked first |

---

## HERDR-001 — `pane split --current` over the socket splits the wrong pane

**Kind** client-bug · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read; not observed live)

**cyrup**: `cyrup-ext-subagents/src/inspectors/herdr/client.rs::dispatch`, arm
`["pane", "split", rest @ ..]`. It builds `PaneSplitParams::new(SplitDirection::Right)` and never
sets `target_pane_id`. Its comment says `--current` is "`PaneSplitParams`' own default
(`target_pane_id: None`) — so it needs no translation." The argv comes from
`inspectors/herdr/project_panes.rs` (the project-pane open: `["pane","split","--current",
"--direction","right","--cwd",root,…]`) and `inspectors/herdr/actions.rs` (the same plus `--ratio`
and `--env`). Production reaches this adapter through `SocketHerdrClient::for_pane` /
`from_process_env` (`inspectors/herdr/plugin.rs:94`, `extension/tool/routing.rs:1593`), which
already holds the caller's `HerdrPane` and therefore its pane id. The adapter also ignores the
`--direction` value (always `Right`). Every current caller passes `right`, so that part is latent.

**herdr `v0.9.1`**: `git -C tmp/herdr show v0.9.1:src/cli/pane.rs`, in the `pane split` parser.
`"--current"` sets `pane_id = Some(env_pane_id … .ok_or("--current requires HERDR_PANE_ID")?)`,
which becomes `PaneSplitParams.target_pane_id`. `git show v0.9.1:src/app/api/panes.rs`
`handle_pane_split`: when `target_pane_id` is `None` and `workspace_id` is `None`, the target is
`self.state.active` → that workspace's `focused_pane_id()`. `socket-api.mdx` @v0.9.1 says the same:
*"Methods whose schema makes `pane_id` optional use the server's active focused pane when it is
omitted."* So `--current` (the caller's pane) and an omitted target (whatever the human is focused
on) are different targets.

**Impact**: a project pane or inspector pane opens beside whatever pane the human is focused on.
That can be in another tab or another workspace. It does not open beside the cyrup pane that asked.
When the human is looking at cyrup's own pane the two coincide, which is why this is easy to miss.
The CLI route (`cyrup-intercom` `HerdrLauncher`, which shells `herdr pane split --current …`) is
correct. So the same feature lands in different places depending on which route served it.

**Fix**: give `SocketHerdrClient` the caller's pane id (it is constructed from a `HerdrPane`; keep
`pane.pane_id()`). In the `pane split` arm, map `--current` to
`params.target_pane_id = Some(caller_pane_id)` and honour `--pane <id>` / a positional id. Parse
`--direction` (`right`/`down`) instead of hard-coding `Right`. Delete the comment's equivalence
claim.

**Verify**: a `dispatch` unit test against the fake server asserting that the `pane.split` request
line carries `"target_pane_id":"<HERDR_PANE_ID>"` for `--current`, and `"direction":"down"` for
`--direction down`.

## HERDR-002 — The crate is pinned to an untagged commit it calls `v0.9.1`

**Kind** tooling · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup**: `crates/cyrup-herdr/Cargo.toml` `description` ("herdr v0.9.1, d59d060") and the
`lib.rs` crate doc ("Pinned to herdr **v0.9.1** (`tmp/herdr` @ `d59d060`)"). Every `tmp/herdr/...:N`
citation in the crate is a working-tree line at that commit. The crate doc's census, "herdr
publishes 105 renamed methods … 28 + 77 = 105", counts `d59d0603`, and its not-ported table lists
`clear` among the pane-geometry methods.

**herdr**: `d59d0603` is `preview-2026-09-16-2c29fb29e302-25-gd59d0603`, an untagged `main` commit.
`v0.9.1` (`065ef9d6`) is a release commit on a side branch and is **not** its ancestor. Both report
`version = "0.9.1"` in `Cargo.toml`, which is presumably how the label arose. On API paths they
differ by exactly two things (`git -C tmp/herdr diff v0.9.1 d59d0603 -- src/api src/app/api src/cli.rs`):
(1) `pane.clear` exists only at `d59d0603` (`v0.9.1` has 104 methods). (2) At `v0.9.1`,
`handle_connection_with_stop` answers an undeserialisable request with `id: ""` always, and a
subscription setup failure with the internal probe's id (`<id>:sub:<n>:probe`). `d59d0603`
(`241063f7`) echoes the request id in both cases.

**Impact**: nothing breaks at run time. `transport::request` and `HerdrClient::subscribe` treat an
error envelope as fatal **whatever its id**, so both behaviours are handled (checked; see
`## Conformance record`). The cost is evidentiary: a reader checking the crate against "v0.9.1"
finds a 105th method and line numbers that do not match the tag. README's rule (upstream cited only
at a named tag) cannot be applied to the crate's citations as written.

**Fix**: restate the pin as either `v0.9.1` (`065ef9d6`) or the preview tag that contains
`d59d0603` (`preview-2026-09-21-0ff0f27e2226`). Re-anchor the census (104 at `v0.9.1`, or 105 at the
preview tag). State the error-id difference in `transport.rs`'s doc, which today describes only the
`d59d0603` behaviour.

**Verify**: `git -C tmp/herdr cat-file -e <pinned-tag>:src/api/schema.rs`, and the crate's census
equals `git show <pinned-tag>:src/api/schema.rs | grep -c '#\[serde(rename = "'`.

## HERDR-003 — A stopped remote herdr reads as "incomplete identity"

**Kind** client-bug · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read)

**cyrup**: `crates/cyrup-herdr/src/remote.rs::parse_endpoint`. It requires `socket`, `version` (a
string) and `protocol` (a number) **before** it looks at `running`/`compatible`, and it answers
"Remote Herdr endpoint discovery returned incomplete identity." when any of them is missing. The
discovery command is `herdr status server --json` (`remote.rs` discovery script).

**herdr `v0.9.1`**: `git -C tmp/herdr show v0.9.1:src/cli/status.rs`, `server_status_json`. The
`ServerRuntimeStatus::NotRunning` arm writes `running: false, version: None, protocol: None,
compatible: None`, so a stopped server's JSON has `"version": null, "protocol": null`.

**Impact**: when the saved machine's herdr server is simply not running (the common case), placement
reports a malformed-identity failure instead of "The selected remote Herdr session is stopped or
incompatible." The operator is pointed at the wrong cause. This is byte-for-byte pi-subagents'
`parseHerdrEndpoint` (`git -C tmp/pi-subagents show v0.68.0:src/runs/shared/herdr-connection.ts:49-60`
has the same check order), so fixing it is a deliberate divergence from pi in favour of herdr's
actual output. Record it with a `CYRUP-DELTA` marker.

**Fix**: in `parse_endpoint`, check `running != true` (and `compatible`/`endpoint_compatible`)
before requiring `version`/`protocol`. Keep the socket-and-session check first.

**Verify**: a unit test feeding `server_status_json`'s `not_running` shape
(`{"status":"not_running","running":false,"version":null,"protocol":null,…,"socket":"/x","session":null}`)
expects the "stopped or incompatible" sentence.

---

## Conformance record: what was checked and found to agree with herdr `v0.9.1`

Recorded so the next pass does not re-derive it. Each row read both sides.

| surface | result |
|---|---|
| Envelope `{"id","method","params"}`, adjacent tagging, `params` mandatory (`ping` sends `{}`) | agrees (`v0.9.1:src/api/schema.rs:35-47`) |
| Params of all 28 methods (`PingParams`, `EmptyParams`, `WorkspaceCreateParams`, `TabCreateParams`, `TabTarget`, `TabRenameParams`, `AgentTarget`, `AgentViewSetParams`/`Filter`/`Field`/`Value`/`Sort` (untagged + snake_case), `AgentViewClearParams`, `AgentStartParams`, `AgentPromptParams`/`WaitOptions`, `PaneSplitParams`, `PaneProcessInfoParams`, `PaneListParams`, `PaneCurrentParams`, `PaneTarget`, `PaneSendInputParams`, `PaneReadParams`, `PaneReportAgent{,Session}Params`, `PaneReportMetadataParams`, `PaneClearAgentAuthorityParams`, `PaneReleaseAgentParams`, `PaneWaitForOutputParams`, `EventsSubscribeParams`/`Subscription`) | every field name, optionality and default agrees. cyrup always writes fields herdr defaults (`focus`, `right_click`, `strip_ansi`, `format`, `clear_*`), with herdr's default values. `BTreeMap` for herdr's `HashMap` is a wire no-op |
| Result type per method (handler → accessor) | agrees for all 28 (`v0.9.1:src/app/api/{panes,tabs,agents,agent_view,workspaces,session}.rs`, `src/api/wait.rs`) |
| Response structs (`PaneInfo`, `AgentInfo`, `TabInfo`, `WorkspaceInfo`, `SessionSnapshot`, `PaneLayoutSnapshot`, `PaneProcessInfo`, `PaneReadResult`, `AgentSessionInfo`/`AgentSessionRefKind`, `ServerCapabilities`) | agree field for field. No `deny_unknown_fields` anywhere, as `socket-api.mdx:959` @v0.9.1 requires |
| Unknown result `type` / unknown event name / unknown `AgentStatus` / unknown error `code` | `ResponseResult::Unrecognised` → `UnexpectedResult`, `Event::Unrecognised`, `AgentStatus::Unrecognised`, `ApiErrorCode::Other`. None is fatal to the connection model. Unknown method → herdr `invalid_request` → `is_unsupported_method()` (`v0.9.1:src/api/server.rs` `serde_json::from_str::<Request>` error arm) |
| Error envelope with empty or foreign id | treated as fatal whatever its id (`transport.rs::request`, `client.rs::subscribe`). Correct for `v0.9.1`'s `id: ""` and `<id>:sub:<n>:probe` |
| Lifecycle vs subscription event wire names | agrees: `EventKind` snake_case (`"pane_closed"`); the three subscription events dotted with untagged `data` (`v0.9.1:src/api/schema/events.rs`) |
| Subscribe handshake and bootstrap order | agrees with `socket-api.mdx` @v0.9.1 (subscribe, await ack, buffer while `session.snapshot`, apply) and with `stream_subscriptions`' `event_start_sequence` floor |
| Reconnect | re-bootstraps (fresh snapshot in band) on any stream end, as `socket-api.mdx` requires ("Call `session.snapshot` again after reconnecting") |
| Version negotiation | none, correctly. `ping.protocol` is the binary protocol (22 throughout); herdr's JSON compatibility rule is per-method errors, which the client follows |
| Socket-path ladder (`HERDR_SOCKET_PATH` → `HERDR_SESSION` → `<config_dir>/herdr.sock`; `XDG_CONFIG_HOME`, platform dirs, `validate_name`) | agrees with `v0.9.1:src/session.rs::active_api_socket_path`/`active_name`/`validate_name` and `src/config/io.rs::config_dir`. Two edge differences, neither filed: cyrup treats an **empty** `HERDR_SOCKET_PATH`/`XDG_CONFIG_HOME` as unset (herdr joins the empty string), and cyrup falls through to the default socket on an invalid `HERDR_SESSION`, where herdr's CLI refuses to start |
| Status bridge params (`cyrup-ext-subagents/src/herdr/reporter.rs`, `view.rs`) | `source "cyrup:subagents"` passes `normalize_metadata_source`; agent `cyrup` passes `normalize_reported_agent_label`; state-label keys `idle`/`working`/`done` pass `normalize_state_labels`; TTL 120 000 in `1..=86_400_000`; the clear call is not an `invalid_metadata_request`; a wall-clock-seeded strictly increasing `seq` satisfies `accept_hook_report` (`v0.9.1:src/terminal/state.rs:1673-1687`) across a cyrup restart in the same pane; agent-view sort fields exist in `AgentViewBuiltinSortField` |
| Event consumer (`herdr/consumer.rs`) | subscribes `pane.agent_status_changed{pane_id}` (no status filter → no initial event, per `v0.9.1:src/api/subscriptions.rs`), `pane.closed`, `pane.exited`; decodes each in the right envelope |
| Placement (`placement/native.rs`, `external.rs`) | `agent.start` name `<kind>-<last 20 of run id>` satisfies `valid_agent_name` (run ids are lowercase hex; ≤ 27 chars); `timeout_ms 45 000` inside `(settle, 300 000]`; kinds `claude`/`cursor`/`codex` parse; the "is not an available shell" retry matches `agent_pane_busy`'s `v0.9.1` message; `agent.prompt` `wait.until` / client deadline `wait + 5 s` agree with `src/api/wait.rs` |
| `HerdrLauncher` CLI route (`cyrup-intercom/src/project_pane.rs`) | `pane split --current --direction right --cwd … [--focus]`, `pane run <id> <cmd>`, `pane close <id>` all exist with those flags at `v0.9.1:src/cli/pane.rs` |
| `herdr machine list --json` (`machine.rs`) | reads `MachineListRow {id,label,target,session,enabled,selected}` (`v0.9.1:src/cli/machine.rs`) |

## Drift after `v0.9.1` that the client already tolerates (not items)

- **`pane.clear`** (`cc7c696c`, in `d59d0603` and the preview tag). No cyrup need.
- **`PaneInfo.restore_error`** (`0ff0f27e`, at the preview tag). An additive optional field that
  the client ignores. No consumer needs it.
- **Subscription streams now end with an error envelope** (`65927cef`, at the preview tag):
  `events_lost` ("resubscribe and resync with session.snapshot"), `server_unavailable` or
  `internal_error`, after the ack. `stream.rs::read_next` makes any post-ack error terminal, and
  `ReconnectingEvents` re-bootstraps. That is exactly the resync herdr asks for, so the behaviour
  conforms. Only `stream.rs`'s doc sentence "herdr's own stream loop writes only events once it has
  acked" is stale at the preview tag. At `v0.9.1` the same overflow was silent: a 512-event ring,
  one event per subscription per 100 ms tick. That is a herdr defect the client cannot see, so the
  status bridge's `pane.closed` could be missed under heavy event load on `v0.9.1`.

## Post-tag leads (herdr `preview-2026-09-21-0ff0f27e2226..9c96f7dd`)

README cites upstream only at tags, so these are leads, not items. All API-path commits in the
range were read.

- `28360107` *distinguish agent completion from startup and session changes*: adds
  `AgentInfo.completion_seq` (optional). This is additive, and `AgentInfo` has no
  `deny_unknown_fields`. Placement's `external.rs` infers completion from `idle`/`done` plus pane
  text. `completion_seq` could replace that inference once tagged; it becomes a `not-used`
  candidate when a release carries it.
- `3602c757` *refresh forwarded ssh agents after reconnect*: adds `server.ssh_agent.register`
  (a connection-held method, like `events.subscribe`) and `ServerCapabilities.ssh_agent_registration`.
  It also adds `capabilities.ssh_agent_registration` to `herdr status server --json`. All additive.
  It is relevant to saved-machine placement only if a remote child needs the local ssh agent, which
  `remote.rs` deliberately does not forward (`ForwardAgent=no`).
- `1c6397b1` *reuse ssh authentication and add machine recovery commands*: adds
  `herdr machine status [--json]` / `machine reconnect`. `machine list --json` is unchanged.
- `dd33ddf2` *move worktree lookups async*: `worktree.*` internals only. cyrup sends no worktree
  method.
