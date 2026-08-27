---
stage: aug
status: done
updated: 2026-08-27 06:00
---

# The Initialization Spine And The One Production `ProxyEnv`

## Objective

`crates/cyrup-mcp` can build an `McpState`, and it can answer proxy-mode calls. **The two halves have
never been joined.** `initialize_mcp` stops at 13a §8 and returns a runtime with no connections, no
cache, no lifecycle registration and no callbacks; `ProxyEnv` — the trait every proxy mode calls
through — has no implementor outside `#[cfg(test)]`. Thirty-two units are all blocked behind, or all
write through, **one mechanism**: a live `McpState` with the verbs that mutate it, reachable from a
real session.

This task builds that mechanism: the `init.ts` live-state verbs, the one production `ProxyEnv`, and
the `initialize_mcp` body from §9 through §15. Execute as ordered sub-waves under **one** owner, not
as thirty-two tasks — grouping by file would split producers from consumers at every seam.

---

## READ THIS FIRST — the tree moved under this task

This file was written on 2026-08-22. Three things have changed or were mis-stated, and the first
invalidates roughly forty citations in the previous revision.

### 1 · `proxy.rs` no longer exists

`crates/cyrup-mcp/src/proxy.rs` (7,594 lines) was split into
[`crates/cyrup-mcp/src/proxy/`](../../crates/cyrup-mcp/src/proxy/) — 14 files, with
[`proxy/mod.rs`](../../crates/cyrup-mcp/src/proxy/mod.rs) glob re-exporting every submodule so every
`crate::proxy::X` path still resolves. **Every `proxy.rs:NNNN` citation in the previous revision is
dead.** The remap, re-read today:

| the old citation | what it is | where it lives now |
|---|---|---|
| `proxy.rs:1436-1585` | the `ProxyEnv` trait | [`proxy/env.rs:245-397`](../../crates/cyrup-mcp/src/proxy/env.rs) |
| `proxy.rs:1447` | `ProxyEnv::lazy_connect` | [`proxy/env.rs:256`](../../crates/cyrup-mcp/src/proxy/env.rs) |
| `proxy.rs:1465` / `:1478` | `call_tool` / `read_resource` | [`proxy/env.rs:274`](../../crates/cyrup-mcp/src/proxy/env.rs) / [`:287`](../../crates/cyrup-mcp/src/proxy/env.rs) |
| `proxy.rs:1499` | `update_server_metadata` | [`proxy/env.rs:307`](../../crates/cyrup-mcp/src/proxy/env.rs) |
| `proxy.rs:1517-1524` | `resolve_server_url` + its doc | [`proxy/env.rs:322-334`](../../crates/cyrup-mcp/src/proxy/env.rs) |
| `proxy.rs:1554` / `:1556` | `format_schema` / `render_ts_shape` | [`proxy/env.rs:363`](../../crates/cyrup-mcp/src/proxy/env.rs) / [`:365`](../../crates/cyrup-mcp/src/proxy/env.rs) |
| `proxy.rs:1577-1583` | `all_tool_names` + its doc | [`proxy/env.rs:385-393`](../../crates/cyrup-mcp/src/proxy/env.rs) |
| `proxy.rs:1583-1587` | the `ProxyCtx::tool_metadata` MCP-207 note | [`proxy/env.rs:398-402`](../../crates/cyrup-mcp/src/proxy/env.rs) |
| `proxy.rs:1602` | the `ProxyCtx::tool_metadata` field | [`proxy/env.rs:407`](../../crates/cyrup-mcp/src/proxy/env.rs) |
| `proxy.rs:1610` | `ProxyCtx::new` | [`proxy/env.rs:419`](../../crates/cyrup-mcp/src/proxy/env.rs) |
| `proxy.rs:1629` / `:1646` | the two approval wrappers | [`proxy/env.rs:437`](../../crates/cyrup-mcp/src/proxy/env.rs) / [`:454`](../../crates/cyrup-mcp/src/proxy/env.rs) |
| `proxy.rs:1943-1955` | `execute_status`' six-rung ladder | [`proxy/discovery.rs:55-68`](../../crates/cyrup-mcp/src/proxy/discovery.rs) |
| `proxy.rs:2244-2246` | `describe`'s `render_ts_shape` fork | [`proxy/discovery.rs:357-359`](../../crates/cyrup-mcp/src/proxy/discovery.rs) |
| `proxy.rs:2467` | `search`'s `format_schema(…, "    ")` | [`proxy/discovery.rs:577-580`](../../crates/cyrup-mcp/src/proxy/discovery.rs) |
| `proxy.rs:2881` | `notify_tool_metadata_updated("proxy-connect")` | [`proxy/auth.rs:379`](../../crates/cyrup-mcp/src/proxy/auth.rs) |
| `proxy.rs:3574` | the `Expected parameters:` suffix | [`proxy/call.rs:704`](../../crates/cyrup-mcp/src/proxy/call.rs) |
| `proxy.rs:391-407` | `ToolMetadata` | [`proxy/tool_metadata.rs:40-56`](../../crates/cyrup-mcp/src/proxy/tool_metadata.rs) |
| `proxy.rs:4862` / `:4932` / `:5094` | the `#[cfg(test)]` opening, `FakeEnv`, the `ProxyCtx::new` caller | [`proxy/mod.rs:87`](../../crates/cyrup-mcp/src/proxy/mod.rs), [`proxy/testsupport.rs:91`](../../crates/cyrup-mcp/src/proxy/testsupport.rs), [`proxy/testsupport.rs:253`](../../crates/cyrup-mcp/src/proxy/testsupport.rs) |
| `proxy.rs:5409` | the unrelated "are disabled" comment | [`proxy/results.rs:195`](../../crates/cyrup-mcp/src/proxy/results.rs) |

Two consequences beyond bookkeeping:

* **`ProxyEnv` has 32 methods, not 30.** Counted today over
  [`proxy/env.rs:245-397`](../../crates/cyrup-mcp/src/proxy/env.rs); `close` (`:258`) and
  `handle_url_elicitation_required` (`:295`) were missed by the previous count.
* **The deliverable module cannot be called `env.rs`.** A top-level `crates/cyrup-mcp/src/env.rs`
  would sit beside `crates/cyrup-mcp/src/proxy/env.rs`, and the workspace denies
  `rustdoc::broken_intra_doc_links` while `.cargo/config.toml` sets `--document-private-items`: an
  intra-doc `[`env`]` written in either file resolves ambiguously and fails the build. **The module is
  `crates/cyrup-mcp/src/live.rs`**, declared `pub mod live;` in
  [`lib.rs`](../../crates/cyrup-mcp/src/lib.rs) between `lifecycle` (`:139`) and `oauth` (`:140`).

### 2 · `ProxyCtx` carries a duplicate metadata map that nothing writes

`McpState::tool_metadata` ([`state.rs:89`](../../crates/cyrup-mcp/src/state.rs)) has **zero readers and
zero writers in the entire crate**: `grep -rn '\.tool_metadata' crates/cyrup-mcp/src` outside `proxy/`
returns nothing at all. Every proxy read goes through `ProxyCtx::with_metadata`
([`proxy/env.rs:425`](../../crates/cyrup-mcp/src/proxy/env.rs)) onto `ProxyCtx`'s own map, and the only
production writer of *that* is [`proxy/auth.rs:360`](../../crates/cyrup-mcp/src/proxy/auth.rs).

A production `ProxyEnv` makes this fatal rather than untidy. `RuntimeEnv` cannot reach the `ProxyCtx`
that holds it — the ctx owns the `Arc<dyn ProxyEnv>`, so a strong handle back would cycle — so
`RuntimeEnv::update_server_metadata` must write `McpState::tool_metadata` while `execute_status` keeps
reading `ProxyCtx::tool_metadata`. `mcp({action:"status"})` would report every connected server as
`not connected`, permanently. **Collapsing the two is part of D0, not a Wave 3 hand-off.**

### 3 · There are two independent metadata-cache readers

`crate::dirs` owns the writer side — `MetadataCache` / `ServerCacheEntry` with **non-`Option`**
`config_hash: String`, `resources: Vec<CachedResource>`, `cached_at: i64`
([`dirs.rs:560-605`](../../crates/cyrup-mcp/src/dirs.rs)) — plus `load_metadata_cache(path)`
([`dirs.rs:644`](../../crates/cyrup-mcp/src/dirs.rs)) and `save_metadata_cache(path, cache)`
([`dirs.rs:669`](../../crates/cyrup-mcp/src/dirs.rs)).

`crate::registration` owns a **separate, lenient reader** over the same bytes —
`MetadataCache` at [`registration.rs:612`](../../crates/cyrup-mcp/src/registration.rs),
`ServerCacheEntry` at `:626` with every field `Option`, and `load_metadata_cache(dirs)` at `:830`.
`dirs.rs:571-577` names the split itself and says *"the `/mcp` panel would show no cached data for
servers whose tools are registered and working. See the report's note on unifying the two cache
readers."*

