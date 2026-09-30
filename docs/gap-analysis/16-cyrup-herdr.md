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
own kind. Ids are `HERDR-NNN`. **Next free id: `HERDR-007`.**

> ### CLOSURES 2026-09-28 — both remaining lows (`HERDR-002`, `HERDR-003`); no partials
>
> Landed on `claude/lows-next`, not yet committed. Each row and body section carries its evidence.
> **The combined tree has not passed the gates.** The integration pass ran only
> `cargo fmt --all -- --check`, which passed and changed no files. Clippy and `cargo nextest` were
> not run on the combined diff, because `/` had 2.9 GB free, below the pass's 4 GB floor (`target/`
> is 24 GB). Until they run, treat "green" as per-lane only. **Counted set after this pass
> (`count_open_items.py`): 0 critical · 0 high · 0 medium · 0 low = 0 open, 4 closed** (was 0 medium ·
> 2 low = 2 open, 2 closed). This area has no open rows. By row:
>
> - `HERDR-002` — the crate is pinned to the tag `v0.9.1` (`065ef9d6`), and every `tmp/herdr/...:N`
>   citation is a line of `git show v0.9.1:<path>`. The census is 104 = 28 ported + 76 absent. This
>   is a doc and citation change, so there is no red-first test.
> - `HERDR-003` — a stopped remote herdr reads as "stopped or incompatible". This is a
>   `[CYRUP-DELTA]` against pi's `parseHerdrEndpoint`, because herdr's `not_running` JSON has `null`
>   `version`/`protocol`. The lane showed two tests red without the fix.
>
> **Ledger corrections.** (1) `git -C tmp/herdr rev-parse v0.9.1` returns the **annotated tag
> object**, not `065ef9d6`. The commit is `v0.9.1^{commit}`. (2) At `v0.9.1`,
> `docs/next/website/src/content/docs/socket-api.mdx` is the release documentation, and it is byte
> for byte `d59d0603`'s `docs/preview/` copy (`git diff` of the two blobs is empty). The tag's own
> `docs/preview/` copy is older. The re-measure's "`socket-api.mdx` @v0.9.1" means the `docs/next/`
> copy. (3) `tmp/herdr` has moved since the 2026-09-24 re-measure. It is a shallow clone (one
> graft; `git rev-list --count d59d0603` = 50) whose HEAD is now `fff6c820`
> (`preview-2026-09-21-0ff0f27e2226-39-gfff6c820`), and it carries 90 tags, 57 of them `v*`, back
> to `v0.7.5`. The pins table below records the clone as it was on 2026-09-24. Every citation in
> this file is at a tag or a named commit, so none moves. (4) The JSON Schema count of 103 holds
> at the tag: `pane.graphics.stream` is `schemars(skip)` (`v0.9.1:src/api/schema.rs:207`), one fewer
> than the 104 methods.
>
> **Left, outside this area.** The other `@ d59d060` anchors in `cyrup-ext-subagents`
> (`herdr/mod.rs`, `herdr/reporter.rs`, `inspectors/herdr/actions.rs`,
> `inspectors/shell_command.rs`) and `cyrup-it` `herdr_status_bridge_integration.rs` name the
> commit correctly and resolve as written, so they were left alone.
> `inspectors/herdr/{client,focus}.rs` still argue that "no release tags" means it cannot be
> established when a method appeared. Now that `tmp/herdr` has release tags back to `v0.7.5`, that
> could be settled. It is a SUBA-side claim, and this pass did not change it.

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
| ~~HERDR-001~~ | ~~medium~~ **CLOSED 2026-09-27** | client-bug | S | The inspector's socket adapter turns `pane split --current` into "split the focused pane of the active workspace". herdr's `--current` means the caller's own pane (`HERDR_PANE_ID`) — **CLOSED 2026-09-27**: `SocketHerdrClient` now carries the caller's pane id and the `pane split` arm targets it, honouring `--current`, `--pane <id>` and a positional id, and parsing `--direction` instead of hard-coding `Right` (`crates/cyrup-ext-subagents/src/inspectors/herdr/client.rs:707-793`). Verify: `cyrup-ext-subagents inspectors::herdr::client::tests::{the_pane_split_request_line_carries_the_callers_pane_id,pane_split_targets_the_callers_pane_and_honours_direction}`. |
| ~~HERDR-002~~ | ~~low~~ **CLOSED 2026-09-28** | tooling | S | **CLOSED 2026-09-28** (on `claude/lows-next`): the crate is now pinned to the tag, herdr **`v0.9.1` (`065ef9d6`)**: `crates/cyrup-herdr/Cargo.toml:3` and the `lib.rs` crate doc (`:3-15`), which says every `tmp/herdr/<path>:N` citation is a line of `git -C tmp/herdr show v0.9.1:<path>`, a bare `socket-api.mdx:N` is the tag's `docs/next/` copy, and `d59d0603` was the first-written commit and differs on API paths in exactly two ways. The census is re-anchored to the tag: 104 methods (`lib.rs:81`), 28 ported + 76 absent (`:84`, `:94`), and the not-ported table's rows add to 76 (2+4+14+4+22+2+27+1; `clear` is gone from the geometry row). `transport.rs::request`'s doc (`transport.rs:284-294`) now states both error-id shapes: `v0.9.1` writes id `""` for every undeserialisable line (`server.rs:179-191`) and the probe id `<id>:sub:<n>:probe` for a failed subscribe setup (`subscriptions.rs:184-192`, `:210`), while `241063f7` on `main` echoes the request id. The remap was checked mechanically: all 334 full-path citations were paired HEAD-to-working and resolve at `v0.9.1` to the text `d59d0603` had (for ranges spanning `d59d0603`-only code, the start and end lines are the same function boundaries), and 134 bare `file.rs:N` remaps were checked the same way. The verifier also relabelled three places outside the crate that called `d59d060` "v0.9.1": `cyrup-ext-subagents/src/inspectors/herdr/{client,focus}.rs` module docs and `cyrup-ext-subagents/Cargo.toml:61`. Doc and citation change only, so there is no red-first test; Verify is the row's own: `git -C tmp/herdr show v0.9.1:src/api/schema.rs \| grep -c '#\[serde(rename = "'` = 104 = the crate's census. Original: The crate's pin "herdr v0.9.1 (`d59d060`)" names an untagged `main` commit that is not `v0.9.1`, and every `tmp/herdr/...:N` citation in the crate is at that commit |
| ~~HERDR-003~~ | ~~low~~ **CLOSED 2026-09-28** | client-bug | S | **CLOSED 2026-09-28** (on `claude/lows-next`), as a deliberate divergence from pi: `crates/cyrup-herdr/src/remote.rs::parse_endpoint` now checks the absolute `socket` (`remote.rs:190-194`), then session identity (`:195-203`), then `running`/`compatible`/`endpoint_compatible` (`:204-212`), and only then requires a string `version` and numeric `protocol` (`:213-229`). A machine whose herdr server is stopped now gets "The selected remote Herdr session is stopped or incompatible." instead of "returned incomplete identity". The `[CYRUP-DELTA]` doc (`remote.rs:165-172`) names the dependency: herdr `v0.9.1`'s `NotRunning` arm writes `version: null, protocol: null` (`git -C tmp/herdr show v0.9.1:src/cli/status.rs`, `:340-352`), while pi-subagents' `parseHerdrEndpoint` checks `version`/`protocol` alongside `socket`, before `running`. Its check order is the same at `v0.68.0` and at the pinned `v0.71.0` (the file is identical; the function is at `:50-60`). Every refusal sentence is still pi's. Tests (the lane showed them red without the fix): `cyrup-herdr tests::machine_remote::remote::a_stopped_server_reads_as_stopped_not_as_incomplete_identity` (`machine_remote.rs:237`; it also pins that the socket and session checks still come first and that a running server without a version is still incomplete) and `…::discovery_of_a_stopped_remote_herdr_reports_it_stopped` (`:277`; `SshTransport::discover_endpoint` over an ssh stand-in whose `herdr status server --json` prints `v0.9.1`'s `not_running` JSON). Original: A stopped remote herdr is reported as "returned incomplete identity" rather than "stopped or incompatible", because `version`/`protocol` are `null` in herdr's `not_running` status JSON and are checked first |
| ~~HERDR-004~~ | ~~low~~ **CLOSED 2026-09-28** | client-bug | S | **Filed and closed 2026-09-28** (on `claude/lows-next`): a refused `pane.report_agent` could strand the pane on stale state for up to 45 s. The de-duplicator recorded a report when it was handed to the reporter, not when herdr accepted it, so a same-state edge arriving while that report was in flight was swallowed; if the report then failed, nothing re-sent it until the refresh tick (`METADATA_REFRESH`, 45 s). Now `StateModel` notes a swallowed edge (`swallowed_since_handout`) and `invalidate_last_report` returns the report to send again at once, and `send_state` re-sends it — at most once per swallowed edge, so a refusing herdr is not retried in a loop. Found as an intermittent failure of `cyrup-it` `herdr_status_bridge_integration::a_rejected_report_never_disturbs_the_agent` under full parallel load (it fires the next edge once the fake server has received the report, before the bridge has read the refusal). Tests: `herdr::state::tests::an_edge_swallowed_behind_a_failed_report_is_re_sent_at_the_failure` (fails with the new branch gutted) and the updated `an_invalidated_report_is_sent_again`. |
| ~~HERDR-005~~ | ~~low~~ **CLOSED 2026-09-30** | client-bug | S | **Filed and closed 2026-09-30** (on `claude/lows-batch3`). A timed-out placed run could leave its remote runtime dir behind: the relay's remote loop (`crates/cyrup-herdr/src/relay.rs:67` `RELAY_SCRIPT`) outlives the local ssh kill and rewrites `.relay-*` files every 200 ms, so `rm -rf` could see `rmdir: Directory not empty`, and `finish()` swallowed the failure (`let _ =`). Surfaced as a load-only failure of `cyrup-ext-subagents placement::tests::a_timed_out_placed_run_closes_its_pane` (1/12025 in a full workspace run). Fix: `REMOVE_SCRIPT` (`crates/cyrup-ext-subagents/src/placement/native.rs:674-688`) keeps its prefix/dir/not-symlink guards, then moves the dir into a private `mktemp -d` holder and removes that, so no path-based writer can add entries to the tree being deleted; a failed removal now logs upstream's "Could not remove the exact owned remote runtime directory" (`:880-892`). Upstream (`herdr-placed-run.ts` @v0.68.0 `#performCleanup`/`removeRemoteRuntimeDir`) has no such race because its only remote writer lives in the pane. Verify: `placement::tests::a_timed_out_placed_run_removes_its_runtime_dir_while_the_relay_still_writes` (a fake `rm` widens the window; 10/10 red without the fix, leaving exactly one `.relay-*` file), and 30/30 of both timed-out tests under 3-core CPU saturation. |
| ~~HERDR-006~~ | ~~low~~ **CLOSED 2026-09-30** | client-bug | S | **CLOSED 2026-09-30** (on `claude/lows-batch4`): the relay's remote loop now ends when its runtime dir is gone, and neither an upload nor `run.sh` can bring a removed dir back. `RELAY_SCRIPT` (`crates/cyrup-herdr/src/relay.rs:79,119`) checks `[ -d "$rt" ]` at the top of the 60 s start-wait loop and of the main loop and exits 66 (the status the preflight already uses for a missing dir; locally it is a lost stream, and a reconnect is refused by the preflight, ending as `RelayEnd::Unknown("… The placed run's runtime directory is gone.")`); `REMOVE_SCRIPT` (HERDR-005) moves `$rt` away, so the check fires within one 200 ms poll. `UPLOAD_SCRIPT` (`relay.rs:144`) replaced `mkdir -p "$d"` with `[ -d "$d" ] || exit 66`; accepted consequence: an upload issued before `run.sh` has made the channel dirs now fails and is retried by `upload_dir` on the next 250 ms tick instead of creating them. The `run.sh` template (`crates/cyrup-ext-subagents/src/placement/native.rs:629,636`) starts with `[ -d "$rt" ] || exit 66` and builds its channel dirs with one plain `mkdir` per level (parents first, idempotent), never `mkdir -p`. Note on the old behaviour: under dash the failed `: > "$new"` aborted the loop by accident (status 2); under bash it kept looping (verified against the pre-fix script: dash exited, bash stayed alive after `$rt` was moved). Verify: `cyrup-herdr` `tests::relay::{the_remote_relay_ends_when_its_runtime_dir_is_removed (dash and bash), the_remote_relay_does_not_wait_out_the_start_deadline_for_a_removed_dir, an_upload_into_a_removed_run_creates_nothing}` (red without the fix: `assertion left == right failed: /bin/sh: ExitStatus(512) left: Some(2) right: Some(66)`, the start-wait test timing out, and the upload test finding the recreated dir); `cyrup-ext-subagents` `placement::tests::a_timed_out_placed_run_leaves_no_relay_running_on_the_machine` (through `run_sync`, dash and a bash `sh` with stderr discarded; red without the loop check: `bash sh=true: the relay is still running after its run was cleaned up: [1548]`; the fake ssh now puts `$HOME/.local/bin` first on the remote `PATH`, as a real login does, so the machine's `sh` can be swapped) and `the_run_script_refuses_to_recreate_a_removed_runtime_dir` (renders and runs `run.sh` under dash and bash: status 66 and no dir for a removed `$rt`, four channel dirs plus `started` for a live one; red without the guard). *Earlier text:* **NEW 2026-09-30** (found while closing HERDR-005). The relay's remote loop is never stopped: killing the local ssh client does not end the remote command, so `RELAY_SCRIPT` (`crates/cyrup-herdr/src/relay.rs:67`) runs on the machine until its stdout/stderr pipe breaks, after the run it serves has settled. Related: `UPLOAD_SCRIPT` and `run.sh`'s startup use `mkdir -p`, so an upload or a still-starting `run.sh` in flight during cleanup could recreate a removed runtime dir. Fix direction: the relay exits when `$rt` disappears (and the upload does not recreate a removed root), plus a test through the fake machine that no relay process survives `finish()`. |

