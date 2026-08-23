---
stage: new
status: done
updated: 2026-08-22 15:58
---

# The Initialization Spine And The One Production `ProxyEnv`

## Description

Thirty-one `medium`/`low` units in the cyrup-mcp port are all blocked behind, or all write through,
**one missing mechanism**: a live `McpState` with the verbs that mutate it, reachable from a real
session.

Two facts fix the shape of this work, and both were re-verified against the tree today.

**1. `initialize_mcp` stops at §8.** [`runtime.rs:125-292`](../../crates/cyrup-mcp/src/runtime.rs)
executes wiring steps 1–12 of 13a §8 — config, auth storage, OAuth runtime, manager + four setters,
lifecycle, the two closures, the `McpState` commit, and the two owner cleanups — then falls straight
through the zero-enabled-servers early return at
[`runtime.rs:285-290`](../../crates/cyrup-mcp/src/runtime.rs) to `Ok(state)` at
[`runtime.rs:291`](../../crates/cyrup-mcp/src/runtime.rs). §9 (cache bootstrap), §10 (per-server
lifecycle registration and cache rehydration), §11 (the bounded connect pass), §12 (the metadata
build), §13 (failure tracking), §14 (the `MCP_DIRECT_TOOLS` bootstrap and the summary notification)
and §15 (the lifecycle callbacks) are **not written**. Fourteen of the thirty-one units are steps of
that one function body.

**2. `ProxyEnv` has no production implementor.** The trait is declared at
[`proxy.rs:1436-1585`](../../crates/cyrup-mcp/src/proxy.rs) with 30 methods. Its **only** `impl` is
`FakeEnv` at [`proxy.rs:4932`](../../crates/cyrup-mcp/src/proxy.rs), inside the `#[cfg(test)]`
opening at [`proxy.rs:4862`](../../crates/cyrup-mcp/src/proxy.rs). `ProxyCtx::new`
([`proxy.rs:1610`](../../crates/cyrup-mcp/src/proxy.rs)) likewise has exactly one caller,
[`proxy.rs:5094`](../../crates/cyrup-mcp/src/proxy.rs), also inside that module. Fourteen of the
trait's methods are the *only* declaration in the crate of a verb this group must implement —
`record_failure`, `clear_failure`, `failure_age_seconds`, `update_status_bar`,
`update_server_metadata`, `update_metadata_cache`, `mark_keep_alive_after_connect`,
`commit_prompt_metadata`, `sync_tool_surface`, `lazy_connect`, `guard_mcp_output`, `all_tool_names`,
`touch`, `increment_in_flight`. Grep confirms there is no free function or `McpState` method behind
any of them.

So: **one production `ProxyEnv`, plus the completed `initialize_mcp` body, is the single mechanism
all 31 units write through or read from.** Grouping them by file would split producers from
consumers at every seam:

* MCP-024 defines `record_failure` / `FAILURE_BACKOFF_MS`, which MCP-033's `lazy_connect` calls,
  MCP-027's reconnect-failure callback calls, and MCP-137's `failedAgoSeconds` reads.
* MCP-028's `update_server_metadata` writes the five maps MCP-021 rehydrates and MCP-031 flushes.
* MCP-078 / MCP-137 / MCP-138 / MCP-032 are **one payload contract**: the shape, its builder, its
  publisher, and its three consumers.
* MCP-006 leads everything that speaks to the user. The crate has **zero** production
  `HostServices::notify` call sites (`grep -rn '\.notify(' crates/cyrup-mcp/src` returns only doc
  comments at [`state.rs:123`](../../crates/cyrup-mcp/src/state.rs) and
  [`owner.rs:293-294`](../../crates/cyrup-mcp/src/owner.rs)), and the `fenced!` list at
  [`owner.rs:374-465`](../../crates/cyrup-mcp/src/owner.rs) covers **31 of the trait's 66 methods**
  (counted over [`services.rs:190-704`](../../crates/cyrup-ext/src/host/services.rs)). An unlisted
  method silently falls through to the trait default — "denied/empty" — *even while the owner is
  active*, which is documented as a known hole at
  [`owner.rs:302-311`](../../crates/cyrup-mcp/src/owner.rs). Every notification obligation in this
  group (MCP-018's disabled notice, MCP-026's bootstrap notice, MCP-027's callbacks, MCP-038's
  `set_active_tools` at [`owner.rs:442`](../../crates/cyrup-mcp/src/owner.rs)) is swallowed until
  the fence is complete.
* MCP-087's `parallel_limit` is in-group because MCP-022 and MCP-130 are its only consumers.
* MCP-017 and MCP-224 are the same defect from two directions: `MaterializedResources::cleanup`
  ([`renderers.rs:860`](../../crates/cyrup-mcp/src/renderers.rs)) has no `add_cleanup` site, and
  `MaterializedResources::new` has **no production caller at all** — every production
  materialization falls to the process-global session at
  [`renderers.rs:724`](../../crates/cyrup-mcp/src/renderers.rs), which nothing ever cleans up.
* MCP-225 is fully implemented and merely unwired (see below), and its only possible wiring point is
  this group's production implementor.

**Execute as ordered sub-waves under ONE owner, not as 31 tasks.**

---

## Verification pass — what the ledger got right, and what it did not

Every unit below was re-read against the tree on 2026-08-22, per
[MCP_HIGH_SEVERITY_BACKLOG.md](MCP_HIGH_SEVERITY_BACKLOG.md)'s rule that a `missing` row is a lead,
not a verdict. **All 31 are genuinely open.** But four rows are wrong about *why*, and one is wrong
about *what*:

| unit | the row says | what the tree says today |
|---|---|---|
| `MCP-225` | *"the env variable is never actually read in production"* ([:739](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | **The unit is fully implemented.** `McpSettings::output_guard(env)` at [`config.rs:1246-1261`](../../crates/cyrup-mcp/src/config.rs), `env_kill_switch` (tri-state, unrecognised ⇒ `None`) at [`config.rs:1086`](../../crates/cyrup-mcp/src/config.rs), `positive_int` above it, `ResolvedOutputGuard` at [`config.rs:1097`](../../crates/cyrup-mcp/src/config.rs), and the bridge `McpOutputGuardOptions::from_resolved` at [`renderers.rs:1000`](../../crates/cyrup-mcp/src/renderers.rs). `grep -rn '\.output_guard(' src` returns **zero** call sites. This is a **wiring task**, ~15 lines, not a port |
| `MCP-087` | *"port `parallelLimit`, argv scan, `toStringRecord`, `normalizeDirectToolInputSchema`"* ([:590](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | **Three of four are done**: `config_path_from_argv` at [`config.rs:1763`](../../crates/cyrup-mcp/src/config.rs), `to_string_record` at [`config.rs:328`](../../crates/cyrup-mcp/src/config.rs), `normalize_direct_tool_input_schema` at [`registration.rs:1540`](../../crates/cyrup-mcp/src/registration.rs). Only `parallel_limit` is absent, and **`MCP-087` and `MCP-130` are the same missing function** |
| `MCP-015` | *"`runtime.rs:139` binds the combined token to `_runtime_signal` (unused)"* ([:508](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | Overturned. `runtime_signal` is bound at [`runtime.rs:149`](../../crates/cyrup-mcp/src/runtime.rs) and **consumed** at [`runtime.rs:208`](../../crates/cyrup-mcp/src/runtime.rs) (`manager.set_runtime_signal`). The owned-`Arc` discipline is also already correct: `ui` is built from `snapshot.services` at [`runtime.rs:144-148`](../../crates/cyrup-mcp/src/runtime.rs). **MCP-015 reduces to exactly MCP-016's two live closures** and has no independent obligation left |
| `MCP-020` | *"the `idleOverride` derivation exists…"* ([:513](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | Restated. Every manager-side primitive exists — `register_server` at [`lifecycle.rs:569`](../../crates/cyrup-mcp/src/lifecycle.rs) (disabled early-return at `:575`, `idle_timeout.is_some()` gate at `:586-591`), `mark_keep_alive` at `:615`, `mark_keep_alive_after_connect` with its three guards at `:634-647`, `LifecycleOverrides` at `:427`. The **only** gap is the caller: `register_server`'s sole call sites are [`extension.rs:983`](../../crates/cyrup-mcp/src/extension.rs) and `:1030`, both past the `#[cfg(test)]` opening at [`extension.rs:817`](../../crates/cyrup-mcp/src/extension.rs) |
| `MCP-036` | *"the fingerprint diff … does not exist"* ([:530](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | Restated, same as `MCP-217`. The diff **is** built: `LateSink::should_register_tool` at [`registration.rs:2032-2034`](../../crates/cyrup-mcp/src/registration.rs), `should_register_proxy` at `:2036`, and `sync_tool_surface` at [`extension.rs:166-254`](../../crates/cyrup-mcp/src/extension.rs) drives it. What is absent is the **removal half**: no `added`/`updated`/`deactivated` counts, no `deactivate_tools`, no re-activation, and no `MCP: direct tools refreshed (+a, ~u, -d)` notice |

**Line numbers in the ledger have drifted by 150–400 lines** across `extension.rs`, `runtime.rs` and
`lifecycle.rs` (e.g. the row for MCP-012 cites `extension.rs:527-530`; the code is at
[`extension.rs:697-699`](../../crates/cyrup-mcp/src/extension.rs); MCP-045's row cites
`extension.rs:535-559`, actual [`extension.rs:713-745`](../../crates/cyrup-mcp/src/extension.rs)).
Every line cited **in this file** was read today.

---

## The deliverable, in one shape

One new module, **`crates/cyrup-mcp/src/env.rs`**, holding two things and nothing else:

1. **The `init.ts` live-state verbs** — §13's failure tracking, §17's metadata writes, §18's status
   bar, §19's `lazyConnect`, and §3.16's snapshot builder — as free functions over `&Arc<McpState>`.
   These are the bodies `initialize_mcp` §9–§15 calls directly.
2. **`RuntimeEnv`** — the crate's one production `ProxyEnv`, every method a one-line delegation to
   (1), to `McpServerManager`, to `crate::oauth`, or to `ProxyCtx`'s already-written helpers.

Why a new module rather than growing `runtime.rs`: that file's own doc
([`runtime.rs:27-35`](../../crates/cyrup-mcp/src/runtime.rs)) declares it has exactly two halves —
the runtime *build* and the *connection* — that "share no state". These verbs are a third thing:
mutation of a committed `McpState`. Putting them in `runtime.rs` would break the invariant that
file's tests depend on (the connection half is testable without an `McpState`).

`RuntimeEnv` is constructed by a new `McpExtension::install_runtime_env(&self, state: &Arc<McpState>)`
placed beside the existing `install_surface_sync` at
[`extension.rs:425-439`](../../crates/cyrup-mcp/src/extension.rs) — same `Weak<McpExtension>` shape,
same call site, same reason. Wave 2's `MCP-011` commit tail calls both.

### Two methods this group does not implement, named rather than smuggled

`ProxyEnv::call_tool` ([`proxy.rs:1465`](../../crates/cyrup-mcp/src/proxy.rs)) and `read_resource`
(`:1478`) are **`MCP-164`, Wave 1**. `RuntimeEnv` declares them and returns
`ProxyCallError` describing the unbuilt seam, in the same spirit as
`ManagerSupervisor::unbound` at [`lifecycle.rs:277`](../../crates/cyrup-mcp/src/lifecycle.rs) — a
loud, greppable failure, never a fabricated success. Wave 1 replaces both bodies.

`ProxyEnv::format_schema` (`:1554`) and `render_ts_shape` (`:1556`) have **no implementation anywhere
in the workspace** and belong to `MCP-211` and `MCP-091`, neither of which is scheduled in any task
file. See *Out-of-group blockers* below.

---

## Sub-wave A — the payload contract

### `MCP-078` — port the status-snapshot types · `partial`

**Unmet.** [`state.rs:405-414`](../../crates/cyrup-mcp/src/state.rs) defines `McpStatusSnapshot` as
four `Vec<String>` fields (`connected`, `idle`, `failed`, `pending_auth`). That is not the upstream
shape. Absent entirely: `MCP_STATUS_SNAPSHOT_VERSION`, the closed six-variant
`McpServerRuntimeStatus`, `McpServerStatusSnapshot`, and the six-key envelope. `grep -rn
'McpServerRuntimeStatus\|SNAPSHOT_VERSION\|failed_ago_seconds' src` returns zero hits.

**Free to change**: the only readers of `McpState::subscribe_status`
([`state.rs:241`](../../crates/cyrup-mcp/src/state.rs)) are
[`lifecycle.rs:2453`](../../crates/cyrup-mcp/src/lifecycle.rs), a test. The two production
publishers — [`runtime.rs:287`](../../crates/cyrup-mcp/src/runtime.rs) and
[`lifecycle.rs:1573`](../../crates/cyrup-mcp/src/lifecycle.rs) — both send `Default::default()`,
which under the new shape becomes *correct*: it is exactly `publishMcpStatusShutdown`'s literal
all-zero payload.

### `MCP-137` — status snapshot construction · `missing`

**Unmet.** `create_mcp_status_snapshot` does not exist. The six-way precedence ladder **does** exist
once, as text, in `execute_status` at
[`proxy.rs:1943-1955`](../../crates/cyrup-mcp/src/proxy.rs). Do not write a third copy: write the
builder over `McpState`, and leave `execute_status` alone — it reads
`ProxyCtx::tool_metadata`, the forward-declared duplicate map whose own doc
([`proxy.rs:1583-1587`](../../crates/cyrup-mcp/src/proxy.rs)) says MCP-207 deletes it. Collapsing the
two is MCP-207's job, and doing it here would edit a file Wave 3 is rewriting.

Ordering is load-bearing: `McpConfig::mcp_servers` is an `IndexMap`
([`config.rs:681`](../../crates/cyrup-mcp/src/config.rs) iterates it directly), so config order is
already preserved — do not introduce a `BTreeMap` anywhere on this path.

### `MCP-138` — publish the status snapshot · `partial`

**Unmet.** The channel only ever carries `Default::default()` (the two sites above). No caller ever
builds a real snapshot. This closes when `update_status_bar` (MCP-032) exists and is called from
§11/§14/§15 and from every lifecycle callback.

---

## Sub-wave B — the fence and the two swallow points

### `MCP-006` — `createOwnedUi` as a fenced services handle · `partial`

**Unmet.** The `fenced!` invocation at
[`owner.rs:374-465`](../../crates/cyrup-mcp/src/owner.rs) lists **31** methods; `HostServices`
declares **66** ([`services.rs:190-704`](../../crates/cyrup-ext/src/host/services.rs)). The 35
missing ones are:

```
append_entry  branch  editor_text  entries  has_pending_messages
http_close_stream  http_poll_stream_chunk  http_request  http_request_stream
proc_kill  proc_poll_exit  proc_read_stderr  proc_read_stdout  proc_spawn  proc_write_stdin
scoped_models  session_name  set_editor_text  set_footer  set_header
set_hidden_thinking_label  set_label  set_session_name  set_theme  set_title
set_tools_expanded  set_working_indicator  set_working_visible
system_prompt  system_prompt_options  theme_by_name  theme_list  thinking_level
tools_expanded  tree
```

Each of those, called through `state.ui`, reaches the trait default instead of the live backend —
**even while the owner is active**. `owner.rs:302-311` records this as a known hole and names
MCP-006 as the unit that closes it. Add all 35 to the `fenced!` list with the inert value the arm's
return type demands (`()` for the setters, `None`/`Value::Null`-shaped empties for the readers,
`Err(Self::inert_reason(&self.owner))` for the `Result` arms — the convention the existing arms
already follow).

### `MCP-030` — `notifyToolMetadataUpdated` must never let a hook break a connect · `partial`

**Unmet.** `McpState::notify_tool_metadata_updated`
([`state.rs:250-258`](../../crates/cyrup-mcp/src/state.rs)) already clones the listener out from
under the lock and invokes it outside — good, and it must stay. What is absent: the panic
containment (`grep -rn catch_unwind src` returns nothing) and the debug line. The crate sets
`clippy::panic = "deny"` (workspace `Cargo.toml:100`), so a panicking listener is a genuine abort
risk on the connect path.

### `MCP-045` — the `tool_result` `isError` override · `partial`

**Unmet.** `McpExtension::on_event`
([`extension.rs:713-745`](../../crates/cyrup-mcp/src/extension.rs)) matches `SessionStart`, `Input`
and `SessionShutdown` and falls through everything else at
[`extension.rs:728`](../../crates/cyrup-mcp/src/extension.rs), whose comment reads
`// MCP-045 fills the isError override.` `EventKind::ToolResult` **is** subscribed
([`registration.rs:119`](../../crates/cyrup-mcp/src/registration.rs)), so the event arrives and is
dropped. `HostEvent::ToolResult` carries `details: Option<Value>`
([`event.rs:284-295`](../../crates/cyrup-ext/src/event.rs)) and `EventPatch::ToolResult` has the
exact four-`Option` shape
at [`contract.rs:51-56`](../../crates/cyrup-ext/src/contract.rs), whose `apply_patch`
([`contract.rs:96-106`](../../crates/cyrup-ext/src/contract.rs)) sets `is_error` only when `Some` —
a 1:1 match for pi's field-by-field merge.

---

## Sub-wave C — the live-state verbs (`env.rs`)

### `MCP-024` — failure tracking with a 60-second backoff · `missing`

**Unmet.** `grep -rn 'BACKOFF\|MAX_FAILURE_MESSAGE' src` returns only unrelated hits
([`lifecycle.rs:111`](../../crates/cyrup-mcp/src/lifecycle.rs) is the keep-alive retry ceiling).
`McpState::failure_tracker` / `failure_messages`
([`state.rs:115`](../../crates/cyrup-mcp/src/state.rs), `:117`) and `ServerFailure`
([`state.rs:386-392`](../../crates/cyrup-mcp/src/state.rs)) exist and are never written by anything
outside tests.

### `MCP-028` — `updateServerMetadata` · `missing`

**Unmet.** No function of that name exists; `ProxyEnv::update_server_metadata`
([`proxy.rs:1499`](../../crates/cyrup-mcp/src/proxy.rs)) is the only declaration, and `FakeEnv`'s
body at [`proxy.rs:4990`](../../crates/cyrup-mcp/src/proxy.rs) is `{}`.

`McpState::tool_metadata` is typed `Mutex<IndexMap<String, ServerToolMetadata>>`
([`state.rs:89`](../../crates/cyrup-mcp/src/state.rs)) where `ServerToolMetadata` is a forward
declaration carrying only `tool_names: Vec<String>`
([`state.rs:366-371`](../../crates/cyrup-mcp/src/state.rs)). MCP-028 and MCP-021 both need the real
`Vec<ToolMetadata>` ([`proxy.rs:391-407`](../../crates/cyrup-mcp/src/proxy.rs)). **This group
discharges the forward declaration** — see the implementation section.

### `MCP-032` — `updateStatusBar` · `partial`

**Unmet.** The pure half is done: `format_mcp_status` at
[`ui.rs:4641`](../../crates/cyrup-mcp/src/ui.rs), `FooterCounts::from_config` at
[`ui.rs:4629`](../../crates/cyrup-mcp/src/ui.rs), `footer_status_text` at
[`ui.rs:4663-4688`](../../crates/cyrup-mcp/src/ui.rs) covering §18 steps 3–10 including the
`configured == 0` and `off` clears. Absent: the stateful wrapper — step 1's **unconditional**
`publish_status` *before* the `!ui` return, and step 5's `connectedCount`. `footer_status_text`'s
only caller today is [`ui.rs:5956`](../../crates/cyrup-mcp/src/ui.rs), a test.

### `MCP-087` + `MCP-130` — `parallelLimit` · `partial` / `missing`

**Unmet, and they are one function.** `grep -rn 'parallel_limit\|buffer_unordered\|\.buffered(' src`
returns nothing. The only bounded fan-out in the crate is
[`lifecycle.rs:962`](../../crates/cyrup-mcp/src/lifecycle.rs)'s `for_each_concurrent`, which
discards result order and is therefore the wrong shape for §11 and §14 — both read results back **by
position** (13a MCP-022: *"the two-pass collision universe depends on all results being present"*).

### `MCP-033` — `lazyConnect` · `missing`

**Unmet.** `ProxyEnv::lazy_connect` ([`proxy.rs:1447`](../../crates/cyrup-mcp/src/proxy.rs)) is the
only declaration. Note the seam **flattens upstream's throw into a `bool`** — that is not a defect
but it does move the load-bearing detail: since an abort cannot be rethrown through this signature,
the entire observable content of §19 step 8's *"rethrows only when the signal is actually aborted"*
becomes **an abort must not `record_failure`, and a stray abort error on a live signal must**.
Reproduce that discriminator exactly.

### `MCP-225` — `resolveMcpOutputGuardOptions` and the `MCP_OUTPUT_GUARD` kill switch · **implemented, unwired**

**Unmet only as wiring.** See the verification table. Every piece exists; the `Vec<Content>` ⇄
`Vec<McpContentBlock>` bridge even carries a doc comment naming this exact call site
([`renderers.rs:568-572`](../../crates/cyrup-mcp/src/renderers.rs): *"the bridge `proxy.rs`'s
`ProxyEnv::guard_mcp_output(Vec<Content>, …) -> GuardedOutput` needs on the way in"*).

---

## Sub-wave D — `initialize_mcp` §9–§15

All eleven units below are edits to one function body,
[`runtime.rs:125-292`](../../crates/cyrup-mcp/src/runtime.rs). They cannot be split.

### `MCP-018` — the zero-enabled-servers early return · `partial`

**Unmet.** The structural half is at
[`runtime.rs:285-290`](../../crates/cyrup-mcp/src/runtime.rs): publish, return, no cache work, no
lifecycle. Absent: the `MCP: All {n} server(s) are disabled` info notice gated on
`all_entries > 0 && has_ui`. `grep -rn 'are disabled' src` returns one unrelated comment at
[`proxy.rs:5409`](../../crates/cyrup-mcp/src/proxy.rs). The lenient-`disabled` predicate the row
worries about **is already correct**: `ServerEntry::is_disabled`
([`config.rs:906`](../../crates/cyrup-mcp/src/config.rs)) with its doc at `:897` — *"Only the literal
boolean `true` disables a server"* — and `enabled_servers` at
[`config.rs:681-683`](../../crates/cyrup-mcp/src/config.rs).

### `MCP-019` — metadata-cache bootstrap · `missing`

**Unmet.** `grep -rn 'bootstrap_all\|bootstrapAll' src` returns zero hits. The primitives exist:
`McpDirs::metadata_cache` ([`dirs.rs:178`](../../crates/cyrup-mcp/src/dirs.rs)),
`load_metadata_cache` ([`dirs.rs:644`](../../crates/cyrup-mcp/src/dirs.rs)), `save_metadata_cache`
([`dirs.rs:669`](../../crates/cyrup-mcp/src/dirs.rs)), `CACHE_VERSION = 1`
([`dirs.rs:471`](../../crates/cyrup-mcp/src/dirs.rs)). `save_metadata_cache` has **no production
caller** (`grep` shows only `dirs.rs` internals and tests) — this unit gives it its first.

The two-way split is the whole unit: *file absent* ⇒ `bootstrap_all = true` **and** write the empty
cache; *file present but unparseable* ⇒ rewrite empty and **do not** bootstrap. Collapsing them turns
the corrupt-cache path from cheap into a connect storm.

### `MCP-020` — per-server lifecycle registration and idle-override derivation · `partial`

**Unmet: the caller.** See the verification table — every callee exists. Also found while reading:
`initialize_mcp` constructs the lifecycle manager with a hardcoded
`Arc::new(|_| false)` for `hasPendingAuth`
([`runtime.rs:219`](../../crates/cyrup-mcp/src/runtime.rs)), so **the idle sweep and the health check
will reap a server in the middle of an OAuth login**. The two consumers are
[`lifecycle.rs:1020`](../../crates/cyrup-mcp/src/lifecycle.rs) and `:1130`. This is in-scope: §8 step
7 is the line that binds it.

### `MCP-021` — rehydrate metadata from a hash-valid cache entry · `missing`

**Unmet.** No rehydration into `McpState` exists. The cache-validity rule and the reconstruction
walk are, however, **already built on the registration side**: `is_server_cache_valid` at
[`registration.rs:860-882`](../../crates/cyrup-mcp/src/registration.rs), the `valid_entry` helper at
`:884-891`, `resolve_direct_tools` at `:1111`, `resolve_cached_prompts` at `:1802`, and
`resolve_tool_prefix`. `ServerCacheEntry` carries `tools` / `resources` / `prompts` / `instructions`
/ `cached_at` ([`dirs.rs:567-604`](../../crates/cyrup-mcp/src/dirs.rs)). The rehydrator composes
those; it must not re-derive prefixing.

`promptMetadataLive` is the load-bearing negative: a cache-rehydrated prompt list is deliberately
**not** added to `McpState::prompt_metadata_live`
([`state.rs:97`](../../crates/cyrup-mcp/src/state.rs)), which is what flags it non-live.

### `MCP-022` — the bounded startup connect pass · `missing`

**Unmet.** Nothing in `initialize_mcp` iterates servers or calls `manager.connect`
([`server_manager.rs:1690-1697`](../../crates/cyrup-mcp/src/server_manager.rs)).

### `MCP-026` — the `MCP_DIRECT_TOOLS` cache-bootstrap pass · `missing`

**Unmet.** `missing_configured_direct_tool_servers`
([`registration.rs:985`](../../crates/cyrup-mcp/src/registration.rs)) and `direct_tools_override`
([`runtime.rs:312`](../../crates/cyrup-mcp/src/runtime.rs)) exist; the §14 pass does not.

**Decide the message deliberately.** 13a MCP-026 says the byte-exact
`MCP: direct tools for {names} will be available after restart` *"becomes false for cyrup and must be
changed **together with** an actual late registration — pick one deliberately rather than leaving the
message and adding the registration."* HA-1 has landed (`LateRegistrar` at
[`native.rs:768`](../../crates/cyrup-ext/src/native.rs), `LateSink` at
[`registration.rs:2021-2063`](../../crates/cyrup-mcp/src/registration.rs)), so **pick the
registration**: call `sync_tool_surface()` after the bootstrap pass and emit
`MCP: direct tools for {names} are now available` instead. Record the string change as a deliberate
divergence at the call site.

### `MCP-027` — lifecycle callbacks · `partial`

**Unmet.** All three setters exist —
[`lifecycle.rs:712`](../../crates/cyrup-mcp/src/lifecycle.rs) (`set_reconnect_callback`), `:719`
(`set_reconnect_failure_callback`), `:744` (`set_idle_shutdown_callback`) — and none is called
outside `lifecycle.rs`'s own tests.

### `MCP-016` — the sampling and elicitation wiring gates · `partial`

**Unmet: the wiring, not the predicates.** `McpSettings::sampling(has_ui)` at
[`config.rs:1227-1229`](../../crates/cyrup-mcp/src/config.rs), `sampling_auto_approve` at `:1233`,
`elicitation(has_ui)` at `:1239`, and `ContextSnapshot::is_tui_mode` at
[`runtime.rs:88-90`](../../crates/cyrup-mcp/src/runtime.rs) are all exact. `initialize_mcp` reads
none of them.

**A structural gap the row misses:** `McpServerManager` stores the two configs
([`server_manager.rs:1338`](../../crates/cyrup-mcp/src/server_manager.rs), `:1343`) and **nothing
ever reads them** — `grep -n 'self.sampling\|self.elicitation' server_manager.rs` shows only the two
setters and the teardown at `:2474`. There is no `HandlerFactory` anywhere in `server_manager.rs`.
`ConnectionBuilder::new` installs `bare_handler_factory()`
([`runtime.rs:1933-1944`](../../crates/cyrup-mcp/src/runtime.rs)), which hard-codes
`sampling: None, elicitation: None`, and `with_handler_factory`
([`runtime.rs:2289`](../../crates/cyrup-mcp/src/runtime.rs)) has no caller. So the gate has nowhere
to land until the factory is built. That factory is this unit's real deliverable; the *handlers*
remain MCP-118/121/122.

### `MCP-015` — snapshot before the first await · `partial` → **restated**

See the verification table: this reduces to MCP-016's two live closures and has nothing else left.
It closes when the handler factory reads `HostServices::current_model()` through the owned `Arc`
rather than a re-read slot, and derives its per-request signal from
`crate::abort::combine` ([`abort.rs:60`](../../crates/cyrup-mcp/src/abort.rs)).

### `MCP-017` — owner cleanups in LIFO order, plus the list-changed listener · `partial`

**Unmet, three parts.**

1. `cleanup_materialized_binary_resources` is never registered. `add_cleanup` has exactly two
   production sites, [`runtime.rs:270`](../../crates/cyrup-mcp/src/runtime.rs) (oauth) and `:277`
   (lifecycle) — the rest are in `owner.rs`/`lifecycle.rs` tests. **Worse than the row states:**
   `MaterializedResources::new` has *no* production caller at all, so every production
   materialization uses the process-global at
   [`renderers.rs:772-774`](../../crates/cyrup-mcp/src/renderers.rs) / `:724`, which no owner owns
   and nothing ever cleans.
2. The LIFO order is therefore two-deep, not three.
3. `set_metadata_list_changed_listener`
   ([`server_manager.rs:1351`](../../crates/cyrup-mcp/src/server_manager.rs)) has no production
   installer. The manager's side is complete — `publish_metadata_changed` at
   [`server_manager.rs:2500-2526`](../../crates/cyrup-mcp/src/server_manager.rs) with the
   `Arc::ptr_eq` identity guard and the outside-the-lock invocation.
   `runtime.rs:198-200` already documents that this is step 11.

### `MCP-046` — the abort call-site discipline · `partial`

**Unmet.** `throw_if_inactive` has exactly two call sites in `initialize_mcp`
([`runtime.rs:235`](../../crates/cyrup-mcp/src/runtime.rs), `:237`), both inside the `open_browser`
closure. §8's four checkpoints — after the startup connect pass, at the top of every pass-two
iteration, after the `MCP_DIRECT_TOOLS` bootstrap, and before `start_health_checks` — do not exist
because the code they guard does not exist. This unit is the audit that closes with sub-wave D.

---

## Sub-wave E — the extension seams

### `MCP-012` — `startLoadTimeInitialization` · `partial`

**Unmet.** [`extension.rs:697-699`](../../crates/cyrup-mcp/src/extension.rs) calls the gate
(`needs_load_time_initialization`, [`runtime.rs:301-303`](../../crates/cyrup-mcp/src/runtime.rs) —
correct, including the `lazy-keep-alive` exclusion) and then only logs. No `tokio::spawn`, no
generation re-check, no synthetic print-mode context.

### `MCP-013` — the `MCP_DIRECT_TOOLS` blocking wait · `partial`

**Unmet.** `on_session_start` ([`extension.rs:455-479`](../../crates/cyrup-mcp/src/extension.rs))
contains no `MCP_DIRECT_TOOLS` read and no await on an initialization. The predicate is built
(`missing_configured_direct_tool_servers`,
[`registration.rs:985`](../../crates/cyrup-mcp/src/registration.rs)) and so is the sentinel
normalisation (`direct_tools_override`, [`runtime.rs:312`](../../crates/cyrup-mcp/src/runtime.rs),
`DIRECT_TOOLS_NONE_SENTINEL` at `:307`).

### `MCP-027a` — `sendMessage`'s `triggerTurn` convergence gate · `missing`

**Unmet, and currently inexpressible.** `pub type SendMessage = Arc<dyn Fn(String) + Send + Sync>`
([`state.rs:69`](../../crates/cyrup-mcp/src/state.rs)) takes no options, so there is no flag to
branch on. Its one production construction is
[`runtime.rs:242-251`](../../crates/cyrup-mcp/src/runtime.rs), whose body is a `tracing::debug!`
saying `send_message not yet wired`. The host side is ready: the fenced
`inject_message(content, custom_type, display, trigger_turn)` at
[`owner.rs:451-457`](../../crates/cyrup-mcp/src/owner.rs), and
`McpLifecycleManager::ensure_converged` at
[`lifecycle.rs:898-901`](../../crates/cyrup-mcp/src/lifecycle.rs) returning a single-flight
`BoxFuture`. Deliver-on-failure must be a real arm, not a `?`.

### `MCP-031` — `flushMetadataCache` on shutdown · `missing`

**Unmet.** `on_session_shutdown` ([`extension.rs:565-578`](../../crates/cyrup-mcp/src/extension.rs))
takes the state out of its slot, binds it to `_state`, and **drops it** — it never calls
`shutdown_state`. `shutdown_state` itself is built
([`lifecycle.rs:1562-1605`](../../crates/cyrup-mcp/src/lifecycle.rs)) and takes a `MetadataFlush`
([`lifecycle.rs:394`](../../crates/cyrup-mcp/src/lifecycle.rs)); the only implementation is
`no_metadata_flush` at [`lifecycle.rs:400-404`](../../crates/cyrup-mcp/src/lifecycle.rs), whose log
line literally reads *"`flush_metadata_cache` is pending MCP-031"*.

Wiring `shutdown_state` into `on_session_shutdown` is **MCP-009/MCP-010, Wave 2**. This unit supplies
the `MetadataFlush` that wave installs; land the function and its `MetadataFlush` constructor, and
coordinate the one-line call-site with Wave 2's owner.

---

## Sub-wave F — hand to Wave 3's `MCP-217` owner

Per the grouping decision: `MCP-217` shrank to exactly `MCP-038`'s obligation
([MCP_HIGH_SEVERITY_BACKLOG.md](MCP_HIGH_SEVERITY_BACKLOG.md), Wave 3), and both units below are
edits to the same `sync_tool_surface` body. One owner, or the removal half gets written twice.

### `MCP-036` — `syncDirectTools`'s diff, re-activation and renderer declaration · `partial`

**Unmet: the removal half.** See the verification table for what is already built.
`sync_tool_surface` computes `changed` at
[`extension.rs:245`](../../crates/cyrup-mcp/src/extension.rs) as
`!surface.tool_names.is_empty() || !surface.command_names.is_empty()` — which counts *registrations*,
never removals, and yields no `(added, updated, deactivated)` triple. `grep -rn 'direct tools
refreshed' src` returns zero hits.

### `MCP-038` — `deactivateTools` · `missing`

**Unmet.** `grep -rn 'deactivate_tools\|set_active_tools' src` outside `owner.rs` returns nothing.
`McpExtension::fallback_deactivated_tools`
([`extension.rs:101`](../../crates/cyrup-mcp/src/extension.rs), accessor at `:386`) has a reader and
**no writer**. cyrup lands on upstream's `unregisterTool === undefined` branch — `ExtensionRegistry`
has no `unregister_tool` — which is a supported upstream configuration, so steps 5–6 of §20's
`deactivateTools` are the whole port.

---

## Sub-wave G — materialized-resource retry

### `MCP-224` — the cleanup drain and retry · `partial`

**Unmet.** `MaterializedResources::cleanup`
([`renderers.rs:860-873`](../../crates/cyrup-mcp/src/renderers.rs)) removes the directory and zeroes
the counters; its own `TODO(MCP-224)` at `:847-856` enumerates precisely what is absent — the pending
set, the per-directory attempt counters capped at 3, the single 30 s timer guarded by *"already
pending or nothing retryable"*, the timer-clear when the set empties, and the aggregate error. Sub-wave
D gives the session an owner and a cleanup site; this adds the retry behind it.

---

## Implementation

### D0 — discharge the `ServerToolMetadata` forward declaration

`state.rs`:

```rust
// Replaces the forward declaration at state.rs:366-371. `tool-metadata.ts`'s per-server metadata is
// `ToolMetadata[]`, and MCP-021/MCP-028 are the writers that need every field of it.
pub use crate::proxy::ToolMetadata;
```

and change field 4:

```rust
    /// 4 · Per-server tool metadata — `state.toolMetadata: Map<string, ToolMetadata[]>`,
    /// insertion-ordered because that order decides which server wins a fuzzy name match.
    pub tool_metadata: Mutex<IndexMap<String, Vec<ToolMetadata>>>,
```

Leave `ProxyCtx::tool_metadata` ([`proxy.rs:1602`](../../crates/cyrup-mcp/src/proxy.rs)) alone: its
doc already assigns its deletion to MCP-207, and editing it here collides with Wave 3.

### A — the payload types (`state.rs`)

```rust
/// `types.ts` `MCP_STATUS_SNAPSHOT_VERSION` (13c §3.16).
pub const MCP_STATUS_SNAPSHOT_VERSION: u32 = 1;

/// `McpServerRuntimeStatus` — a CLOSED six-variant enum. The `kebab-case` rename is what produces
/// `needs-auth` / `not-connected`, which are the wire spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpServerRuntimeStatus {
    Connected,
    Cached,
    Failed,
    NeedsAuth,
    NotConnected,
    Disabled,
}

/// The per-server object: exactly six keys, two of them OMITTED when absent — never `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatusSnapshot {
    pub name: String,
    pub status: McpServerRuntimeStatus,
    pub tool_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_ago_seconds: Option<u64>,
    /// ALWAYS emitted, even for an enabled server: it duplicates `status == Disabled` and consumers
    /// read both (13c §3.16).
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatusSnapshot {
    pub version: u32,
    pub servers: Vec<McpServerStatusSnapshot>,
    pub total_tools: usize,
    pub total_resources: usize,
    pub connected_count: usize,
    pub disabled_count: usize,
}

impl Default for McpStatusSnapshot {
    /// `publishMcpStatusShutdown`'s literal all-zero payload, `servers: []`. This is why the two
    /// existing `publish_status(Default::default())` sites (runtime.rs:287, lifecycle.rs:1573)
    /// become CORRECT under the new shape rather than merely still compiling.
    fn default() -> Self {
        Self {
            version: MCP_STATUS_SNAPSHOT_VERSION,
            servers: Vec::new(),
            total_tools: 0,
            total_resources: 0,
            connected_count: 0,
            disabled_count: 0,
        }
    }
}
```

### C — `env.rs`, the live-state verbs

```rust
//! `init.ts`'s live-state verbs (13a §13, §17, §18, §19; 13c §3.16) and the crate's one production
//! [`crate::proxy::ProxyEnv`].
//!
//! These are deliberately NOT in `runtime.rs`: that file's module doc declares two halves — the
//! runtime BUILD and the CONNECTION — that "share no state", and the connection half is testable
//! without an `McpState`. Mutating a committed `McpState` is a third thing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cyrup_core::CancelToken;
use crate::server_manager::ConnectionStatus;
use crate::state::{
    McpServerRuntimeStatus, McpServerStatusSnapshot, McpState, McpStatusSnapshot, ServerFailure,
    MCP_STATUS_SNAPSHOT_VERSION,
};

/// `init.ts` `FAILURE_BACKOFF_MS = 60 * 1000` (13a §13).
pub const FAILURE_BACKOFF_MS: u64 = 60_000;
/// `init.ts` `MAX_FAILURE_MESSAGE_CHARS = 8 * 1024`.
pub const MAX_FAILURE_MESSAGE_CHARS: usize = 8 * 1024;
/// `init.ts`'s two `parallelLimit(…, 10, …)` call sites (MCP-022, MCP-026, MCP-130).
pub const STARTUP_CONNECT_CONCURRENCY: usize = 10;
```

**`parallel_limit` (MCP-087 / MCP-130).**

```rust
/// `utils.ts` `parallelLimit(items, limit, f)` — at most `limit` in flight, results **by original
/// index**.
///
/// `buffered` is the whole port: it keeps `limit` futures in flight and yields in input order, which
/// is exactly `parallelLimit`'s two properties. `buffer_unordered` / `for_each_concurrent` (the
/// shape `lifecycle.rs:962` uses) is WRONG here — §11's pass two reads results back by position.
pub async fn parallel_limit<T, R, F, Fut>(items: Vec<T>, limit: usize, f: F) -> Vec<R>
where
    F: Fn(T) -> Fut,
    Fut: std::future::Future<Output = R>,
{
    use futures::StreamExt as _;
    let limit = limit.max(1);
    futures::stream::iter(items.into_iter().map(f))
        .buffered(limit)
        .collect::<Vec<R>>()
        .await
}
```

**Failure tracking (MCP-024).**

```rust
/// `clearFailure(state, serverName)` — idempotent, and the first thing `record_failure` calls.
pub fn clear_failure(state: &McpState, server: &str) {
    if let Ok(mut tracker) = state.failure_tracker.lock() {
        tracker.shift_remove(server);
    }
    if let Ok(mut messages) = state.failure_messages.lock() {
        messages.shift_remove(server);
    }
}

/// `recordFailure(state, serverName, message)`.
///
/// **Two deliberate deviations from upstream's bookkeeping, both with identical observable
/// behaviour.** (1) There is no timer map. Upstream's `clearTimeout` exists so a superseded timer
/// cannot clear a newer failure; the `last_failure == failed_at` check below already guarantees
/// that, so a 22nd `McpState` field would buy nothing. (2) `timer.unref?.()` needs no analog — a
/// tokio task does not hold the process open — but the select on the owner token is REQUIRED, not
/// optional: without it a clean shutdown waits out the full 60 s.
pub fn record_failure(state: &Arc<McpState>, server: &str, message: &str) {
    clear_failure(state, server);
    let failed_at = Instant::now();
    let previous = state
        .failure_tracker
        .lock()
        .ok()
        .and_then(|t| t.get(server).map(|f| f.count))
        .unwrap_or(0);
    if let Ok(mut tracker) = state.failure_tracker.lock() {
        tracker.insert(
            server.to_string(),
            ServerFailure { last_failure: failed_at, count: previous.saturating_add(1) },
        );
    }
    if let Ok(mut messages) = state.failure_messages.lock() {
        messages.insert(server.to_string(), truncate_failure_message(message));
    }

    // `WeakMap<McpExtensionState, …>`: the task must not keep the state alive.
    let weak = Arc::downgrade(state);
    let owner = state.owner.token();
    let name = server.to_string();
    tokio::spawn(async move {
        tokio::select! {
            biased;
            () = owner.cancelled() => {}
            () = tokio::time::sleep(Duration::from_millis(FAILURE_BACKOFF_MS)) => {
                let Some(state) = weak.upgrade() else { return };
                if !state.owner.is_active() {
                    return;
                }
                // `failureTracker.get(serverName) === failedAt`: a re-insert must NOT be cleared by
                // the older timer.
                let still_ours = state
                    .failure_tracker
                    .lock()
                    .is_ok_and(|t| t.get(&name).is_some_and(|f| f.last_failure == failed_at));
                if still_ours {
                    clear_failure(&state, &name);
                    update_status_bar(&state);
                }
            }
        }
    });
}

/// `message.slice(0, MAX_FAILURE_MESSAGE_CHARS)`, on a char boundary. Upstream slices UTF-16 code
/// units and is safe only because the string is ASCII in practice; a hostile server's stderr is not.
fn truncate_failure_message(message: &str) -> String {
    if message.len() <= MAX_FAILURE_MESSAGE_CHARS {
        return message.to_string();
    }
    let cut = message
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|i| *i <= MAX_FAILURE_MESSAGE_CHARS)
        .last()
        .unwrap_or(0);
    message.get(..cut).unwrap_or("").to_string()
}

/// `getActiveFailureAgeSeconds(state, name)` — `None` outside the 60 s window.
///
/// Upstream's falsy-`failedAt` arm (an epoch-`0` timestamp counting as absent) has no analog: the
/// record holds an `Instant`, which has no zero value, and absence is `None`.
#[must_use]
pub fn failure_age_seconds(state: &McpState, server: &str) -> Option<u64> {
    let tracker = state.failure_tracker.lock().ok()?;
    let age = tracker.get(server)?.last_failure.elapsed();
    (age <= Duration::from_millis(FAILURE_BACKOFF_MS))
        .then(|| age.as_secs_f64().round().max(0.0) as u64)
}
```

**The snapshot builder (MCP-137) and the status bar (MCP-032).**

```rust
/// `createMcpStatusSnapshot(state)` (13c §3.16). Never connects, never queries.
///
/// Iterates `config.mcpServers` — an `IndexMap`, so this is config-file order. A `BTreeMap` anywhere
/// on this path makes the `/mcp` panel and the footer list servers alphabetically.
#[must_use]
pub fn create_mcp_status_snapshot(state: &McpState) -> McpStatusSnapshot {
    let mut servers = Vec::with_capacity(state.config.mcp_servers.len());
    let (mut total_tools, mut total_resources) = (0usize, 0usize);
    let (mut connected_count, mut disabled_count) = (0usize, 0usize);

    for (name, definition) in &state.config.mcp_servers {
        let disabled = definition.is_disabled();
        let connection = (!disabled).then(|| state.manager.get_connection(name)).flatten();
        let status_of = connection.as_ref().map(|c| c.status());
        let metadata_len = (!disabled)
            .then(|| state.tool_metadata.lock().ok().and_then(|m| m.get(name).map(Vec::len)))
            .flatten();

        // `metadata?.length ?? (connected ? connection.tools.length : 0)`
        let tool_count = metadata_len.unwrap_or_else(|| {
            match (status_of, connection.as_ref()) {
                (Some(ConnectionStatus::Connected), Some(c)) => c.tools().len(),
                _ => 0,
            }
        });
        // `resourceCounts?.get(name) ?? (connected ? connection.resources.length : undefined)`
        let resource_count = if disabled {
            None
        } else {
            state
                .resource_counts
                .lock()
                .ok()
                .and_then(|m| m.get(name).copied())
                .or_else(|| match (status_of, connection.as_ref()) {
                    (Some(ConnectionStatus::Connected), Some(c)) => Some(c.resources().len()),
                    _ => None,
                })
        };
        let failed_ago = (!disabled).then(|| failure_age_seconds(state, name)).flatten();

        // First match wins.
        let status = if disabled {
            disabled_count += 1;
            McpServerRuntimeStatus::Disabled
        } else if status_of == Some(ConnectionStatus::Connected) {
            connected_count += 1;
            McpServerRuntimeStatus::Connected
        } else if status_of == Some(ConnectionStatus::NeedsAuth) {
            McpServerRuntimeStatus::NeedsAuth
        } else if failed_ago.is_some() {
            McpServerRuntimeStatus::Failed
        } else if metadata_len.is_some() {
            McpServerRuntimeStatus::Cached
        } else {
            McpServerRuntimeStatus::NotConnected
        };

        if !disabled {
            total_tools += tool_count;
            total_resources += resource_count.unwrap_or(0);
        }
        servers.push(McpServerStatusSnapshot {
            name: name.clone(),
            status,
            tool_count,
            resource_count,
            // "present only when `status === 'failed'` AND it is defined"
            failed_ago_seconds: (status == McpServerRuntimeStatus::Failed).then_some(failed_ago).flatten(),
            disabled,
        });
    }

    McpStatusSnapshot {
        version: MCP_STATUS_SNAPSHOT_VERSION,
        servers,
        total_tools,
        total_resources,
        connected_count,
        disabled_count,
    }
}

/// `updateStatusBar(state)` (13a §18).
///
/// Step 1 publishes ALWAYS, before the `!ui` return — a headless run still feeds the watch, which is
/// what `/mcp` and the proxy tool's `status` mode read. Steps 3-10 are `ui::footer_status_text`.
/// Step 11's `ui.theme.fg("accent", …)` has no analog and collapses to upstream's own no-theme arm.
pub fn update_status_bar(state: &McpState) {
    let snapshot = create_mcp_status_snapshot(state);
    let connected = snapshot.connected_count;
    state.publish_status(snapshot);

    let Some(ui) = state.ui.as_ref() else { return };
    let counts = crate::ui::FooterCounts::from_config(&state.config, connected);
    let text = crate::ui::footer_status_text(&state.config, counts);
    cyrup_ext::HostServices::set_status(ui.as_ref(), "mcp", text.as_deref());
}
```

**`update_server_metadata` (MCP-028).**

```rust
/// `updateServerMetadata(state, serverName)` (13a §17).
pub fn update_server_metadata(state: &McpState, server: &str) {
    let Some(connection) = state.manager.get_connection(server) else { return };
    if connection.status() != ConnectionStatus::Connected {
        return;
    }
    let Some(definition) = state.config.mcp_servers.get(server) else { return };

    // The arm the row calls out: a server disabled WHILE connected disappears from the surface on
    // the next refresh instead of lingering. All five maps, then return.
    if definition.is_disabled() {
        forget_server_metadata(state, server);
        return;
    }

    // The collision universe here is `state.toolMetadata` — every server's CURRENT names — not the
    // startup snapshot MCP-023 builds. Getting this wrong makes prefixed names order-dependent.
    let universe = state.tool_metadata.lock().ok().map(|m| m.clone()).unwrap_or_default();
    let metadata = crate::proxy::build_tool_metadata(
        &state.config,
        server,
        definition,
        &connection.tools(),
        &connection.resources(),
        &universe,
    );

    if let Ok(mut map) = state.tool_metadata.lock() {
        map.insert(server.to_string(), metadata);
    }
    if let Ok(mut counts) = state.resource_counts.lock() {
        counts.insert(server.to_string(), connection.resources().len());
    }
    // Only from a LIVE list, and only when discovery did not fail.
    if !connection.prompt_discovery_failed() {
        let prompts = reconstruct_live_prompt_metadata(&connection.prompts());
        if let Ok(mut map) = state.prompt_metadata.lock() {
            map.insert(server.to_string(), prompts);
        }
        if let Ok(mut live) = state.prompt_metadata_live.lock() {
            live.insert(server.to_string());
        }
    }
    // SET or DELETE — `instructions: None` means delete the entry, not leave it.
    if let Ok(mut map) = state.server_instructions.lock() {
        match connection.instructions() {
            Some(text) => {
                map.insert(server.to_string(), text.to_string());
            }
            None => {
                map.shift_remove(server);
            }
        }
    }
}

/// The five-map delete the disabled arm and `unregisterServer` share.
fn forget_server_metadata(state: &McpState, server: &str) {
    if let Ok(mut m) = state.tool_metadata.lock() { m.shift_remove(server); }
    if let Ok(mut m) = state.resource_counts.lock() { m.shift_remove(server); }
    if let Ok(mut m) = state.prompt_metadata.lock() { m.shift_remove(server); }
    if let Ok(mut m) = state.prompt_metadata_live.lock() { m.remove(server); }
    if let Ok(mut m) = state.server_instructions.lock() { m.shift_remove(server); }
}
```

> **`build_tool_metadata` is the one cross-wave collision this group has.** It is `MCP-207`, Wave 3,
> and MCP-021, MCP-028 and MCP-033 all need it. The signature above is the contract; whichever agent
> reaches it first writes it in `proxy.rs` beside `ToolMetadata`, and the other reads it. Do not
> write a second copy, and do not stub it — a `Vec::new()` body would silently empty the tool surface
> on every reconnect, which is exactly the class of failure `MCP-037a` recorded.

**`lazy_connect` (MCP-033).**

```rust
/// `lazyConnect(state, serverName, signal)` (13a §19) — `true` iff the server ended `connected`.
///
/// **The seam flattens upstream's throw into a `bool`** (`ProxyEnv::lazy_connect`, proxy.rs:1447),
/// so §19 step 1's `throwIfAborted` and step 8's conditional rethrow both surface as `false`. What
/// must survive that flattening is the DISCRIMINATOR: an abort on an actually-cancelled signal must
/// NOT `record_failure`, while a stray abort error on a live signal MUST. Collapsing the two lets a
/// server-side cancellation poison the next 60 seconds of that server's availability.
pub async fn lazy_connect(state: &Arc<McpState>, server: &str, cancel: &CancelToken) -> bool {
    // 1
    let owned = crate::abort::combine(&state.owner.token(), Some(cancel));
    if owned.is_cancelled() {
        return false;
    }
    // 2-3
    if let Some(connection) = state.manager.get_connection(server) {
        match connection.status() {
            ConnectionStatus::NeedsAuth => return false,
            ConnectionStatus::Connected => {
                update_server_metadata(state, server);
                state.lifecycle.mark_keep_alive_after_connect(server);
                return true;
            }
            _ => {}
        }
    }
    // 4 — inside the backoff, do not retry.
    if failure_age_seconds(state, server).is_some() {
        return false;
    }
    // 5
    let Some(definition) = state.config.mcp_servers.get(server) else { return false };
    if definition.is_disabled() {
        return false;
    }
    // 6
    if let Some(ui) = state.ui.as_ref() {
        let text = crate::ui::format_mcp_status(&state.config, &format!("connecting to {server}..."));
        cyrup_ext::HostServices::set_status(ui.as_ref(), "mcp", text.as_deref());
    }
    // 7-8
    match state.manager.connect(server, definition, Some(&owned)).await {
        Ok(connection) if connection.status() == ConnectionStatus::NeedsAuth => false,
        Ok(_) => {
            clear_failure(state, server);
            update_server_metadata(state, server);
            update_metadata_cache(state, server, MetadataCacheOptions::default());
            state.notify_tool_metadata_updated(server, "lazy-connect");
            state.lifecycle.mark_keep_alive_after_connect(server);
            update_status_bar(state);
            true
        }
        Err(error) => {
            if crate::abort::is_abort_error(&error, Some(&owned)) && owned.is_cancelled() {
                // The rethrow arm. No failure is recorded for a genuine cancellation.
                return false;
            }
            let message = error.to_string();
            record_failure(state, server, &message);
            tracing::debug!(
                "MCP: lazy connect failed for {server}: {}",
                crate::ui::sanitize_terminal_text(&message)
            );
            update_status_bar(state);
            false
        }
    }
}
```

### D — `initialize_mcp` §9–§15

Insert between the two `add_cleanup` calls
([`runtime.rs:277-292`](../../crates/cyrup-mcp/src/runtime.rs)) and the final `Ok(state)`. The
§8-step-7 fix (MCP-020) replaces
[`runtime.rs:219`](../../crates/cyrup-mcp/src/runtime.rs):

```rust
// Step 7. `hasPendingAuth` is the OAuth RUNTIME's — a hardcoded `false` here lets the idle sweep
// and the health check reap a server in the middle of its login (lifecycle.rs:1020, :1130).
let pending_auth: crate::lifecycle::PendingAuthCheck = {
    let runtime = Arc::clone(&oauth_runtime);
    let base = auth_storage_options.base_dir();
    Arc::new(move |name: &str| {
        let (runtime, base, name) = (Arc::clone(&runtime), base.clone(), name.to_string());
        Box::pin(async move { crate::oauth::has_pending_auth(&runtime, &name, Some(&base)).await })
    })
};
let lifecycle = Arc::new(McpLifecycleManager::new(Arc::clone(&manager), pending_auth));
```

This requires widening `PendingAuthCheck`
([`lifecycle.rs:362`](../../crates/cyrup-mcp/src/lifecycle.rs)) from `Fn(&str) -> bool` to
`Arc<dyn Fn(&str) -> BoxFuture<'static, bool> + Send + Sync>`, and `.await`ing it at its two
consumers, both of which are already inside `async fn`s
([`lifecycle.rs:1020`](../../crates/cyrup-mcp/src/lifecycle.rs), `:1130`). Do **not** mirror the
pending set into a synchronous field: `oauth::has_pending_auth`
([`oauth.rs:2044-2059`](../../crates/cyrup-mcp/src/oauth.rs)) reads a `tokio::sync::Mutex`, and a
second copy of that state is precisely the drift this codebase keeps finding.

Then the body, in order:

```rust
    // ── §9 — cache bootstrap (MCP-019) ───────────────────────────────────────────────────────
    // The two-way split IS the unit: collapsing "no usable cache" into one arm turns the
    // corrupt-cache path from cheap into a connect storm.
    let cache_path = dirs.metadata_cache();
    let cache_file_exists = cache_path.exists();
    let mut cache = crate::dirs::load_metadata_cache(&cache_path);
    let bootstrap_all = !cache_file_exists;
    if !cache_file_exists || cache.is_none() {
        let _ = crate::dirs::save_metadata_cache(&cache_path, &crate::dirs::MetadataCache::default());
        cache = cache.or_else(|| Some(crate::dirs::MetadataCache::default()));
    }

    // ── §10 — per-server lifecycle registration (MCP-020) + rehydration (MCP-021) ────────────
    for (name, definition) in state.config.enabled_servers() {
        let mode = definition.lifecycle_mode();
        let persists = matches!(mode, ServerLifecycle::Eager | ServerLifecycle::LazyKeepAlive);
        // `definition.idleTimeout ?? (persistsAfterFirstSpawn ? 0 : undefined)` — the `?? 0` is what
        // stops an eager or lazy-keep-alive server ever idling out by default.
        let idle_timeout = definition.idle_timeout.or(persists.then_some(0.0));
        state.lifecycle.register_server(
            name,
            definition.clone(),
            idle_timeout.map(|t| LifecycleOverrides { idle_timeout: Some(t) }),
        );
        // ONLY `keep-alive` at registration; `lazy-keep-alive` waits for its first connect.
        if crate::runtime::marks_keep_alive_at_registration(mode) {
            state.lifecycle.mark_keep_alive(name);
        }
        // Step 6 — rehydrate from a hash-valid entry.
        if let Some(entry) = cache.as_ref().and_then(|c| c.servers.get(name))
            && crate::dirs::try_compute_server_hash(definition, &env, &home)
                .is_ok_and(|h| crate::dirs::is_server_cache_valid(entry, &h, crate::dirs::CACHE_MAX_AGE_MS))
        {
            crate::env::rehydrate_from_cache(&state, name, definition, entry);
        }
    }

    // ── §11 — the bounded startup connect pass (MCP-022 / MCP-087 / MCP-130) ─────────────────
    let startup: Vec<String> = state
        .config
        .enabled_servers()
        .filter(|(_, d)| bootstrap_all || d.lifecycle_mode().is_prewarmed())
        .map(|(name, _)| name.clone())
        .collect();
    if !startup.is_empty() {
        if let Some(ui) = state.ui.as_ref() {
            let text = crate::ui::format_mcp_status(
                &state.config,
                &format!("connecting to {} servers...", startup.len()),
            );
            cyrup_ext::HostServices::set_status(ui.as_ref(), "mcp", text.as_deref());
        }
        let results = crate::env::parallel_limit(
            startup.clone(),
            crate::env::STARTUP_CONNECT_CONCURRENCY,
            |name| { /* connect; needs-auth ⇒ the byte-exact auth message; abort ⇒ rethrow */ },
        )
        .await;
        // MCP-046 checkpoint 1.
        owner.throw_if_inactive()?;
        // ── §12 — the two-pass metadata build (MCP-023, Wave 2) ─────────────────────────────
        // Pass one over EVERY successful connection, then pass two per server with
        // `owner.throw_if_inactive()?` at the top of each iteration (MCP-046 checkpoint 2).
    }

    // ── §14 — the MCP_DIRECT_TOOLS bootstrap (MCP-026) ──────────────────────────────────────
    // Re-reads `$MCP_DIRECT_TOOLS` AND the cache from disk here rather than reusing the factory's
    // values, because upstream does — this is a different module and the cache has just moved.
    // MCP-046 checkpoint 3 follows it.

    // ── §15 — lifecycle callbacks (MCP-027) and health checks ───────────────────────────────
    // Every body opens with the owner guard: that is what keeps a generation-N timer from writing
    // into generation N+1.
    state.lifecycle.set_reconnect_callback({
        let state = Arc::clone(&state);
        Arc::new(move |server: String| {
            let state = Arc::clone(&state);
            Box::pin(async move {
                if !state.owner.is_active() { return Ok(()) }
                crate::env::update_server_metadata(&state, &server);
                crate::env::update_metadata_cache(&state, &server, Default::default());
                state.notify_tool_metadata_updated(&server, "lifecycle-reconnect");
                crate::env::clear_failure(&state, &server);
                crate::env::update_status_bar(&state);
                Ok(())
            })
        })
    });
    // …set_reconnect_failure_callback: owner guard, `record_failure`, `update_status_bar`
    // …set_idle_shutdown_callback: owner guard, the `{server} shut down (idle {m}m)` debug, status

    // ── Step 11 — the list-changed listener (MCP-017) ───────────────────────────────────────
    // Installed AFTER the state commits, so a hook fired mid-build cannot see a half-installed
    // surface. `preserve_empty_resources: false` is the load-bearing detail: THIS empty
    // `resources/list` is authoritative and must overwrite the cache.
    state.manager.set_metadata_list_changed_listener(Some({
        let state = Arc::clone(&state);
        Arc::new(move |server: &str, reason: &str| {
            if !state.owner.is_active() { return }
            crate::env::update_server_metadata(&state, server);
            crate::env::update_metadata_cache(
                &state, server,
                MetadataCacheOptions { preserve_empty_resources: false },
            );
            state.notify_tool_metadata_updated(server, reason);
            crate::env::update_status_bar(&state);
        })
    }));

    // MCP-046 checkpoint 4, then the health checks.
    owner.throw_if_inactive()?;
    state.lifecycle.start_health_checks(runtime_signal.clone());
    crate::env::update_status_bar(&state);
    Ok(state)
```

**MCP-017's cleanup order.** `MaterializedResources` becomes `McpState` field 22, constructed in
`initialize_mcp` with the owner's token so a blob in flight at stop is omitted rather than orphaned,
and registered as the **first** cleanup — before the oauth and lifecycle registrations at
[`runtime.rs:270`](../../crates/cyrup-mcp/src/runtime.rs) / `:277` — so LIFO runs it last:

```rust
    let materialized = Arc::new(crate::renderers::MaterializedResources::new(Some(owner.token())));
    // FIRST, so it runs LAST: lifecycle.graceful_shutdown -> shutdown_oauth -> cleanup binaries.
    owner.add_cleanup(Box::new({
        let materialized = Arc::clone(&materialized);
        move || Box::pin(async move { materialized.cleanup().map_err(McpError::from) })
    }));
```

Thread `Some(state.materialized.as_ref())` into `resolve_mcp_result_content`
([`renderers.rs:702`](../../crates/cyrup-mcp/src/renderers.rs)) from the tool-result path. That path
is Wave 1/3's; until it exists the field is constructed, owned and cleaned, and the process-global at
[`renderers.rs:772`](../../crates/cyrup-mcp/src/renderers.rs) stops being the only session there is.

### C/D — `RuntimeEnv`

```rust
/// The crate's ONE production [`crate::proxy::ProxyEnv`]. Every method is a delegation; the call
/// order and branch structure live in `proxy.rs`, which is what makes `FakeEnv` and this type
/// interchangeable under 13d's conformance suite.
pub struct RuntimeEnv {
    state: Arc<McpState>,
    dirs: crate::dirs::McpDirs,
    /// `Weak`, for the same reason `install_surface_sync`'s listener is (extension.rs:423-424): the
    /// state this env holds is owned by the extension, so a strong handle would cycle.
    extension: std::sync::Weak<crate::extension::McpExtension>,
}

#[async_trait::async_trait]
impl crate::proxy::ProxyEnv for RuntimeEnv {
    fn get_connection(&self, server: &str) -> Option<ConnectionStatus> {
        self.state.manager.get_connection(server).map(|c| c.status())
    }
    fn is_connecting(&self, server: &str) -> bool { self.state.manager.is_connecting(server) }
    fn touch(&self, server: &str) { self.state.manager.touch(server); }
    fn increment_in_flight(&self, server: &str) { self.state.manager.increment_in_flight(server); }
    fn decrement_in_flight(&self, server: &str) { self.state.manager.decrement_in_flight(server); }

    fn failure_age_seconds(&self, server: &str) -> Option<u64> {
        crate::env::failure_age_seconds(&self.state, server)
    }
    fn record_failure(&self, server: &str, message: &str) {
        crate::env::record_failure(&self.state, server, message);
    }
    fn clear_failure(&self, server: &str) { crate::env::clear_failure(&self.state, server); }
    fn update_status_bar(&self) { crate::env::update_status_bar(&self.state); }
    fn update_server_metadata(&self, server: &str) {
        crate::env::update_server_metadata(&self.state, server);
    }
    fn update_metadata_cache(&self, server: &str) {
        crate::env::update_metadata_cache(&self.state, server, Default::default());
    }
    fn mark_keep_alive_after_connect(&self, server: &str) {
        self.state.lifecycle.mark_keep_alive_after_connect(server);
    }
    async fn lazy_connect(&self, server: &str, cancel: &CancelToken) -> bool {
        crate::env::lazy_connect(&self.state, server, cancel).await
    }
    fn sync_tool_surface(&self) {
        if let Some(ext) = self.extension.upgrade() { ext.sync_tool_surface(); }
    }

    // MCP-084 — delegate, never mint a second copy: the config digest and the connect path have to
    // agree about what a server's URL IS (proxy.rs:1517-1524).
    fn resolve_server_url(&self, definition: &ServerEntry) -> McpResult<Option<String>> {
        crate::credentials::resolve_server_url(definition.url.as_deref(), &self.env)
    }
    fn supports_oauth(&self, definition: &ServerEntry) -> bool {
        crate::oauth::supports_oauth(definition)
    }

    // MCP-231 / MCP-232 — `ProxyCtx` already carries both bodies (proxy.rs:1629, :1646) and their
    // docs name this implementor as the forwarder.
    fn is_tool_call_approval_required(&self, server: &str, tool: &ToolMetadata) -> bool { … }
    async fn ensure_tool_call_approved(&self, …) -> ApprovalOutcome { … }

    // MCP-225 — the whole wiring, using the bridge renderers.rs:568-572 was built for.
    async fn guard_mcp_output(
        &self,
        content: Vec<Content>,
        options: crate::proxy::OutputGuardOptions,
    ) -> crate::proxy::GuardedOutput {
        let resolved = self
            .state
            .config
            .settings_or_default()
            .output_guard(std::env::var("MCP_OUTPUT_GUARD").ok().as_deref());
        let mut guard_options = crate::renderers::McpOutputGuardOptions::from_resolved(resolved);
        guard_options.prefix = &options.prefix;
        guard_options.suffix = &options.suffix;
        guard_options.empty_text_fallback = options.empty_text_fallback.as_deref();
        guard_options.raw_mcp_result = options.raw_mcp_result.as_ref();

        let blocks = crate::renderers::McpContentBlock::vec_from_core(&content);
        let guarded = crate::renderers::guard_mcp_output(&blocks, &guard_options);
        crate::proxy::GuardedOutput {
            mcp_result: guarded.mcp_result.clone(),
            output_guard: guarded.output_guard.clone(),
            content: guarded.into_core_content(),
        }
    }

    fn all_tool_names(&self) -> Option<Vec<String>> {
        // `getPiTools?.()`. `None` is upstream's optional-parameter branch, NOT a defect: never
        // synthesise a built-in name list as a floor (proxy.rs:1577-1583).
        self.state.ui.as_ref().and_then(|ui| cyrup_ext::HostServices::all_tool_names(ui.as_ref()))
    }

    // ── Wave 1 (MCP-164) fills these two. Loud, greppable, never a fabricated success. ─────────
    async fn call_tool(&self, server: &str, …) -> Result<CallToolOutcome, ProxyCallError> { … }
    async fn read_resource(&self, server: &str, …) -> Result<Vec<Content>, ProxyCallError> { … }
}
```

And the construction site, beside `install_surface_sync`:

```rust
// extension.rs, next to install_surface_sync (extension.rs:425)
/// Build this generation's `ProxyCtx` over the one production `ProxyEnv` and stash it where the
/// dispatcher (MCP-214) can find it. Called from `start_initialization`'s commit, exactly where
/// `install_surface_sync` is.
pub fn install_runtime_env(&self, state: &Arc<McpState>) {
    let env = Arc::new(crate::env::RuntimeEnv::new(
        Arc::clone(state),
        self.dirs.clone(),
        self.self_weak.get().cloned().unwrap_or_default(),
    ));
    let ctx = Arc::new(crate::proxy::ProxyCtx::new(Arc::clone(state), env));
    if let Ok(mut slot) = self.proxy_ctx.lock() {
        *slot = Some(ctx);
    }
}
```

### F — the removal half (`sync_tool_surface`)

`sync_tool_surface` ([`extension.rs:166-254`](../../crates/cyrup-mcp/src/extension.rs)) already seeds
`LateSink` with the three "known" maps and adopts the new surface. Add, between the
`register_surface` call at `:223` and the adoption at `:227`:

```rust
    // §20 step 2's `added`/`updated` split, computed against the PREVIOUS map — which is still
    // `sink.known_tools`, because `register_surface` never touched it.
    let (mut added, mut updated) = (0usize, 0usize);
    for name in &surface.tool_names {
        if sink.known_tools.contains_key(name) { updated += 1 } else { added += 1 }
    }
    // §20 step 3 — every previously-registered name absent from the new resolution.
    let deactivated: Vec<String> = sink
        .known_tools
        .keys()
        .filter(|name| !surface.direct_tool_fingerprints.contains_key(*name))
        .cloned()
        .collect();
    self.deactivate_tools(&deactivated);
    // §20 step 2's re-activation arm: a tool that comes back must leave the fallback set AND be put
    // back into the active list, or it stays invisible for the rest of the session.
    self.reactivate_tools(&surface.tool_names);
```

and, after the adoption:

```rust
    let changed = added + updated + deactivated.len();
    if changed > 0 && has_ui {
        services.notify(
            &format!("MCP: direct tools refreshed (+{added}, ~{updated}, -{})", deactivated.len()),
            cyrup_ext::NotifyKind::Info,
        );
    }
```

`deactivate_tools` is §20 steps 5–6 only — cyrup has no `unregister_tool` on `ExtensionRegistry`,
which lands it on upstream's own `unregisterTool === undefined` branch:

```rust
/// `deactivateTools(toolNames)` (13a §20) — the `setActiveTools` fallback, which is the ONLY branch
/// cyrup has. Record the accepted delta at the call site: a deactivated MCP tool stops being
/// callable but its name stays in the registry for the session, exactly as upstream behaves against
/// a host without `unregisterTool`.
fn deactivate_tools(&self, names: &[String]) {
    if names.is_empty() { return }
    let Some(services) = self.host_services() else { return };
    let remove: std::collections::HashSet<&str> = names.iter().map(String::as_str).collect();
    let active = services.active_tools();
    let Some(active) = active.filter(|a| !a.is_empty()) else {
        // `getActiveToolsIfReady()` returned undefined/empty: remember them and return.
        if let Ok(mut slot) = self.fallback_deactivated_tools.lock() {
            slot.extend(names.iter().cloned());
        }
        return;
    };
    let next: Vec<String> = active.iter().filter(|n| !remove.contains(n.as_str())).cloned().collect();
    // "called ONLY when the filtered list is actually shorter".
    if next.len() != active.len() {
        if let Ok(mut slot) = self.fallback_deactivated_tools.lock() {
            slot.extend(names.iter().cloned());
        }
        services.set_active_tools(&next);
    }
}
```

---

## Out-of-group blockers found while reading

These are **not** this group's units. They are named so the owner routes them instead of
discovering them mid-flight.

1. **`MCP-211` (`formatSchema`) and `MCP-091` (`renderTsShape`) do not exist and are unscheduled.**
   `grep -rn 'format_schema\|ts_shape' src` finds only the two `ProxyEnv` declarations
   ([`proxy.rs:1554`](../../crates/cyrup-mcp/src/proxy.rs), `:1556`), the `FakeEnv` bodies at
   `:5034` / `:5037`, and three call sites — `:2246` and `:2467` (`describe` / `search`) and `:3574`
   (the direct-tool `Expected parameters:` error suffix). All three are **Wave 3's** paths
   (`MCP-043`, `MCP-214`), not this group's. `RuntimeEnv` must supply a body to compile;
   `render_ts_shape` returning `None` is upstream's own real branch (the caller forks to
   `Parameters:` + `format_schema`, [`proxy.rs:2244-2246`](../../crates/cyrup-mcp/src/proxy.rs)) and
   is therefore honest, but `format_schema` is model-facing text and must not be improvised. **Route
   `MCP-211` to Wave 3's owner** — that wave is the only consumer — and land `RuntimeEnv`'s two
   methods as one-line delegations to `crate::proxy::format_schema` / `render_ts_shape` so the
   signature is fixed and there is exactly one place to fill.

2. **`ConnectionBuilder` never gets `with_auth_provider`.** `initialize_mcp` installs the builder at
   [`runtime.rs:193-196`](../../crates/cyrup-mcp/src/runtime.rs) with the default
   `NoStoredCredentials` ([`runtime.rs:1900-1906`](../../crates/cyrup-mcp/src/runtime.rs)), so once
   §11 exists **every HTTP server ends at `needs-auth` even when its credential is already in the
   store**. The comment at [`runtime.rs:189-192`](../../crates/cyrup-mcp/src/runtime.rs) names this.
   That is `MCP-115`, section 05, and it will be the first thing a §11 end-to-end run reports. Do not
   fix it here; expect it in the result.

---

## Acceptance Criteria

Behavioural and source-observable only. Each line is checkable by reading the tree or driving the
extension; none asks for a new test suite, a benchmark or a document.

**The spine**

- [ ] `initialize_mcp` ([`runtime.rs`](../../crates/cyrup-mcp/src/runtime.rs)) executes §9–§15 and
      returns only after `start_health_checks` and a final `update_status_bar`; the zero-enabled-server
      early return is still the only other exit.
- [ ] `owner.throw_if_inactive()?` appears at exactly four points in `initialize_mcp` outside the
      `open_browser` closure: after the startup connect pass, at the top of every pass-two iteration,
      after the `MCP_DIRECT_TOOLS` bootstrap, and before `start_health_checks` (MCP-046).
- [ ] `crates/cyrup-mcp/src/env.rs` exists and holds `RuntimeEnv` plus the §13/§17/§18/§19/§3.16
      verbs; `grep -rn 'impl ProxyEnv\|ProxyEnv for' src` returns `RuntimeEnv` **and** `FakeEnv`, and
      `RuntimeEnv`'s definition is outside every `#[cfg(test)]`.
- [ ] `McpExtension::install_runtime_env` exists beside `install_surface_sync`, stashes an
      `Arc<ProxyCtx>`, and has a public accessor for it.
- [ ] `call_tool` and `read_resource` are the **only** `RuntimeEnv` methods that report an unbuilt
      seam, and each names `MCP-164` in its body.

**The payload contract**

- [ ] `McpStatusSnapshot` carries `version` / `servers` / `total_tools` / `total_resources` /
      `connected_count` / `disabled_count`; `McpServerStatusSnapshot` carries exactly six keys with
      `resource_count` and `failed_ago_seconds` under `skip_serializing_if = "Option::is_none"` and
      `disabled` unconditional; `McpServerRuntimeStatus` has six variants serialising as
      `connected` / `cached` / `failed` / `needs-auth` / `not-connected` / `disabled`.
- [ ] `create_mcp_status_snapshot` iterates `config.mcp_servers` directly (config order), and
      `failed_ago_seconds` is `Some` only when `status == Failed`.
- [ ] `McpStatusSnapshot::default()` is `publishMcpStatusShutdown`'s all-zero payload, so
      [`runtime.rs:287`](../../crates/cyrup-mcp/src/runtime.rs) and
      [`lifecycle.rs:1573`](../../crates/cyrup-mcp/src/lifecycle.rs) become correct without edits.
- [ ] `update_status_bar` publishes **before** the `state.ui.is_none()` return, and is called from
      §11, §14, §15, all three lifecycle callbacks, the list-changed listener, `lazy_connect`'s
      success and failure arms, and `record_failure`'s expiry task.

**The fence and the swallow points**

- [ ] The `fenced!` invocation in [`owner.rs`](../../crates/cyrup-mcp/src/owner.rs) lists all 66
      `HostServices` methods; the 35 names enumerated in sub-wave B each appear exactly once.
- [ ] `McpState::notify_tool_metadata_updated` wraps the listener call in
      `catch_unwind(AssertUnwindSafe(..))` and logs
      `MCP: metadata update hook failed for {server}: {message}` at debug on both the panic and the
      error path; a panicking listener does not propagate.
- [ ] `McpExtension::on_event` returns
      `HookOutcome::Mutate(EventPatch::ToolResult { is_error: Some(true), content: None, details: None, usage: None })`
      for `details.error ∈ {"tool_error", "call_failed"}` and `HookOutcome::Noop` for every other
      value including `auth_required`, absent `details`, and `details: null`.

**The verbs**

- [ ] `FAILURE_BACKOFF_MS = 60_000` and `MAX_FAILURE_MESSAGE_CHARS = 8 * 1024` exist as named
      constants; `record_failure` calls `clear_failure` first, truncates on a char boundary, and its
      expiry task selects on the owner token *and* re-checks `last_failure == failed_at` before
      clearing.
- [ ] `failure_age_seconds` returns `None` strictly outside the 60 s window and `Some(round(secs))`
      inside it.
- [ ] `parallel_limit` exists, is used by both §11 and §14, and is `buffered`-based — `grep -rn
      'buffer_unordered' src/env.rs` returns nothing.
- [ ] `update_server_metadata` bails on a missing/non-connected connection and on a missing
      definition, deletes from **all five** maps when the definition is disabled, and writes
      `prompt_metadata` + `prompt_metadata_live` only when `!connection.prompt_discovery_failed()`.
- [ ] `McpState::tool_metadata` is `Mutex<IndexMap<String, Vec<ToolMetadata>>>`; the
      `ServerToolMetadata` forward declaration is gone from
      [`state.rs`](../../crates/cyrup-mcp/src/state.rs).
- [ ] `lazy_connect` implements the four `false` guards in §19's order, and its error arm calls
      `record_failure` for a stray abort on a live signal but **not** for an abort on a cancelled
      one.
- [ ] `RuntimeEnv::guard_mcp_output` reads `$MCP_OUTPUT_GUARD` through
      `McpSettings::output_guard`; `grep -rn '\.output_guard(' src` returns a production call site.

**The `initialize_mcp` steps**

- [ ] A config whose servers are all disabled emits `MCP: All {n} server(s) are disabled` as
      `NotifyKind::Info` exactly once, and only when `has_ui` and at least one entry exists.
- [ ] `bootstrap_all` is `true` only when the cache file was **absent**; an unparseable file is
      rewritten empty and does **not** bootstrap. `save_metadata_cache` has a production caller.
- [ ] Every enabled server reaches `register_server` with
      `idle_timeout = definition.idle_timeout.or(persists.then_some(0.0))`, and `mark_keep_alive` is
      called only for `keep-alive`.
- [ ] A hash-valid cache entry populates `tool_metadata`, `resource_counts`, `prompt_metadata` and
      `server_instructions`, and **does not** add the server to `prompt_metadata_live`.
- [ ] `McpLifecycleManager::new`'s `hasPendingAuth` is bound to `oauth::has_pending_auth`;
      `grep -n 'Arc::new(|_| false)' src/runtime.rs` returns nothing.
- [ ] The three lifecycle callbacks and the list-changed listener are installed, each body opens with
      `state.owner.is_active()`, and the listener passes `preserve_empty_resources: false`.
- [ ] `add_cleanup` is called three times in `initialize_mcp`, with `MaterializedResources::cleanup`
      **first** so LIFO runs it last; `MaterializedResources::new` has a production caller and
      `McpState` owns the session.
- [ ] The `MCP_DIRECT_TOOLS` bootstrap re-reads the env var and the cache from inside
      `initialize_mcp`, excludes servers already connected in §11, and its completion message matches
      the behaviour actually shipped (see MCP-026).
- [ ] `initialize_mcp` reads `settings.sampling(has_ui)` / `settings.elicitation(has_ui)` /
      `is_tui_mode()` and installs a `HandlerFactory` through
      `ConnectionBuilder::with_handler_factory`; a `has_ui:false` context with `samplingAutoApprove`
      unset produces a handler advertising no sampling capability.

**The extension seams**

- [ ] `init` spawns the pre-warm task when `needs_load_time_initialization` is true, and the task
      re-checks the generation before doing anything; the `pre-warm pending` debug line is gone.
- [ ] `on_session_start` reads `MCP_DIRECT_TOOLS`, skips the sentinel, and awaits the initialization
      only when `missing_configured_direct_tool_servers` is non-empty.
- [ ] `SendMessage` carries the `trigger_turn` flag; with it unset the send is synchronous, with it
      set the send is deferred behind `ensure_converged` and **still delivers** on a convergence
      failure with one debug line; `runtime.rs`'s `send_message not yet wired` debug is gone.
- [ ] `flush_metadata_cache` exists, iterates `manager.get_all_connections()` for
      `ConnectionStatus::Connected`, and is available as a `MetadataFlush`; `no_metadata_flush`'s
      *"pending MCP-031"* log line is gone.

**Handed off (sub-waves F and G)**

- [ ] `sync_tool_surface` computes `(added, updated, deactivated)`, calls `deactivate_tools` for the
      removals and re-activates a returning tool, and notifies
      `MCP: direct tools refreshed (+{a}, ~{u}, -{d})` only when the total is non-zero and there is a
      UI.
- [ ] `fallback_deactivated_tools` has a writer; `set_active_tools` is called only when the filtered
      list is genuinely shorter.
- [ ] `MaterializedResources` carries a pending-cleanup set with per-directory attempts capped at 3,
      a single 30 s retry task guarded against double-scheduling, and an aggregate error over
      `Vec<std::io::Error>`; the `TODO(MCP-224)` at
      [`renderers.rs:847`](../../crates/cyrup-mcp/src/renderers.rs) is gone.
