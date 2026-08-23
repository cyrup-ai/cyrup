---
stage: aug
status: done
updated: 2026-08-22 15:11
---

# Plan The Remaining High-Severity cyrup-mcp Backlog

## Description

The audit in [13-cyrup-mcp-STATUS.md](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) records
**73 of 147 `high` units open** ([:406](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)), inside
198 open units (98 missing + 100 partial,
[:351-357](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)). `13i` is excluded here — it has its own
scoping task, [MCP_13I_SCOPING.md](MCP_13I_SCOPING.md) — which leaves **58 open `high` units**
across `13a`–`13h`.

This task produces the batching. The deliverable is a plan, not code.

Open critical-or-high by section
([:369-386](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)):

| § | open critical+high | of which `high`, excl. 13i |
|---|---:|---:|
| `13c` servers/transports/cache | 23 | 20 |
| `13i` protocol/verification | 16 | — (excluded) |
| `13a` activation/lifecycle | 10 | 10 |
| `13h` panels/commands | 10 | 8 |
| `13b` config ladder | 9 | 8 |
| `13e` tools/naming/approval | 7 | 6 |
| `13d` proxy modes | 3 | 3 |
| `13g` oauth | 2 | 2 |
| `13f` credentials | 1 | 1 |

---

## First finding: the census is a snapshot, and it is now stale

The census header says so itself — *"The census below is **as of the audit** and is not rewritten by
later work"* ([:31](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) — and the audit is dated
2026-08-21 ([:10](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)). Waves 2–5 and HA-1 all landed
after it.

**A `missing` row is a lead, not a verdict**, for two independent reasons: the audit's own skeptic
pass overturned 15 rulings by construction
([:19-24](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)), *and* the tree has moved since. So a
sample of rows was read against the Rust before anything was scheduled.

### Spot-check of `missing` / `partial` rows against the tree, 2026-08-22

19 rows sampled. **12 no longer hold on the grounds the row states** — 10 overturned outright, 2
restated (the row names the wrong gap, and the real one is smaller). 7 held up exactly as written.