---

## HERDR-001 — `pane split --current` over the socket splits the wrong pane

**Kind** client-bug · **Severity** medium · **Effort** S · **Confidence** confirmed (both sides read; not observed live) · **CLOSED 2026-09-27**: herdr at the pin: in the `pane split` parser, `"--current"` sets `pane_id = Some(env_pane_id … .ok_or("--current requires HERDR_PANE_ID")?)` which becomes `PaneSplitParams.target_pane_id` (`tmp/herdr/src/cli/pane.rs`, `:636` initialising it to the caller's pane, `:645-667` for the flags that overwrite that slot, `:723-727` making `--direction` mandatory), while `handle_pane_split` with `target_pane_id: None` and `workspace_id: None` targets `self.state.active`'s `focused_pane_id()` (`src/app/api/panes.rs`) — and `socket-api.mdx` says the same. So `--current` (the caller's pane) and an omitted target (whatever the human is focused on) are DIFFERENT targets, which the adapter's comment had claimed were equivalent. Fixed at `crates/cyrup-ext-subagents/src/inspectors/herdr/client.rs:715`, where `target_pane_id` is initialised to `Some(caller_pane_id.to_owned())` as herdr's own parser does, with `:724` honouring a positional id, `:730` `--pane <id>`, `:735-736` `--current` mapping to the caller's pane, and `:739-744` parsing `--direction right|down` instead of hard-coding `Right`; `:787` is the usage line and `:793` passes the resolved id into `PaneSplitParams`. The client already held the caller's `HerdrPane` through `SocketHerdrClient::for_pane` / `from_process_env`, so the pane id needed only to be threaded, and the false equivalence comment is replaced by `:559-560` and `:707-708` citing herdr's real lines. A project pane or inspector pane now opens beside the cyrup pane that asked for it, not beside whatever pane the human happens to be focused on — possibly in another tab or workspace — so the socket route and the CLI route (`cyrup-intercom`'s `HerdrLauncher`, which was already correct) now land the same feature in the same place. Verify: `cyrup-ext-subagents inspectors::herdr::client::the_pane_split_request_line_carries_the_callers_pane_id` (the row's own Verify: the `pane.split` request line carries `"target_pane_id":"<HERDR_PANE_ID>"`) and `…::pane_split_targets_the_callers_pane_and_honours_direction` (`--direction down`, the positional and `--pane` forms); `client.rs:984` records that gutting the `target_pane_id` initialiser back to `None` turns the first two rows red.

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

> **CLOSED 2026-09-28** (on `claude/lows-next`). The pin is `v0.9.1` (`065ef9d6`) in
> `Cargo.toml:3` and `lib.rs:3-15`. Every `tmp/herdr/<path>:N` citation is re-anchored to
> `git show v0.9.1:<path>` (334 full-path and 134 bare citations checked mechanically). The census
> is 104 = 28 + 76, and `transport.rs:284-294` states both error-id shapes. Evidence is in the
> `## Open items` row. **Ledger correction:** `git rev-parse v0.9.1` returns the annotated tag
> object; the commit is `v0.9.1^{commit}` = `065ef9d6`. See the closures block at the top for the
> rest.

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

> **CLOSED 2026-09-28** (on `claude/lows-next`), as a `[CYRUP-DELTA]` against pi-subagents'
> `parseHerdrEndpoint` (`v0.71.0:src/runs/shared/herdr-connection.ts:50-60`, identical to
> `v0.68.0`). `parse_endpoint` judges liveness (`remote.rs:204-212`) before it requires
> `version`/`protocol` (`:213-229`), because herdr `v0.9.1`'s `NotRunning` arm writes both as `null`
> (`status.rs:340-352`). Tests: `tests::machine_remote::remote::{a_stopped_server_reads_as_stopped_not_as_incomplete_identity,discovery_of_a_stopped_remote_herdr_reports_it_stopped}`.
> Not a gap: cyrup refuses a negative `protocol` as incomplete identity, while pi accepts any
> number. herdr writes a `u32`, so the difference cannot arise. (The pi citation below says `:49-60`;
> the function starts at `:50`.)

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

## HERDR-004 — A refused state report is not re-sent for an edge swallowed while it was in flight

> **Filed and CLOSED 2026-09-28** (on `claude/lows-next`).

**Kind** client-bug · **Severity** low · **Effort** S · **Confidence** observed (intermittent `cyrup-it` failure under load; mechanism read in the code)

**cyrup** — `crates/cyrup-ext-subagents/src/herdr/state.rs` `settle` recorded `last_reported` when a report was handed out; `reporter.rs` `send_state` called `invalidate_last_report` only after `report_agent` returned `Err`. An edge that settled to the same state in between was de-duplicated against a report herdr never accepted, and only the 45 s refresh tick (`reporter.rs` `METADATA_REFRESH`) re-sent it.

**herdr / pi** — cyrup is its own lifecycle authority for the pane, so this reporter has no pi counterpart; pi-subagents' `herdr-status.ts` metadata lane does not de-duplicate at all and relies on the next transition or TTL refresh.

**Impact** — a pane can show the wrong agent state for up to 45 s after herdr refuses or drops one report, whenever the edge that followed asked for the same state.

**Fix (landed)** — `StateModel::swallowed_since_handout` is set when `settle` de-duplicates and cleared on every hand-out; `invalidate_last_report` now returns `Some(report)` when it was set, and `send_state` loops to send it. A failure with no swallowed edge behind it returns `None`, so retries stay bounded by edges.

**Verify** — `herdr::state::tests::an_edge_swallowed_behind_a_failed_report_is_re_sent_at_the_failure`; `cyrup-it` `herdr_status_bridge_integration::a_rejected_report_never_disturbs_the_agent` is now deterministic.

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