This decides §9 and §10 step 6, and getting it backwards destroys usable cache — see
[§9](#the-body-in-order).

---

## Verification pass — every premise re-checked against the tree on 2026-08-27

Per [MCP_HIGH_SEVERITY_BACKLOG.md](MCP_HIGH_SEVERITY_BACKLOG.md)'s rule that a `missing` row is a lead
and not a verdict. **All the units are genuinely open.** These premises are not.

| premise (previous revision or STATUS) | what the tree says today |
|---|---|
| *"`ProxyCtx` already carries both approval bodies … and their docs name this implementor as the forwarder"* | **Half right, and the wrong half is load-bearing.** The bodies are free functions: `is_tool_call_approval_required(config, server, tool, Option<&IndexMap<String, Vec<ToolMetadata>>>)` at [`proxy/approval.rs:77`](../../crates/cyrup-mcp/src/proxy/approval.rs) and `ensure_tool_call_approved(state, server, tool, args, origin, cancel, &IndexMap<String, Vec<ToolMetadata>>)` at [`proxy/approval.rs:272`](../../crates/cyrup-mcp/src/proxy/approval.rs). The `ProxyCtx` methods are thin wrappers `RuntimeEnv` **cannot** call. It calls the free functions with the map from `McpState::tool_metadata`, which only typechecks after D0 |
| *"All three setters exist"* (MCP-027) | **There are five, and upstream installs five.** [`lifecycle.rs:712`](../../crates/cyrup-mcp/src/lifecycle.rs) `set_reconnect_callback`, `:719` `set_reconnect_failure_callback`, **`:727` `set_health_restored_callback`**, **`:735` `set_auth_required_callback`**, `:744` `set_idle_shutdown_callback`, against [`init.ts:418-451`](../../tmp/pi-mcp-adapter/init.ts). Omitting `health_restored` leaves a recovered server marked `failed` for the full 60 s; omitting `auth_required` leaves a `needs-auth` server marked `failed` |
| *"returns only after `start_health_checks` and a final `update_status_bar`"* | **The tail is a publish, not `updateStatusBar`** ([`init.ts:453-458`](../../tmp/pi-mcp-adapter/init.ts)): `owner.throwIfInactive()` → `startHealthChecks(runtimeSignal)` → `if (mcpFooterStatus === "off") ui?.setStatus("mcp", undefined)` → `publishMcpStatusSnapshot(state)`. `update_status_bar` there would additionally *write* the footer, which this path deliberately leaves to whatever §11 last set |
| *"the zero-enabled-server early return is the only other exit"* | **There is a fifth exit.** [`init.ts:301`](../../tmp/pi-mcp-adapter/init.ts) — `if (initialSignal?.aborted) return state;` immediately after the connect pass and **before** `owner.throwIfInactive()`; plus `if (initialSignal?.aborted) continue;` at [`init.ts:330`](../../tmp/pi-mcp-adapter/init.ts). A caller-cancelled init returns the built state, not an `Err` |
| *"`is_server_cache_valid(entry, &hash, CACHE_MAX_AGE_MS)`"* | **Wrong signature and wrong module.** It is `is_server_cache_valid(entry: &registration::ServerCacheEntry, definition: &ServerEntry, max_age_ms: f64)` at [`registration.rs:860`](../../crates/cyrup-mcp/src/registration.rs), hashing internally through `server_hasher()`. `try_compute_server_hash` ([`dirs.rs:1086`](../../crates/cyrup-mcp/src/dirs.rs)) is not on this path. The exact §10-step-6 guard already exists as `valid_entry(cache, name, definition)` at [`registration.rs:884`](../../crates/cyrup-mcp/src/registration.rs), private — make it `pub(crate)` and call it |
| *"`match connection.instructions() { Some(text) => insert, None => remove }`"* | **Misses the truthy test.** [`init.ts:495-499`](../../tmp/pi-mcp-adapter/init.ts) is `if (connection.instructions) … else delete` — an **empty string deletes**. [`proxy/auth.rs:369`](../../crates/cyrup-mcp/src/proxy/auth.rs) already spells this right with `.filter(\|text\| !text.is_empty())`; the new verb must match it |
| *"`let previous = …count`"* placed **after** `clear_failure` | **Always reads 0.** `clear_failure` removes the entry first. `ServerFailure::count` ([`state.rs:391`](../../crates/cyrup-mcp/src/state.rs)) is cyrup-only — upstream's `failureTracker` maps name → timestamp ([`init.ts:65`](../../tmp/pi-mcp-adapter/init.ts)) and has no count — so nothing upstream pins the order, but a count read after the clear makes the field a lie |
| *"`for name in &surface.tool_names { … added/updated }"* | **Two errors.** `surface.tool_names` already holds only tools that actually registered (`register_surface` pushes at [`registration.rs:2161`](../../crates/cyrup-mcp/src/registration.rs) *after* the `should_register_tool` gate at `:2158`), which is right — but it also holds `PROXY_TOOL_NAME` ([`:2185`](../../crates/cyrup-mcp/src/registration.rs)), which is never in `known_tools` and would be counted as `added` on every description change. And re-activation is **per tool, inside the registration branch, gated on the fallback-set removal returning true** ([`index.ts:223-228`](../../tmp/pi-mcp-adapter/index.ts)), not a blanket pass over the list |
| *"MCP-207 … whichever agent reaches it first writes it"* | **Not routable.** `build_tool_metadata` blocks `update_server_metadata` (MCP-028), `RuntimeEnv::connect`/`reconnect`, and MCP-021's cache twin. Without it this group does not compile, and a `Vec::new()` stub silently empties the tool surface on every reconnect. **MCP-207 is absorbed** — see [C0](#c0--mcp-207-buildtoolmetadata-and-reconstructtoolmetadata) |
| MCP-225 — *"the env variable is never actually read in production"* ([:739](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | **True as stated, and the unit itself is fully implemented.** `McpSettings::output_guard(env)` at [`config.rs:1246-1261`](../../crates/cyrup-mcp/src/config.rs), `env_kill_switch` (tri-state, unrecognised ⇒ `None`) at [`config.rs:1086`](../../crates/cyrup-mcp/src/config.rs), `positive_int` at [`config.rs:1062`](../../crates/cyrup-mcp/src/config.rs), `ResolvedOutputGuard` at [`config.rs:1096`](../../crates/cyrup-mcp/src/config.rs), the bridge `McpOutputGuardOptions::from_resolved` at [`renderers.rs:1000`](../../crates/cyrup-mcp/src/renderers.rs), `guard_mcp_output` at [`renderers.rs:1053`](../../crates/cyrup-mcp/src/renderers.rs). `grep -rn '\.output_guard(' src` returns **zero** call sites. A ~20-line wiring task |
| MCP-087 — *"port `parallelLimit`, argv scan, `toStringRecord`, `normalizeDirectToolInputSchema`"* ([:590](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | **Three of four are done**: `config_path_from_argv` at [`config.rs:1763`](../../crates/cyrup-mcp/src/config.rs), `to_string_record` at [`config.rs:328`](../../crates/cyrup-mcp/src/config.rs), `normalize_direct_tool_input_schema` at [`registration.rs:1540`](../../crates/cyrup-mcp/src/registration.rs). Only `parallel_limit` is absent, and **MCP-087 and MCP-130 are the same missing function** |
| MCP-015 — *"`runtime.rs:139` binds the combined token to `_runtime_signal` (unused)"* ([:508](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | Overturned, still. `runtime_signal` is bound at [`runtime.rs:149`](../../crates/cyrup-mcp/src/runtime.rs) and consumed at `:208`. The owned-`Arc` discipline is correct too: `ui` is built from `snapshot.services` behind the `has_ui` gate at [`runtime.rs:144-148`](../../crates/cyrup-mcp/src/runtime.rs). **MCP-015 reduces to MCP-016's two live closures** |
| MCP-020 — *"the `idleOverride` derivation exists…"* ([:513](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | Restated: every callee exists. `register_server` at [`lifecycle.rs:569`](../../crates/cyrup-mcp/src/lifecycle.rs) (disabled early-return `:575`, `idle_timeout.is_some()` gate `:583-589`), `mark_keep_alive` at `:615`, `mark_keep_alive_after_connect` with its three guards at `:634-647`, `LifecycleOverrides` at `:427`. The **only** gap is the caller |
| MCP-036 — *"the fingerprint diff … does not exist"* ([:530](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)) | Restated: the diff **is** built. `LateSink::should_register_tool` at [`registration.rs:2032-2034`](../../crates/cyrup-mcp/src/registration.rs) is literally `previous !== fingerprint`; `should_register_proxy` at `:2036`; `sync_tool_surface` at [`extension.rs:166-254`](../../crates/cyrup-mcp/src/extension.rs) drives it. Absent is the **removal half** |
| MCP-006's 35 missing `fenced!` methods | **Confirmed by name-set diff, exactly 35.** `HostServices` declares 66 over [`services.rs:190-704`](../../crates/cyrup-ext/src/host/services.rs); the `fenced!` block spans [`owner.rs:374-466`](../../crates/cyrup-mcp/src/owner.rs) (not `:465`) and lists 31. The previous revision's list is correct, name for name |

**Line drift.** The ledger's non-`proxy.rs` citations have drifted 150–400 lines (MCP-012's row cites
`extension.rs:527-530`; the code is at
[`extension.rs:697-699`](../../crates/cyrup-mcp/src/extension.rs) — MCP-045's cites
`extension.rs:535-559`, actual [`extension.rs:713-745`](../../crates/cyrup-mcp/src/extension.rs)).
Every line cited **in this file** was read on 2026-08-27.

---

## Where the work goes

| file | what changes |
|---|---|
| **`crates/cyrup-mcp/src/live.rs`** (new) | the `init.ts` live-state verbs + `RuntimeEnv`, the crate's one production `ProxyEnv` |
| [`lib.rs`](../../crates/cyrup-mcp/src/lib.rs) `:139` | `pub mod live;` |
| [`state.rs`](../../crates/cyrup-mcp/src/state.rs) `:89`, `:250-258`, `:362-382`, `:400-414` | D0's two discharges, MCP-030's panic fence, the §3.16 payload types, `McpState` field 22 |
| [`proxy/env.rs`](../../crates/cyrup-mcp/src/proxy/env.rs) `:398-421`, `:425-435`, `:468-470` | delete `ProxyCtx::tool_metadata`, project `McpState::tool_metadata` |
| [`registration.rs`](../../crates/cyrup-mcp/src/registration.rs) `:369-373`, `:458`, `:884`, `~:1100`, `:1823-1840` | `CandidateIndex`'s additional table, `build_tool_metadata`, `reconstruct_tool_metadata`, `reconstruct_prompt_metadata`, `valid_entry` visibility |
| [`runtime.rs`](../../crates/cyrup-mcp/src/runtime.rs) `:193-196`, `:219`, `:242-251`, `:253-291` | the handler factory, the §8-step-7 fix, `send_message`, the §9–§15 body |
| [`lifecycle.rs`](../../crates/cyrup-mcp/src/lifecycle.rs) `:362`, `:394-404`, `:773`, `:1020`, `:1130` | `PendingAuthCheck` widened to async, the real `MetadataFlush` |
| [`extension.rs`](../../crates/cyrup-mcp/src/extension.rs) `:113`, `:166-254`, `:425-439`, `:455`, `:697-699`, `:713-745` | `install_runtime_env`, the surface diff, the pre-warm, the `isError` override |
| [`owner.rs`](../../crates/cyrup-mcp/src/owner.rs) `:374-466` | the 35 missing `fenced!` arms |
| [`renderers.rs`](../../crates/cyrup-mcp/src/renderers.rs) `:756-880` | MCP-224's retry drain |

### Why `live.rs` and not `runtime.rs`

[`runtime.rs:27-35`](../../crates/cyrup-mcp/src/runtime.rs) declares that file has exactly two halves —
the runtime **build** and the **connection** — that "share no state", and that the connection half is
testable without an `McpState`, an owner or a reactor. These verbs are a third thing: mutation of a
*committed* `McpState`. Adding them there breaks the invariant that file's own tests depend on.

### The two methods this group does not implement, named rather than smuggled

`ProxyEnv::call_tool` ([`proxy/env.rs:274`](../../crates/cyrup-mcp/src/proxy/env.rs)) and
`read_resource` ([`:287`](../../crates/cyrup-mcp/src/proxy/env.rs)) are **MCP-164, Wave 1**.
`RuntimeEnv` declares them and returns a `ProxyCallError::Other` naming MCP-164 — the same discipline
as `ManagerSupervisor::unbound` at [`lifecycle.rs:277`](../../crates/cyrup-mcp/src/lifecycle.rs): a
loud, greppable failure, never a fabricated success. `format_schema` and `render_ts_shape` are
MCP-211 / MCP-091; see *Out-of-group blockers*.

---

## D0 — discharge the two forward declarations and collapse the duplicate map

Three edits, in this order, because everything downstream typechecks against them.

**1 · `ServerToolMetadata` → `Vec<ToolMetadata>`.** Replace
[`state.rs:362-371`](../../crates/cyrup-mcp/src/state.rs):

```rust
/// `tool-metadata.ts`'s per-server metadata is `ToolMetadata[]`, and MCP-021/MCP-028 are the writers
/// that need every field of it.
///
/// **Landed by this group.** `crate::state::ServerToolMetadata` stays a valid path for anything
/// already written against it and now names the real type.
pub use crate::proxy::ToolMetadata as ServerToolMetadata;
```

and field 4 at [`state.rs:87-89`](../../crates/cyrup-mcp/src/state.rs):

```rust
    /// 4 · Per-server tool metadata — `state.toolMetadata: Map<string, ToolMetadata[]>`,
    /// insertion-ordered because that order decides which server wins a fuzzy name match, which
    /// disabled server is named in an error, and the output order of the unsorted regex search.
    pub tool_metadata: Mutex<IndexMap<String, Vec<ToolMetadata>>>,
```

**2 · `PromptMetadata` → `PromptCommandSpec`.** Upstream's `PromptMetadata`
([`types.ts:584-591`](../../tmp/pi-mcp-adapter/types.ts): `serverName`, `originalName`, `commandName`,
`title?`, `description`, `arguments`) is **field-for-field**
`crate::registration::PromptCommandSpec`
([`registration.rs:1790-1797`](../../crates/cyrup-mcp/src/registration.rs)). Upstream proves the
identity itself: [`index.ts:280`](../../tmp/pi-mcp-adapter/index.ts) feeds
`state.promptMetadata.values()` and [`index.ts:283`](../../tmp/pi-mcp-adapter/index.ts) feeds
`resolveCachedPrompts(...)` into the same `registerPromptCommands(specs: Iterable<PromptMetadata>)`.
Replace [`state.rs:373-382`](../../crates/cyrup-mcp/src/state.rs):

```rust
/// `prompts.ts`'s per-prompt metadata: the prompt's name, its arguments and the slash command it
/// becomes.
///
/// **Landed by this group.** `types.ts:584-591`'s six fields are exactly
/// [`crate::registration::PromptCommandSpec`], which `resolve_cached_prompts` and
/// `register_prompt_commands` already produce and consume — the identity upstream itself relies on
/// (`index.ts:280` and `:283` feed both into one function).
pub use crate::registration::PromptCommandSpec as PromptMetadata;
```

Without this, `McpState::prompt_metadata` can only hold a bare name and MCP-021/MCP-028's prompt half
is unwritable.

**3 · Collapse `ProxyCtx::tool_metadata`.** Its own doc
([`proxy/env.rs:398-402`](../../crates/cyrup-mcp/src/proxy/env.rs)) says *"once MCP-207 lands, delete
this field and project `McpState::tool_metadata` instead — every read site below goes through
`ProxyCtx::with_metadata`, so the swap is one function."* Edit 1 removes the blocker:

```rust
pub struct ProxyCtx {
    /// The generation's runtime record: config, owner, UI handle, `serverInstructions` — and, since
    /// this group discharged `ServerToolMetadata`, `state.toolMetadata` itself.
    pub state: Arc<McpState>,
    /// The late-bound collaborators.
    pub env: Arc<dyn ProxyEnv>,
}

impl ProxyCtx {
    #[must_use]
    pub fn new(state: Arc<McpState>, env: Arc<dyn ProxyEnv>) -> Self {
        Self { state, env }
    }

    /// The one read path onto `state.toolMetadata`. A poisoned lock degrades to "no metadata",
    /// never to a panic (the crate denies `clippy::panic` and `init` must not fail).
    pub(crate) fn with_metadata<R>(
        &self,
        f: impl FnOnce(&IndexMap<String, Vec<ToolMetadata>>) -> R,
    ) -> R {
        match self.state.tool_metadata.lock() {
            Ok(guard) => f(&guard),
            Err(_) => f(&IndexMap::new()),
        }
    }

    /// The one write path onto `state.toolMetadata`.
    pub(crate) fn with_metadata_mut<R>(
        &self,
        f: impl FnOnce(&mut IndexMap<String, Vec<ToolMetadata>>) -> R,
    ) -> Option<R> {
        self.state.tool_metadata.lock().ok().map(|mut guard| f(&mut guard))
    }
}
```

Fourteen read sites and three write sites already go through those two functions, so nothing else in
`proxy/` changes. The one direct field access outside them is
[`proxy/testsupport.rs:255`](../../crates/cyrup-mcp/src/proxy/testsupport.rs), which becomes
`ctx.state.tool_metadata.lock()`.

---

## Sub-wave A — the §3.16 payload contract (MCP-078, MCP-137, MCP-138)

**Verified.** [`state.rs:400-414`](../../crates/cyrup-mcp/src/state.rs) defines `McpStatusSnapshot` as
four `Vec<String>` fields. `grep -rn 'McpServerRuntimeStatus\|SNAPSHOT_VERSION\|failed_ago_seconds'
src` returns zero. The only reader of `McpState::subscribe_status`
([`state.rs:241`](../../crates/cyrup-mcp/src/state.rs)) is
[`lifecycle.rs:2453`](../../crates/cyrup-mcp/src/lifecycle.rs), a test; the two production publishers —
[`runtime.rs:287`](../../crates/cyrup-mcp/src/runtime.rs) and
[`lifecycle.rs:1573`](../../crates/cyrup-mcp/src/lifecycle.rs) — both send `Default::default()`, which
under the new shape becomes **correct**: it is exactly `publishMcpStatusShutdown`'s literal all-zero
payload ([`mcp-status.ts:92-106`](../../tmp/pi-mcp-adapter/mcp-status.ts)).

Replace [`state.rs:400-414`](../../crates/cyrup-mcp/src/state.rs), matching
[`types.ts:18-44`](../../tmp/pi-mcp-adapter/types.ts) key for key:

```rust
/// `types.ts:18` `MCP_STATUS_SNAPSHOT_VERSION` (13c §3.16).
pub const MCP_STATUS_SNAPSHOT_VERSION: u32 = 1;

/// `types.ts:20-26` `McpServerRuntimeStatus` — a CLOSED six-variant union. The `kebab-case` rename is
/// what produces `needs-auth` / `not-connected`, which are the wire spellings.
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

/// `types.ts:28-35` — exactly six keys, two of them OMITTED when absent, never `null`.
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
    /// read both (`types.ts:34` is not optional).
    pub disabled: bool,
}

/// `types.ts:37-44`.
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
    /// `publishMcpStatusShutdown`'s literal all-zero payload (`mcp-status.ts:95-102`), `servers: []`.
    /// Hand-written rather than derived: `#[derive(Default)]` would give `version: 0`, and the two
    /// existing `publish_status(Default::default())` sites become CORRECT only with `version: 1`.
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

MCP-137's builder and MCP-138's publisher are in [sub-wave C](#the-snapshot-builder-mcp-137). Leave
`execute_status` ([`proxy/discovery.rs:34`](../../crates/cyrup-mcp/src/proxy/discovery.rs)) alone: its
ladder at `:55-68` is the *text* rendering with its own glyphs and `details` keys, and after D0 it
reads the same map the builder does, so there is no drift left to remove.

---

## Sub-wave B — the fence and the two swallow points

### MCP-006 — `createOwnedUi` as a fenced services handle · `partial`

**Verified by name-set diff.** The `fenced!` invocation spans
[`owner.rs:374-466`](../../crates/cyrup-mcp/src/owner.rs) and lists **31** methods; `HostServices`
([`services.rs:190-704`](../../crates/cyrup-ext/src/host/services.rs)) declares **66**. Add these 35
arms, each with the inert value its return type demands:

```rust
        // --- editor / theme / layout: pure paint ---------------------------------------------
        fn editor_text(&self) -> String => String::new();
        fn set_editor_text(&self, text: &str, is_paste: bool) => ();
        fn theme_list(&self) -> serde_json::Value => serde_json::Value::Null;
        fn theme_by_name(&self, name: &str) -> Option<serde_json::Value> => None;
        fn set_theme(&self, name: &str) -> Result<(), String> => Err(Self::inert_reason(&self.owner));
        fn tools_expanded(&self) -> bool => false;
        fn set_tools_expanded(&self, expanded: bool) => ();
        fn set_working_visible(&self, visible: bool) => ();
        fn set_working_indicator(&self, opts: Option<&serde_json::Value>) => ();
        fn set_hidden_thinking_label(&self, label: Option<&str>) => ();
        fn set_header(&self, content: &str) => ();
        fn set_footer(&self, content: &str) => ();
        fn set_title(&self, title: &str) => ();
        fn thinking_level(&self) -> Option<String> => None;

        // --- session / transcript ---------------------------------------------------------------
        fn entries(&self) -> serde_json::Value => serde_json::Value::Null;
        fn branch(&self) -> serde_json::Value => serde_json::Value::Null;
        fn tree(&self) -> serde_json::Value => serde_json::Value::Null;
        fn session_name(&self) -> Option<String> => None;
        fn set_session_name(&self, name: &str) => ();
        fn set_label(&self, entry_id: &str, label: Option<&str>) => ();
        fn append_entry(
            &self,
            custom_type: &str,
            data: &serde_json::Value
        ) -> Result<String, String> => Err(Self::inert_reason(&self.owner));
        fn has_pending_messages(&self) -> bool => false;
        fn system_prompt(&self) -> Option<String> => None;
        fn system_prompt_options(&self) -> Option<serde_json::Value> => None;
        fn scoped_models(&self) -> serde_json::Value => serde_json::Value::Null;

        // --- http: a stale generation must not issue a request -----------------------------------
        fn http_request(
            &self,
            req: &cyrup_ext::host::HttpRequest
        ) -> Result<cyrup_ext::host::HttpResponse, String> => Err(Self::inert_reason(&self.owner));
        fn http_request_stream(
            &self,
            req: &cyrup_ext::host::HttpRequest
        ) -> Result<cyrup_ext::host::HttpStreamResponse, String> => Err(Self::inert_reason(&self.owner));
        fn http_poll_stream_chunk(
            &self,
            handle: u32
        ) -> Result<Option<Vec<u8>>, String> => Err(Self::inert_reason(&self.owner));
        fn http_close_stream(&self, handle: u32) => ();

        // --- proc: a stale generation spawning a child is the exact leak the owner prevents -------
        fn proc_spawn(
            &self,
            spec: &cyrup_ext::host::ProcSpawnSpec
        ) -> Result<u32, String> => Err(Self::inert_reason(&self.owner));
        fn proc_write_stdin(&self, handle: u32, data: &[u8]) -> Result<u32, String> => Err(Self::inert_reason(&self.owner));
        fn proc_read_stdout(&self, handle: u32, max_bytes: u32) -> Result<Vec<u8>, String> => Err(Self::inert_reason(&self.owner));
        fn proc_read_stderr(&self, handle: u32, max_bytes: u32) -> Result<Vec<u8>, String> => Err(Self::inert_reason(&self.owner));
        fn proc_poll_exit(&self, handle: u32) -> Option<i32> => None;
        fn proc_kill(&self, handle: u32) -> Result<(), String> => Err(Self::inert_reason(&self.owner));
```

Take each signature verbatim from `services.rs`: the `fenced!` macro
([`owner.rs:351-366`](../../crates/cyrup-mcp/src/owner.rs)) expands
`fn $name(&$me $(, $arg: $ty)*)` into a real trait method, so a mismatched parameter type is a compile
error while a mismatched *name* silently falls through to the trait default. That fall-through is the
hole [`owner.rs:302-311`](../../crates/cyrup-mcp/src/owner.rs) documents and this unit closes.

Why it is in this group: every notification obligation here is swallowed until the fence is complete —
MCP-018's disabled notice, §13's startup summary, MCP-026's bootstrap notice, MCP-036's refresh
notice, MCP-038's `set_active_tools` ([`owner.rs:442`](../../crates/cyrup-mcp/src/owner.rs)). The crate
has **zero** production `HostServices::notify` call sites today (`grep -rn '\.notify(' src` returns
only doc comments at [`state.rs:123`](../../crates/cyrup-mcp/src/state.rs) and
[`owner.rs:293-294`](../../crates/cyrup-mcp/src/owner.rs)); this group adds the first five.

### MCP-030 — `notifyToolMetadataUpdated` must never let a hook break a connect · `partial`

**Verified.** `McpState::notify_tool_metadata_updated`
([`state.rs:250-258`](../../crates/cyrup-mcp/src/state.rs)) already clones the listener out from under
the lock and invokes it outside — that is right and must stay. Absent: the containment and the debug
line. `grep -rn catch_unwind src` returns nothing, and the workspace denies `clippy::panic`, so a
panicking listener is a genuine abort on the connect path.

```rust
    pub fn notify_tool_metadata_updated(&self, server: &str, reason: &str) {
        let listener = match self.on_tool_metadata_updated.lock() {
            Ok(slot) => slot.clone(),
            Err(_) => None,
        };
        let Some(listener) = listener else { return };
        // `init.ts:546-557`'s try/catch. A hook must never break a connect, and this crate denies
        // `clippy::panic` — a panicking listener would abort the process rather than merely fail.
        // `AssertUnwindSafe` is sound here: everything the closure can reach is behind `Mutex`/`Arc`,
        // and a poisoned lock already degrades to "no metadata" at every read site.
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            listener(server, reason);
        }));
        if let Err(payload) = caught {
            let message = payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panicked".to_string());
            tracing::debug!("MCP: metadata update hook failed for {server}: {message}");
        }
    }
```

The `.catch` on upstream's promise arm ([`init.ts:548-553`](../../tmp/pi-mcp-adapter/init.ts)) needs no
counterpart: cyrup's listener is `Arc<dyn Fn(&str, &str)>`
([`state.rs:73`](../../crates/cyrup-mcp/src/state.rs)) and the one installed by
`install_surface_sync` ([`extension.rs:433-438`](../../crates/cyrup-mcp/src/extension.rs)) is
synchronous.

### MCP-045 — the `tool_result` `isError` override · `partial`

**Verified.** `McpExtension::on_event`
([`extension.rs:713-745`](../../crates/cyrup-mcp/src/extension.rs)) matches `SessionStart`, `Input` and
`SessionShutdown` and falls through everything else at
[`extension.rs:727-728`](../../crates/cyrup-mcp/src/extension.rs), whose comment reads
`// MCP-045 fills the isError override.` `EventKind::ToolResult` **is** subscribed
([`registration.rs:119`](../../crates/cyrup-mcp/src/registration.rs)), so the event arrives and is
dropped.

Both shapes are exact: `HostEvent::ToolResult` carries `details: Option<Value>`
([`event.rs:282-295`](../../crates/cyrup-ext/src/event.rs)), and `EventPatch::ToolResult`
([`contract.rs:46-56`](../../crates/cyrup-ext/src/contract.rs)) has the four-`Option` shape whose
`apply_patch` ([`contract.rs:96-112`](../../crates/cyrup-ext/src/contract.rs)) sets `is_error` only
when `Some` — a 1:1 match for `error-signal.ts`'s field-by-field merge. Add before the `_` arm:

```rust
            // `error-signal.ts` `toolErrorOverride` — re-flag EXACTLY two `details.error` codes.
            // `auth_required` is deliberately NOT one of them: it is a prompt to run `/mcp-auth`,
            // not a tool failure, and flagging it would make the model retry instead of authenticate.
            HostEvent::ToolResult { details: Some(details), is_error: false, .. }
                if matches!(
                    details.get("error").and_then(serde_json::Value::as_str),
                    Some("tool_error" | "call_failed")
                ) =>
            {
                HookOutcome::Mutate(cyrup_ext::EventPatch::ToolResult {
                    content: None,
                    details: None,
                    is_error: Some(true),
                    usage: None,
                })
            }
```

`content` / `details` / `usage` stay `None` so `apply_patch` leaves them untouched — that is the whole
reason the patch is four `Option`s rather than a replacement.

---

## Sub-wave C — `live.rs`, the live-state verbs

```rust
//! `init.ts`'s live-state verbs (13a §13, §17, §18, §19; 13c §3.16) and the crate's one production
//! [`crate::proxy::ProxyEnv`].
//!
//! These are deliberately NOT in [`crate::runtime`]: that module's doc declares two halves — the
//! runtime BUILD and the CONNECTION — that "share no state", and the connection half is testable
//! without an `McpState`, an owner or a reactor. Mutating a *committed* `McpState` is a third thing.
//!
//! Named `live` rather than `env` because [`crate::proxy::env`] already exists and the workspace
//! denies `rustdoc::broken_intra_doc_links` under `--document-private-items`: an intra-doc `[`env`]`
//! in either module would resolve ambiguously and fail the build.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cyrup_core::CancelToken;

use crate::proxy::{ConnectionStatus, ToolMetadata};
use crate::state::{
    McpServerRuntimeStatus, McpServerStatusSnapshot, McpState, McpStatusSnapshot, ServerFailure,
    MCP_STATUS_SNAPSHOT_VERSION,
};

/// `init.ts:40` `FAILURE_BACKOFF_MS = 60 * 1000` (13a §13).
pub const FAILURE_BACKOFF_MS: u64 = 60_000;
/// `init.ts:41` `MAX_FAILURE_MESSAGE_CHARS = 8 * 1024`.
pub const MAX_FAILURE_MESSAGE_CHARS: usize = 8 * 1024;
/// `init.ts:284` and `:383`'s two `parallelLimit(…, 10, …)` call sites (MCP-022, MCP-026, MCP-130).
pub const STARTUP_CONNECT_CONCURRENCY: usize = 10;
```

### `parallel_limit` (MCP-087 / MCP-130) — one function, two units

**Verified.** `grep -rn 'parallel_limit\|buffer_unordered\|\.buffered(' src` returns nothing. The only
bounded fan-out in the crate is [`lifecycle.rs:962`](../../crates/cyrup-mcp/src/lifecycle.rs)'s
`for_each_concurrent`, which discards result order — the wrong shape here.

```rust
/// `utils.ts` `parallelLimit(items, limit, f)` — at most `limit` in flight, results **by original
/// index**.
///
/// `buffered` is the whole port: it keeps `limit` futures in flight and yields in input order, which
/// is exactly `parallelLimit`'s two properties. `buffer_unordered` / `for_each_concurrent` is WRONG
/// here — `init.ts:305` and `:327` walk `results` twice and `init.ts:382` filters against it by name,
/// all of which assume every element is present.
pub async fn parallel_limit<T, R, F, Fut>(items: Vec<T>, limit: usize, f: F) -> Vec<R>
where
    F: Fn(T) -> Fut,
    Fut: std::future::Future<Output = R>,
{
    use futures::StreamExt as _;
    // A `limit` of 0 would stall the stream; upstream's callers only ever pass 10.
    let limit = limit.max(1);
    futures::stream::iter(items.into_iter().map(f))
        .buffered(limit)
        .collect::<Vec<R>>()
        .await
}
```

### Failure tracking (MCP-024)

**Verified.** `grep -rn 'BACKOFF\|MAX_FAILURE_MESSAGE' src` returns only
[`lifecycle.rs:111`](../../crates/cyrup-mcp/src/lifecycle.rs), the unrelated keep-alive retry ceiling.
`McpState::failure_tracker` / `failure_messages`
([`state.rs:115`](../../crates/cyrup-mcp/src/state.rs), `:117`) and `ServerFailure`
([`state.rs:384-392`](../../crates/cyrup-mcp/src/state.rs)) exist and are never written outside tests.

```rust
/// `init.ts:53-60` `clearFailure(state, serverName)` — idempotent, and the first thing
/// [`record_failure`] calls.
pub fn clear_failure(state: &McpState, server: &str) {
    if let Ok(mut tracker) = state.failure_tracker.lock() {
        tracker.shift_remove(server);
    }
    if let Ok(mut messages) = state.failure_messages.lock() {
        messages.shift_remove(server);
    }
}

/// `init.ts:62-81` `recordFailure(state, serverName, message)`.
///
/// **Two deliberate deviations from upstream's bookkeeping, both observably identical.** (1) There is
/// no timer map. Upstream's `clearTimeout` (`init.ts:58`) exists so a superseded timer cannot clear a
/// newer failure; the `last_failure == failed_at` check below already guarantees that
/// (`init.ts:72`), so a 23rd `McpState` field would buy nothing. (2) `timer.unref?.()`
/// (`init.ts:79`) needs no analog — a tokio task does not hold the process open — but the select on
/// the owner token is REQUIRED, not optional: without it a clean shutdown waits out the full 60 s.
pub fn record_failure(state: &Arc<McpState>, server: &str, message: &str) {
    // Read the streak BEFORE the clear wipes it. `ServerFailure::count` is cyrup-only — upstream's
    // `failureTracker` maps name -> timestamp (`init.ts:65`) and has no count — so nothing upstream
    // pins this ordering, but a count read after `clear_failure` is always 0.
    let previous = state
        .failure_tracker
        .lock()
        .ok()
        .and_then(|tracker| tracker.get(server).map(|failure| failure.count))
        .unwrap_or(0);
    clear_failure(state, server);

    let failed_at = Instant::now();
    if let Ok(mut tracker) = state.failure_tracker.lock() {
        tracker.insert(
            server.to_string(),
            ServerFailure { last_failure: failed_at, count: previous.saturating_add(1) },
        );
    }
    if let Ok(mut messages) = state.failure_messages.lock() {
        messages.insert(server.to_string(), truncate_failure_message(message));
    }

    // `WeakMap<McpExtensionState, …>` (`init.ts:42`): the task must not keep the state alive.
    let weak = Arc::downgrade(state);
    let owner = state.owner.token();
    let name = server.to_string();
    tokio::spawn(async move {
        tokio::select! {
            biased;
            () = owner.cancelled() => {}
            () = tokio::time::sleep(Duration::from_millis(FAILURE_BACKOFF_MS)) => {
                let Some(state) = weak.upgrade() else { return };
                // `if (!state.owner.isActive()) { … return; }` (`init.ts:68`).
                if !state.owner.is_active() {
                    return;
                }
                // `failureTracker.get(serverName) === failedAt` (`init.ts:72`): a re-insert must NOT
                // be cleared by the older timer.
                let still_ours = state.failure_tracker.lock().is_ok_and(|tracker| {
                    tracker.get(&name).is_some_and(|failure| failure.last_failure == failed_at)
                });
                if still_ours {
                    clear_failure(&state, &name);
                    // `publishMcpStatusSnapshot(state)` (`init.ts:75`) — the SNAPSHOT only, not the
                    // footer: this fires on a timer with no user action behind it.
                    state.publish_status(create_mcp_status_snapshot(&state));
                }
            }
        }
    });
}

/// `message.slice(0, MAX_FAILURE_MESSAGE_CHARS)` (`init.ts:66`), on a char boundary.
///
/// Upstream slices UTF-16 code units and is safe only because the string is ASCII in practice; a
/// hostile server's stderr is not. The cap is bytes here and the cut walks back to the nearest
/// boundary — at most three bytes shorter than upstream's for the same input.
fn truncate_failure_message(message: &str) -> String {
    if message.len() <= MAX_FAILURE_MESSAGE_CHARS {
        return message.to_string();
    }
    let mut cut = MAX_FAILURE_MESSAGE_CHARS;
    while cut > 0 && !message.is_char_boundary(cut) {
        cut -= 1;
    }
    message.get(..cut).unwrap_or_default().to_string()
}

/// `mcp-status.ts:15-21` `getActiveFailureAgeSeconds(state, name)` — `None` outside the 60 s window.
///
/// Upstream's falsy-`failedAt` arm (`init.ts:606`, an epoch-`0` timestamp counting as absent) has no
/// analog: the record holds an `Instant`, which has no zero value, and absence is `None`.
#[must_use]
pub fn failure_age_seconds(state: &McpState, server: &str) -> Option<u64> {
    let tracker = state.failure_tracker.lock().ok()?;
    let age = tracker.get(server)?.last_failure.elapsed();
    // `if (ageMs > FAILURE_BACKOFF_MS) return undefined` — strictly greater, so 60.000 s is inside.
    (age <= Duration::from_millis(FAILURE_BACKOFF_MS)).then(|| age.as_secs_f64().round() as u64)
}
```

Land `failure_message` beside it — [`init.ts:612-615`](../../tmp/pi-mcp-adapter/init.ts) is the same
window gate over `failure_messages`, and `/mcp` plus MCP-137's `failed` rung both want it.

### The snapshot builder (MCP-137)

**Verified.** `create_mcp_status_snapshot` does not exist. The six-way ladder exists once, as *text*,
in `execute_status` ([`proxy/discovery.rs:55-68`](../../crates/cyrup-mcp/src/proxy/discovery.rs)) —
after D0 both read the same map, so this is a second *shape*, not a second source of truth.

Ordering is load-bearing: `McpConfig::mcp_servers` is an `IndexMap`
([`config.rs:638`](../../crates/cyrup-mcp/src/config.rs)), so config-file order is already preserved.
A `BTreeMap` anywhere on this path lists servers alphabetically in `/mcp` and in the footer.

```rust
/// `mcp-status.ts:24-77` `createMcpStatusSnapshot(state)` (13c §3.16). Never connects, never queries.
#[must_use]
pub fn create_mcp_status_snapshot(state: &McpState) -> McpStatusSnapshot {
    let mut servers = Vec::with_capacity(state.config.mcp_servers.len());
    let (mut total_tools, mut total_resources) = (0usize, 0usize);
    let (mut connected_count, mut disabled_count) = (0usize, 0usize);

    for (name, definition) in &state.config.mcp_servers {
        // `definition?.disabled === true` — only the literal boolean (`config.rs:906`).
        let disabled = definition.is_disabled();
        let connection = (!disabled).then(|| state.manager.get_connection(name)).flatten();
        let status_of = connection.as_ref().map(|c| c.status());
        let metadata_len = (!disabled)
            .then(|| state.tool_metadata.lock().ok().and_then(|m| m.get(name).map(Vec::len)))
            .flatten();

        // `metadata?.length ?? (connection?.status === "connected" ? connection.tools.length : 0)`
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

        // `mcp-status.ts:42-55` — first match wins, and the two counters increment INSIDE the ladder.
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

        // `totalTools += disabled ? 0 : toolCount` and
        // `if (!disabled && resourceCount !== undefined) totalResources += resourceCount`.
        if !disabled {
            total_tools += tool_count;
            total_resources += resource_count.unwrap_or(0);
        }
        servers.push(McpServerStatusSnapshot {
            name: name.clone(),
            status,
            tool_count,
            resource_count,
            // `...(status === "failed" && failedAgoSeconds !== undefined ? {failedAgoSeconds} : {})`
            failed_ago_seconds: (status == McpServerRuntimeStatus::Failed)
                .then_some(failed_ago)
                .flatten(),
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
```

`ServerConnection::tools()` / `resources()`
([`server_manager.rs:935`](../../crates/cyrup-mcp/src/server_manager.rs), `:949`) clone their `Vec` out
from under a `Mutex`. That is a cold arm — it fires only when the server has *no* metadata entry — and
there is no `len()`-only accessor to reach for.

### `update_status_bar` (MCP-032)

**Verified.** The pure half is done: `format_mcp_status` at
[`ui.rs:4641`](../../crates/cyrup-mcp/src/ui.rs), `FooterCounts` at
[`ui.rs:4614`](../../crates/cyrup-mcp/src/ui.rs) with `from_config` at `:4629`, `footer_status_text` at
[`ui.rs:4663-4688`](../../crates/cyrup-mcp/src/ui.rs) covering
[`init.ts:572-601`](../../tmp/pi-mcp-adapter/init.ts) steps 3-10 including the `configured == 0` and
`off` clears. Its only caller today is [`ui.rs:5956`](../../crates/cyrup-mcp/src/ui.rs), a test.

```rust
/// `init.ts:568-602` `updateStatusBar(state)` (13a §18).
///
/// Step 1 publishes ALWAYS, before the `!ui` return: a headless run still feeds the watch, which is
/// what `/mcp` and the proxy tool's `status` mode read. Step 11's `ui.theme.fg("accent", …)` has no
/// analog — `HostServices` exposes a theme *name* and no `fg(role, text)` — and collapses to
/// upstream's own `ui.theme ? … : formattedStatus` no-theme arm.
pub fn update_status_bar(state: &McpState) {
    let snapshot = create_mcp_status_snapshot(state);
    // `connectedCount` (`init.ts:579-582`) is "connected AND the definition exists AND is not
    // disabled" — exactly what the snapshot's ladder just counted, over `config.mcpServers` instead
    // of over the connection map. Same set, one pass.
    let connected = snapshot.connected_count;
    state.publish_status(snapshot);

    let Some(ui) = state.ui.as_ref() else { return };
    let counts = crate::ui::FooterCounts::from_config(&state.config, connected);
    let text = crate::ui::footer_status_text(&state.config, counts);
    cyrup_ext::HostServices::set_status(ui.as_ref(), "mcp", text.as_deref());
}
```

### `update_server_metadata` (MCP-028) and `update_metadata_cache`

**Verified.** Neither function exists; `ProxyEnv::update_server_metadata`
([`proxy/env.rs:307`](../../crates/cyrup-mcp/src/proxy/env.rs)) and `update_metadata_cache`
([`:309`](../../crates/cyrup-mcp/src/proxy/env.rs)) are the only declarations, and `FakeEnv`'s bodies
([`proxy/testsupport.rs:151`](../../crates/cyrup-mcp/src/proxy/testsupport.rs)) are `{}`.

```rust
/// `init.ts:471-500` `updateServerMetadata(state, serverName)` (13a §17).
pub fn update_server_metadata(state: &McpState, server: &str) {
    let Some(connection) = state.manager.get_connection(server) else { return };
    if connection.status() != ConnectionStatus::Connected {
        return;
    }
    let Some(definition) = state.config.mcp_servers.get(server) else { return };

    // `init.ts:477-484` — a server disabled WHILE connected disappears from the surface on the next
    // refresh instead of lingering. All five maps, then return.
    if definition.is_disabled() {
        forget_server_metadata(state, server);
        return;
    }

    // The collision universe here is `state.toolMetadata` — every server's CURRENT names — not the
    // startup snapshot (`init.ts:488` passes `state.toolMetadata`; `init.ts:340` passes
    // `startupKnownMetadata`). Getting this wrong makes prefixed names order-dependent.
    let universe = state.tool_metadata.lock().map(|guard| guard.clone()).unwrap_or_default();
    let built = crate::registration::build_tool_metadata(
        &connection.tools(),
        &connection.resources(),
        definition,
        server,
        state.config.tool_prefix(),
        Some(&state.config.mcp_servers),
        Some(&universe),
        false,
    );

    if let Ok(mut map) = state.tool_metadata.lock() {
        map.insert(server.to_string(), built.metadata);
    }
    if let Ok(mut counts) = state.resource_counts.lock() {
        counts.insert(server.to_string(), connection.resources().len());
    }
    // `init.ts:491-494` — only from a LIVE list, and only when discovery did not fail.
    if !connection.prompt_discovery_failed() {
        let prompts = crate::registration::reconstruct_prompt_metadata(
            server,
            &connection.prompts(),
            state.config.tool_prefix(),
            definition,
        );
        if let Ok(mut map) = state.prompt_metadata.lock() {
            map.insert(server.to_string(), prompts);
        }
        if let Ok(mut live) = state.prompt_metadata_live.lock() {
            live.insert(server.to_string());
        }
    }
    // `if (connection.instructions) … else delete` (`init.ts:495-499`) — a TRUTHY test, so an EMPTY
    // string DELETES. `proxy/auth.rs:369` already spells this correctly.
    if let Ok(mut map) = state.server_instructions.lock() {
        match connection.instructions().filter(|text| !text.is_empty()) {
            Some(text) => {
                map.insert(server.to_string(), text.to_string());
            }
            None => {
                map.shift_remove(server);
            }
        }
    }
}

/// The five-map delete `init.ts:478-482` performs, shared with `unregisterServer`.
fn forget_server_metadata(state: &McpState, server: &str) {
    if let Ok(mut m) = state.tool_metadata.lock() { m.shift_remove(server); }
    if let Ok(mut m) = state.resource_counts.lock() { m.shift_remove(server); }
    if let Ok(mut m) = state.prompt_metadata.lock() { m.shift_remove(server); }
    if let Ok(mut m) = state.prompt_metadata_live.lock() { m.remove(server); }
    if let Ok(mut m) = state.server_instructions.lock() { m.shift_remove(server); }
}

/// `init.ts:502-543` `updateMetadataCache(state, serverName, options)`'s options.
#[derive(Debug, Clone, Copy)]
pub struct MetadataCacheOptions {
    /// `options.preserveEmptyResources !== false` (`init.ts:528`). The default is "preserve"; the
    /// list-changed listener passes `false` because THAT empty `resources/list` is authoritative.
    pub preserve_empty_resources: bool,
}

impl MetadataCacheOptions {
    /// Upstream's `{}` — the absent key reads as `!== false`, i.e. preserve. Spelled as a named
    /// constructor rather than a `Default` impl so no call site can silently mean the other thing.
    #[must_use]
    pub fn preserving() -> Self {
        Self { preserve_empty_resources: true }
    }
}
```

`update_metadata_cache`'s body is a direct port of
[`init.ts:502-543`](../../tmp/pi-mcp-adapter/init.ts) over primitives that all exist on the **writer**
side: `McpDirs::metadata_cache` ([`dirs.rs:178`](../../crates/cyrup-mcp/src/dirs.rs)),
`dirs::load_metadata_cache` ([`dirs.rs:644`](../../crates/cyrup-mcp/src/dirs.rs)),
`dirs::save_metadata_cache` ([`dirs.rs:669`](../../crates/cyrup-mcp/src/dirs.rs), which merges — the
one-entry `saveMetadataCache({version:1, servers:{[name]: entry}})` at `init.ts:542` is a merge
upstream too), `dirs::ServerCacheEntry` ([`dirs.rs:560-605`](../../crates/cyrup-mcp/src/dirs.rs)) and
`try_compute_server_hash` ([`dirs.rs:1086`](../../crates/cyrup-mcp/src/dirs.rs)) for `configHash`.
Reproduce all three conditional keys: `prompts` falls back to the existing entry's when
`promptDiscoveryFailed` **and** the hash matches (`init.ts:519-521`), `resources` is `[]` under
`exposeResources: false` (`:518`), and `instructions` is omitted rather than nulled (`:538`).
`RuntimeEnv` therefore holds an `McpDirs`, cloned from `McpExtension::dirs`
([`extension.rs:77`](../../crates/cyrup-mcp/src/extension.rs)).

### `lazy_connect` (MCP-033)

**Verified.** `ProxyEnv::lazy_connect` ([`proxy/env.rs:256`](../../crates/cyrup-mcp/src/proxy/env.rs))
is the only declaration; the four production callers are all in
[`proxy/call.rs`](../../crates/cyrup-mcp/src/proxy/call.rs) — `:291`, `:312`, `:409`, `:425`.

The seam **flattens upstream's throw into a `bool`**, which is not a defect but does move the
load-bearing detail. [`init.ts:652-655`](../../tmp/pi-mcp-adapter/init.ts) is
`if (isAbortError(error, ownedSignal)) { throwIfAborted(ownedSignal); }`, which **falls through** to
`recordFailure` when the rethrow does not fire. So the entire observable content of §19 step 8 is: *an
abort on an actually-cancelled signal must not `record_failure`; a stray abort error on a live signal
must.* Collapsing the two lets a server-side cancellation poison the next 60 seconds of that server's
availability.

```rust
/// `init.ts:617-662` `lazyConnect(state, serverName, signal)` (13a §19) — `true` iff the server
/// ended `connected`.
pub async fn lazy_connect(state: &Arc<McpState>, server: &str, cancel: &CancelToken) -> bool {
    // 1 — `combineAbortSignals(state.owner?.signal, signal)` then `throwIfAborted`.
    let owned = crate::abort::combine(&state.owner.token(), Some(cancel));
    if owned.is_cancelled() {
        return false;
    }
    // 2-3 — needs-auth is checked FIRST (`init.ts:621`), connected second (`:624`).
    if let Some(connection) = state.manager.get_connection(server) {
        match connection.status() {
            ConnectionStatus::NeedsAuth => return false,
            ConnectionStatus::Connected => {
                update_server_metadata(state, server);
                state.lifecycle.mark_keep_alive_after_connect(server);
                return true;
            }
            ConnectionStatus::Closed => {}
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
            update_metadata_cache(state, server, MetadataCacheOptions::preserving());
            state.notify_tool_metadata_updated(server, "lazy-connect");
            state.lifecycle.mark_keep_alive_after_connect(server);
            update_status_bar(state);
            true
        }
        Err(error) => {
            // `if (isAbortError(...)) throwIfAborted(ownedSignal)` — the rethrow arm. No failure is
            // recorded for a genuine cancellation; a stray abort on a LIVE signal falls through and
            // IS recorded, exactly as upstream's non-throwing `throwIfAborted` does.
            if crate::abort::is_abort_error(&error, Some(&owned)) && owned.is_cancelled() {
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

`is_abort_error` ([`abort.rs:140`](../../crates/cyrup-mcp/src/abort.rs)) answers `true` when the token
is cancelled **or** the error is `McpError::Aborted`, so the `&& owned.is_cancelled()` conjunct is what
separates the two arms. Do not simplify it away.

### `rehydrate_from_cache` (MCP-021)

**Verified.** No rehydration into `McpState` exists. `promptMetadataLive` is the load-bearing negative:
a cache-rehydrated prompt list is deliberately **not** added to `McpState::prompt_metadata_live`
([`state.rs:95-97`](../../crates/cyrup-mcp/src/state.rs)), which is what flags it non-live.

```rust
/// `init.ts:256-269` — populate this generation's maps from a hash-valid cache entry.
///
/// Takes `crate::registration`'s LENIENT `ServerCacheEntry` (registration.rs:626), not
/// `crate::dirs`'s: that is the reader half, and it is the one `resolve_direct_tools` and
/// `resolve_cached_prompts` already registered this session's surface from.
///
/// Four writes, three conditional, and one deliberate omission: `promptMetadataLive` is NOT touched.
/// That set is the "came from a live `prompts/list`" flag, and adding a cache-rehydrated server to it
/// would make `update_metadata_cache` treat stale prompts as authoritative.
pub fn rehydrate_from_cache(
    state: &McpState,
    server: &str,
    definition: &crate::config::ServerEntry,
    entry: &crate::registration::ServerCacheEntry,
    cache: &crate::registration::MetadataCache,
) {
    let prefix = state.config.tool_prefix();
    let metadata = crate::registration::reconstruct_tool_metadata(
        server,
        entry,
        prefix,
        definition,
        Some(&state.config.mcp_servers),
        Some(cache),
    );
    if let Ok(mut map) = state.tool_metadata.lock() {
        map.insert(server.to_string(), metadata);
    }
    // `if (Array.isArray(cachedEntry.resources))` — an ABSENT list writes no count at all, which is
    // not the same as writing 0: 0 means "asked, got none". `entry.resources` is the raw `Option`,
    // NOT the `entry.resources()` accessor, which flattens absent into `&[]`.
    if let Some(resources) = entry.resources.as_ref()
        && let Ok(mut counts) = state.resource_counts.lock()
    {
        counts.insert(server.to_string(), resources.len());
    }
    // `if (cachedEntry.prompts?.length)` — NON-EMPTY, not merely present.
    if !entry.prompts().is_empty()
        && let Ok(mut map) = state.prompt_metadata.lock()
    {
        map.insert(
            server.to_string(),
            crate::registration::reconstruct_prompt_metadata(
                server,
                entry.prompts(),
                prefix,
                definition,
            ),
        );
    }
    // `if (cachedEntry.instructions)` — truthy, so an empty string writes nothing.
    if let Some(text) = entry.instructions.as_deref().filter(|t| !t.is_empty())
        && let Ok(mut map) = state.server_instructions.lock()
    {
        map.insert(server.to_string(), text.to_string());
    }
}
```

The cache-validity guard is **not** re-derived here: `valid_entry(cache, name, definition)`
([`registration.rs:884`](../../crates/cyrup-mcp/src/registration.rs)) is exactly
`cachedEntry && isServerCacheValid(cachedEntry, definition)` and is already the guard
`resolve_direct_tools` opens with at
[`registration.rs:1130`](../../crates/cyrup-mcp/src/registration.rs). Change its `fn` to
`pub(crate) fn` and call it. Do **not** reach for `try_compute_server_hash`:
`is_server_cache_valid` ([`registration.rs:860`](../../crates/cyrup-mcp/src/registration.rs)) hashes
internally through `server_hasher()`, and a second hash on this path is the reader/writer drift that
seam exists to prevent.

---

## C0 — MCP-207: `build_tool_metadata` and `reconstruct_tool_metadata`

**This is not a Wave 3 hand-off; it is inside this group's blocking set.** `update_server_metadata`
(MCP-028) is called by `lazy_connect`, by the reconnect callback, by the list-changed listener, by
`RuntimeEnv::update_server_metadata` and by §14's bootstrap pass — and it cannot be written without
`build_tool_metadata`. `RuntimeEnv::connect` / `reconnect` need it too, to fill
`ConnectOutcome::metadata` ([`proxy/env.rs:50`](../../crates/cyrup-mcp/src/proxy/env.rs)). A
`Vec::new()` stub would silently empty the tool surface on every reconnect — the exact failure class
MCP-037a recorded.

Both functions go in [`registration.rs`](../../crates/cyrup-mcp/src/registration.rs), beside
`resolve_direct_tools`, because that file already owns every helper they need: `resolve_tool_prefix`,
`format_tool_name`, `is_ui_tool_visible_to_model`, `resource_base_tool_name`
([`:511`](../../crates/cyrup-mcp/src/registration.rs)), `is_tool_allowed`
([`:458`](../../crates/cyrup-mcp/src/registration.rs)), `has_tool_filters`
([`:1062`](../../crates/cyrup-mcp/src/registration.rs)), `build_candidate_index`
([`:1071`](../../crates/cyrup-mcp/src/registration.rs)) and `valid_entry`
([`:884`](../../crates/cyrup-mcp/src/registration.rs)). Putting them in `live.rs` would fork the
name-resolution walk, which is the drift class this crate keeps finding.

### The one primitive that must grow: `CandidateIndex`

[`registration.rs:366-373`](../../crates/cyrup-mcp/src/registration.rs) says it outright:
*"`additionalCurrentCandidatesByToolName` is deliberately absent: it exists only for
`tool-metadata.ts`'s speculative arms (MCP-207)."* That is now. Per
[`types.ts:816-826`](../../tmp/pi-mcp-adapter/types.ts) and
[`types.ts:846-875`](../../tmp/pi-mcp-adapter/types.ts):

```rust
pub struct CandidateIndex {
    all_current: HashSet<String>,
    /// `additionalCurrentCandidatesByToolName` (`types.ts:818`) — `tool-metadata.ts`'s speculative
    /// arm, and the ONLY consumer. `direct-tools.ts` never supplies it, so it stays empty for every
    /// index `build_candidate_index` mints and no existing behaviour changes.
    additional_by_tool: HashMap<String, HashSet<String>>,
    matcher: HashMap<String, Option<Regex>>,
    matching_count: HashMap<String, usize>,
}
```

`has_other_current_match` ([`:384`](../../crates/cyrup-mcp/src/registration.rs)) gains a
`tool_name: &str` parameter, and its membership test becomes `hasCandidate`
([`types.ts:853-854`](../../tmp/pi-mcp-adapter/types.ts)):
`self.all_current.contains(c) || self.additional_by_tool.get(tool_name).is_some_and(|s| s.contains(c))`.
`is_tool_allowed` ([`:458`](../../crates/cyrup-mcp/src/registration.rs)) already takes `tool_name` as
its first argument, so it forwards it and every existing call site is unchanged.

### `build_tool_metadata`

Port [`tool-metadata.ts:9-140`](../../tmp/pi-mcp-adapter/tool-metadata.ts) whole, in upstream's
parameter order:

```rust
/// `tool-metadata.ts:9` `buildToolMetadata(...)` (MCP-207).
///
/// `known_metadata` is the collision universe: `state.toolMetadata` from `updateServerMetadata`
/// (`init.ts:488`), or the startup snapshot from §12 pass two (`init.ts:340`, which also passes
/// `include_missing_configured_candidates = true`). The two are NOT interchangeable — the startup
/// snapshot exists precisely so pass two sees every server that connected, including ones later in
/// the map that `state.toolMetadata` does not carry yet.
#[must_use]
pub fn build_tool_metadata(
    tools: &[rmcp::model::Tool],
    resources: &[rmcp::model::Resource],
    definition: &ServerEntry,
    server_name: &str,
    prefix: ToolPrefix,
    configured_servers: Option<&IndexMap<String, ServerEntry>>,
    known_metadata: Option<&IndexMap<String, Vec<ToolMetadata>>>,
    include_missing_configured_candidates: bool,
) -> BuiltToolMetadata;

/// `{ metadata, failedTools }` (`tool-metadata.ts:18`).
pub struct BuiltToolMetadata {
    pub metadata: Vec<ToolMetadata>,
    /// Names whose `_meta.ui.resourceUri` extraction threw (`tool-metadata.ts:100-104`), plus the
    /// literal `"(unnamed)"` for a nameless tool (`:81`). §12 turns a non-empty list into
    /// `MCP: {server} - {n} tools skipped` (`init.ts:356-361`).
    pub failed_tools: Vec<String>,
}
```

Six details that *are* the port, each of which a paraphrase loses:

1. **A nameless tool pushes `"(unnamed)"` into `failed_tools` and is skipped**
   ([`:80-83`](../../tmp/pi-mcp-adapter/tool-metadata.ts)). `resolve_direct_tools`
   ([`registration.rs:1146-1152`](../../crates/cyrup-mcp/src/registration.rs)) silently `continue`s for
   the same input — correct *there*, since it has no `failedTools` channel, and wrong here.
2. **Gate order:** `isToolAllowed` → `formatToolName` → `seenNames` → **then** `uiVisibility`
   ([`:84-97`](../../tmp/pi-mcp-adapter/tool-metadata.ts)). The visibility test is *after* the name
   reservation, so a hidden tool still consumes its name. Reversing it changes which server wins a
   collision.
3. **The resource arm applies no visibility filter and no `BUILTIN_NAMES` check**
   ([`:117-136`](../../tmp/pi-mcp-adapter/tool-metadata.ts)); the builtin check belongs to
   `resolve_direct_tools` alone.
4. `description: tool.description ?? ""` for tools;
   ``resource.description ?? `Read resource: ${uri}` `` for resources.
5. `input_schema` is carried through **unnormalised** — `normalize_direct_tool_input_schema`
   ([`registration.rs:1540`](../../crates/cyrup-mcp/src/registration.rs)) belongs to registration, not
   to metadata.
6. **The selector-candidate index is built only when the definition actually has filters**
   ([`:26`](../../tmp/pi-mcp-adapter/tool-metadata.ts): `hasToolFilters && configuredServers`), and its
   three arms ([`:51-75`](../../tmp/pi-mcp-adapter/tool-metadata.ts)) differ: a server with known
   metadata contributes its resolved names plus its originals' candidates; a server *without*
   contributes into `additional_by_tool` only when `known_metadata` is absent or
   `include_missing_configured_candidates` is set, and the `-`→`_` normalised spellings only under the
   latter.

### `reconstruct_tool_metadata`

[`metadata-cache.ts:185-269`](../../tmp/pi-mcp-adapter/metadata-cache.ts) — the same walk over a
`registration::ServerCacheEntry` instead of a live connection, and it is **not** `resolve_direct_tools`:
that one additionally applies the `directTools` selector (`ToolFilter`,
[`registration.rs:1133-1137`](../../crates/cyrup-mcp/src/registration.rs)) and the `BUILTIN_NAMES`
collision check ([`:1170`](../../crates/cyrup-mcp/src/registration.rs)), neither of which belongs to
`state.toolMetadata`. Differences from `build_tool_metadata`: no `failedTools`; visibility is checked
**before** `formatToolName` ([`:221-232`](../../tmp/pi-mcp-adapter/metadata-cache.ts)); resources are
skipped when `!resource.name || !resource.uri` ([`:247`](../../tmp/pi-mcp-adapter/metadata-cache.ts));
and the index's other-server arm reads each server's own cache entry through `isServerCacheValid`
([`:203`](../../tmp/pi-mcp-adapter/metadata-cache.ts)) rather than `knownMetadata`.

### `reconstruct_prompt_metadata`

[`metadata-cache.ts:318-342`](../../tmp/pi-mcp-adapter/metadata-cache.ts). After D0 its return type is
`Vec<crate::state::PromptMetadata>` = `Vec<PromptCommandSpec>`, and `resolve_cached_prompts`
([`registration.rs:1803`](../../crates/cyrup-mcp/src/registration.rs)) already carries the body inline
at `:1823-1840` — lift that inner loop into the shared function and have `resolve_cached_prompts` call
it, so the cache path and the live path mint command names one way.

---

## Sub-wave D — `initialize_mcp` §9–§15

All of these are edits to one function body,
[`runtime.rs:125-292`](../../crates/cyrup-mcp/src/runtime.rs). They cannot be split.

### The §8-step-7 fix, first (MCP-020's other half)

[`runtime.rs:219`](../../crates/cyrup-mcp/src/runtime.rs) constructs the lifecycle manager with a
hardcoded `Arc::new(|_| false)` for `hasPendingAuth`, so **the idle sweep and the health check will
reap a server in the middle of an OAuth login** — the consumers are
[`lifecycle.rs:1020`](../../crates/cyrup-mcp/src/lifecycle.rs) and `:1130`, through the accessor at
`:773`. The real predicate is `oauth::has_pending_auth`
([`oauth.rs:2044-2059`](../../crates/cyrup-mcp/src/oauth.rs)), `async` because it reads a
`tokio::sync::Mutex`.

```rust
// Step 7. `hasPendingAuth` is the OAuth RUNTIME's, so an authenticating server is never reaped.
let pending_auth: crate::lifecycle::PendingAuthCheck = {
    let runtime = Arc::clone(&oauth_runtime);
    let storage = auth_storage_options.clone();
    Arc::new(move |name: &str| {
        let (runtime, storage, name) = (Arc::clone(&runtime), storage.clone(), name.to_string());
        Box::pin(async move {
            crate::oauth::has_pending_auth(&runtime, &name, storage.base_dir.as_deref()).await
        })
    })
};
let lifecycle = Arc::new(McpLifecycleManager::new(Arc::clone(&manager), pending_auth));
```

This widens `PendingAuthCheck` ([`lifecycle.rs:362`](../../crates/cyrup-mcp/src/lifecycle.rs)) from
`Arc<dyn Fn(&str) -> bool + Send + Sync>` to
`Arc<dyn Fn(&str) -> BoxFuture<'static, bool> + Send + Sync>`, makes
`McpLifecycleManager::has_pending_auth` ([`:773`](../../crates/cyrup-mcp/src/lifecycle.rs)) `async`,
and adds `.await` at its two consumers — both already inside `async fn`s. Do **not** mirror the pending
set into a synchronous field: a second copy of that state is precisely the drift this codebase keeps
finding. `AuthStorageOptions::base_dir` is `Option<PathBuf>`
([`credentials.rs:1789-1794`](../../crates/cyrup-mcp/src/credentials.rs)), and `None` is meaningful —
`has_pending_auth`'s `base_dir: None` arm ([`oauth.rs:2054-2057`](../../crates/cyrup-mcp/src/oauth.rs))
matches by server name across every key, which is the right answer when no dir was configured.

### The body, in order

Insert between the lifecycle `add_cleanup` at
[`runtime.rs:277-283`](../../crates/cyrup-mcp/src/runtime.rs) and the final `Ok(state)` at
[`runtime.rs:291`](../../crates/cyrup-mcp/src/runtime.rs).

```rust
    // ── §8 tail — the zero-enabled-servers early return (MCP-018) ────────────────────────────
    // The structural half is already at runtime.rs:286-289. Absent is the notice, gated on
    // `allServerEntries.length > 0 && hasUI` (init.ts:217-223) — so a config with NO servers at all
    // says nothing, and a headless run says nothing.
    if state.config.enabled_servers().next().is_none() {
        let all = state.config.mcp_servers.len();
        if all > 0 && let Some(ui) = state.ui.as_ref() {
            cyrup_ext::HostServices::notify(
                ui.as_ref(),
                &format!("MCP: All {all} server(s) are disabled"),
                cyrup_ext::NotifyKind::Info,
            );
        }
        state.publish_status(crate::live::create_mcp_status_snapshot(&state));
        return Ok(state);
    }

    // ── §9 — cache bootstrap (MCP-019) ──────────────────────────────────────────────────────
    // The two-way split IS the unit (init.ts:228-239). Collapsing "no usable cache" into one arm
    // turns the corrupt-cache path from cheap into a connect storm.
    //
    // The PROBE is `dirs`', the READ is `registration`'s (the lenient reader, registration.rs:830),
    // and the WRITE is `dirs`'. That asymmetry is deliberate: the strict `dirs` reader answers `None`
    // for a file the lenient one parses fine, and rewriting on THAT would destroy the very cache
    // `resolve_direct_tools` and `resolve_cached_prompts` registered this session's surface from.
    // The accepted delta is that cyrup's `!cache` arm is narrower than upstream's single reader; see
    // the note at dirs.rs:571-577 on unifying the two.
    let cache_path = dirs.metadata_cache();
    let cache_file_exists = cache_path.exists();
    let mut cache = crate::registration::load_metadata_cache(&dirs);
    let mut bootstrap_all = false;
    if !cache_file_exists {
        bootstrap_all = true;
        let _ = crate::dirs::save_metadata_cache(&cache_path, &crate::dirs::MetadataCache::default());
    } else if cache.is_none() {
        let _ = crate::dirs::save_metadata_cache(&cache_path, &crate::dirs::MetadataCache::default());
        cache = Some(crate::registration::MetadataCache::default());
    }

    // ── §10 — per-server lifecycle registration (MCP-020) + rehydration (MCP-021) ────────────
    for (name, definition) in state.config.enabled_servers() {
        let mode = definition.lifecycle_mode();
        let persists = matches!(mode, ServerLifecycle::Eager | ServerLifecycle::LazyKeepAlive);
        // `definition.idleTimeout ?? (persistsAfterFirstSpawn ? 0 : undefined)` (init.ts:246) — the
        // `?? 0` is what stops an eager or lazy-keep-alive server ever idling out by default.
        let idle_timeout = definition.idle_timeout.or(persists.then_some(0.0));
        state.lifecycle.register_server(
            name,
            definition.clone(),
            idle_timeout.map(|t| LifecycleOverrides { idle_timeout: Some(t) }),
        );
        // ONLY `keep-alive` at registration (init.ts:252); `lazy-keep-alive` waits for its first
        // connect. `marks_keep_alive_at_registration` (runtime.rs:331) is that predicate.
        if crate::runtime::marks_keep_alive_at_registration(mode) {
            state.lifecycle.mark_keep_alive(name);
        }
        // Step 6 — rehydrate from a hash-valid entry (init.ts:256-269). `valid_entry` IS
        // `cachedEntry && isServerCacheValid(cachedEntry, definition)`; do not re-derive it.
        if let Some(cache) = cache.as_ref()
            && let Some(entry) = crate::registration::valid_entry(Some(cache), name, definition)
        {
            crate::live::rehydrate_from_cache(&state, name, definition, entry, cache);
        }
    }

    // ── §11 — the bounded startup connect pass (MCP-022 / MCP-087 / MCP-130) ────────────────
    let startup: Vec<(String, crate::config::ServerEntry)> = state
        .config
        .enabled_servers()
        .filter(|(_, d)| bootstrap_all || d.lifecycle_mode().is_prewarmed())
        .map(|(name, entry)| (name.clone(), entry.clone()))
        .collect();

    if let Some(ui) = state.ui.as_ref()
        && !startup.is_empty()
    {
        let text = crate::ui::format_mcp_status(
            &state.config,
            &format!("connecting to {} servers...", startup.len()),
        );
        cyrup_ext::HostServices::set_status(ui.as_ref(), "mcp", text.as_deref());
    }

    // `{name, definition, connection, error}` (init.ts:284-299). `error` is Some for a real failure
    // AND for needs-auth (which carries the byte-exact `/mcp-auth` line); BOTH are None for an abort
    // on a live signal, which pass two skips silently.
    let results = crate::live::parallel_limit(
        startup.clone(),
        crate::live::STARTUP_CONNECT_CONCURRENCY,
        |(name, definition)| {
            let (manager, signal) = (Arc::clone(&state.manager), runtime_signal.clone());
            async move {
                match manager.connect(&name, &definition, Some(&signal)).await {
                    Ok(c) if c.status() == ConnectionStatus::NeedsAuth => (
                        name.clone(),
                        definition,
                        None,
                        // BYTE-EXACT (init.ts:288). The `/mcp-auth {name}` form is what the user
                        // copies; a reworded line is a support burden, not a style choice.
                        Some(format!("OAuth authentication required. Run /mcp-auth {name}.")),
                    ),
                    Ok(c) => (name, definition, Some(c), None),
                    Err(e) if crate::abort::is_abort_error(&e, Some(&signal)) => {
                        (name, definition, None, None)
                    }
                    Err(e) => (name, definition, None, Some(e.to_string())),
                }
            }
        },
    )
    .await;

    // `if (initialSignal?.aborted) return state;` (init.ts:301) — BEFORE the owner check, and it
    // returns `Ok`, not `Err`. This is the FIFTH exit from this function.
    if snapshot.initial_signal.as_ref().is_some_and(CancelToken::is_cancelled) {
        return Ok(state);
    }
    // MCP-046 checkpoint 1 (init.ts:302).
    owner.throw_if_inactive()?;

    // ── §12 — the two-pass metadata build (MCP-023) ─────────────────────────────────────────
    // Pass one over EVERY successful connection first (init.ts:304-325): a SIMPLE prefixed list, no
    // collision resolution, because it IS the collision universe pass two resolves against.
    // Pass two per server (init.ts:327-362): `owner.throw_if_inactive()?` at the TOP of each
    // iteration (MCP-046 checkpoint 2), then `build_tool_metadata(..., Some(&startup_known), true)`,
    // the five map writes, `update_metadata_cache`, `notify_tool_metadata_updated(name, "startup")`,
    // `mark_keep_alive_after_connect`, and `MCP: {name} - {n} tools skipped` as a WARNING when
    // `failed_tools` is non-empty.
    // The failure arm (init.ts:329-337), in order: `if (initialSignal?.aborted) continue;` FIRST,
    // then `record_failure` only when there IS an `error`, then the byte-exact
    // `MCP: Failed to connect to {name}: {sanitize_terminal_text(error)}` as NotifyKind::Error AND
    // on stderr — upstream emits both, and the stderr line is what a headless run has.

    // ── §13 — the startup summary (init.ts:364-372) ─────────────────────────────────────────
    // `notifyOnStartupConnect !== false`, `connectedCount > 0`, and a UI. Two forms: with failures
    // `MCP: {c}/{total} servers connected ({t} tools)`, without `MCP: {c} servers connected
    // ({t} tools)`. `{total}` is `startupServers.length`, NOT the config count.

    // ── §14 — the MCP_DIRECT_TOOLS bootstrap (MCP-026) ──────────────────────────────────────
    // Re-reads `$MCP_DIRECT_TOOLS` AND the cache from disk here rather than reusing the factory's
    // values, because upstream does (init.ts:374-377) — this is a different module and the cache has
    // just moved. `envDirect !== "__none__"` skips the WHOLE block (init.ts:375), which is a
    // different shape from `direct_tools_override`'s `Some(vec![])`: test the RAW value first, then
    // pass `direct_tools_override(raw)` to `missing_configured_direct_tool_servers`.
    // Excludes servers already connected in §11 (init.ts:382), concurrency 10, and per success
    // `update_server_metadata` -> `update_metadata_cache` -> `notify_tool_metadata_updated(name,
    // "direct-tools-bootstrap")` -> `mark_keep_alive_after_connect` -> `clear_failure`.
    // A missing definition is the `MCP server "{name}" is not configured` error (init.ts:387).
    // MCP-046 checkpoint 3 is init.ts:411, INSIDE the `missingCacheServers.length > 0` arm.
    //
    // THE MESSAGE. 13a MCP-026 says the byte-exact `MCP: direct tools for {names} will be available
    // after restart` "becomes false for cyrup and must be changed TOGETHER WITH an actual late
    // registration — pick one deliberately rather than leaving the message and adding the
    // registration." HA-1 has landed (`LateRegistrar` at native.rs:768, `LateSink` at
    // registration.rs:2021), so PICK THE REGISTRATION: call `sync_tool_surface()` after the pass and
    // emit `MCP: direct tools for {names} are now available`. Record the string change as a
    // deliberate divergence at the call site.

    // ── §15 — lifecycle callbacks (MCP-027) ─────────────────────────────────────────────────
    // FIVE, not three (init.ts:418-451). Every body opens with the owner guard: that is what keeps a
    // generation-N timer from writing into generation N+1.
    state.lifecycle.set_reconnect_callback({
        let state = Arc::clone(&state);
        Arc::new(move |server: String| {
            let state = Arc::clone(&state);
            Box::pin(async move {
                if !state.owner.is_active() { return Ok(()) }
                crate::live::update_server_metadata(&state, &server);
                crate::live::update_metadata_cache(
                    &state, &server, crate::live::MetadataCacheOptions::preserving(),
                );
                state.notify_tool_metadata_updated(&server, "lifecycle-reconnect");
                crate::live::clear_failure(&state, &server);
                crate::live::update_status_bar(&state);
                Ok(())
            })
        })
    });
    // set_reconnect_failure_callback — owner guard, `record_failure(message)`, `update_status_bar`
    // set_health_restored_callback  — owner guard, `clear_failure`,            `update_status_bar`
    // set_auth_required_callback    — owner guard, `clear_failure`,            `update_status_bar`
    // set_idle_shutdown_callback    — owner guard, the `{server} shut down (idle {m}m)` debug using
    //                                 init.ts:664-673's effective-timeout ladder, `update_status_bar`

    // ── Step 11 — the list-changed listener (MCP-017) ───────────────────────────────────────
    // Installed AFTER the state commits, so a hook fired mid-build cannot see a half-installed
    // surface (init.ts:200-206; the ordering runtime.rs:200 documents).
    // `preserveEmptyResources: false` is the load-bearing detail: THIS empty `resources/list` is
    // authoritative and must overwrite the cache.
    state.manager.set_metadata_list_changed_listener(Some({
        let state = Arc::clone(&state);
        Arc::new(move |server: &str, reason: &str| {
            if !state.owner.is_active() { return }
            crate::live::update_server_metadata(&state, server);
            crate::live::update_metadata_cache(
                &state, server,
                crate::live::MetadataCacheOptions { preserve_empty_resources: false },
            );
            state.notify_tool_metadata_updated(server, reason);
            crate::live::update_status_bar(&state);
        })
    }));

    // ── The tail (init.ts:453-458) ──────────────────────────────────────────────────────────
    // MCP-046 checkpoint 4, health checks, the `off` footer clear, and a PUBLISH — not an
    // `update_status_bar`, which would additionally write the footer this path deliberately leaves
    // to whatever §11 last set.
    owner.throw_if_inactive()?;
    state.lifecycle.start_health_checks(runtime_signal.clone());
    if state.config.settings_or_default().mcp_footer_status() == crate::config::FooterStatus::Off
        && let Some(ui) = state.ui.as_ref()
    {
        cyrup_ext::HostServices::set_status(ui.as_ref(), "mcp", None);
    }
    state.publish_status(crate::live::create_mcp_status_snapshot(&state));
    Ok(state)
```

`manager` is moved into `McpStateParts` at
[`runtime.rs:253-264`](../../crates/cyrup-mcp/src/runtime.rs) and comes back as `state.manager`;
`runtime_signal` is moved into `manager.set_runtime_signal` at
[`runtime.rs:208`](../../crates/cyrup-mcp/src/runtime.rs) and does **not**, so clone it before that
line.

### MCP-017's cleanup order and the materialized session

**Verified, and worse than the row states.** `add_cleanup` has exactly two production sites,
[`runtime.rs:270`](../../crates/cyrup-mcp/src/runtime.rs) (oauth) and `:277` (lifecycle).
`MaterializedResources::new` ([`renderers.rs:763`](../../crates/cyrup-mcp/src/renderers.rs)) has **no
production caller at all**, so every production materialization falls to the process-global at
[`renderers.rs:770-774`](../../crates/cyrup-mcp/src/renderers.rs), reached from
[`renderers.rs:724`](../../crates/cyrup-mcp/src/renderers.rs) — which no owner owns and nothing ever
cleans.

Upstream registers it in `startInitialization`, **before** `initializeMcp`
([`index.ts:293`](../../tmp/pi-mcp-adapter/index.ts)), making it the first cleanup overall and so the
last to run. cyrup has no `start_initialization` yet (MCP-011, Wave 2), so build it here and register
it as the **first** `add_cleanup` in `initialize_mcp` — before the oauth block at
[`runtime.rs:268`](../../crates/cyrup-mcp/src/runtime.rs) — which lands it in the same LIFO position:

```rust
    // `McpState` field 22, built with the owner's token so a blob in flight at stop is omitted with
    // `runtime stopped` (renderers.rs:781) rather than orphaned in the temp dir.
    let materialized = Arc::new(crate::renderers::MaterializedResources::new(Some(owner.token())));
    // …threaded into McpStateParts, then IMMEDIATELY after the commit and BEFORE runtime.rs:268.
    // FIRST registered, so LIFO runs it LAST:
    //   lifecycle.graceful_shutdown -> shutdown_oauth -> cleanup binaries.
    owner.add_cleanup(Box::new({
        let materialized = Arc::clone(&state.materialized);
        move || {
            let materialized = Arc::clone(&materialized);
            Box::pin(async move { materialized.cleanup().map_err(crate::errors::McpError::from) })
        }
    }));
```

Thread `Some(state.materialized.as_ref())` into `resolve_mcp_result_content`
([`renderers.rs:702`](../../crates/cyrup-mcp/src/renderers.rs)) from the tool-result path when Wave 1/3
builds it; until then the field is constructed, owned and cleaned, and the process-global stops being
the only session there is.

### MCP-016 / MCP-015 — the sampling and elicitation gates, and the handler factory

**Verified: the wiring, not the predicates.** `McpSettings::sampling(has_ui)` at
[`config.rs:1227`](../../crates/cyrup-mcp/src/config.rs), `sampling_auto_approve` at `:1233`,
`elicitation(has_ui)` at `:1239`, and `ContextSnapshot::is_tui_mode` at
[`runtime.rs:86`](../../crates/cyrup-mcp/src/runtime.rs) are all exact. `initialize_mcp` reads none.

**The structural gap the row misses.** `McpServerManager` stores both configs
([`server_manager.rs:1263`](../../crates/cyrup-mcp/src/server_manager.rs), `:1265`) through setters at
`:1338` / `:1343`, and **nothing ever reads them** —
`grep -n 'self.sampling\|self.elicitation' server_manager.rs` shows only those setters and the teardown
at `:2474-2477`. `ConnectionBuilder::new` installs `bare_handler_factory()`
([`runtime.rs:1940-1948`](../../crates/cyrup-mcp/src/runtime.rs)), which hard-codes
`sampling: None, elicitation: None`, and `with_handler_factory`
([`runtime.rs:2296`](../../crates/cyrup-mcp/src/runtime.rs)) has **no non-test caller**. So the gate has
nowhere to land until the factory exists.

**That factory is this unit's deliverable.** It is a `HandlerFactory`
([`runtime.rs:2312`](../../crates/cyrup-mcp/src/runtime.rs)) closing over the manager, reading
`manager.sampling` / `manager.elicitation` at **call** time so a later `set_sampling_config` is
observed, and building `McpClientHandlerParts`
([`runtime.rs:1403-1415`](../../crates/cyrup-mcp/src/runtime.rs)) whose
`sampling.is_some()` / `elicitation.is_some()` is what `build_client_capabilities`
([`runtime.rs:1220`](../../crates/cyrup-mcp/src/runtime.rs)) advertises. Install it on the builder at
[`runtime.rs:193-196`](../../crates/cyrup-mcp/src/runtime.rs). The *handlers themselves* stay
MCP-118/121/122; what lands here is the factory plus the two gates that decide whether a slot is
`Some`, and `allow_url = snapshot.is_tui_mode()` on the elicitation mode.

MCP-015 closes with it: the factory's `getCurrentModel` reads `HostServices::current_model()` through
the **owned `Arc`** (`state.ui`, already fenced) rather than a re-read slot, and its per-request signal
is `crate::abort::combine(&owner.token(), Some(ctx_signal))`
([`abort.rs:60`](../../crates/cyrup-mcp/src/abort.rs)) — the two live closures
[`runtime.rs:19-25`](../../crates/cyrup-mcp/src/runtime.rs) already documents.

### MCP-046 — the abort call-site discipline

**Verified.** `throw_if_inactive` has exactly two call sites in `initialize_mcp`
([`runtime.rs:235`](../../crates/cyrup-mcp/src/runtime.rs), `:237`), both inside the `open_browser`
closure. The four §8 checkpoints correspond to [`init.ts:302`](../../tmp/pi-mcp-adapter/init.ts),
[`:328`](../../tmp/pi-mcp-adapter/init.ts), [`:411`](../../tmp/pi-mcp-adapter/init.ts) and
[`:453`](../../tmp/pi-mcp-adapter/init.ts). This unit is the audit that closes with the rest of
sub-wave D; it adds no code of its own.

---

## Sub-wave E — the extension seams

### MCP-012 — `startLoadTimeInitialization` · `partial`

**Verified.** [`extension.rs:697-699`](../../crates/cyrup-mcp/src/extension.rs) calls the gate
(`needs_load_time_initialization`, [`runtime.rs:301-303`](../../crates/cyrup-mcp/src/runtime.rs) —
correct, including the `lazy-keep-alive` exclusion via `is_prewarmed`) and then only logs
`MCP: eager/keep-alive servers configured — pre-warm pending`. No `tokio::spawn`, no generation
re-check, no synthetic print-mode context. Replace the debug with the spawn; the task's first act must
be to re-read `self.generation` and compare it against the value captured before the spawn.

### MCP-013 — the `MCP_DIRECT_TOOLS` blocking wait · `partial`

**Verified.** `on_session_start` ([`extension.rs:455-479`](../../crates/cyrup-mcp/src/extension.rs))
contains no `MCP_DIRECT_TOOLS` read and no await on an initialization. Both predicates are built:
`missing_configured_direct_tool_servers` at
[`registration.rs:985`](../../crates/cyrup-mcp/src/registration.rs), and the sentinel normalisation
`direct_tools_override` at [`runtime.rs:314`](../../crates/cyrup-mcp/src/runtime.rs) with
`DIRECT_TOOLS_NONE_SENTINEL` at `:307`.

### MCP-027a — `sendMessage`'s `triggerTurn` convergence gate · `missing`

**Verified, and currently inexpressible.** `pub type SendMessage = Arc<dyn Fn(String) + Send + Sync>`
([`state.rs:69`](../../crates/cyrup-mcp/src/state.rs)) takes no options, so there is no flag to branch
on; its own doc at `:61-68` says so. Its one production construction is
[`runtime.rs:242-251`](../../crates/cyrup-mcp/src/runtime.rs), whose body is a `tracing::debug!` reading
`send_message not yet wired`. The host side is ready: the fenced
`inject_message(content, custom_type, display, trigger_turn)` at
[`owner.rs:451-457`](../../crates/cyrup-mcp/src/owner.rs), and `McpLifecycleManager::ensure_converged`
at [`lifecycle.rs:899`](../../crates/cyrup-mcp/src/lifecycle.rs) returning a single-flight `BoxFuture`.
Grow the alias to `Arc<dyn Fn(String, bool) + Send + Sync>` and update `McpStateParts`
([`state.rs:175-176`](../../crates/cyrup-mcp/src/state.rs)) and the builder. **Deliver-on-failure must
be a real arm, not a `?`**: a convergence failure logs one debug line and *still injects*.

### MCP-031 — `flushMetadataCache` on shutdown · `missing`

**Verified.** `on_session_shutdown` ([`extension.rs:565-578`](../../crates/cyrup-mcp/src/extension.rs))
takes the state out of its slot, binds it to `_state`, and **drops it** — it never calls
`shutdown_state`. `shutdown_state` itself is built
([`lifecycle.rs:1562-1605`](../../crates/cyrup-mcp/src/lifecycle.rs)) and takes a `MetadataFlush`
([`lifecycle.rs:394`](../../crates/cyrup-mcp/src/lifecycle.rs)); the only implementation is
`no_metadata_flush` at [`lifecycle.rs:400-404`](../../crates/cyrup-mcp/src/lifecycle.rs), whose log line
literally reads *"`flush_metadata_cache` is pending MCP-031"*.

`MetadataFlush` is `Arc<dyn Fn(&Arc<McpState>) -> McpResult<()> + Send + Sync>` — **synchronous**, and
so is upstream's `flushMetadataCache` ([`init.ts:560-566`](../../tmp/pi-mcp-adapter/init.ts)), so the
shapes already agree:

```rust
/// `init.ts:560-566` `flushMetadataCache(state)` — the [`crate::lifecycle::MetadataFlush`]
/// `shutdown_state` takes, replacing `no_metadata_flush`.
#[must_use]
pub fn metadata_flush() -> crate::lifecycle::MetadataFlush {
    Arc::new(|state: &Arc<McpState>| {
        for (name, connection) in state.manager.get_all_connections() {
            if connection.status() == ConnectionStatus::Connected {
                update_metadata_cache(state, &name, MetadataCacheOptions::preserving());
            }
        }
        Ok(())
    })
}
```

`get_all_connections` ([`server_manager.rs:1474`](../../crates/cyrup-mcp/src/server_manager.rs)) returns
an `IndexMap<String, Arc<ServerConnection>>`. Wiring `shutdown_state` into `on_session_shutdown` is
MCP-009/MCP-010, Wave 2; this unit supplies the flush that wave installs, so land the function and
coordinate the one-line call site with Wave 2's owner.

---

## Sub-wave F — the removal half (MCP-036, MCP-038)

Both are edits to one body, `McpExtension::sync_tool_surface`
([`extension.rs:166-254`](../../crates/cyrup-mcp/src/extension.rs)). One owner, or the removal half is
written twice.

**Verified.** `sync_tool_surface` computes `changed` at
[`extension.rs:245`](../../crates/cyrup-mcp/src/extension.rs) as
`!surface.tool_names.is_empty() || !surface.command_names.is_empty()` — which counts *registrations*,
never removals, and yields no `(added, updated, deactivated)` triple.
`grep -rn 'direct tools refreshed\|deactivate_tools' src` returns zero.
`McpExtension::fallback_deactivated_tools` (field at
[`extension.rs:101`](../../crates/cyrup-mcp/src/extension.rs), accessor at `:386`) has a reader and **no
writer**. `set_active_tools` appears exactly once in the crate, as the fenced arm at
[`owner.rs:442`](../../crates/cyrup-mcp/src/owner.rs).

Between `register_surface` at [`extension.rs:222-223`](../../crates/cyrup-mcp/src/extension.rs) and the
adoption at `:227`:

```rust
        // `syncDirectTools`' `(previous ? updated : added)` (index.ts:229), computed against the
        // PREVIOUS map — still `sink.known_tools`, because `register_surface` never touched it.
        //
        // Iterating `surface.tool_names` rather than every resolved spec is the point: that list
        // already holds ONLY what registered (register_surface pushes at registration.rs:2161, after
        // the `should_register_tool` gate at :2158, and LateSink::should_register_tool IS
        // `previous !== fingerprint`), so a tool whose fingerprint did not change is neither added
        // nor updated. PROXY_TOOL_NAME is also pushed there (:2185) and is never in `known_tools`,
        // so it must be excluded or every description change reads as an added tool.
        let (mut added, mut updated) = (0usize, 0usize);
        for name in surface
            .tool_names
            .iter()
            .filter(|n| n.as_str() != crate::registration::PROXY_TOOL_NAME)
        {
            if sink.known_tools.contains_key(name) {
                updated += 1;
            } else {
                added += 1;
            }
            // index.ts:223-228 — the re-activation arm, PER TOOL and gated on this tool actually
            // having been in the fallback set. A tool that comes back must leave that set AND be put
            // back into the active list, or it stays invisible for the rest of the session.
            self.reactivate_tool(name);
        }
        // index.ts:233-237 — every previously-registered name absent from the NEW resolution.
        // `direct_tool_fingerprints` (registration.rs:1920) holds EVERY resolved spec, registered or
        // not, which is exactly upstream's `nextNames` (index.ts:212).
        let deactivated: Vec<String> = sink
            .known_tools
            .keys()
            .filter(|name| !surface.direct_tool_fingerprints.contains_key(*name))
            .cloned()
            .collect();
        self.deactivate_tools(&deactivated);
```

and, replacing the `changed` computation at
[`extension.rs:245-253`](../../crates/cyrup-mcp/src/extension.rs):

```rust
        // index.ts:257-263 — the sum of all three, and a UI.
        let changed = added + updated + deactivated.len();
        if changed > 0
            && let Some(services) = self.host_services()
        {
            services.notify(
                &format!(
                    "MCP: direct tools refreshed (+{added}, ~{updated}, -{})",
                    deactivated.len()
                ),
                cyrup_ext::NotifyKind::Info,
            );
        }
        changed > 0
```

`deactivate_tools` and `reactivate_tool`, from
[`index.ts:186-203`](../../tmp/pi-mcp-adapter/index.ts) and
[`index.ts:223-228`](../../tmp/pi-mcp-adapter/index.ts). cyrup has no `unregister_tool` on
`ExtensionRegistry`, which lands it on upstream's own `unregisterTool === undefined` branch — a
supported upstream configuration, so `unregistered` is always empty and `fallbackNames` is always
`toolNames`:

```rust
    /// `index.ts:186-203` `deactivateTools(toolNames)` — the `setActiveTools` fallback, the ONLY
    /// branch cyrup has. A deactivated MCP tool stops being callable but its name stays in the
    /// registry for the session, exactly as upstream behaves against a host without
    /// `unregisterTool`. Record that as an accepted delta at the call site.
    fn deactivate_tools(&self, names: &[String]) {
        if names.is_empty() {
            return;
        }
        let Some(services) = self.host_services() else { return };
        let remove: std::collections::HashSet<&str> = names.iter().map(String::as_str).collect();
        // `getActiveToolsIfReady()` returning undefined (index.ts:176-184, the "Action methods cannot
        // be called during extension loading" arm) is `active_tools() == None`.
        let Some(active) = services.active_tools().filter(|a| !a.is_empty()) else {
            if let Ok(mut slot) = self.fallback_deactivated_tools.lock() {
                slot.extend(names.iter().cloned());
            }
            return;
        };
        let next: Vec<String> =
            active.iter().filter(|n| !remove.contains(n.as_str())).cloned().collect();
        // `if (nextActiveTools.length !== activeTools.length)` — the fallback set is recorded ONLY on
        // this branch too (index.ts:198-201).
        if next.len() != active.len() {
            if let Ok(mut slot) = self.fallback_deactivated_tools.lock() {
                slot.extend(names.iter().cloned());
            }
            services.set_active_tools(&next);
        }
    }

    /// `index.ts:223-228` — a tool re-registered after having been deactivated leaves the fallback
    /// set and is appended to the active list. The `delete` returning true is the gate: a tool that
    /// was never deactivated must not cause a `setActiveTools` write.
    fn reactivate_tool(&self, name: &str) {
        let removed = self
            .fallback_deactivated_tools
            .lock()
            .map(|mut slot| {
                let before = slot.len();
                slot.retain(|n| n != name);
                slot.len() != before
            })
            .unwrap_or(false);
        if !removed {
            return;
        }
        let Some(services) = self.host_services() else { return };
        let Some(active) = services.active_tools() else { return };
        if !active.iter().any(|n| n == name) {
            let mut next = active;
            next.push(name.to_string());
            services.set_active_tools(&next);
        }
    }
```

Both call `self.host_services()` ([`extension.rs:323`](../../crates/cyrup-mcp/src/extension.rs)) — the
raw backend, not the fenced `state.ui` — because `sync_tool_surface` runs on paths where no `McpState`
exists yet, which is why the surrounding function already reads config from disk rather than from
`state`.

---

## Sub-wave G — MCP-224, the materialized-resource retry

**Verified.** `MaterializedResources::cleanup`
([`renderers.rs:860-873`](../../crates/cyrup-mcp/src/renderers.rs)) removes the directory and zeroes the
counters; its own `TODO(MCP-224)` at
[`renderers.rs:847-856`](../../crates/cyrup-mcp/src/renderers.rs) enumerates exactly what is absent —
the pending set, the per-directory attempt counters capped at 3, the single 30 s timer guarded by
*"already pending or nothing retryable"*, the timer-clear when the set empties, and the aggregate error
over `Vec<std::io::Error>`. Sub-wave D gives the session an owner and a cleanup site; this adds the
retry behind it. The timer selects on the same owner token the session was built with, for the reason
`record_failure`'s does.

---

## `RuntimeEnv` — the one production `ProxyEnv`

Thirty-two methods. Sixteen are the **only** declaration in the crate of a verb this group implements:
`lazy_connect`, `touch`, `increment_in_flight`, `decrement_in_flight`, `failure_age_seconds`,
`record_failure`, `clear_failure`, `update_status_bar`, `update_server_metadata`,
`update_metadata_cache`, `mark_keep_alive_after_connect`, `commit_prompt_metadata`,
`sync_tool_surface`, `guard_mcp_output`, `all_tool_names`, `handle_url_elicitation_required`.

```rust
/// The crate's ONE production [`crate::proxy::ProxyEnv`]. Every method is a delegation; the call
/// order and branch structure live in `proxy/`, which is what makes `FakeEnv` and this type
/// interchangeable under 13d's conformance suite (MCP-196).
pub struct RuntimeEnv {
    state: Arc<McpState>,
    /// Where `update_metadata_cache` writes. Cloned from `McpExtension::dirs` (extension.rs:77).
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
        crate::live::failure_age_seconds(&self.state, server)
    }
    fn record_failure(&self, server: &str, message: &str) {
        crate::live::record_failure(&self.state, server, message);
    }
    fn clear_failure(&self, server: &str) { crate::live::clear_failure(&self.state, server); }
    fn update_status_bar(&self) { crate::live::update_status_bar(&self.state); }
    fn update_server_metadata(&self, server: &str) {
        crate::live::update_server_metadata(&self.state, server);
    }
    fn update_metadata_cache(&self, server: &str) {
        crate::live::update_metadata_cache(
            &self.state, server, crate::live::MetadataCacheOptions::preserving(),
        );
    }
    fn mark_keep_alive_after_connect(&self, server: &str) {
        self.state.lifecycle.mark_keep_alive_after_connect(server);
    }
    async fn lazy_connect(&self, server: &str, cancel: &CancelToken) -> bool {
        crate::live::lazy_connect(&self.state, server, cancel).await
    }
    fn sync_tool_surface(&self) {
        if let Some(ext) = self.extension.upgrade() { ext.sync_tool_surface(); }
    }

    // MCP-084 — delegate, never mint a second copy: the config digest and the connect path have to
    // agree about what a server's URL IS (proxy/env.rs:322-334).
    fn resolve_server_url(&self, definition: &ServerEntry) -> McpResult<Option<String>> {
        crate::credentials::resolve_server_url(
            definition.url.as_deref(),
            &crate::credentials::process_env(),
        )
    }
    fn supports_oauth(&self, definition: &ServerEntry) -> bool {
        crate::oauth::supports_oauth(definition)
    }

    // MCP-231 / MCP-232 — the FREE functions in proxy/approval.rs (`:77`, `:272`), NOT the `ProxyCtx`
    // wrappers at proxy/env.rs:437/:454: the ctx holds this env, so reaching back would cycle. Both
    // take the metadata map directly, which is why D0's `Vec<ToolMetadata>` discharge is a
    // prerequisite rather than a tidy-up.
    fn is_tool_call_approval_required(&self, server: &str, tool: &ToolMetadata) -> bool {
        match self.state.tool_metadata.lock() {
            Ok(metadata) => crate::proxy::is_tool_call_approval_required(
                &self.state.config, server, tool, Some(&metadata),
            ),
            // A poisoned lock reaches the `tool_metadata == None` asymmetry proxy/approval.rs:60-75
            // documents honestly, rather than by guessing a map.
            Err(_) => crate::proxy::is_tool_call_approval_required(
                &self.state.config, server, tool, None,
            ),
        }
    }
    async fn ensure_tool_call_approved(
        &self,
        server: &str,
        tool: &ToolMetadata,
        arguments: &Value,
        origin: ApprovalOrigin,
        cancel: &CancelToken,
    ) -> ApprovalOutcome {
        // CLONED, unlike the sync arm: the gate awaits a human and a `std::sync::MutexGuard` cannot
        // be held across an await (the reasoning at proxy/env.rs:448-452).
        let metadata =
            self.state.tool_metadata.lock().map(|guard| guard.clone()).unwrap_or_default();
        crate::proxy::ensure_tool_call_approved(
            &self.state, server, tool, arguments, origin, cancel, &metadata,
        )
        .await
    }

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
        // synthesise a built-in name list as a floor (proxy/env.rs:385-393).
        self.state.ui.as_ref().and_then(|ui| cyrup_ext::HostServices::all_tool_names(ui.as_ref()))
    }

    // ── Wave 1 (MCP-164) fills these two. Loud, greppable, never a fabricated success. ─────────
    async fn call_tool(&self, …) -> Result<CallToolOutcome, ProxyCallError> {
        Err(ProxyCallError::Other(McpError::Other(
            "MCP: tools/call is not wired — MCP-164".to_string(),
        )))
    }
    async fn read_resource(&self, …) -> Result<Vec<Content>, ProxyCallError> { /* the same shape */ }
}
```

`connect` / `reconnect` build a `ConnectOutcome`
([`proxy/env.rs:47-57`](../../crates/cyrup-mcp/src/proxy/env.rs)) from `manager.connect` /
`manager.reconnect` plus `build_tool_metadata` — that is what C0 unblocks. `close` and
`handle_url_elicitation_required` delegate to `manager.close` and to the accepted-elicitation registry
at [`server_manager.rs:2582-2614`](../../crates/cyrup-mcp/src/server_manager.rs).
`commit_prompt_metadata` is `update_server_metadata`'s prompt half, extracted so
[`proxy/auth.rs:362-364`](../../crates/cyrup-mcp/src/proxy/auth.rs) can call it alone.

### The construction site

`McpExtension` gains an eighteenth field, `proxy_ctx: Mutex<Option<Arc<ProxyCtx>>>`, beside `dispatch`
at [`extension.rs:113`](../../crates/cyrup-mcp/src/extension.rs), and a method beside
`install_surface_sync` at [`extension.rs:425`](../../crates/cyrup-mcp/src/extension.rs) — same
`Weak<McpExtension>` shape, same `self_weak` guard, same call site:

```rust
    /// Build this generation's [`crate::proxy::ProxyCtx`] over the one production
    /// [`crate::proxy::ProxyEnv`] and stash it where the dispatcher (MCP-214) can find it.
    ///
    /// Called from the commit tail, exactly where [`Self::install_surface_sync`] is, and for the same
    /// reason: the env holds the committed state, so it cannot exist before the commit.
    pub fn install_runtime_env(&self, state: &Arc<McpState>) {
        let Some(weak) = self.self_weak.get().cloned() else {
            // Built without `into_arc` — the in-crate unit tests hold the value directly. With no
            // self handle the env's `sync_tool_surface` could not call back, so install nothing
            // rather than a half-wired context.
            tracing::debug!("MCP: no self handle bound; runtime env not installed");
            return;
        };
        let env = Arc::new(crate::live::RuntimeEnv::new(
            Arc::clone(state),
            self.dirs.clone(),
            weak,
        ));
        let ctx = Arc::new(crate::proxy::ProxyCtx::new(
            Arc::clone(state),
            env as Arc<dyn crate::proxy::ProxyEnv>,
        ));
        if let Ok(mut slot) = self.proxy_ctx.lock() {
            *slot = Some(ctx);
        }
    }

    /// This generation's proxy context, for the dispatcher (MCP-214).
    #[must_use]
    pub fn proxy_ctx(&self) -> Option<Arc<crate::proxy::ProxyCtx>> {
        self.proxy_ctx.lock().ok().and_then(|slot| slot.clone())
    }
```

Wave 2's MCP-011 commit tail calls `install_surface_sync` and `install_runtime_env` together.

---

## Out-of-group blockers found while reading

Not this group's units. Named so the owner routes them instead of discovering them mid-flight.

1. **`MCP-211` (`formatSchema`) and `MCP-091` (`renderTsShape`) do not exist and are unscheduled.**
   `grep -rn 'format_schema\|render_ts_shape' src` finds only the two `ProxyEnv` declarations
   ([`proxy/env.rs:363`](../../crates/cyrup-mcp/src/proxy/env.rs), `:365`), the `FakeEnv` bodies at
   [`proxy/testsupport.rs:193`](../../crates/cyrup-mcp/src/proxy/testsupport.rs)/`:196`, and four call
   sites — [`proxy/discovery.rs:357-359`](../../crates/cyrup-mcp/src/proxy/discovery.rs) and `:577-580`
   (`describe` / `search`), and [`proxy/call.rs:704`](../../crates/cyrup-mcp/src/proxy/call.rs) (the
   direct-tool `Expected parameters:` suffix). All are Wave 3's paths. `RuntimeEnv` must supply a body
   to compile: land both as one-line delegations to `crate::proxy::format_schema` /
   `crate::proxy::render_ts_shape` so the signature is fixed and there is exactly one place to fill,
   and **route MCP-211 to Wave 3's owner** — that wave is its only consumer. `render_ts_shape`
   returning `None` is upstream's own real branch (the caller forks to `Parameters:` +
   `format_schema`), and is therefore honest; `format_schema` is model-facing text and must not be
   improvised.

2. **`ConnectionBuilder` never gets `with_auth_provider` in production.** `initialize_mcp` installs the
   builder at [`runtime.rs:193-196`](../../crates/cyrup-mcp/src/runtime.rs) with the default
   `NoStoredCredentials` ([`runtime.rs:1913-1928`](../../crates/cyrup-mcp/src/runtime.rs)), so once §11
   exists **every HTTP server ends at `needs-auth` even when its credential is already in the store**.
   The comment at [`runtime.rs:187-192`](../../crates/cyrup-mcp/src/runtime.rs) names this. That is
   `MCP-115`, section 05, and it will be the first thing a §11 end-to-end run reports. Do not fix it
   here; expect it in the result.

3. **`ServerConnection::tools` / `resources` / `prompts` stay empty until MCP-119.**
   [`server_manager.rs:806-818`](../../crates/cyrup-mcp/src/server_manager.rs) says so at the fields, in
   its own words: *"the field is here because the record's shape is part of MCP-100's contract … it
   stays empty until that unit lands."* So §11's `build_tool_metadata` produces an empty surface until
   MCP-119 lands, and the snapshot's `connection.tools().len()` fallback answers 0. Everything in this
   group is still correct and reachable; the *content* arrives with that unit.

4. **The two metadata-cache readers should be unified.** `dirs.rs:571-577` already asks for it. This
   group is the first code that reads through one and writes through the other in the same function,
   so it is where the divergence becomes observable. Not in scope; record it.

---

## Definition of Done

Source-observable. Each line is checkable by reading the tree.

**Structure**

- [ ] `crates/cyrup-mcp/src/live.rs` exists, is declared `pub mod live;` in `lib.rs`, and holds the
      §13/§17/§18/§19/§3.16 verbs plus `RuntimeEnv`. No `crates/cyrup-mcp/src/env.rs` was created.
- [ ] `grep -rn 'impl ProxyEnv\|ProxyEnv for' crates/cyrup-mcp/src` returns `RuntimeEnv` **and**
      `FakeEnv`, and `RuntimeEnv`'s definition is outside every `#[cfg(test)]`.
- [ ] `McpExtension::install_runtime_env` exists beside `install_surface_sync`, stashes an
      `Arc<ProxyCtx>` in a `proxy_ctx` field, and has a public accessor.
- [ ] `call_tool`, `read_resource`, `format_schema` and `render_ts_shape` are the **only** `RuntimeEnv`
      methods that do not reach a real implementation; the first two name `MCP-164` in their bodies and
      the second two delegate to `crate::proxy::` functions.

**D0**

- [ ] `McpState::tool_metadata` is `Mutex<IndexMap<String, Vec<ToolMetadata>>>` and
      `McpState::prompt_metadata` is `Mutex<IndexMap<String, Vec<PromptCommandSpec>>>`; neither
      `ServerToolMetadata` nor `PromptMetadata` is declared as a `struct` in `state.rs` any more.
- [ ] `ProxyCtx` has no `tool_metadata` field; `with_metadata` / `with_metadata_mut` lock
      `self.state.tool_metadata`; every `ProxyCtx::new` call site passes two arguments.

**The payload contract**

- [ ] `McpStatusSnapshot` carries `version` / `servers` / `total_tools` / `total_resources` /
      `connected_count` / `disabled_count`; `McpServerStatusSnapshot` carries exactly six keys with
      `resource_count` and `failed_ago_seconds` under `skip_serializing_if = "Option::is_none"` and
      `disabled` unconditional; `McpServerRuntimeStatus` has six variants serialising as
      `connected` / `cached` / `failed` / `needs-auth` / `not-connected` / `disabled`.
- [ ] `McpStatusSnapshot::default()` is hand-written and sets `version` to
      `MCP_STATUS_SNAPSHOT_VERSION`, so `lifecycle.rs:1573`'s existing `Default::default()` publish
      becomes `publishMcpStatusShutdown`'s payload without an edit.
- [ ] `create_mcp_status_snapshot` iterates `config.mcp_servers` directly (config order, no `BTreeMap`
      on the path), and `failed_ago_seconds` is `Some` only when `status == Failed`.

**The fence and the swallow points**

- [ ] The `fenced!` invocation in `owner.rs` lists all 66 `HostServices` methods — the 35 names in
      sub-wave B each appear exactly once, with the return type `services.rs` declares.
- [ ] `McpState::notify_tool_metadata_updated` wraps the listener call in
      `catch_unwind(AssertUnwindSafe(..))` and logs
      `MCP: metadata update hook failed for {server}: {message}` at debug; a panicking listener does not
      propagate.
- [ ] `McpExtension::on_event` returns
      `HookOutcome::Mutate(EventPatch::ToolResult { is_error: Some(true), content: None, details: None, usage: None })`
      for `details.error ∈ {"tool_error", "call_failed"}` and `HookOutcome::Noop` for every other value
      including `auth_required`, absent `details`, and `details: null`.

**The verbs**

- [ ] `FAILURE_BACKOFF_MS = 60_000` and `MAX_FAILURE_MESSAGE_CHARS = 8 * 1024` exist as named
      constants; `record_failure` reads the previous `count` **before** calling `clear_failure`,
      truncates on a char boundary, and its expiry task selects on the owner token *and* re-checks
      `last_failure == failed_at` before clearing.
- [ ] `failure_age_seconds` returns `None` strictly outside the 60 s window and `Some(round(secs))`
      inside it; the failure-message reader gates on the same window.
- [ ] `parallel_limit` exists, is `buffered`-based, and is the helper both §11 and §14 use;
      `grep -rn 'buffer_unordered' crates/cyrup-mcp/src/live.rs` returns nothing.
- [ ] `update_server_metadata` bails on a missing or non-connected connection and on a missing
      definition, deletes from **all five** maps when the definition is disabled, writes
      `prompt_metadata` + `prompt_metadata_live` only when `!connection.prompt_discovery_failed()`, and
      treats an **empty** `instructions` as a delete.
- [ ] `update_metadata_cache` takes a `MetadataCacheOptions` with no `Default` impl, and
      `dirs::save_metadata_cache` has a production caller.
- [ ] `lazy_connect` implements the four `false` guards in `init.ts:621-634`'s order, and its error arm
      calls `record_failure` for a stray abort on a **live** signal but not for an abort on a cancelled
      one.
- [ ] `rehydrate_from_cache` guards on `crate::registration::valid_entry`, takes
      `registration::ServerCacheEntry`, writes `resource_counts` only for a present `resources` array
      and `prompt_metadata` only for a **non-empty** `prompts` array, and **never** touches
      `prompt_metadata_live`.
- [ ] `RuntimeEnv::guard_mcp_output` reads `$MCP_OUTPUT_GUARD` through `McpSettings::output_guard`;
      `grep -rn '\.output_guard(' crates/cyrup-mcp/src` returns a production call site.

**C0**

- [ ] `crate::registration::build_tool_metadata` returns `{metadata, failed_tools}`, pushes
      `"(unnamed)"` for a nameless tool, applies `uiVisibility` **after** the `seenNames` reservation,
      and takes both `known_metadata` and `include_missing_configured_candidates`.
- [ ] `crate::registration::reconstruct_tool_metadata` and `reconstruct_prompt_metadata` exist;
      `resolve_cached_prompts` calls the latter rather than carrying its own copy of the walk.
- [ ] `CandidateIndex` carries `additional_by_tool`, and `has_other_current_match` takes the tool name
      and consults it.
- [ ] No second copy of the name-resolution walk exists: `format_tool_name`, `is_tool_allowed`,
      `resolve_tool_prefix` and `valid_entry` each have exactly one definition.

**The `initialize_mcp` steps**

- [ ] A config whose servers are all disabled emits `MCP: All {n} server(s) are disabled` as
      `NotifyKind::Info` exactly once, and only when there is a UI and at least one entry.
- [ ] `bootstrap_all` is `true` only when the cache file was **absent**; an unparseable file is
      rewritten empty and does **not** bootstrap; the read goes through
      `crate::registration::load_metadata_cache` and the write through `crate::dirs::save_metadata_cache`.
- [ ] Every enabled server reaches `register_server` with
      `idle_timeout = definition.idle_timeout.or(persists.then_some(0.0))`, and `mark_keep_alive` is
      called only for `keep-alive`.
- [ ] `McpLifecycleManager::new`'s `hasPendingAuth` is bound to `oauth::has_pending_auth`;
      `grep -n 'Arc::new(|_| false)' crates/cyrup-mcp/src/runtime.rs` returns nothing, and
      `PendingAuthCheck` returns a `BoxFuture`.
- [ ] `owner.throw_if_inactive()?` appears at exactly four points in `initialize_mcp` outside the
      `open_browser` closure, and the `initial_signal.is_cancelled()` early return sits immediately
      before the first of them.
- [ ] **All five** lifecycle callbacks are installed, each body opening with `state.owner.is_active()`;
      `grep -n 'set_health_restored_callback\|set_auth_required_callback' crates/cyrup-mcp/src/runtime.rs`
      returns two hits.
- [ ] The list-changed listener is installed after the commit and passes
      `preserve_empty_resources: false`.
- [ ] `initialize_mcp`'s tail is `throw_if_inactive` → `start_health_checks` → the `off` footer clear →
      `publish_status(create_mcp_status_snapshot(..))` → `Ok(state)`; there is no `update_status_bar` on
      that line.
- [ ] `add_cleanup` is called three times in `initialize_mcp`, with `MaterializedResources::cleanup`
      **first** so LIFO runs it last; `MaterializedResources::new` has a production caller and
      `McpState` owns the session as field 22.
- [ ] The `MCP_DIRECT_TOOLS` bootstrap tests the **raw** value against `__none__` to skip the whole
      block, re-reads the env var and the cache from inside `initialize_mcp`, excludes servers already
      connected in §11, and ships `sync_tool_surface()` plus
      `MCP: direct tools for {names} are now available` with the divergence recorded at the call site.
- [ ] `initialize_mcp` reads `settings.sampling(has_ui)` / `settings.elicitation(has_ui)` /
      `is_tui_mode()` and installs a `HandlerFactory` through `ConnectionBuilder::with_handler_factory`
      that reads the manager's stored configs at call time; `with_handler_factory` has a non-test
      caller.

**The extension seams**

- [ ] `init` spawns the pre-warm task when `needs_load_time_initialization` is true and the task
      re-checks the generation before doing anything; the `pre-warm pending` debug line is gone.
- [ ] `on_session_start` reads `MCP_DIRECT_TOOLS`, skips the sentinel, and awaits the initialization
      only when `missing_configured_direct_tool_servers` is non-empty.
- [ ] `SendMessage` carries the `trigger_turn` flag; with it set the send is deferred behind
      `ensure_converged` and **still delivers** on a convergence failure with one debug line;
      `runtime.rs`'s `send_message not yet wired` debug is gone.
- [ ] `crate::live::metadata_flush()` exists, iterates `manager.get_all_connections()` for
      `ConnectionStatus::Connected`, and is a `MetadataFlush`; `no_metadata_flush`'s
      *"pending MCP-031"* log line is gone.

**Sub-waves F and G**

- [ ] `sync_tool_surface` computes `added` / `updated` over `surface.tool_names` **excluding**
      `PROXY_TOOL_NAME`, computes `deactivated` against `surface.direct_tool_fingerprints`, calls
      `deactivate_tools` for the removals, re-activates **per tool** inside the registration loop, and
      notifies `MCP: direct tools refreshed (+{a}, ~{u}, -{d})` only when the total is non-zero and
      there is a services handle.
- [ ] `fallback_deactivated_tools` has a writer; `set_active_tools` is called only when the filtered
      list is genuinely shorter.
- [ ] `MaterializedResources` carries a pending-cleanup set with per-directory attempts capped at 3, a
      single 30 s retry task guarded against double-scheduling and selecting on the owner token, and an
      aggregate error over `Vec<std::io::Error>`; the `TODO(MCP-224)` at
      [`renderers.rs:847`](../../crates/cyrup-mcp/src/renderers.rs) is gone.

**Whole-tree**

- [ ] `cargo check --workspace --all-targets` and `cargo doc --workspace --no-deps --bins` both exit 0.
      The second matters: `.cargo/config.toml` sets `--document-private-items` and the workspace denies
      `rustdoc::broken_intra_doc_links`, which is the lint that made the new module `live.rs` and not
      `env.rs`.