| unit | the row's claim | what the Rust says today | verdict |
|---|---|---|---|
| `MCP-100` | "The entire manager is unbuilt… absent: `ServerConnection`" ([:610](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | `ServerConnection` at [server_manager.rs:787-825](../../crates/cyrup-mcp/src/server_manager.rs) carries every field the row names | **overturned** |
| `MCP-116` | "No connection record carries `credentials_invalidated`" ([:435](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | field at `server_manager.rs:799`; step-7 carry-forward at `server_manager.rs:1807-1820` | **overturned** |
| `MCP-125` | "the disabled and stopped guards… neither string exists" ([:438](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | `server_manager.rs:101` and `:105`; both fire at `server_manager.rs:1707-1712`, **before** the single-flight map, with a MEASURED note saying so | **overturned** |
| `MCP-126` | "Nothing of §3.12 exists: the generation bump, the attempt cancel… removal before awaiting cleanup" ([:439](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | `close_inner` bumps the generation and takes the attempt handle under one guard at `server_manager.rs:2185-2194`; `connection_closed_while_connecting` at `:124` | **overturned** |
| `MCP-131` | "`ManagerSupervisor::close`/`close_all` are no-ops (lifecycle.rs:307-322)"; "`spawn_stdio_transport` has only test callers" ([:440](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | both delegate to the real manager at [lifecycle.rs:335-341](../../crates/cyrup-mcp/src/lifecycle.rs); `spawn_stdio_transport` has a production caller at [runtime.rs:2467](../../crates/cyrup-mcp/src/runtime.rs) | **overturned** |
| `MCP-134` | "The predicate does not exist" ([:441](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | `is_terminated_session` at `server_manager.rs:2769` | **overturned** |
| `MCP-140` | "The **serialisers are absent**… grepping all 19 files for a `ServerCacheEntry {` construction" ([:444](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | `serialize_tools` / `serialize_resources` / `serialize_prompts` at [dirs.rs:761/786/808](../../crates/cyrup-mcp/src/dirs.rs) — but **only test callers**. The gap is the call site, not the serialisers | **restated** |
| `MCP-084` | "`resolveServerUrl` is not implemented anywhere… each of its three exact strings returns nothing" ([:425](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | `resolve_server_url` at [credentials.rs:3478](../../crates/cyrup-mcp/src/credentials.rs), all three strings at `:3452` / `:3460` / `:3493`; wired by wave 5 ([:152-153](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | **overturned** |
| `MCP-231` | "The whole predicate is unwritten" ([:455](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | `is_tool_call_approval_required` at [proxy.rs:4591](../../crates/cyrup-mcp/src/proxy.rs), with presence-not-truthiness at `:4599-4604` | **overturned** |
| `MCP-232` (critical) | "The gate itself is unimplemented… Missing: the cache lookup/insert against `approved_tool_calls`" ([:399](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | free fn `ensure_tool_call_approved` at `proxy.rs:4787`, cache at `:4806` / `:4845`, production caller at `:3543` | **overturned** |
| `MCP-037` | "HA-1: a native extension has no handle to `ExtensionHost::register_late_tool`" ([:418](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | landed — see the HA-1 section below | **overturned** |
| `MCP-217` | "no `syncDirectTools`… and no `syncToolSurface` entry point" ([:454](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | `sync_tool_surface` at [extension.rs:166](../../crates/cyrup-mcp/src/extension.rs); the fingerprint diff at [registration.rs:2143-2172](../../crates/cyrup-mcp/src/registration.rs); `LateSink` at `:2019-2062`. The `deactivateTools` fallback pass is still absent — `fallback_deactivated_tools` (`extension.rs:101`) has no writer | **restated: `missing` → `partial`** |
| `MCP-008/009/010/011` | `on_session_start` never builds | confirmed: [extension.rs:455-479](../../crates/cyrup-mcp/src/extension.rs) bumps the generation, drops `_previous_state` at `:458`, and only *logs* the post-await re-check at `:474` | **confirmed open** |
| `MCP-119` | "No discovery at all" ([:436](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | confirmed: no `list_all_tools` / `list_tools` call anywhere in production; `ServerConnection::tools`' own doc (`server_manager.rs:806-811`) says it stays empty until this unit | **confirmed open** |
| `MCP-164` | "The rmcp invocation itself does not exist" ([:448](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | confirmed: `ProxyEnv::call_tool` / `read_resource` (`proxy.rs:1465` / `:1478`) have exactly one implementor, `FakeEnv` at `proxy.rs:4932`, inside the `#[cfg(test)]` opening at `proxy.rs:4862` | **confirmed open** |
| `MCP-207` | "The whole live `tools`+`resources` → `Vec<ToolMetadata>` pipeline is absent" ([:451](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | confirmed: no `build_tool_metadata`; `ConnectOutcome`'s doc defers it by name at `proxy.rs:1232-1236` | **confirmed open** |
| `MCP-073` | `resolveServerFromToolName` absent ([:422](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | confirmed: zero hits crate-wide | **confirmed open** |
| `MCP-076` | "compiles with bare `Regex::new(&out).ok()`, without the `size_limit` / `dfa_size_limit` ceilings" ([:424](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | confirmed at [registration.rs:325](../../crates/cyrup-mcp/src/registration.rs) (the row cites `:334`; the file has shifted) | **confirmed open** |
| `MCP-092` | the dual-dialect JSON Schema validator ([:426](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | confirmed: the only `$schema` readers are `agent_plugin.rs:501` and `:677`, which are *equality* checks on plugin/mcp manifests — a different mechanism | **confirmed open** |

**What the sample means for scheduling.** The overturns cluster in exactly the families waves 2–5
and HA-1 touched — the manager, the approval gate, late registration. They are not randomly
distributed, so the correct inference is *"re-read any row in a family a wave has since touched"*,
not *"the audit is unreliable"*. Rows in families nothing has touched (`13b` naming, `13h` commands,
`13g` oauth) held up in every case sampled.

**Every wave below must open by re-reading its units' rows against the tree.** Budget it; it is
cheap and it has a 12-in-19 hit rate right now.

---

## Second finding: HA-1 has landed, and it unblocks three units

[HOST_LATE_TOOL_REGISTRATION.md](../done/2026-08-22-14-00/HOST_LATE_TOOL_REGISTRATION.md) is
`completed`. Verified against the tree today, both sides of the seam:

**Host side** — `cyrup-ext`:
- `trait LateRegistrar` at [native.rs:768](../../crates/cyrup-ext/src/native.rs), with
  `register_tool` / `register_command` / `register_tool_renderer` / `owner`. Deliberately **not**
  `cfg(feature = "wasm-host")`, per its own doc.
- `NativeExtension::set_late_registrar` at `native.rs:697`, bound before `init`.
- `ExtensionHost::register_late_tool` / `register_late_command` at
  [facade.rs:707](../../crates/cyrup-ext/src/facade.rs) / `:724`; `late_registrar_for` at `:736`;
  `add_commands_listener` at `:758`, fired by `notify_commands_changed`.
- The TUI already subscribes:
  [extension_ui.rs:271](../../crates/cyrup-tui/src/app/extension_ui.rs).

**Consumer side** — `cyrup-mcp`:
- the stash at [extension.rs:118](../../crates/cyrup-mcp/src/extension.rs), set at `:783`.
- `sync_tool_surface` at `extension.rs:166`, called from `extension.rs:435` and
  [proxy.rs:4510](../../crates/cyrup-mcp/src/proxy.rs).
- `LateSink` at [registration.rs:2019-2062](../../crates/cyrup-mcp/src/registration.rs) writing
  through to the registrar, with the fingerprint diff at `:2143` and the proxy-description diff at
  `:2172`.

### Re-sequencing, unit by unit

| unit | ledger says | actual | action |
|---|---|---|---|
| `MCP-037` | `missing` ([:418](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | **implemented** — both shapes the row offered, option (i), plus the command verb | **remove from the backlog** |
| `MCP-217` | `missing` ([:454](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | **partial** — everything but the `deactivateTools` fallback pass | **shrinks to one obligation**; stays in Wave 3 |
| `MCP-395` | `partial`, "the live half has nothing to land on and none of the three additions have been made" ([:466](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | all three additions landed (`register_late_command`, the change notification, the TUI subscriber). What remains is a **live caller**: `sync_tool_surface` must be reached from a real connect | **re-sequenced** after Wave 1 + Wave 2; no longer blocked on HA-1 |

Nothing else in the 58 named HA-1 as its blocker. `MCP-382` (HA-2, argument completions) is a
different host addition and is `medium` — out of scope here.

---

## Third finding: the real root blocker is the request seam, not HA-1

HA-1 was the *registration* seam. The one still closed is the **request** seam, and it is what
actually orders the plan:

- `trait ConnectionResource`
  ([server_manager.rs:510-534](../../crates/cyrup-mcp/src/server_manager.rs)) exposes exactly
  `close` / `has_session_id` / `child_pid` / `stderr_detail`. There is no way to issue a request.
- The peer exists and is hidden: `McpConnection` holds `peer: Peer<RoleClient>` at
  [runtime.rs:2112](../../crates/cyrup-mcp/src/runtime.rs).
- `NewConnection` (`server_manager.rs:1130-1137`) has no field for tools/resources/prompts, and
  `ServerConnection`'s `tools` doc (`server_manager.rs:806-811`) records that it stays empty until
  MCP-119.

Wave 5 named this itself: *"`McpConnection`'s `Peer` is unreachable through `ConnectionResource`…
nothing outside `runtime.rs` can issue a request on a connection the builder made"*
([:188-191](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)).

Widening that seam is one change to two types. Five units are blocked behind it and **none of them
can be split away from it** — which is precisely the PR #30 failure mode the description records:
grouping by file put `runtime.rs` in a different agent's set than the unit whose obligation needed
it.

---

## Already closed — do not schedule

| family | units | evidence |
|---|---|---|
| hashing / metadata identity | `MCP-141`, `MCP-142`, `MCP-146` (critical) | [:29-49](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md), [:50-93](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) — every digest now equals upstream's, measured on node 22 |
| transport / connection | `MCP-101`, `MCP-105`, `MCP-109`, `MCP-114`, `MCP-115a`, `MCP-124` | wave-5 table, [:129-139](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| consent gates | `MCP-231`, `MCP-232` (critical) | `proxy.rs:4591` and `proxy.rs:4787`, verified above |
| late registration | `MCP-037` | `native.rs:768` / `facade.rs:724`, verified above |
| manager guards | `MCP-100`, `MCP-116`, `MCP-125`, `MCP-126`, `MCP-131`, `MCP-134` | verified above |
| URL / secret resolution | `MCP-084` | `credentials.rs:3478`, verified above |

Four of those rows — `MCP-141`, `MCP-142`, `MCP-146`, `MCP-232` — are `critical`, not `high`, and
were never among the 58; they are listed because their families are the ones whose rows went stale,
and a reader checking a neighbouring row needs to know why. **Of the 58 `high` units, 15 come off
the list**: `MCP-037`, `MCP-084`, `MCP-100`, `MCP-101`, `MCP-105`, `MCP-109`, `MCP-114`, `MCP-115a`,
`MCP-116`, `MCP-124`, `MCP-125`, `MCP-126`, `MCP-131`, `MCP-134`, `MCP-231`. That leaves **43** to
batch.

Each still needs its census row re-ruled by whoever touches the family. "The row's grounds no longer
hold" is not the same claim as "the unit's full obligation is met" — `MCP-140` and `MCP-217` are the
worked examples of the difference above.

## Already filed as their own tasks — do not re-scope

| unit(s) | task |
|---|---|
| `MCP-119` | [MCP_DISCOVERY_PAGINATION.md](MCP_DISCOVERY_PAGINATION.md) |
| `MCP-135` | [MCP_SESSION_RECOVERY.md](MCP_SESSION_RECOVERY.md) |
| `MCP-115` residual (the 401 with a JSON-RPC body) | [MCP_401_JSON_RPC_BODY.md](MCP_401_JSON_RPC_BODY.md) |
| `MCP-370` + the `mcp_direct_tools` filter half | [MCP_DIRECT_TOOLS_FILTERS.md](MCP_DIRECT_TOOLS_FILTERS.md) |
| the `lenient` / typed-reader config decision (`MCP-144`'s prerequisite) | [MCP_CONFIG_LENIENT_TYPES.md](MCP_CONFIG_LENIENT_TYPES.md) |
| all 15 open `high` units in `13i` | [MCP_13I_SCOPING.md](MCP_13I_SCOPING.md) |

`MCP-119` and `MCP-135` are filed separately but **belong to Wave 1's set and must be executed with
it** — see Wave 1.

---

# The waves

43 units. Grouped by shared obligation — the mechanism or contract each set moves — not by file.
Where a wave touches a file another wave also touches, that file is listed in both: an agent that
cannot edit a file its obligation reaches is the failure PR #30 recorded.

Waves 4, 5, 8 and 9 have **no prerequisite** and can start immediately, in parallel with Wave 1.

```
        ┌── Wave 1: the request seam ──┬── Wave 3: the tool-execution path
        │                              └── Wave 2: the session-start build ── Wave 7: the command surface
  (now) ┼── Wave 4: pattern + schema compilation
        ├── Wave 5: tool-name resolution
        ├── Wave 6: cache identity + interpolation   [after MCP_CONFIG_LENIENT_TYPES decides]
        ├── Wave 8: oauth residuals
        └── Wave 9: two standalones
```

---

## Wave 1 — The request seam

**Shared obligation.** Nothing outside `runtime.rs` can issue an MCP request on a live connection,
and no connection record can carry a discovered surface. Every unit here is blocked on widening
`ConnectionResource` and `NewConnection`, and each one's obligation *is* a request through that
seam. This is one change to two types with five consumers; splitting it splits the seam from its
users.

**Units (4, plus 1 already filed).**

| unit | title | row |
|---|---|---|
| `MCP-119` | Paginated discovery with capability gating and per-list failure policy | [:436](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) — filed as [MCP_DISCOVERY_PAGINATION.md](MCP_DISCOVERY_PAGINATION.md) |
| `MCP-164` | Port `executeCall`'s invocation paths and result shaping | [:448](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-140` | Metadata cache: serialisers and reconstructors — **restated**: the serialisers exist ([dirs.rs:761/786/808](../../crates/cyrup-mcp/src/dirs.rs)), the live call site does not | [:444](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-135` | `withSessionRecovery` retry wrapper | [:442](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) — filed as [MCP_SESSION_RECOVERY.md](MCP_SESSION_RECOVERY.md) |

`MCP-134` (`isTerminatedSession`) is **not** in this wave — it is already built at
`server_manager.rs:2769`. Its consumer is: `ManagerSupervisor::should_reconnect_after_refresh`
([lifecycle.rs:347-351](../../crates/cyrup-mcp/src/lifecycle.rs)) still returns a hardcoded `false`
"pending MCP-120". That wiring rides along with `MCP-135`.

**Files.** `crates/cyrup-mcp/src/server_manager.rs`, `runtime.rs`, `proxy.rs`, `dirs.rs`,
`lifecycle.rs`.

**Must land before it.** Nothing. This is the root.

**How the result is verified.** A loopback fixture server that answers `initialize`, `tools/list`,
`resources/list` and `prompts/list`, driven through the real `ConnectionBuilder` (installed at
[runtime.rs:170-186](../../crates/cyrup-mcp/src/runtime.rs)) rather than a stub factory: assert a
connected `ServerConnection` carries the discovered surface, that a `tools/call` issued through
`ProxyEnv` reaches the peer, and that the cache entry written from that surface is byte-identical to
one built from the same lists by hand. Ablate by restoring `ServerConnection::new`'s empty-vec
construction and confirm the assertions fail — per the description's rule, a fix not proven by an
ablation is not pinned.

**The trap.** `MCP-119` and `MCP-135` have their own task files, so the obvious move is to hand them
to two agents and `MCP-164`/`MCP-140` to a third. **Do not.** All four write `server_manager.rs`'s
seam. One agent, one branch; the two filed tasks are that agent's reading, not other agents' work.

---

## Wave 2 — The session-start build

**Shared obligation.** `McpExtension::on_session_start` never builds a runtime, so nothing this port
has built is reachable from a live session. Every unit here is a step of the one generation
protocol, and they share mutable state (`generation`, `owner`, `state`, `init_task`) that cannot be
edited by two agents at once.

The evidence, verified today: [extension.rs:455-479](../../crates/cyrup-mcp/src/extension.rs) bumps
the generation, takes and **drops** `_previous_state` at `:458`, calls nothing, and only *logs* the
post-await staleness check at `:474`. `runtime::initialize_mcp`
([runtime.rs:125](../../crates/cyrup-mcp/src/runtime.rs)) still has one caller, `runtime.rs:403`,
inside `#[cfg(test)]` — the wave-5 correction says so in as many words
([:118-124](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)).

**Units (8).**

| unit | title | row |
|---|---|---|
| `MCP-008` | The `session_start` generation protocol, abort-before-await | [:410](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-009` | The `session_shutdown` handler | [:411](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-010` | `shutdownState`, preserving the metadata-flush error | [:412](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-011` | `startInitialization`'s triple staleness check and metadata-update hook install | [:413](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-014` | Re-`init` per session, and the build-before-dispose inversion | [:414](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-023` | The two-pass startup metadata build | [:415](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-025` | Startup connect notifications, terminal sanitising, skipped-tool warnings | [:416](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-029` | `updateMetadataCache` write rules | [:417](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |

**Files.** `crates/cyrup-mcp/src/extension.rs`, `lifecycle.rs`, `runtime.rs`, `state.rs`, `dirs.rs`.

**Must land before it.** Wave 1, for `MCP-023` and `MCP-029` specifically: both record metadata
derived from a connection's discovered surface, and there is no surface until Wave 1 lands.
`MCP-008`/`009`/`010`/`011`/`014` are structurally independent of Wave 1 and can start in parallel —
but they cannot be *demonstrated* end to end until it does, which is why the wave is sequenced
whole.

**How the result is verified.** Drive two `HostEvent::SessionStart` dispatches through the real
handle: assert generation N−1's owner is stopped before generation N builds; that the shutdown-time
status snapshot and the metadata flush both happen, with the flush error winning; and that a
continuation superseded mid-drain does **not** become the live runtime. Ablate the triple staleness
check and confirm the superseded continuation takes over.

**Watch for.** The wave-5 report flags a latent `sync_tool_surface` / generation-swap race that is
"unreachable while `on_session_start` is MCP-008's stub"
([HOST_LATE_TOOL_REGISTRATION.md](../done/2026-08-22-14-00/HOST_LATE_TOOL_REGISTRATION.md)). This
wave is what makes it reachable. It belongs to this wave, not to a follow-up.

---

## Wave 3 — The tool-execution path

**Shared obligation.** The model-visible tool that actually executes a call. `MCP-207` produces the
metadata `MCP-214` consumes; `MCP-214a` is `MCP-214`'s recovery arm; `MCP-043` is which of two
disjoint tool types the model actually reaches; `MCP-249` is the error shape all of them emit.
Splitting any of them hands one agent the producer and another the consumer.

**Units (6).**

| unit | title | row |
|---|---|---|
| `MCP-207` | `buildToolMetadata` | [:451](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-214` | The direct-tool execute state machine | [:452](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-214a` | `recoverAuthConnection` and the per-server request options | [:453](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-043` | The `mcp` gateway tool: registration, the init wait, the dispatch order | [:419](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-217` | Post-init dynamic tool registration — **reduced to** the `deactivateTools` fallback pass; `fallback_deactivated_tools` ([extension.rs:101](../../crates/cyrup-mcp/src/extension.rs)) has no writer | [:454](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-249` | Freeze the details schema this subsystem emits — `server_unavailable` still returns zero hits crate-wide | [:456](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |

**Files.** `crates/cyrup-mcp/src/proxy.rs`, `registration.rs`, `extension.rs`.

**Must land before it.** Wave 1 — `MCP-214`'s state machine ends in a `tools/call` /
`resources/read`, which is the seam. `MCP-207` alone could start earlier, but its only consumer is
in this wave.

**How the result is verified.** One direct tool and one gateway call executed end to end against
Wave 1's loopback fixture. Assert on the way through that the **already-built** approval gate
(`ensure_tool_call_approved`, `proxy.rs:4787`) is on the path and not bypassed — it has a production
caller at `proxy.rs:3543` today and must still have one after this wave rewrites the call path.
Ablate `MCP-043` by re-pointing registration at `proxy::McpTool` and confirm the model-visible tool
changes behaviour, which is the whole claim of that row.

**Note.** `MCP-043`'s row says `proxy::McpTool::new` is constructed only in tests. Re-verified: the
`#[cfg(test)]` module opens at `proxy.rs:4862` and every construction is after it. This row held up.

---

## Wave 4 — Untrusted pattern and schema compilation

**Shared obligation.** Compiling a user-supplied pattern or schema and running it against input,
with resource ceilings. Two units, one mechanism, one risk class.

**Units (2).**

| unit | title | row |
|---|---|---|
| `MCP-076` | Port glob matching and `isToolIncluded`/`isToolExcluded`/`isToolAllowed` — [registration.rs:325](../../crates/cyrup-mcp/src/registration.rs) is bare `Regex::new(&out).ok()` with no `size_limit` / `dfa_size_limit`, while the `proxy.rs` copy sets both | [:424](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-092` | Port the dual-dialect JSON Schema validator | [:426](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |

**Files.** `crates/cyrup-mcp/src/registration.rs`, `proxy.rs`, `config.rs`.

**Must land before it.** Nothing. Start today.

**How the result is verified.** A pathological glob (nested quantifiers over a long candidate set)
is refused at compile rather than run; the two copies of the compiler are asserted to produce the
same ceiling. For `MCP-092`, the dialect router (`$schema` absent ⇒ unstamped; one trailing `#`
stripped) checked against upstream's own routing on node 22, not against the prose — the description
records that every parity bug worth finding in PR #30 came from executing upstream.

**Coupling to note.** `MCP-092`'s validator is also what `13i`'s `MCP-465` (elicitation response
validation) needs. Flag it to [MCP_13I_SCOPING.md](MCP_13I_SCOPING.md) as a shared dependency rather
than letting that section build a second one.

---

## Wave 5 — Tool-name resolution

**Shared obligation.** The tool-name grammar and its inverse — one prefix scheme, read forwards
(`formatToolName`) and backwards (`resolveServerFromToolName`), with a legacy alias set in between.
Both units are wrong or absent in the same grammar.

**Units (2).**

| unit | title | row |
|---|---|---|
| `MCP-073` | Port `resolveServerFromToolName` with its ambiguity fail-safe — zero hits crate-wide | [:422](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-075` | Port `getToolNameCandidates` (the legacy candidate set) — `format_legacy_tool_name` at [proxy.rs:509-530](../../crates/cyrup-mcp/src/proxy.rs) derives its legacy prefix from `get_server_prefix`, which has **already** sanitised, so the second pass re-escapes an escape | [:423](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |

**Files.** `crates/cyrup-mcp/src/proxy.rs`.

**Must land before it.** Nothing. Start today; it can share an agent with Wave 4, which blocks
nothing either.

**How the result is verified.** A differential table generated by running upstream's own
`resolveServerFromToolName` and `getToolNameCandidates` on node 22 over a fixture set that includes
nesting prefixes (`gh` vs `gh_api`), a server name needing escapes, and the `-mcp` suffix strip
(`strip_mcp_suffix`, `proxy.rs:483`). The ambiguity fail-safe asserted with two servers whose
prefixes nest: the answer must be `None`, not a longest-match guess.

---

## Wave 6 — Cache identity and value interpolation

**Shared obligation.** What goes into a hashed pre-image, and what a `!` / `!!` / `{env:NAME}`
expression resolves to before it gets there. One contract spanning two crates — `cyrup-mcp` writes
it, `cyrup-ext-subagents` reads it — which is the drift the hashing wave existed to close
([:29-49](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)).

**Units (6).**

| unit | title | row |
|---|---|---|
| `MCP-070` | Enforce the absent-vs-null hash pre-image contract | [:421](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-139` | Metadata cache: path, schema, version, load and merge-save | [:443](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-143` | `interpolateEnvVars` is missing its third pattern `{env:NAME}` | [:445](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-144` | `!`/`!!` secret-expression semantics in hashed values | [:446](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-145` | `isServerCacheValid` including the throw-to-false rule | [:447](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-094` | Reconcile `mcp_direct_tools` with this section's contract | [:427](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |

**Files.** `crates/cyrup-mcp/src/dirs.rs`, `config.rs`, `secrets.rs`, `registration.rs`;
`crates/cyrup-ext/src/caps/proc.rs`;
`crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs`.

**Must land before it.** [MCP_CONFIG_LENIENT_TYPES.md](MCP_CONFIG_LENIENT_TYPES.md). That task
decides whether a non-string `env` member throws, degrades or is dropped — and `MCP-144` and
`MCP-145` cannot be specified until it does, because the throw-to-false rule *is* the answer to that
question. Four measured divergences in this family are recorded unfixed at
[:276-305](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md); they are this wave's inputs, not
separate work.

**How the result is verified.** A differential table asserting **both** crates against upstream's
own `stableStringify` + `computeServerHash` on node 22 — never against each other. That distinction
is what surfaced the fifth divergence (`auth: null`) the hashing wave found
([:83-89](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) and is non-negotiable here.

**Free to change.** `dirs::save_metadata_cache` still has no production call site
([:55-56](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)), so no deployed digest has to be
invalidated. **Wave 2 changes that** — `MCP-029` gives it one. Sequence Wave 6 before Wave 2 lands
its metadata write, or accept a migration.

---

## Wave 7 — The command surface

**Shared obligation.** `/mcp`'s handler does not exist, so every one of its subcommands is
unreachable. `MCP-381` is the prologue and the eight-way switch; the rest are its arms. They cannot
be written by different agents — an arm has nowhere to attach until the switch exists, and the
owner-fenced prologue is the contract every arm inherits.

**Units (8).**

| unit | title | row |
|---|---|---|
| `MCP-381` | `/mcp`: registration, the owner-fenced prologue and the eight-way switch | [:460](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-386` | Port `reconnectServer` / `reconnectServers` | [:461](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-387` | Port `/mcp setup` and the reload-after-write flow | [:462](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-388` | Port `logoutServer` | [:463](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-390` | Port `authenticateServer` and `/mcp-auth` | [:464](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-392` | Port `buildMcpPanelCallbacks`'s connection-status derivation | [:465](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-398` | Port the prompt command handler | [:467](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-395` | HA-1's command leg — **reduced to** a live caller for `sync_tool_surface` (see the HA-1 section) | [:466](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |

**Intra-wave order.** `MCP-381` first, alone. Then the arms in any order. Then `MCP-392`, whose
eight-rung status ladder reads state the arms write.

**Files.** `crates/cyrup-mcp/src/ui.rs`, `registration.rs`, `oauth.rs`, `extension.rs`.

**Must land before it.** Wave 2 — the prologue awaits `initPromise` and there is no init promise
until `on_session_start` builds one. Wave 1 for `MCP-386`, whose `reconnect` must actually reconnect.
`MCP-394` (critical, `openMcpPanel`'s orchestration, `TODO(MCP-394)` at
[:401](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) is the panel half of the same surface and
should go to this wave's agent even though it is not a `high` unit — the alternative is two agents
in `ui.rs`.

**How the result is verified.** Each arm driven through `execute_command` with a scripted UI, the
load-bearing strings asserted byte-exact — `MCP-388`'s
`OAuth credentials were cleared for "{name}", but its connection could not be closed: {msg}` is the
example the row itself picks out, because it is the string that distinguishes two outcomes a user
must be able to tell apart. Ablate the owner fence and confirm a superseded generation's `/mcp`
still writes to the live panel.

---

## Wave 8 — OAuth residuals

**Shared obligation.** Two places where a typed failure or a named cancellation is flattened into a
string and loses information the caller needs.

**Units (2).**

| unit | title | row |
|---|---|---|
| `MCP-324` | `getValidToken`'s refresh path and its fall-through — the credential-store rethrow is a **string-prefix** test: [oauth.rs:3582](../../crates/cyrup-mcp/src/oauth.rs) does `error.to_string().starts_with(CREDENTIAL_STORE_PREFIX)`, with the constant at `:3600` | [:458](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |
| `MCP-326` | The manual/headless leg: parsing and the callback-versus-paste race — `combined_signal` ([oauth.rs:2478](../../crates/cyrup-mcp/src/oauth.rs)) builds a bare `CancelToken` carrying no reason, so an external abort cannot reject with the identical reason value | [:459](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) |

**Files.** `crates/cyrup-mcp/src/oauth.rs`, `abort.rs`, `credentials.rs`, `errors.rs`,
`server_manager.rs`.

**Must land before it.** Nothing. Start today.

**Include in the same set.** The latent hazard recorded at
[:310-320](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md): `McpError::CredentialStore` loses its
class crossing `From<&ManagerError>`, so `is_credential_store_failure()` answers `false` for a store
failure raised inside the connection factory. Same mechanism as `MCP-324` — a typed failure reduced
to an untyped one — and the recorded fix shapes (make `AuthStoreError` `Clone`, or give
`ManagerError` a `CredentialStore` arm) touch `credentials.rs` and `server_manager.rs`, which is why
it must be this agent's and not a separate one's.

**How the result is verified.** An abort raised with a named reason asserted to arrive at the
awaiting side carrying *that* reason value, not a generic cancellation. A store failure raised
inside the factory asserted to be recognised structurally by `is_credential_store_failure` — and the
same failure with its message rewritten still recognised, which is the assertion the prefix test
cannot pass.

---

## Wave 9 — Two standalones

Neither shares an obligation with anything else. They are here so they are not lost, and they are
the right work for an agent with a short window.

| unit | § | title | evidence | files |
|---|---|---|---|---|
| `MCP-068` | 13b | Port env-var overrides, including the `__none__` sentinel — `MCP_UI_DEBUG` still has **zero** readers in `crates/cyrup-mcp/src` | [:420](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) | `config.rs`, `lib.rs` |
| `MCP-260` | 13f | Re-exec under `keyctl session -` via a hidden `__mcp-keyring-helper` subcommand — `crates/cyrup/src/mcp_keyring_helper_cmd.rs` still does not exist | [:457](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) | `crates/cyrup/src/` (new module + `lib.rs`), `crates/cyrup-mcp/src/credentials.rs` |

**Must land before them.** Nothing.

**How the results are verified.** `MCP-068`: each override read with the variable set, unset, and set
to `__none__`, and the sentinel asserted to differ from unset. `MCP-260`: the subcommand selected
from a real argv before normal dispatch, and the re-exec asserted to run under a fresh keyring
session — measured against the process, not inferred from the code path.

---

## Not scheduled here, and why

| unit | § | disposition |
|---|---|---|
| `MCP-115` residual | 13c | filed as [MCP_401_JSON_RPC_BODY.md](MCP_401_JSON_RPC_BODY.md); the mechanism is documented at [:251-274](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) and it fails safe |
| `MCP-191` | 13d | the plan itself marks it `open-decision` ([:695](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) — `auth-start`/`auth-complete` deriving no distinct permission targets is a decision to take, not code to write. Route with the `13i` scoping decisions |
| `MCP-196` | 13d | the 47-case proxy-mode conformance suite. It is a verification unit, and it belongs with `13i`'s verification family (`MCP-483`/`MCP-484`/`MCP-490`/`MCP-496`) rather than in an implementation wave. Hand to [MCP_13I_SCOPING.md](MCP_13I_SCOPING.md)'s dependency order |
| the 15 open `high` units in `13i` | 13i | [MCP_13I_SCOPING.md](MCP_13I_SCOPING.md) |

**One cross-section note for the `13i` scoping task.** `MCP-471`'s row calls the human-interaction
gate `missing` ([:476](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)), but
`McpState::human_wait_ctx` exists at [state.rs:151](../../crates/cyrup-mcp/src/state.rs), is written
from `McpExtension::on_event` at [extension.rs:742](../../crates/cyrup-mcp/src/extension.rs), and is
consumed at [owner.rs:566-570](../../crates/cyrup-mcp/src/owner.rs). That row needs re-ruling before
`13i` schedules it.

---

## Accounting

| bucket | units |
|---|---:|
| open `high`, excluding `13i` | 58 |
| already closed or overturned by landed work | 15 |
| filed as their own tasks (`MCP-115` residual) | 1 |
| deferred as decisions / verification (`MCP-191`, `MCP-196`) | 2 |
| **batched into waves 1–9** | **40** |
| of which already have a task file (`MCP-119`, `MCP-135`) | 2 |

Waves: 1 → 4 units · 2 → 8 · 3 → 6 · 4 → 2 · 5 → 2 · 6 → 6 · 7 → 8 · 8 → 2 · 9 → 2. Total 40.

---

## Definition of done

This task is planning. It is done when all of the following are true of **this file**:

- [x] The 58 open `high` units outside `13i` are enumerated by section, and every one is accounted
      for in exactly one bucket: a wave, closed, filed elsewhere, or deferred with a reason.
- [x] Units are grouped by **shared obligation** — the mechanism or contract each set moves — and
      each wave states that obligation in one sentence before naming its units.
- [x] Every wave names its units, the files it will touch, what must land before it, and how its
      result would be verified.
- [x] The units blocked on HA-1 are named, HA-1's landing is verified against
      [native.rs](../../crates/cyrup-ext/src/native.rs) and
      [facade.rs](../../crates/cyrup-ext/src/facade.rs), and each is re-sequenced.
- [x] A sample of `missing` / `partial` rows is checked against the Rust, with the result reported
      including the rows that did **not** hold.
- [x] Units closed by the hashing, transport, consent-gate and late-registration work are listed and
      excluded from the waves.
- [x] Every claim carries a `file:line`, and every link resolves from `.flux/todo/`.

Not part of this task: writing code, tests, benchmarks or documentation; editing anything under
`docs/`; editing any source file. The waves' own verification approaches are described so an
implementing agent knows what evidence to produce — producing it is that agent's work, not this one's.

## Known limits of this plan

- **The census is a snapshot.** 12 of 19 sampled rows no longer held as written. Every wave re-reads its own
  rows first; the counts above will move.
- **Nothing here was built or run.** No `cargo` invocation was made. Every ruling is a reading of
  source at a cited line, with the same caveat the audit puts on itself
  ([:26-27](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)).
- **The `medium` tier is untouched** — 91 open units ([:361-367](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)).
  Several sit inside these waves' files (`MCP-208`, `MCP-215`, `MCP-217a`, `MCP-217b`, `MCP-382`,
  `MCP-394a`) and will be cheaper to take with the wave that is already in the file than alone
  later. Each wave's agent should sweep its files' `medium` rows before closing.
