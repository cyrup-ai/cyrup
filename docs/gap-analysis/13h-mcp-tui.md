# 13h · TUI: the MCP panels, slash commands and prompts

> Part of **[13 — cyrup-mcp](13-cyrup-mcp.md)**, which holds the thesis, the seam map, the
> architecture and the one canonical table of every port unit. Method and phasing are in
> **[MCP-PORT-METHODOLOGY.md](MCP-PORT-METHODOLOGY.md)**.

**Provenance.** Upstream is `pi-mcp-adapter` v2.25.0. cyrup is branch `david/cyrup`. rmcp is the
checkout at `/Users/davidmaple/cyrup.ai/rmcp` (`rmcp-v3.1.2-7-gf713ebd`). cyrup is referenced by
symbol and file only.

This subsystem is the adapter's entire human-facing surface. Everything else in `pi-mcp-adapter`
talks to servers or to the model; these files talk to the person at the terminal. There are exactly
two interactive panels — `mcp-panel.ts` (the server/tool browser, which doubles as the OAuth picker
under an `authOnly` flag) and `mcp-setup-panel.ts` (the onboarding/config-writing flow) — plus
`commands.ts`, which owns `/mcp`'s eight subcommands, `/mcp-auth`, the three headless renderers and
the three panel entry points, and `prompts.ts`, which turns each MCP *server-provided prompt
template* into its own slash command. `panel-keys.ts` is the keybinding resolver both panels share.
`glimpse-ui.ts` sits in this file set by assignment only: it is the MCP-UI native-window preview and
falls under Cut 2 — recorded in **Out of scope**, not as a gap.

**The hosting is already solved; the asynchrony is not.** Both panels are pi `Component`s —
`render(width): string[]` plus `handleInput(data)` — handed to `ctx.ui.custom(factory, { overlay:
true, overlayOptions: { anchor: "center", width: 82 | 92 } })`. cyrup has the exact counterpart:
`HostServices::open_overlay(Box<dyn InteractiveOverlay>)` (`crates/cyrup-ext/src/host/services.rs`),
painted by `ExtensionOverlay` (`crates/cyrup-tui/src/overlay.rs`), with
`cyrup-ext-subagents`' `FleetOverlay` (`crates/cyrup-ext-subagents/src/tui/fleet_overlay.rs`) as a
shipped precedent for the identical problem. So this section proposes no new TUI framework. What it
must solve is that every panel callback doing real work — `authenticate`, `reconnect`,
`copyToClipboard`, `openPath`, `adoptImports`, `scaffoldProjectConfig`, `addKnownServer` — is
**async, started from a synchronous keystroke handler, and settled back into panel state while the
panel stays open**. In JS that is a `.then()` plus `requestRender()`. In cyrup
`InteractiveOverlay::handle_key` is `&mut self` and synchronous and **there is no push-render channel
from an extension to the host** — the host only pulls, via `tick()` at the overlay's own
`refresh_ms()` cadence. `FleetOverlay`'s answer is the answer here: spawn onto a captured
`tokio::runtime::Handle`, hold a `oneshot::Receiver` per in-flight job, `try_recv` in `tick`, and
clear the busy latch even when the sender was dropped. Both panels take that shape, and both must
therefore declare a non-zero `refresh_ms()` even though upstream's panels arm no repaint timer at
all. The mechanism difference costs up to one tick of visible staleness on every async settle; that
residue is filed as work, not blessed.

**`crates/cyrup-mcp` is a native built-in extension crate**, the same shape as
`cyrup-ext-subagents` — compiled into the binary, not a WASM guest, and therefore **not sandboxed by
`HostServices`**. It links `rmcp`, `tokio` and `keyring` directly and reaches the host only for
genuine host concerns: drawing UI (`open_overlay`), telling the user something (`notify`,
`set_status`), asking the user something (`confirm`, `input`), session control
(`control(ControlOp::Reload)`, `control(ControlOp::SendUserMessage)`), cancellation
(`is_run_cancelled`, `HostCtx::begin_human_wait`) and registration (`InitApi::register_command`).
"`HostServices` has no X" is therefore almost never a blocker in this section — a clipboard write, a
URL open, a file read and a keyring probe are all things the crate does itself. The one thing a
native genuinely cannot improvise is a *handle to the host*: `HostCtx` carries only `mode`, `has_ui`,
`cwd` and a tier check, so the crate must stash the `Arc<dyn HostServices>` that
`NativeExtension::set_host_services` hands it before `init` — the same stash
`cyrup-ext-subagents`, `cyrup-intercom` and `cyrup-permission-system` already do.

**ratatui delegates *drawing*. It does not delegate behaviour, and this directory records that the
carve-out was previously applied far too broadly.** The host paints a `Clear`ed rect and a
`Paragraph` from the `OverlayLine`s the extension returns, so what is genuinely out of scope is the
`fg()` helper, the raw `\x1b[…m` literals, and the box-glyph assembly in `row()`/`padLine()` — those
compute cells the host now computes. Everything else in both panels **draws nothing and is in
scope**: the flattened item list and its filter state machine, fuzzy ranking, the visible-window
formula, key semantics and their order, toggling and dirty tracking, sort order, the hint-bar wrap
policy, terminal-injection sanitation, token estimation, and every string the user reads. The
*colours* are not excluded either — which slot is dim and which is cyan is transcribed to
`OverlaySpan` fields, because it is what the user sees. Nor is the width arithmetic: `innerW`,
`contentW`, `previewW` and the `pad` flag decide *what fits*, and a panel that drops them renders
different content, not merely different pixels.

**Three host additions touch this section, two of them already named elsewhere.** `HA-3` (overlay
geometry — upstream asks for fixed 82/92 columns; `ExtensionOverlay` hardcodes a percentage) is
cosmetic and belongs to the panel units. `HA-2` (labelled, dynamic argument completions for a
native command) is this section's `/mcp <TAB>` instance of the seam section 01 files as MCP-041 —
**one host addition, two consumers, not two additions**. `HA-1`'s **command leg** is what MCP prompts
need: `ExtensionHost::register_late_tool` exists for tools, but there is no `register_late_command`,
no command dirty flag and no catalog-refresh path, so the live half of prompt registration is a
from-zero design rather than a small hook. Everything else in the section is `hand-written` policy
over existing host verbs.

---

### How it lands

| # | adapter capability | upstream mechanism | cyrup mechanism | verdict |
| --- | --- | --- | --- | --- |
| U-1 | `/mcp` browser panel: server list, tool tree, always-on fuzzy filter, description search, token estimates, direct-tool toggles, save | `mcp-panel.ts` `createMcpPanel` + `commands.ts` `openMcpPanel`, `ctx.ui.custom(…, {overlay:true, width:82})` | a pure `McpPanelModel` plus an `impl InteractiveOverlay` opened with `HostServices::open_overlay`; precedents `FleetOverlay`, `PermissionSystemSettingsOverlay` | **`host-verb`** |
| U-2 | `/mcp setup` onboarding panel: four screens, dynamic action list, write previews, presets | `mcp-setup-panel.ts` `createMcpSetupPanel` + `openMcpSetup` (width 92) | same shape, second overlay type | **`host-verb`** |
| U-3 | `/mcp-auth` server picker | `openMcpAuthPanel` (width 82, `authOnly: true`) | the **same** panel type with an `auth_only` flag | **`host-verb`** |
| U-4 | panel geometry: fixed 82 / 92 columns, centre anchor, and no clipping of the body | `overlayOptions` | `ExtensionOverlay` hardcodes width-percent / min-width / max-height-percent / margin, and overflow is `.take(rect.height)`, which `InteractiveOverlay`'s own contract calls lossless-by-design — so the panel must window its own body | **`host-addition` HA-3** (geometry, cosmetic) + `hand-written` (self-windowing) |
| U-5 | panel keybindings read back from user config (`tui.select.*`, `mcp.panel.save`) | `panel-keys.ts` `createPanelKeys(keybindings)` | no extension-facing read of the resolved `action id → keys` map, although `ExtensionRegistry::resolve_shortcuts` already takes that map as a parameter; the fallback is upstream's own no-manager defaults | **`hand-written`** with a named residue — the read-back option is MCP-363a's decision |
| U-6 | headless `status` / `tools` / `prompts` listings | `commands.ts` `showStatus` / `showTools` / `showPrompts`, one multi-line `ui.notify` each | `HostServices::notify(&str, NotifyKind::Info)`, one call per listing | **`host-verb`** |
| U-7 | `/mcp`'s eight-way subcommand switch and `/mcp-auth`, both registered before any server connects | `pi.registerCommand("mcp" \| "mcp-auth", {description, handler})` in `index.ts` | `InitApi::register_command` in `init`; dispatch in `NativeExtension::execute_command`, which runs at `CtxTier::Command` | **`hand-written`** |
| U-8 | `/mcp <TAB>` and `/mcp reconnect <server><TAB>` — labelled, live completions | `getArgumentCompletions(prefix)` returning `{value, label}` pairs | `CommandDescriptor.completions` is a static `Vec<String>` fixed at `init`; `ExtensionHost::command_completions` is WASM-only, label-less, and has no consumer in `cyrup-tui` | **`host-addition` HA-2** (= MCP-041's seam) |
| U-9 | a config write from a command or panel takes effect immediately | `ctx.reload()` after `openMcpSetup` / a failed live refresh | `HostServices::control(ControlOp::Reload)`, Command-tier, guarded by `HostCtx::require_command_tier` | **`host-verb`** |
| U-10 | interactive OAuth: footer status key, an OSC-8 link in a notification, confirm-then-paste fallback | `ui.setStatus` + `ui.notify` + `ui.confirm` + `ui.input`, all carrying the flow's own abort signal | `set_status` / `notify` / `confirm` / `input`, with `DialogOptions.signal_id` for the inner signal and `HostCtx::begin_human_wait` around the dialog pair | **`host-verb`** |
| U-11 | every MCP prompt in a valid on-disk cache is a slash command **on the first frame** | `resolveCachedPrompts` at extension load → `pi.registerCommand` per prompt | `InitApi::register_command` during `init` — the cache read is synchronous, so this half ports as-is | **`hand-written`** |
| U-12 | prompts discovered from a **live** connection become slash commands mid-session | `syncPromptCommands()` re-run on every tool-metadata update, de-duplicated by name | nothing: no `register_late_command`, no command dirty flag, and the `/` menu's registry is rebuilt only on session swap or an `enableSkillCommands` toggle | **`host-addition` HA-1** (command leg) |
| U-13 | invoking a prompt command pushes the rendered template into the conversation as a user turn | `pi.sendUserMessage(text)` | `HostServices::control(ControlOp::SendUserMessage { content, opts })` | **`host-verb`** |
| U-14 | saving direct-tool toggles rewrites config **and** refreshes the live tool surface without a reload | `writeDirectToolsConfig` + `onDirectToolsConfigChanged` → `syncToolSurface` | the writer is `hand-written`; the in-session refresh needs `register_late_tool` reachable from a native — HA-1's tool leg (MCP-037). Without it every save takes the reload path | **`hand-written`** + **`host-addition` HA-1** |
| U-15 | MCP-UI apps rendered in a native macOS window instead of the browser | `glimpse-ui.ts` `isGlimpseAvailable` / `openGlimpseWindow`, driven by `ui-session.ts` | none | **`cut`** (Cut 2 — see Out of scope) |
### The two interactive panels — `mcp-panel.ts`, `mcp-setup-panel.ts`, `panel-keys.ts`

Upstream is `pi-mcp-adapter` v2.25.0. Two panels carry the adapter's whole interactive surface:
`mcp-panel.ts` (1,015 lines — the server/tool browser, which doubles as the OAuth picker under an
`authOnly` flag) and `mcp-setup-panel.ts` (666 lines — the onboarding/config-writing flow), plus
`panel-keys.ts` (53 lines), the keybinding resolver both share. Roughly 90 lines of the first and 40
of the second are box-drawing; **every layout decision** — which twelve rows are visible, where the
window starts, what a row's cell budget is, how the hint bar wraps — computes what the user sees and
is in scope.

**Where this lands: `HostServices::open_overlay` with the `InteractiveOverlay` trait.** Both panels
are pi `Component`s (`render(width): string[]` + `handleInput(data: string)`) handed to
`ctx.ui.custom(factory, { overlay: true, overlayOptions: { anchor: "center", width: 82 | 92 } })`.
cyrup's counterpart is complete and shipped: `HostServices::open_overlay(Box<dyn InteractiveOverlay>)`
(`crates/cyrup-ext/src/host/services.rs`), whose whole surface is
`render(width, height) -> Vec<OverlayLine>`, `handle_key(OverlayKey) -> OverlayOutcome`, `refresh_ms`
and `tick` (`crates/cyrup-ext/src/host/overlay.rs`), painted by `ExtensionOverlay`
(`crates/cyrup-tui/src/overlay.rs`) and fed by `App::handle_input`, which routes to the overlay
z-stack **before** the selector and before the global keymap — so `ctrl+c`, `ctrl+a`, `ctrl+r`,
`ctrl+s`, `ctrl+y`, `?`, `space` and every printable reach the panel, and an unhandled key never
leaks to the editor beneath. **This is a `host-verb`, not a core change.** Two live precedents are to
be followed rather than re-derived:

| precedent | what it settles |
|---|---|
| `FleetOverlay` (`crates/cyrup-ext-subagents/src/tui/fleet_overlay.rs`) | the async-job shape: a captured `tokio::runtime::Handle`, `Pending<T> = Option<oneshot::Receiver<T>>`, `spawn_action`, `drain_jobs` in `tick`, and the `TryRecvError::Closed` arm that **must still clear the busy latch** or every later action is silently refused |
| `PermissionSystemSettingsOverlay` (`crates/cyrup-permission-system/src/config_modal.rs`) | the result-escape shape: `open_overlay` consumes the box and returns `bool`, so the outcome is read off an `Arc`-shared object the overlay writes through (`ConfigController::take_last_error`) **after** `open_overlay` returns — and `false` is pi's `if (!ctx.hasUI)` branch, not an error |

`LiveHostServices::open_overlay` (`crates/cyrup-session-svc/src/host_services.rs`) blocks the
extension's task (never the run loop's) until teardown with no timeout, and returns `false` without
blocking when no renderer is attached. `ExtensionOverlay`'s `Drop` fires the release one-shot on
every teardown path, so a session swap or a quit can never strand the blocked task, and it is where
`cleanup()`/`dispose()` land.

**Three structural consequences, recorded as work rather than absorbed.**

1. **`refresh_ms()` must be non-zero even though upstream arms no repaint timer.** Upstream pushes
   every repaint with `requestRender()`; cyrup has only `tick`, and `refresh_ms` defaults to `0` =
   "never tick me". 250 ms is the smallest cadence that does not busy-render (`FleetOverlay` carries
   750 ms because pi's own component declares one). The residue: up to 250 ms of visible staleness on
   every async settle, plus a wakeup every 250 ms for the panel's life. The close is a push channel
   from extension to host, which does not exist.
2. **`open_overlay` returns `bool`, not `McpPanelResult`.** See MCP-369; the block-until-closed
   contract gives the happens-before, and the permission-system overlay is the worked example.
3. **The 60 s inactivity timer becomes a deadline compared in `tick`**, not a `setTimeout`. See
   MCP-362.

**Neither panel is verifiable by unit test alone.** Model-level tests pin the state machines,
`buildResult`, the sanitizers and the frame text; they cannot catch what only an assembled app shows
— the box the host actually paints, the width the component is actually handed, an empty-state frame,
a cursor that scrolls off, a hint bar that wraps into the border. **A panel is not done until it has
been run in a real terminal**, both panels opened, keys pressed, at more than one width. Every render
unit below says so in its `verify` line.

#### 1. `McpPanel` — the data model

Built once in `McpPanel`'s constructor from three inputs: the effective `McpConfig`, an optional
`MetadataCache` the caller loaded from disk, and a `Map<String, ServerProvenance>`. It is a
**snapshot** — the panel never re-reads config; the only post-construction mutation source is
`refreshCacheAfterReconnect` (§1.7).

```
ServerState {
  name: String,
  expanded: bool,                    // false for every server at construction
  source: "user" | "project" | "import",   // prov?.kind ?? "user"
  importKind: Option<String>,        // only when prov.importKind is present
  includeTools: Option<Vec<String>>, // only when definition.includeTools is present
  excludeTools: Option<Vec<String>>,
  exposeResources: bool,             // definition.exposeResources !== false
  connectionStatus: ConnectionStatus,
  failureMessage: Option<String>,    // callbacks.getFailureMessage?.(name) ?? null
  tools: Vec<ToolState>,
  hasCachedData: bool,               // a VALID cache entry, not merely a present one
}
ToolState {
  name: String,          // the RAW MCP tool name, never prefixed
  description: String,
  isDirect: bool,        // live, user-editable
  wasDirect: bool,       // the baseline; `dirty` is any isDirect != wasDirect
  estimatedTokens: usize,
}
ConnectionStatus = connected | idle | failed | needs-auth | connecting | disabled
```

`connecting` is in the enum but is **never** returned by `McpPanelCallbacks::getConnectionStatus`
(whose declared return type in `types.ts` omits it); only the panel sets it, for the duration of a
reconnect.

**Construction order, per server, in `Object.entries(config.mcpServers)` order** — insertion order of
the merged config object, which is user-visible as row order, so an `IndexMap` or
`Vec<(String, ServerEntry)>`, never a `BTreeMap`:

1. `authOnly && !callbacks.canAuthenticate(name)` ⇒ the server is skipped entirely.
2. `serverCache = cache?.servers?.[name]` **only if** `isServerCacheValid(entry, definition)`, else
   `undefined`. An invalid or stale entry is treated as absent — that is what drives the
   `(not cached)` row.
3. `toolFilter: true | string[] | false` — `definition.directTools` if *defined at all* (including
   `false`), else `config.settings.directTools` if truthy, else `false`. The asymmetry is deliberate:
   a per-server `directTools: false` wins, but a global `false` falls through to the same `false`.
4. Tools are built **only when** `serverCache && !authOnly && !isServerDisabled(definition)`. So in
   `authOnly` every server has zero tools, and a disabled server shows no tools even with a valid
   cache.
5. Per `serverCache.tools`: skip when `!isUiToolVisibleToModel(tool.uiVisibility)`; skip when
   `!isToolAllowed(...)` (§1.2). Otherwise
   `isDirect = toolFilter === true || (Array.isArray(toolFilter) && toolFilter.includes(tool.name))`,
   matched against the **raw** name.
   *(Cut 2 note: `uiVisibility` is MCP-Apps metadata and the MCP-UI subsystem is cut. The cut half is
   the producer — nothing in the ported tree ever writes a non-`undefined` `uiVisibility` into the
   cache. The half that remains is the predicate's total form, which the port keeps verbatim:
   `undefined`, or a list containing `"model"`, passes; anything else is filtered. Keeping it costs
   one `Option` read and means a cache written by an upstream-compatible producer still filters
   correctly.)*
6. When `definition.exposeResources !== false`, each `serverCache.resources` becomes a synthetic tool
   named **`read_${resourceNameToToolName(resource.name)}`** with description
   `resource.description ?? "Read resource: ${resource.uri}"`, and `estimateTokens` is fed a
   `CachedTool` carrying only `{name, description?}` — deliberately **no** `inputSchema`, so a
   resource tool's estimate is systematically smaller than a real tool's.

##### 1.1 `estimateTokens`

```
ceil((tool.name.len() + (tool.description?.len() ?? 0)
      + JSON.stringify(tool.inputSchema ?? {}).len()) / 4) + 10
```

`.len()` is JS string length (UTF-16 code units). `JSON.stringify({})` is `"{}"`, length 2 — so the
floor for a bare name is `ceil((n+2)/4) + 10`. The `stringify` must reproduce V8's key order, which
for a schema deserialized from JSON is insertion order; a `BTreeMap`-backed value sorts keys and
changes the byte count, hence every estimate. Use a key-order-preserving JSON value
(`serde_json` with `preserve_order`) and `serde_json::to_string` (no whitespace), which matches
`JSON.stringify(x)` over the object/array/string/number/bool/null subset a JSON Schema contains.

##### 1.2 `getOtherCurrentCandidates` — the cross-server collision set

The subtlest thing in the file. It exists to defend one behaviour: an `excludeTools` pattern written
for server A must not silently also match a *legacy* name form belonging to some other live tool.

For a given `(serverName, toolName)` it iterates **every enabled server in `config.mcpServers`,
including `serverName` itself** — for the current server it uses the `currentEntry` argument rather
than re-validating the cache, so a mid-construction entry stays consistent — collects each one's
**current-form** name candidates, and then **deletes** the candidates belonging to
`(serverName, toolName)` itself. The set therefore contains this server's *sibling* tools' current
names as well as every other server's; only the one pair is deleted, never the whole server. Porting
this as "every *other* server" is wrong and silently widens what a legacy pattern may match.

`isToolAllowed` consumes the set through `matchesToolSelector` (`types.ts`), whose three-way shape
matters:

1. a **current-form** candidate match returns `true` unconditionally — the cross-server set is never
   consulted for current forms;
2. with **no** `otherCurrentCandidates` argument it falls back to matching the whole candidate set,
   legacy included, unrestricted;
3. with the set supplied, the legacy-only candidates are `full − current`, and a pattern matches only
   when it hits a legacy candidate **and** hits none of `otherCurrentCandidates`.

Details that decide correctness:

- Disabled servers contribute nothing.
- Each other server uses **its own** effective prefix, `resolveToolPrefix(otherDefinition, this.prefix)`
  — not the panel's global one.
- Resources contribute their `read_...` base names when `exposeResources !== false`.
- `getToolNameCandidates(name, server, prefix, /* includeLegacy */ false)` yields the *current* forms
  only: `{raw, format(prefix), format("server"), format("short"), format("mcp")}`. With
  `includeLegacy` the function adds **thirteen** more expressions for **eighteen** total, deduped
  into a `Set` — not seventeen.
- The panel's own `this.prefix` is `config.settings?.toolPrefix ?? "server"`, and it is **not**
  per-server-resolved when filtering that server's own tools (construction passes `this.prefix`, not
  `resolveToolPrefix(definition, this.prefix)`) — an upstream asymmetry the port reproduces verbatim
  rather than "fixes".

`isToolAllowed(name, server, prefix, includeTools, excludeTools, otherCurrentCandidates)` is
`isToolIncluded && !isToolExcluded`. `isToolIncluded` is vacuously `true` when `includeTools` is
absent or empty. Patterns support `*` and `?` globs via `globToRegExp`: escape the class
``.+^${}()|[]\``, then `*` → `.*` and `?` → `.`, anchored `^...$`. In Rust that is a hand-built
`regex::Regex`, **not** a glob crate, because the escape set is specific (`-` and `,` are not
escaped, which is harmless; `[` is). And `matchesToolPattern` only reaches `globToRegExp` when the
pattern literally contains `*` or `?`; a pattern with neither is an exact set membership test, so a
regex-for-everything port changes matching for any pattern containing a regex metacharacter.

##### 1.3 `fuzzyScore`

```
lq = query.to_lowercase(); lt = text.to_lowercase();
if lt.contains(lq) { return 100.0 + (lq.len() as f64 / lt.len() as f64) * 50.0 }
score = 0; qi = 0; consecutive = 0;
for i in 0..lt.len() while qi < lq.len():
    if lt[i] == lq[qi] { score += 10 + consecutive; consecutive += 5; qi += 1 }
    else { consecutive = 0 }
return if qi == lq.len() { score } else { 0 }
```

Substring hits score 100–150; subsequence hits score by run length; a non-subsequence scores 0. Only
`> 0` / `== 0` is ever tested, plus one relative comparison (`* 0.6`, §1.4) — but the exact formula is
cheap and the `0.6` weighting is only meaningful against it. Indexing is by JS UTF-16 code unit;
iterate `chars()` over the lowercased strings and compare `char`s, which agree for every character
below U+10000. The `to_lowercase()` vs `toLowerCase()` divergence for locale-sensitive characters
(Turkish dotless i) is accepted and recorded.

##### 1.4 `rebuildVisibleItems` — the flattened list

`visibleItems: Vec<VisibleItem>` where `VisibleItem = { type: "server"|"tool", serverIndex, toolIndex? }`.
Called after every state change that could alter it, and **always** followed by
`cursorIndex = min(cursorIndex, max(0, visibleItems.len() - 1))` — the cursor is clamped, never reset
to 0.

```
query = if descSearchActive { descQuery } else { nameQuery }
mode  = if descSearchActive { "desc" } else { "name" }
items = []
for (si, server) in servers.enumerate():
    if query != "" && authOnly:
        if mode == "name" && fuzzyScore(query, server.name) > 0 { push Server(si) }
        continue                        # tools are never listed in authOnly
    push Server(si)
    if server.expanded || query != "":  # a query force-expands every server
        for (ti, tool) in server.tools.enumerate():
            if query != "":
                score = if mode == "name" {
                    max(fuzzyScore(query, tool.name), fuzzyScore(query, server.name) * 0.6)
                } else {
                    fuzzyScore(query, tool.description)
                }
                if score == 0 { continue }
            push Tool(si, ti)
if query != "" && !authOnly:
    keep = { it.serverIndex : it in items where it is Tool }
    items.retain(|it| it is Tool || keep.contains(it.serverIndex))
```

Three behaviours the port must not lose:

1. **A non-empty query force-expands every server**, regardless of `expanded`.
2. **Server-name matches propagate to tools at 0.6 weight** in name mode, so typing a server name
   lists all of its tools even when no tool name matches.
3. **The final filter drops server headers with no surviving tools**, with a self-referential
   predicate evaluated against the *pre-filter* list — which is what makes it one pass rather than a
   fixpoint. In Rust: collect the set of `serverIndex`es owning at least one `Tool`, then `retain`.

In `descSearchActive` mode the query is `descQuery`, so an empty `descQuery` — the state immediately
after pressing `?` — takes the `query == ""` path: every server header, no tools, nothing filtered.

##### 1.5 Key handling — the top-level state machine

Preamble on **every** keystroke, before any dispatch: `resetInactivityTimeout()`;
`importNotice = None`; and `authNotice = None` **only if `authInFlight.is_none()`** — an in-flight
OAuth notice survives keystrokes, every other notice does not.

Dispatch order is load-bearing:

| # | guard | effect |
|---|---|---|
| 1 | `confirmingDiscard` | delegate to §1.6 and **return** — so `ctrl+s` does *not* save from inside the discard modal |
| 2 | `ctrl+c` | `cleanup()`; `done({cancelled:true, changes:{}})` |
| 3 | `keys.save(data)` | `cleanup()`; `done(buildResult())` — works during desc-search too |
| 4 | `descSearchActive` | delegate to the modal branch and **return** (§1.5.1) |
| 5 | `escape` | if `nameQuery` non-empty, clear it, rebuild, clamp; else if `dirty`, `confirmingDiscard = true` and `discardSelected = 1`; else cancel |
| 6 | `keys.selectUp` / `keys.selectDown` | `moveCursor(-1)` / `moveCursor(+1)` |
| 7 | `space` | toggle the cursor item (§1.8), guarded `if item && !authOnly` |
| 8 | `keys.selectConfirm` | **server** row: return if `disabled`; if `authOnly` or `status == needs-auth` then authenticate (§1.7); else flip `expanded`, rebuild, clamp. **tool** row: flip `isDirect`, set the import notice if newly-direct and `source == "import"`, `updateDirty()` |
| 9 | `ctrl+a` | authenticate the cursor's server (§1.7) |
| 10 | `ctrl+r` | reconnect the cursor's server (§1.7) |
| 11 | `ctrl+y` | copy the cursor server's failure message — **only** when `status == "failed"` and `failureMessage.is_some()` (§1.9) |
| 12 | `data == "?"` | return immediately if `authOnly`; else enter desc-search: `descSearchActive = true`, `descQuery = ""`, rebuild, clamp |
| 13 | `backspace` | pop one char from `nameQuery` if non-empty, rebuild, clamp |
| 14 | `data.len() == 1 && data.char_code(0) >= 32` | append to `nameQuery`, rebuild, clamp |

`moveCursor` **clamps and does not wrap**: `cursorIndex = max(0, min(len-1, cursorIndex + delta))`,
and is a no-op on an empty list. Note `?` is intercepted at step 12, *before* the printable catch-all
at step 14, so `?` can never be typed into the name query; every other printable can, except that
`space` is claimed at step 7, so a literal space is unenterable in either query.

###### 1.5.1 Description-search modal

Entered by `?`, refused in `authOnly`. Inside it, only these are handled — everything else is
swallowed:

| key | effect |
|---|---|
| `escape` **or** `keys.selectConfirm` | exit: `descSearchActive = false`, `descQuery = ""`, rebuild, clamp |
| `backspace` | pop from `descQuery` (no-op when empty), rebuild, clamp |
| `keys.selectUp` / `keys.selectDown` | move cursor |
| `space` | toggle the cursor item — upstream's comment is *"Toggle even while in desc search"* |
| printable (`len == 1 && code >= 32`) | append to `descQuery`, rebuild, clamp |

`ctrl+c` and `keys.save` still work because they are tested at steps 2–3, *above* the modal branch.
Exiting always clears `descQuery`, so the description filter is never sticky.

##### 1.6 Discard-confirmation modal

Armed only by `escape` with `dirty == true` and an empty `nameQuery`. `discardSelected` starts at
**1** (Keep & Close).

| key | effect |
|---|---|
| `ctrl+c` | `cleanup()`; cancel with no changes |
| `escape`, `n`, `N` | `confirmingDiscard = false` — back to editing, changes intact |
| `keys.selectConfirm` | `cleanup()`; `discardSelected == 0` cancels, else `done(buildResult())` |
| `y`, `Y` | `cleanup()`; cancel with no changes |
| `left`, `right`, `tab` | flip `discardSelected` between 0 and 1 |

Everything else, including `ctrl+s`, is ignored.

##### 1.7 The async callbacks — authenticate and reconnect

**`authenticateServer(server)`** — the panel's own method, not the `commands.ts` free function of the
same name it eventually calls:

1. Return immediately if `authInFlight.is_some()` — one at a time, globally.
2. Return if `status` is `connecting` or `disabled`.
3. If `!callbacks.canAuthenticate(name)`: set `authNotice = "{name} does not use OAuth authentication."`
   and return. (`{name}` is `sanitizeDisplayText(server.name)` throughout this section.)
4. `authInFlight = Some(name)`; `authNotice = "Authenticating {name}..."`; `requestRender()`.
5. `callbacks.authenticate(name)` settles:
   - **`ok`**: `status = callbacks.getConnectionStatus(name)`;
     `authNotice = "OAuth finished for {name}. Reconnecting..."`; `authInFlight = None`;
     `requestRender()`; then `reconnectServer(server, { afterAuth: true })`.
   - **`!ok`**: `status = getConnectionStatus(name)`; `authNotice = "OAuth failed for {name}"` plus
     either `": {msg}"` when `result.message` is non-empty or
     `". Check the notification for details."` when it is not; `authInFlight = None`;
     `requestRender()`.
   - **rejects**: `status = getConnectionStatus(name)`;
     `authNotice = "OAuth failed for {name}: {msg}"`; `authInFlight = None`; `requestRender()`.

   The status is refreshed from the callback in all three arms, *before* the notice is set.

**`reconnectServer(server, {afterAuth})`**:

1. Return if `status` is `connecting` or `disabled`.
2. `status = "connecting"`; `requestRender()`.
3. `callbacks.reconnect(name)` settles:
   - **`Ok(connected: bool)`**: `status = getConnectionStatus(name)`;
     `failureMessage = getFailureMessage(name) ?? None`; if `status == "connected"`:
     `entry = callbacks.refreshCacheAfterReconnect(name)`; if `Some(entry)` then
     `cache ??= {version:1, servers:{}}`, `cache.servers[name] = entry`,
     `rebuildServerTools(server, entry)`; and `hasCachedData = true` **unconditionally within the
     connected branch**, i.e. even when the cache re-read returned `None`.
     If `afterAuth`: `authNotice = connected && status == "connected"`
     ? `"OAuth finished for {name}. Reconnected."`
     : `"OAuth finished for {name}, but reconnect did not complete. Press ctrl+r to retry."`
   - **rejects**: `status = "failed"` set **directly**, not re-derived;
     `authNotice = "Reconnect failed for {name}: {msg}"`.

**`rebuildServerTools(server, entry)`** preserves user edits across a reconnect:

- snapshot `existingState: Map<name, isDirect>` from the current `tools`;
- rebuild the tool list from `entry` using the same visibility/allow filters as construction, but
  passing `server` (a `ServerState`) where construction passed the config `definition` — so
  `getOtherCurrentCandidates` reads `server.includeTools`/`excludeTools`, the values captured at
  construction;
- per rebuilt tool: `isDirect = existingState.get(name).unwrap_or(false)`, and
  `wasDirect = if existed { old_tool.wasDirect } else { false }` — so a tool that vanished and came
  back is treated as never having been direct, while a surviving tool keeps its dirty-comparison
  baseline;
- resources are rebuilt when `server.exposeResources` (the captured bool);
- then `rebuildVisibleItems()` + `updateDirty()`. **`cursorIndex` is not clamped here** — a shrinking
  list can leave the cursor past the end until the next keystroke clamps it, and `render` tolerates
  it by skipping the row. Reproduce with checked indexing (`Vec::get`), not by "fixing" the clamp.

##### 1.8 Toggling, dirtiness, and the tri-state result

`toggleItem` — a no-op when `authOnly`:

- **server row**: `newState = !server.tools.iter().all(|t| t.isDirect)` — "all on unless already all
  on". `all()` on an *empty* list is `true`, so a server with no tools toggles to `false` and nothing
  happens. Sets every tool to `newState`; sets the import notice when `source == "import" && newState`.
- **tool row**: flip `isDirect`; set the import notice when now-direct and `source == "import"`.

Import notice text, three identical sites:
`"Imported from {importKind ?? "external"} — will copy to user config on save"` (em dash U+2014).

`updateDirty`: `dirty = servers.any(|s| s.tools.any(|t| t.isDirect != t.wasDirect))`.

`buildResult` — per server, **only when that server has at least one changed tool**:

| condition | `changes[server]` |
|---|---|
| `direct.len() == tools.len() && tools.len() > 0` | `true` |
| `direct.len() == 0` | `false` |
| otherwise | `Vec<String>` of the direct tools' **raw** names, in list order |

Servers with no change are absent from the map entirely, which is what stops `writeDirectToolsConfig`
from touching their config files.

##### 1.9 `ctrl+y` — copy the failure message

Requires `status == "failed"` and `failureMessage.is_some()`. Copies
`sanitizeDisplayText(server.failureMessage)` — the sanitized form, not the raw one — through pi's
`copyToClipboard`. On success `authNotice = "Copied error for {name} to clipboard"`; on failure
`authNotice = "Failed to copy error for {name}: {msg}"`. Both `requestRender()`. The hint
`ctrl+y copy error` is shown **only** when the cursor's server currently has a failure message
(`selectedServerHasFailureMessage`).

##### 1.10 Terminal-injection sanitation

Two functions, both mandatory, both defending against a malicious MCP server that names a tool with
embedded escape sequences.

`sanitizeDisplayText(text)` = `sanitizeTerminalText(text ?? "")` (`utils.ts`): `stripOscSequences`,
then strip every CSI and two-byte escape, then replace runs of C0/DEL/C1 control characters
(U+0000–U+001F, U+007F–U+009F) with a single space, then collapse all whitespace runs to one space,
then trim. Applied to server names, import kinds, tool names, descriptions, notice lines, failure
messages and every interpolated error string.

`stripOscSequences` (`utils.ts`) removes `ESC ]` and C1 `OSC` introducers **and their payloads even
when unterminated** — it scans for `BEL` (0x07), `ST` (0x9C) or `ESC \` and, finding none, consumes
to end of string. `__tests__/mcp-panel-rendering.test.ts` asserts this directly: an input ending in an
unterminated `OSC 8 ;; https://secret.invalid/truncated` must not leak the URL.

`sanitizeRowContent(content)` is a different function, applied to the **assembled** row after the
panel's own colour escapes have been added. It strips OSC, then walks the string: a matched ANSI
escape is **copied through** verbatim; any C0/DEL/C1 control character sets a `pendingSpace` flag and
is dropped; the next non-control character emits one space first *if* the result so far is non-empty
and does not already end in a space. This preserves the panel's styling while guaranteeing every
emitted row is a single line with no control bytes — the whole-frame property the rendering test
asserts across every line.

##### 1.11 Render layout

`innerW = width - 2`. Three row primitives:

- `row(content)` = `fg(border,"│") + truncateToWidth(" " + sanitizeRowContent(content), innerW, "…", true) + fg(border,"│")`.
  The **fourth argument `true` is `pad`** (pi's `truncateToWidth(text, maxWidth, ellipsis = "...", pad = false)`
  in `pi/packages/tui/src/utils.ts` @v0.84.1), which right-pads to exactly `maxWidth` — the only thing
  keeping the right border aligned. `visibleWidth` and `truncateToWidth` measure by grapheme cluster
  with ANSI/OSC escapes excluded, and `truncateToWidth` buffers escape codes and flushes them only
  before a cluster that fits.
- `emptyRow()` = `│` + `" ".repeat(innerW)` + `│`
- `divider()` = `fg(border, "├" + "─".repeat(innerW) + "┤")`

Frame, top to bottom:

1. **Title bar**: `titleText = authOnly ? " MCP OAuth " : " MCP Servers "`;
   `borderLen = innerW - visibleWidth(titleText)`; `leftB = floor(borderLen/2)`;
   `rightB = borderLen - leftB`; emitted as `╭` + `─`×leftB + title + `─`×rightB + `╮`.
2. `emptyRow()`.
3. **Search row**, one of three forms, with `searchIcon = fg(border,"◎")` and `cursor = fg(selected,"│")`:
   - desc-search active: `{◎}` + two spaces + `fg(needsAuth,"desc:")` + space + `descQuery` + cursor
   - `nameQuery` non-empty: `{◎}` + two spaces + `nameQuery` + cursor
   - otherwise: `{◎}` + two spaces + `fg(placeholder, italic("search..."))`
4. `emptyRow()`; then, when `noticeLines` is non-empty, one `row(fg(hint, italic(sanitize(line))))`
   per notice followed by another `emptyRow()`.
5. `divider()`.
6. **Body.** When `servers.is_empty()`: `emptyRow()`, one row of
   `fg(hint, italic(authOnly ? "No OAuth-capable MCP servers configured." : "No MCP servers configured."))`,
   `emptyRow()`. Otherwise:
   - `maxVis = MAX_VISIBLE = 12`; `total = visibleItems.len()`;
     `startIdx = max(0, min(cursorIndex - floor(maxVis/2), total - maxVis))` with `total - maxVis`
     computed as a **signed** value, so `total < maxVis` yields 0; `endIdx = min(startIdx + maxVis, total)`.
   - `emptyRow()`, then rows `startIdx..endIdx`. A **server** row emits `renderServerRow`, and when it
     is the cursor row *and* `status == "failed"` *and* `failureMessage.is_some()`, one additional row
     per line of `wrapText(sanitize(failureMessage), innerW - 6)`, each prefixed with four spaces and
     coloured `cancel`. A **tool** row emits `renderToolRow`.
   - `emptyRow()`.
   - When `total > maxVis`: `prog = round(((cursorIndex+1)/total) * 10)`, then
     `row(rainbowProgress(prog,10) + "  " + fg(hint, "{cursorIndex+1}/{total}"))` and an `emptyRow()`.
   - When `importNotice`: `row(fg(needsAuth, italic(sanitize(importNotice))))` + `emptyRow()`.
   - When `authNotice`: `row(fg(needsAuth, italic(sanitize(authNotice))))` + `emptyRow()`.
7. `divider()`; `emptyRow()`.
8. **Status line.** If `confirmingDiscard`:
   `row("Discard unsaved changes?  {discardBtn}   {keepBtn}")`, the selected button
   `inverse(bold(fg(colour, label)))` and the unselected `fg(hint, label)`, labels `"  Discard  "`
   (colour `cancel`) and `"  Keep & Close  "` (colour `confirm`) — the two-space padding inside each
   label is part of the string, and the buttons are separated by three spaces. Else if `authOnly`:
   `row(fg(description, "select a server to authenticate"))`. Else: `directCount` and `totalTokens`
   summed over all servers' direct tools, and
   `stats = directCount > 0 ? "{directCount} direct  ~{totalTokens.toLocaleString()} tokens" : "no direct tools"`,
   emitted as `row(fg(description, stats + (dirty ? fg(needsAuth, "  (unsaved)") : "")))`.
   **`toLocaleString()`** on a number under Node's default locale is thousands-grouped with `,`
   (`12,345`); group with `,` explicitly rather than reach for a locale crate.
9. `emptyRow()`.
10. **Hint bar.** `saveLabel = keys.saveLabel()`. The list is:
    - `authOnly`: `↑↓ navigate`, `⏎ auth`, `ctrl+a auth`, `esc clear/close`, `ctrl+c quit`
    - otherwise: `↑↓ navigate`, `space toggle`, `⏎ expand/auth`, `ctrl+a auth`, `ctrl+r reconnect`,
      `ctrl+y copy error` *(only when the cursor's server has a failure message)*, `? desc search`,
      `{saveLabel} save` *(only when `saveLabel` is `Some`)*, `esc clear/close`, `ctrl+c quit`

    In each entry the key part is `italic(...)` and the label follows after one space. Wrapping:
    `gap = "  "`, `gapW = 2`, `maxW = innerW - 2`; per hint,
    `needed = if curW == 0 { hw } else { gapW + hw }`; if `curW > 0 && curW + needed > maxW`, flush the
    current line as `row(fg(hint, curLine))` and start a new one with this hint (`curW = hw`);
    otherwise append with the gap and `curW += needed`. Flush the remainder.
11. `fg(border, "╰" + "─".repeat(innerW) + "╯")`.

`rainbowProgress(filled, total)` emits `total` dots joined by a single space, dot `i` coloured
`RAINBOW_COLORS[i % 7]` and rendered `●` when `i < filled` else `○`. The seven colours are truecolor
SGR bodies `38;2;178;129;214`, `38;2;215;135;175`, `38;2;254;188;56`, `38;2;228;192;15`,
`38;2;137;210;129`, `38;2;0;175;175`, `38;2;23;143;185` — i.e. `OverlayColor::Rgb(178,129,214)`
through `Rgb(23,143,185)`.

`DEFAULT_THEME` as SGR bodies, mapped onto `OverlaySpan` fields:

| slot | SGR | cyrup |
|---|---|---|
| `border`, `title`, `description`, `hint` | `2` | `dim: true` (`title` is *also* `2`, so the title is not visually distinct from the border) |
| `selected` | `36` | `fg: Cyan` |
| `direct` | `32` | `fg: Green` |
| `needsAuth` | `33` | `fg: Yellow` |
| `placeholder` | `2;3` | `dim: true, italic: true` |
| `confirm` | `32` | `fg: Green` |
| `cancel` | `31` | `fg: Red` |

`bold(s)`, `italic(s)` and `inverse(s)` are the SGR 1/3/7 pairs, mapping to `OverlaySpan { bold }`,
`{ italic }` and `{ reversed }`.

##### 1.12 Row renderers

`renderServerRow(server, isCursor)`:

- `expandIcon = if expanded {"▾"} else {"▸"}`;
  `prefix = if isCursor { fg(selected, expandIcon) } else { fg(border, if expanded {expandIcon} else {"·"}) }`
- `nameStr = if isCursor { bold(fg(selected, name)) } else { name }`
- `importLabel = if source == "import" { fg(description, " ({importKind ?? "import"})") } else { "" }`
  — the fallback here is `"import"`, whereas the notice text's fallback is `"external"`.
- `statusLabel = renderConnectionStatus(server)`
- **Not-cached branch** (`!hasCachedData && !authOnly`): `prefix` + three spaces + `nameStr` +
  `importLabel` + two spaces + `fg(description,"(not cached)")` + `statusLabel` — no toggle icon at all.
- **Normal branch**: `toggleIcon` is `fg(direct,"●")` when `directCount == totalCount && totalCount > 0`,
  `fg(needsAuth,"◐")` when `directCount > 0`, else `fg(description,"○")`. `toolInfo` is empty when
  `totalCount == 0`; otherwise `"{directCount}/{totalCount}"`, plus `"  ~{tokens.toLocaleString()}"`
  when `directCount > 0`, all wrapped in `fg(description, ...)`. Result: `prefix` + space +
  `toggleIcon` + space + `nameStr` + `importLabel` + two spaces + `toolInfo` + `statusLabel`.

`renderConnectionStatus(server)`, first match wins, each with a **two-space** prefix:

| condition | text |
|---|---|
| `authInFlight == Some(server.name)` | `fg(needsAuth, "authenticating")` |
| `status == disabled` | `fg(description, "disabled")` |
| `status == needs-auth` | `fg(needsAuth, "needs auth")` |
| `status == connecting` | `fg(needsAuth, "connecting")` |
| `status == failed` | `fg(cancel, "failed")` |
| `authOnly && status == connected` | `fg(direct, "connected")` |
| `authOnly` | `fg(description, "idle")` |
| otherwise | `""` — in normal mode a connected/idle server shows **no** status label |

`renderToolRow(tool, isCursor, innerW)`: `toggleIcon = if isDirect { fg(direct,"●") } else { fg(description,"○") }`;
`cursor = if isCursor { fg(selected,"▸") } else { " " }`;
`nameStr = if isCursor { bold(fg(selected,name)) } else { name }`;
`prefixLen = 7 + visibleWidth(toolName)`; `maxDescLen = max(0, innerW - prefixLen - 8)`; the
description is rendered **only when `maxDescLen > 5` and it is non-empty**, as
`fg(description, "— " + truncateToWidth(desc, maxDescLen, "…"))` (em dash, then a space). Result: two
spaces + `cursor` + space + `toggleIcon` + space + `nameStr` + space + `descStr`.

`wrapText(text, width)` — greedy word wrap with `max = max(8, width)`; splits on whitespace runs
dropping empties; a word wider than `max` is hard-split by accumulating characters while
`visibleWidth(take + ch) <= max`, emitting each full chunk and carrying the remainder (with a
one-character forced take to guarantee progress); returns `vec![text]` if it produced nothing.

##### 1.13 Lifecycle

`INACTIVITY_MS = 60_000`. `resetInactivityTimeout()` clears and re-arms a timer on **every**
keystroke; on fire it calls `cleanup()` then `done({ cancelled: true, changes: Map::new() })` — a
silent auto-cancel that discards unsaved changes with no confirmation prompt. `cleanup()` clears the
timer; `dispose()` calls `cleanup()`; `invalidate()` is an empty method present only to satisfy pi's
`Component` shape.

#### 2. `panel-keys.ts` — keybinding resolution, in full

`PanelKeys` has five members: `selectUp(data)`, `selectDown(data)`, `selectConfirm(data)`,
`save(data)`, `saveLabel() -> Option<String>`.

`createPanelKeys(keybindings)`:

- **With a manager**: `selectUp/Down/Confirm` delegate to
  `keybindings.matches(data, "tui.select.up" | "tui.select.down" | "tui.select.confirm")`. `save` is
  resolved through `configuredSaveKeys`, which reads `keybindings.getUserBindings?.()["mcp.panel.save"]`:
  - the key **present** (even as `[]`) gives `{keys: as_array(value), configured: true}`
  - the key **absent** gives `{keys: [], configured: false}`

  Then `save(data) = if keys.len() > 0 { keys.any(|k| matchesKey(data, k)) } else { !configured && matchesKey(data, "ctrl+s") }`,
  and `saveLabel() = keys.first().cloned().or(if configured { None } else { Some("ctrl+s") })`.

  The three-way outcome is the point: **unbound in config means saving is disabled entirely and the
  `save` hint disappears**; bound means those keys, with the first as the label; not mentioned means
  the `ctrl+s` default. `__tests__/mcp-panel-keybindings.test.ts` exercises the `[]` case explicitly.
- **Without a manager**: hardcoded `up`, `down`, `return`, `ctrl+s`, label `"ctrl+s"`.

`mcp.panel.save` is an **adapter-defined** id — it exists in no host keybinding table and is read
straight out of the user's raw bindings map.

#### 3. `McpSetupPanel`

Constants: `MIN_PANEL_WIDTH = 24`, `COMPACT_WIDTH = 60`, `COMPACT_ACTION_ROWS = 7`,
`DESKTOP_PREVIEW_WIDTH = 74`, `INACTIVITY_MS = 60_000`. Theme: `border "2"`, `title "36"` (cyan),
`selected "32"` (green), `hint "2"`, `success "32"`, `warning "33"`, `muted "2;3"`.

State: `screen: "empty" | "setup" | "imports" | "paths"` (initialised from `options.mode`),
`actionCursor`, `importCursor`, `pathCursor` (all 0), `selectedImports: Set<ImportKind>` pre-seeded
with **every** detected import, `busy: bool`, `notice: Option<{ text, tone: success|warning|muted }>`.

##### 3.1 The action list (`getActions`)

Rebuilt on **every** call — never cached — and its length changes with `screen`, so the cursor index
means different things on different screens. Order:

| # | id | condition | label | description |
|---|---|---|---|---|
| 1 | `run-setup` | `screen == "empty"` | `Run setup` | ``Inspect detected configs, adopt imports, and scaffold a minimal `.mcp.json`.`` |
| 2 | `adopt-imports` | `discovery.imports.len() > 0` | `Adopt detected compatibility imports` | `Choose which host-specific MCP configs Pi should import into its own override file. {N} source{s} found.` |
| 3 | `view-example` | always | ``View example `.mcp.json` `` | `Preview a working shared MCP config you can paste or adapt.` |
| 4 | `scaffold-project` | no `sources` entry with `id == "shared-project" && exists` | ``Scaffold project `.mcp.json` `` | `Write a minimal project config using the standard shared MCP file path, then reload Pi.` |
| 5 | `show-precedence` | always | `Explain config precedence` | `Show the read order and where Pi writes compatibility settings.` |
| 6 | `open-paths` | `getDetectedPaths().len() > 0` | `Open detected config paths` | `Browse the actual config files that Pi discovered on this machine.` |
| 7..11 | `add-known-server` | one per `KNOWN_SERVER_PRESETS` | `preset.name` | `preset.summary` |
| 12 | `add-repoprompt` | `!repoPrompt.configured && executablePath && targetPath && entry && serverName` all present | `Add RepoPrompt to shared MCP config` | `Write a standard MCP entry for RepoPrompt to the recommended shared target, then reload MCP in-session.` |
| 13 | `close` | always | `Close` | `Exit the onboarding flow.` |

`getDetectedPaths()` = the `path` of every `source` with `exists == true`, followed by the `path` of
every `imports` entry, **deduplicated preserving first-seen order**.

`KNOWN_SERVER_PRESETS` (`config.ts`, interface `KnownServerPreset`), five entries in order,
transcribed from the tag:

| id | name | summary | entry |
|---|---|---|---|
| `deepwiki` | `DeepWiki` | `Ask questions about public GitHub repositories.` | `{url:"https://mcp.deepwiki.com/mcp", protocolVersion:"auto"}` |
| `context7` | `Context7` | `Look up current library documentation and examples.` | `{url:"https://mcp.context7.com/mcp", protocolVersion:"auto"}` |
| `notion` | `Notion` | `Search and work with your Notion workspace.` | `{url:"https://mcp.notion.com/mcp", auth:"oauth", protocolVersion:"auto"}` |
| `github` | `GitHub` | `Work with GitHub through your Copilot account.` | `{url:"https://api.githubcopilot.com/mcp", auth:"oauth", protocolVersion:"auto"}` |
| `chrome-devtools` | `Chrome DevTools` | `Inspect and automate a local Chrome browser.` | `{command:"npx", args:["-y","chrome-devtools-mcp@1.6.0"]}` |

Four carry `protocolVersion: "auto"`; two of those also carry `auth: "oauth"`; `chrome-devtools`
alone is a stdio `npx` entry with no `protocolVersion`. Keep the asymmetry:
`previewKnownServer`/`addKnownServer` write under the key `preset.id`, while the success notice
reports `preset.name` — adding "Chrome DevTools" writes `"chrome-devtools"` and says
`Added Chrome DevTools to ...`.

##### 3.2 Key handling (`handleInput`)

Preamble: `resetInactivityTimeout()`; `if !busy { notice = None }`.

1. `ctrl+c`: `cleanup()`; `done()`.
2. `escape`: if `screen` is `imports` or `paths`, go **back to `discovery.hasAnyConfig ? "setup" : "empty"`**
   — not to the screen you came from — and re-render; otherwise `cleanup()`; `done()`.
3. `if busy { return }` — checked **after** 1 and 2, so `ctrl+c` and `escape` close the panel while an
   async write is still in flight.
4. `screen == "imports"` delegates to §3.3; `screen == "paths"` to §3.4.
5. `keys.selectUp`: `actionCursor = max(0, actionCursor - 1)`; re-render.
   `keys.selectDown`: `actionCursor = min(actions.len() - 1, actionCursor + 1)`; re-render.
   `keys.selectConfirm`: `runAction(selected)` (§3.5).

There is no `space` and no name search on this panel.

##### 3.3 Imports sub-screen (`handleImportsInput` / `renderImports`)

Keys: up/down move `importCursor` clamped to `[0, imports.len()-1]`; `space` toggles the cursor
entry's `kind` in `selectedImports`; `keys.selectConfirm` runs `applySelectedImports()`.

Render: a header row `Select compatibility imports. Space toggles, Enter saves, Esc goes back.`, a
blank row, then one row per import formatted `{cursor} {"[x]"|"[ ]"} {entry.kind}` + two spaces +
`{entry.path}`, where `cursor` is `fg(selected,"›")` on the cursor row and a space otherwise. Then a
blank row, then a live
`formatWritePreview("Compatibility import write preview", previewImports(selected), [], previewW)` —
recomputed from the *currently selected* set on every frame.

`applySelectedImports`: the selected kinds are taken in `discovery.imports` order, not selection
order. An empty selection sets `notice = {text: "Select at least one compatibility import first.", tone: "warning"}`
and returns. Otherwise `runBusy`: `adoptImports(selected)` then `markSetupCompleted()`, then
`notice = result.added.len() > 0`
? `{"Added {added.join(", ")} to {path}. Pi will reload after this panel closes.", success}`
: `{"No changes needed in {path}.", muted}`, then `screen = hasAnyConfig ? "setup" : "empty"` and
`actionCursor = 0`.

##### 3.4 Paths sub-screen (`handlePathsInput` / `renderPaths`)

Keys: up/down move `pathCursor` clamped; `keys.selectConfirm` runs `runBusy(openPath(selected))` then
sets `notice = {"Opened {path}", success}`.

Render: header `Select a detected config path to open. Enter opens it, Esc goes back.`, a blank row,
then one `{cursor} {path}` row per detected path. **No preview block on this screen.**

`openPath` (`utils.ts`) calls `execOpen(pi, targetPath)` with no browser override and no signal:
`open <path>` on macOS, `cmd /c start "" <path>` on Windows, `xdg-open <path>` elsewhere, throwing
`stderr || "Failed to open path (exit code {code})"` on a non-zero exit.

##### 3.5 `runAction` and the busy latch

| action | effect |
|---|---|
| `run-setup` | `screen = "setup"`; `actionCursor = 0`; re-render |
| `adopt-imports` | `screen = "imports"`; `importCursor = 0`; re-render |
| `open-paths` | `screen = "paths"`; `pathCursor = 0`; re-render |
| `scaffold-project` | `runBusy`: `scaffoldProjectConfig()`, `markSetupCompleted()`, `notice = {"Wrote starter config to {path}. Pi will reload after this panel closes.", success}` |
| `add-repoprompt` | `runBusy`: `addRepoPrompt()`, `markSetupCompleted()`, `notice = {"Added {serverName} to {path}. Pi will reload after this panel closes.", success}` |
| `add-known-server` (with a preset) | `runBusy`: `addKnownServer(preset)`, `markSetupCompleted()`, same notice text |
| `close` | `cleanup()`; `done()` |
| anything else (`view-example`, `show-precedence`, a preset-less `add-known-server`) | `notice = {"Review the details below. Press Enter on an action with a side effect to apply it.", muted}`; re-render |

`runBusy(fn)`: `busy = true`; `notice = {"Working...", muted}`; re-render; await `fn`; on error
`notice = {error.message, warning}`; finally `busy = false`; re-render.

##### 3.6 Render and the frame

`panelW = max(24, width)`; `innerW = panelW - 2`; `contentW = max(8, innerW - 4)`;
`previewW = max(12, min(74, contentW))`.

`padLine(text, innerW)`: `inset = 2`; `contentW = max(0, innerW - 4)`;
`fitted = truncateToWidth(text, contentW, "…", true)`; then an *additional*
`max(0, contentW - visibleWidth(fitted))` spaces (redundant given `pad: true`, but present); result
`"│" + 2 spaces + fitted + padding + 2 spaces + "│"`.

`wrapText(text, width)` here is a **different, simpler** function from `mcp-panel.ts`'s: it returns
`[text]` unchanged when `width <= 8`, greedy-wraps otherwise, does **not** hard-split over-long words,
and returns `[""]` when it produced nothing. Both must exist in the port.

Frame:

1. `"┌" + fg(border, "─".repeat(innerW)) + "┐"` — the **corners are uncoloured** here, unlike
   `mcp-panel.ts`, where the whole border string is wrapped in `fg`.
2. `padLine(fg(title, "MCP setup"), innerW)`.
3. `wrapText(discoverySummaryLine(), contentW)`, one `padLine` each.
4. `wrapText(secondarySummaryLine(), contentW)`, each `padLine(fg(muted, line))`.
5. `padLine("")`.
6. When `notice`: tone maps to `success`/`warning`/`hint` — the `muted` tone renders with the **`hint`**
   colour — each wrapped line `padLine(fg(tone, line))`, then `padLine("")`.
7. `"├" + border + "┤"`.
8. The screen body: `renderImports` / `renderPaths` / `renderActions`.
9. `"└" + border + "┘"`.

`discoverySummaryLine`, first match wins:

- `!hasAnyConfig`: `fg(warning, onboardingState.setupCompleted ? "No MCP servers are active right now." : "No MCP config is active yet.")`
- `totalServerCount == 0 && (imports.len() > 0 || repoPrompt.executablePath.is_some())`:
  `fg(warning, "Pi found MCP-related setup options, but none are active in Pi yet.")`
- otherwise: `fg(hint, "Detected {totalServerCount} configured servers across {shared} shared and {piOwned} Pi-owned source{s}.")`,
  where `shared` counts `sources` with `kind == "shared" && serverCount > 0` and `piOwned` the same for
  `kind == "pi"`; the plural `s` is suppressed only when `shared + piOwned == 1`, and
  `configured servers` is never singularised.

`secondarySummaryLine`: computes
`hostNote = " Host discovery is {hostConfigDiscovery}; {N} host source{s} detected."` when
`hostConfigs.len() > 0` else `""`, and `conflictNote = " {N} same-name conflict{s} reported."` when
`conflicts.len() > 0` else `""`; then

- `!hasAnyConfig`: `"Create a shared .mcp.json, adopt host imports, or quick-add RepoPrompt from this screen."` + notes
- `totalServerCount == 0 && imports.len() > 0`: `"Detected {N} compatibility import source{s}. Adopt them into Pi or inspect the underlying files."` + notes
- otherwise: `"Shared MCP files are preferred. Pi-owned files are only for compatibility imports and adapter-specific overrides."` + notes

`renderActions`:

- `compact = innerW < 60`. When compact, `visibleActionRange(total)` windows to 7 rows: `total <= 7`
  gives the whole list; otherwise `half = 3`, `start = min(max(0, actionCursor - 3), max(0, total - 7))`,
  `end = min(total, start + 7)`. When not compact, **all** actions are shown.
- `start > 0` emits `padLine(fg(muted, "… {start} more above"))`.
- For each visible index: if the action is `add-known-server` **and** it is the first visible one or
  the previous action's id is not `add-known-server`, emit a heading row
  `padLine(fg(title, "Add a known server"))` first. Then
  `padLine("{cursor} {truncateToWidth(label, contentWidth(innerW) - 2)}")` with `cursor` being
  `fg(selected,"›")` on the cursor row and a space otherwise. **The action's `description` field is
  never rendered as a row** — it exists only to be read from `Action`; the preview block is what the
  user reads.
- `end < total` emits `padLine(fg(muted, "… {total - end} more below"))`.
- `padLine("")`; the preview block (§3.7), one `padLine` per line; `padLine("")`.
- Hint row: `padLine(fg(muted, compact ? "Enter select · Esc back" : "Enter selects, Esc goes back, Ctrl+C closes."))`.

The action list is **not scroll-windowed at all when `innerW >= 60`**, and the preview is never
windowed. Combined with the host's height clip (the landing-surface notes below, and MCP-368) that is
where a long list is silently cut.

##### 3.7 Previews (`getActionPreview`)

`formatPreview(lines, width)` simply `wrapText`es each line into the output.

`formatWritePreview(title, preview, intro, width)`:

1. each `intro` line, wrapped;
2. a blank line if `intro` was non-empty;
3. `"{title}: {preview.path}"`, wrapped;
4. `preview.existed ? "Existing file detected. Showing exact before/after diff." : "New file will be created. Showing exact content diff."`, wrapped;
5. a blank line;
6. `preview.diffText.split("\n")`, capped at **`maxLines = 18`**, each wrapped;
7. when the diff is longer: `"… {N} more diff line{s}"` (singular only when `N == 1`).

Per-action preview content:

- **`run-setup`**: one line — ``Run setup to adopt host-specific imports, inspect detected paths, and scaffold a minimal `.mcp.json` if needed.``
- **`adopt-imports`**: `formatWritePreview("Compatibility import write preview", previewImports(<currently selected>), intro, w)`
  with `intro = ["Detected imports: {kind} ({serverCount} servers), ...", "Selected imports are written into the Pi agent dir config as Pi-owned compatibility state."]`
- **`view-example`**: eleven literal lines — ``Example shared `.mcp.json`:``, `{`, `  "mcpServers": {`,
  `    "chrome-devtools": {`, `      "command": "npx",`, `      "args": ["-y", "chrome-devtools-mcp@1.6.0"]`,
  `    }`, `  }`, `}`, `` (blank), ``Use Scaffold project `.mcp.json` when you want a safe empty shell instead of a live example server.``
- **`show-precedence`**: `Read order (later entries win):` then the seven numbered paths verbatim —
  `0. detected host configs (opt-in lowest-precedence fallback)`, `1. ~/.config/mcp/mcp.json`,
  `2. ~/.agents/mcp.json`, `3. ~/.agents/mcp/mcp.json`, `4. <Pi agent dir>/mcp.json`, `5. .mcp.json`,
  `6. .pi/mcp.json` — then `Host discovery: {hostConfigDiscovery}. Conflicts reported: {N}.`, then
  **up to 8** conflict lines formatted `{serverName}: {sources.map(path).join(" -> ")} (winner: {winner.path})`,
  then `Pi writes compatibility imports and adapter-only overrides to Pi-owned files.`
- **`open-paths`**: `["Detected paths:", ...paths]`, or `["No config paths were detected."]`
- **`add-repoprompt`**: `previewRepoPrompt()`; `None` gives the single line
  `RepoPrompt is not available to add from this setup screen.`; otherwise
  `formatWritePreview("RepoPrompt write preview", preview, ["Executable: {executablePath ?? "not found"}", "Target: {targetPath ?? "n/a"}", "Server name: {serverName ?? "repoprompt"}"], w)`
- **`add-known-server`**: `formatWritePreview("{preset.name} write preview", previewKnownServer(preset), [preset.summary], w)`;
  a preset-less action gives `Known server preset is unavailable.`
- **`scaffold-project`**: ``formatWritePreview("Starter project `.mcp.json` write preview", previewStarterProject(), ["This writes a minimal `.mcp.json` in the current project using the shared MCP layout.", "It intentionally avoids adding a fake placeholder server that would fail on first reload."], w)``
- **`close`** and the `undefined` default: `Close the setup flow.`

Every `preview*` callback is invoked **inside `render`**, i.e. on every frame: `previewImports`,
`previewKnownServer`, `previewStarterProject` and `previewRepoPrompt` all re-read config files from
disk each time. See MCP-375 for what that costs once `refresh_ms` is non-zero.

##### 3.8 Onboarding state (`onboarding-state.ts`, in full)

File: `getAgentPath("mcp-onboarding.json")` — `<agentDir>/mcp-onboarding.json`.

```
McpOnboardingState { version: 1, sharedConfigHintShown: bool, setupCompleted: bool,
                     lastDiscoveryFingerprint: Option<String> }
```

`loadOnboardingState`: a missing file, an unparseable file, or a non-object root all give
`DEFAULT_STATE` (`{version:1, sharedConfigHintShown:false, setupCompleted:false}`) — never an error.
The two booleans are read with strict `=== true`, so any non-boolean is `false`, and
`lastDiscoveryFingerprint` is kept only when it is a `string`. `version` is **written** as `1` and
**never checked** on read.

`saveOnboardingState`: `mkdir -p` the parent, write `` `${JSON.stringify(state, null, 2)}\n` `` to
`"{path}.{pid}.tmp"`, then `renameSync` onto the real path.

`markSharedConfigHintShown(fingerprint?)` / `markSetupCompleted(fingerprint?)` are read-modify-write
through `updateOnboardingState`, setting their flag and updating `lastDiscoveryFingerprint` to the
argument when given, else preserving the existing one. There is **no file lock** — two processes
racing lose one update. Upstream accepts that; the port does too, and says so.

#### The cyrup landing surface for the panels, measured

##### What already exists and is directly usable

| need | cyrup surface | verdict |
|---|---|---|
| an interactive, focus-capturing modal | `InteractiveOverlay` — `render(width, height) -> Vec<OverlayLine>`, `handle_key(OverlayKey) -> OverlayOutcome`, `refresh_ms() -> u64`, `tick() -> bool` (`crates/cyrup-ext/src/host/overlay.rs`) | reuse as-is |
| opening it | `HostServices::open_overlay` (`crates/cyrup-ext/src/host/services.rs`); `LiveHostServices::open_overlay` (`crates/cyrup-session-svc/src/host_services.rs`) blocks the extension's task on a one-shot until teardown and returns `false` with no block when no renderer is attached — pi's `if (!ctx.hasUI)` branch as a return value | reuse as-is |
| painting it | `ExtensionOverlay` and `to_ratatui_line`/`to_ratatui_span`/`to_ratatui_color` (`crates/cyrup-tui/src/overlay.rs`) | reuse as-is |
| key routing, including `ctrl+c` | `App::handle_input` routes to the overlay z-stack before the selector and before the global keymap; `handle_overlay_key` pops on `Close` and repaints on `Redraw` **and on `Ignored`**; `to_overlay_key` maps `Char/Enter/Esc/Backspace/Delete/Tab/BackTab/arrows/Home/End/PageUp/PageDown/Insert/F(n)` plus `ctrl`/`alt`/`shift` and returns `None` for anything else (`crates/cyrup-tui/src/{app.rs,overlay.rs}`) | reuse as-is |
| an async job settled into panel state | `FleetOverlay` (`crates/cyrup-ext-subagents/src/tui/fleet_overlay.rs`): `Pending<T>`, the captured `tokio::runtime::Handle`, `spawn_action`, `drain_jobs`, and the `Closed`-receiver arm that still clears the latch | reuse the pattern |
| a result escaping a `bool`-returning open | `PermissionSystemSettingsOverlay` + `ConfigController::take_last_error` (`crates/cyrup-permission-system/src/{config_modal.rs,extension.rs}`) | reuse the pattern |
| reaching the host at all | `NativeExtension::set_host_services`, called by `ExtensionHost::load_native_with_services` **before** `init`; stash it, as `cyrup-ext-subagents` and `cyrup-permission-system` both do | mandatory (see the section's `HostServices`-stash unit) |
| notifications / status key | `HostServices::notify(&str, NotifyKind)`, `HostServices::set_status(&str, Option<&str>)` (`None` clears) | 1:1 with `ui.notify` / `ui.setStatus` |
| `ctx.hasUI` / `ctx.mode` | `HostCtx { mode: ExtMode, has_ui: bool, cwd: PathBuf }` | 1:1 |

`ExtensionOverlay::render` probes the component at the resolved box width and the **frame's** height,
then re-fits the box to the returned row count and clips with `.take(rect.height)`. One thing worth
having verified rather than assumed: `box_rect` computes `width` from the frame's width alone and only
`height` from `content_rows`, so the probe width the component is handed is **identical** to the rect
width it is painted into, at any row count. A panel that pads every row to the width it was given can
never be mis-padded.

##### What is genuinely missing, and what merely looked missing

Four things were previously filed as seam gaps. Under the native-built-in thesis — `cyrup-mcp` links
what it needs and calls the host only for host concerns — three of them dissolve and one survives:

| item | disposition |
|---|---|
| clipboard write for `ctrl+y` | **extension-owned.** `cyrup-tui`'s own `copy_to_clipboard` is a subprocess spawn (`pbcopy`, then `wl-copy`, then `xclip -selection clipboard`, `#[cfg(unix)]`, silent on failure); `arboard` is in the tree only for clipboard *image read*. A native crate can spawn the same three, return the error upstream's failure notice needs, and implement the non-unix arm the in-tree helper stubs out. No `HostServices` method, no `UiEffect`, no TUI-thread affinity. See MCP-361. |
| keybinding resolution for `panel-keys.ts` | **extension-owned.** `cyrup-config` is a direct dependency of both shipped native extension crates, exports `migrate_keybindings_config`/`KEYBINDING_IDS`, and owns `<agent_dir>/keybindings.json` — the same document the TUI reads. `cyrup-mcp` reads it, applies the same migration table, and reimplements `matchesKey` against `Key::parse`/`Key::matches` semantics locally (it must not depend on `cyrup-tui`). What it does *not* get for free is the **defaults** for the three canonical ids, which live in `cyrup-tui`'s keymap — the one residue, filed as MCP-363a. |
| opening a detected config path | **extension-owned.** A native crate spawns `open`/`cmd /c start`/`xdg-open` itself, exactly as `cyrup-ext-subagents` supervises its own children. See MCP-372. |
| overlay geometry (`width: 82` / `width: 92`) and the silent height clip | **the one genuine host addition (HA-3), and it is cosmetic.** `open_overlay` takes no options bag and `ExtensionOverlay` hardcodes `OVERLAY_WIDTH_PCT = 95`, `OVERLAY_MIN_WIDTH = 60`, `OVERLAY_MAX_HEIGHT_PCT = 85`, `OVERLAY_MARGIN = 1` — pi-subagents' numbers, adopted as *the* geometry when it was the only consumer. Both panels compute every row against the width they are handed, so they render correctly at any width; the loss is that an 82-column design is painted at 190 on a wide terminal. Height is the sharper half and it is **not** the host's to fix: the seam's own contract says returning more rows than the host can show is "normal and lossless-by-design, not an error", so the panel windows its own body or the rows are gone. See MCP-368. |

##### The shape both panels take in Rust

```
// cyrup-mcp/src/tui/panel.rs — pure, testable, no tokio, no ratatui
pub struct McpPanelModel { /* servers, cursorIndex, nameQuery, … exactly §1 */ }
impl McpPanelModel {
    pub fn new(config: &McpConfig, cache: Option<&MetadataCache>,
               provenance: &HashMap<String, ServerProvenance>,
               status: &dyn PanelStatusSource, opts: PanelOptions) -> Self;
    pub fn handle_key(&mut self, key: OverlayKey) -> PanelInputOutcome; // Ignored|Redraw|Close|Run(PanelJob)
    pub fn render(&self, width: usize) -> Vec<OverlayLine>;             // §1.11
    pub fn finish_job(&mut self, result: PanelJobResult);               // settles auth/reconnect/copy
    pub fn result(&self) -> McpPanelResult;                             // §1.8 buildResult
}

// cyrup-mcp/src/tui/panel_overlay.rs — the host adapter, mirroring fleet_overlay.rs
pub struct McpPanelOverlay {
    model: McpPanelModel,
    handle: tokio::runtime::Handle,
    job: Option<oneshot::Receiver<PanelJobResult>>,
    inactivity_deadline: std::time::Instant,
    out: Arc<Mutex<Option<McpPanelResult>>>,   // how the result escapes open_overlay(-> bool)
}
impl InteractiveOverlay for McpPanelOverlay {
    fn refresh_ms(&self) -> u64 { 250 }
    fn tick(&mut self) -> bool { self.drain_job() || self.check_inactivity() }
    …
}
```

The setup panel takes the same two-file shape (`setup_panel.rs` + `setup_overlay.rs`). Keep
`cyrup-mcp` free of both `ratatui` and `cyrup-tui`: these panels emit `cyrup_ext::OverlayLine`, not
`ratatui::text::Line`. Note the whole `host` module is `#[cfg(feature = "wasm-host")]`, so
`cyrup-mcp`'s host-facing code and its Cargo feature must be gated the same way.

#### Port units — the panels (MCP-350…MCP-380, less MCP-373)

Every `cyrup` line is `absent` unless a concrete landing spot was read; `crates/cyrup-mcp` does not
exist yet, so "absent" is the norm and the seam citations carry the information. `MCP-373`
(`glimpse-ui.ts`) is recorded with the commands half: it is Cut 2, MCP Apps, and no panel touches it.

**MCP-350 — Tracker: the section-08 interactive surface** · n/a · — · `tracker`
**upstream** — `mcp-panel.ts`, `mcp-setup-panel.ts`, `panel-keys.ts`, `commands.ts`, `prompts.ts` —
about 2,700 lines, the adapter's entire interactive surface.
**behavior** — this row proposes no schedulable work of its own. It exists so the section's two
cross-cutting mechanism decisions have somewhere to live and are not rediscovered per unit: **(a)
poll-repaint replaces push-repaint** — upstream repaints on an explicit `requestRender()`, while
`InteractiveOverlay` is polled on its `refresh_ms` cadence, so every unit that would have called
`requestRender` instead mutates state and lets the next poll pick it up; and **(b) the overlay's
`render(w, h) -> Vec<OverlayLine>` / `handle_key(OverlayKey) -> OverlayOutcome` pair is the whole
substrate** — there is no partial redraw, no cursor addressing and no direct terminal write, so any
upstream behaviour that depends on those is re-expressed as full-frame state.
**cyrup** — `HostServices::open_overlay` with `InteractiveOverlay`
(`crates/cyrup-ext/src/host/overlay.rs`), following `FleetOverlay` and
`PermissionSystemSettingsOverlay`.
**verify** — none; a tracker is excluded from every count, per this directory's convention. It
escalates into the counted set only if the poll/push difference is found to cost observable behaviour,
in which case that becomes its own unit.

**MCP-350a — Stash the `HostServices` handle so panels and commands can reach the host** · high · S · `extension-owned`
**upstream** — no direct analogue: pi hands `ctx` to every entry point, so the adapter's panels and
its `/mcp` arms both reach the host through the same ambient object.
**behavior** — a panel must be able to open an overlay, notify, prompt for OAuth and read the theme;
a `/mcp` arm must be able to do the same. Both are reached from paths that do not carry a `HostCtx`.
**cyrup** — `NativeExtension::set_host_services` is late-bound by
`ExtensionHost::load_native_with_services` and is the **only** durable host handle a native extension
gets — `HostCtx` is per-dispatch and does not reach a panel's `handle_key`. The extension therefore
stores the `Arc<dyn HostServices>` at `set_host_services` time (behind the same interior-mutability
pattern `cyrup-ext-subagents` uses) and both the panel constructors and the `/mcp` dispatch read it
from there. This is `extension-owned` — no host change — but it is a **prerequisite for every unit in
this section**, which is scheduling information rather than severity. Note the two verbs that look
usable and are not: `LiveHostServices` never overrides `is_run_cancelled` or `tools_expanded`, so both
return their trait default of `false` on every tier in production.
**verify** — unit: construct the extension, call `set_host_services`, then drive a panel action that
notifies, and assert the notification reached the injected test double.

**MCP-351 — `McpPanel`'s construction from config plus validated cache** · high · M · `hand-written`
**upstream** — `mcp-panel.ts`'s `McpPanel` constructor: `ServerState`/`ToolState` per configured
server in config insertion order, skipping non-authenticatable servers under `authOnly`, treating an
invalid cache entry as absent, resolving the tri-state `directTools` filter, and synthesising a
`read_<name>` tool per exposed resource. §1.
**behavior** — opening `/mcp` lists every configured server in config order with its cached tool
count, `(not cached)` where the hash no longer matches, and pre-ticked direct tools reflecting the
on-disk `directTools` setting. A wrong pre-tick is a wrong write the moment the user saves.
**cyrup** — absent. The nearest analogue is `resolve_mcp_direct_tool_names` and its injectable core
`resolve_mcp_direct_tool_names_in` (`crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs`), which
resolve direct-tool *names* from the same cache and are `pub` across the crate boundary, so a
conformance test against them is feasible; they use a 3-variant `ToolPrefix` and a different resource
prefix — see MCP-370. Use `indexmap::IndexMap` or `Vec<(String, ServerEntry)>` for `mcpServers`,
never a `BTreeMap`: `Object.entries` order is user-visible as row order. `isServerCacheValid` /
`computeServerHash` come from the metadata-cache port. The `uiVisibility` filter keeps its
default-pass total form under Cut 2 (§1 step 5).
**verify** — unit: a fixture config of three servers (one disabled, one with a stale hash, one valid)
produces exactly the expected `ServerState` vector, including `hasCachedData == false` for the stale
one and zero tools for the disabled one.

**MCP-352 — `getOtherCurrentCandidates` and the include/exclude engine it feeds** · high · M · `hand-written`
**upstream** — `mcp-panel.ts`'s `getOtherCurrentCandidates` plus `types.ts`'s `getToolNameCandidates`,
`matchesToolSelector`, `matchesToolPattern`, `globToRegExp`, `isToolIncluded`, `isToolExcluded`,
`isToolAllowed`. §1.2.
**behavior** — `excludeTools: ["foo_bar"]` on server A hides A's `bar`, and does **not** hide server
B's tool that happens to have the same legacy spelling — nor A's own sibling tool literally named
`foo_bar`.
**cyrup** — partially analogous and much narrower: `is_tool_excluded`
(`crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs`) builds four candidates, normalises `-`→`_`,
and has no cross-server set, no `includeTools` and no globs. Port: `HashSet<String>`; `globToRegExp`
becomes a hand-built `regex::Regex` escaping exactly ``.+^${}()|[]\`` with `*`→`.*`, `?`→`.`, anchored
— not a glob crate, whose escape set differs — gated behind the same
`pattern.contains('*') || pattern.contains('?')` test, or a pattern containing a regex metacharacter
starts matching differently. All three arms of `matchesToolSelector` must survive, including the
unconditional current-form hit and the no-set fallback.
**verify** — unit: the candidate matrix (5 current forms, 13 further legacy expressions, 18 total,
across 4 prefix modes) as a table test; a two-server fixture proving cross-server suppression **and**
a single-server fixture proving sibling-tool suppression.

**MCP-353 — `rebuildVisibleItems`: the flattened list plus the filter state machine** · high · M · `hand-written`
**upstream** — `mcp-panel.ts`'s `rebuildVisibleItems`. §1.4.
**behavior** — typing `atlas` shows the `atlassian` header **and all of its tools**; a query matching
no tool of a server hides that server's header entirely; the cursor is clamped, never reset.
**cyrup** — absent. Straight port: compute the set of `serverIndex`es owning at least one `Tool` item,
then `retain`. Follow every call with `cursor = min(cursor, len.saturating_sub(1))`.
**verify** — unit: a 3-server × 4-tool fixture asserting the exact `visibleItems` vector for `""`, a
tool-name query, a server-name query and a no-match query, and the same four under `authOnly`.

**MCP-354 — `fuzzyScore`** · medium · S · `hand-written`
**upstream** — `mcp-panel.ts`'s `fuzzyScore`. §1.3.
**behavior** — search ranks substring matches above scattered ones and refuses non-subsequences; only
`>0`/`==0` and the one `*0.6` comparison are consumed, but the weighting is meaningless against a
different curve.
**cyrup** — absent for this purpose. `crates/cyrup-tui/src/autocomplete` has its own fuzzy scorer (pi
`fuzzy.ts`), a **different** algorithm that must not be substituted. `f64` arithmetic over `char`
sequences of the lowercased strings; the `to_lowercase()` vs `toLowerCase()` divergence for
locale-sensitive characters is recorded and accepted.
**verify** — unit: a table of (query, text, expected score) covering substring, subsequence-with-runs
and non-subsequence.

**MCP-355 — The panel's top-level key dispatch, in order** · critical · M · `hand-written`
**upstream** — `mcp-panel.ts`'s `handleInput`: the 14-step ordered dispatch of §1.5, the per-keystroke
notice reset that preserves an in-flight auth notice, `?` shadowing the printable catch-all, and
`space` shadowing a literal space.
**behavior** — every key does what the hint bar promises, and the order decides the edge cases:
`ctrl+s` saves from inside description search but **not** from inside the discard modal. Mis-order it
and the panel either persists changes the user was being asked about or silently drops them — clause 1
and clause 2 in one bug.
**cyrup** — `App::handle_input` routes every key to the topmost overlay before the selector and before
the global keymap, and `to_overlay_key` (`crates/cyrup-tui/src/overlay.rs`) maps them; nothing
MCP-specific exists. Port as a `match` on `OverlayKey`: `matchesKey(data, "ctrl+c")` becomes
`OverlayKeyCode::Char('c')` with `ctrl`; `"space"` becomes `Char(' ')`; the printable test
`data.len() == 1 && data.charCodeAt(0) >= 32` becomes `OverlayKeyCode::Char(c)` with no `ctrl`/`alt`
and `!c.is_control()`. One divergence to record and not re-derive later: cyrup's host intercepts
`ctrl+shift+d` for `/debug` *before* the overlay; upstream's panel would have ignored it anyway (it is
not printable), so there is no behavioural cost.
**verify** — unit: replay the rendering test's exact key sequence (`Enter`, `Down`, `Enter`, `Esc`,
assert `Keep & Close` is in the frame, `Enter`) and assert `cancelled == false` and
`changes["atlassian"] == ["search\u{7}issues"]`. Not done until the same sequence has been typed into
a real terminal.

**MCP-356 — The description-search modal** · medium · S · `hand-written`
**upstream** — `mcp-panel.ts`'s desc-search branch of `handleInput` and the `desc:` search row. §1.5.1.
**behavior** — `?` switches the search row to `desc:` and filters tools by description text; leaving
always clears the query, so it is never sticky; `ctrl+c` and save still work inside it.
**cyrup** — absent. A `desc_search_active: bool` + `desc_query: String` pair checked as step 4 of
MCP-355's `match`.
**verify** — unit: `?`, three chars, assert the filtered `visibleItems` and the `desc:` row; `Esc`,
assert the query is empty and the full list is back.

**MCP-357 — The discard-confirmation modal** · high · S · `hand-written`
**upstream** — `mcp-panel.ts`'s escape branch plus `handleDiscardConfirmInput`. §1.6.
**behavior** — a user who edits toggles and presses Esc is asked before losing them, and the safe
option (Keep & Close, `discardSelected = 1`) is preselected.
**cyrup** — absent. Straight port. The modal branch **must** be step 1 of the dispatch, above `ctrl+c`
and `save`, or `ctrl+s` saves from inside the prompt.
**verify** — unit: dirty the model, `Esc`, assert `confirmingDiscard` and `discardSelected == 1`;
`Tab`, `Enter`, assert `cancelled == true`; repeat without `Tab` and assert the changes survive; and
assert `ctrl+s` is ignored while the modal is up.

**MCP-358 — Toggling, dirty tracking and the tri-state `buildResult`** · critical · S · `hand-written`
**upstream** — `mcp-panel.ts`'s `toggleItem`, `updateDirty`, `buildResult` and the three import-notice
sites. §1.8.
**behavior** — pressing space on a half-toggled server turns everything on; saving writes
`directTools: true` when all tools are direct, `false` when none are, and an explicit array otherwise
— and never touches a server the user did not change.
**cyrup** — absent. The tri-state is `enum DirectToolsChange { All, None, Named(Vec<String>) }`
serialising to `true` / `false` / an array. Emitting `Named` where upstream emits `All` silently
rewrites a user's `directTools: true` into a brittle name list that stops matching on the next server
update — that is the failure this shape defends against, and why this is `critical`.
**verify** — unit: for a 3-tool server, assert `All` after toggling all three, `None` after toggling
all off, `Named(["a","c"])` for a partial selection, and absence from the map when the net change is
zero; plus the empty-tool-list server that toggles to `false` and changes nothing.

**MCP-359 — In-panel OAuth (`authenticateServer`) on the sync overlay seam** · high · M · `hand-written` + `host-verb`
**upstream** — `mcp-panel.ts`'s `authenticateServer`: the single-flight `authInFlight` latch, the four
exact notice strings, the status refresh from the callback in all three settle arms, and the automatic
`reconnectServer(server, {afterAuth:true})` chain on success. §1.7.
**behavior** — `ctrl+a`, or Enter on a `needs auth` row, shows `authenticating` beside the server, then
a result notice, then reconnects — while the panel stays open and navigable.
**cyrup** — the pattern is shipped: `FleetOverlay::spawn_action` / `drain_jobs`
(`crates/cyrup-ext-subagents/src/tui/fleet_overlay.rs`). `handle_key` returns
`PanelInputOutcome::Run(PanelJob::Authenticate(name))`; the overlay spawns onto the captured
`tokio::runtime::Handle` and holds a `oneshot::Receiver`; `tick` `try_recv`s and calls
`model.finish_job(...)`. `TryRecvError::Closed` **must** still clear `authInFlight`, or every later
auth is silently refused — the exact hazard `FleetOverlay`'s drain documents. `requestRender()` has no
analogue: the repaint is `tick` returning `true`, within one `refresh_ms` (the framing's consequence 1).
**verify** — unit: a scripted `PanelJobResult` through `finish_job` asserts each of the four notice
strings and that the latch clears on a dropped sender. Live: a real terminal with a stub
authenticator — the `authenticating` label appears within one tick and clears within two.

**MCP-360 — In-panel reconnect and `rebuildServerTools`** · high · M · `hand-written`
**upstream** — `mcp-panel.ts`'s `reconnectServer` and `rebuildServerTools`. §1.7.
**behavior** — `ctrl+r` reconnects a server in place and the new tool list appears with the user's
pending toggles intact; a tool that disappeared and returned is treated as never having been direct;
a rejection sets `failed` **directly**, not re-derived.
**cyrup** — absent. Same spawn/drain shape as MCP-359. `refreshCacheAfterReconnect` re-reads the whole
cache file — keep that; it is how the panel observes what `updateMetadataCache` just flushed. Do
**not** clamp `cursorIndex` inside `rebuildServerTools`: upstream does not, and `render` tolerates an
out-of-range cursor by skipping the row, so use `Vec::get`, never indexing. `hasCachedData = true` is
unconditional inside the connected branch, even when the cache re-read returned `None`.
**verify** — unit: pre-toggle two tools, feed a rebuilt cache entry in which one survives and one is
replaced, assert `isDirect`/`wasDirect` for both plus the new tool, and assert the panel does not
panic with the cursor past the end of the shrunken list.

**MCP-361 — `ctrl+y` copies a server's failure message** · medium · S · `extension-owned`
**upstream** — `mcp-panel.ts`'s `ctrl+y` branch and `selectedServerHasFailureMessage`: copies
`sanitizeDisplayText(failureMessage)` through pi's `copyToClipboard`, sets
`Copied error for {name} to clipboard` or `Failed to copy error for {name}: {msg}`, and shows the
`ctrl+y copy error` hint only when the cursor's server has a failure message. §1.9.
**behavior** — a user whose MCP server failed with a long docker/socket error can paste it into an
issue without re-typing it.
**cyrup** — no `HostServices` clipboard method exists, and none is needed: a native crate writes the
clipboard itself. The mechanism to copy is `cyrup-tui`'s own `copy_to_clipboard` — spawn `pbcopy`,
then `wl-copy`, then `xclip -selection clipboard`, first that spawns wins — reimplemented in
`cyrup-mcp` with two things the in-tree helper cannot give: it must **return** the spawn/write error,
because upstream's `Failed to copy error for {name}: {msg}` notice has no source otherwise, and it
must implement the non-unix arm, which in `cyrup-tui` is a silent no-op (a Windows user would
otherwise be told `Copied error … to clipboard` with an empty clipboard). Do **not** reach for
`arboard`: in this tree it is used only for clipboard image *read*, and using it here would introduce
a second, differently-behaving text-write path alongside `/copy`'s.
**verify** — unit: the hint appears and disappears with the cursor's failure state; a failing writer
produces the exact `Failed to copy error for {name}: {msg}` string; the copied text is the
**sanitized** form, asserted against a name carrying a control byte.

**MCP-362 — The 60 s inactivity auto-cancel** · medium · S · `host-verb`
**upstream** — `mcp-panel.ts`'s and `mcp-setup-panel.ts`'s `INACTIVITY_MS`/`resetInactivityTimeout`:
60 000 ms, re-armed on every keystroke; on fire the MCP panel calls
`done({cancelled:true, changes:{}})` — silently discarding unsaved toggles with no confirmation — and
the setup panel calls `done()`. §1.13.
**behavior** — a panel left open for a minute closes itself rather than holding the terminal's focus
indefinitely.
**cyrup** — the seam offers `InteractiveOverlay::refresh_ms` + `tick`; there is no timer inside an
extension. An `Instant` deadline refreshed in `handle_key` and compared in `tick`, returning
`OverlayOutcome::Close` when passed. **Mechanism difference and its residue:** upstream's `setTimeout`
fires at exactly 60 s; a polled deadline fires within one cadence, so the panel lives up to 250 ms
longer, and at `refresh_ms() == 0` it would never auto-cancel at all. Filed as residue, not blessed.
`cleanup()`/`dispose()` map to `Drop`, which `ExtensionOverlay`'s own `Drop` already guarantees runs on
every teardown path.
**verify** — unit: a model with an injected clock closes after 60 s of no keys and does not close when
a key lands at 59 s. Live: leave the panel open for a minute in a real terminal and watch it close.

**MCP-363 — `panel-keys.ts`: resolve the three canonical ids and `mcp.panel.save`** · high · M · `extension-owned` + `hand-written`
**upstream** — `panel-keys.ts` in full: `tui.select.up`/`down`/`confirm` through the host's
`KeybindingsManager.matches`, and the **adapter-defined** id `mcp.panel.save` through
`getUserBindings()["mcp.panel.save"]` with the three-way present-but-empty / present / absent
semantics of §2. Both panels use it.
**behavior** — a user who rebinds `tui.select.up` to `ctrl+p` navigates both MCP panels with `ctrl+p`;
a user who sets `"mcp.panel.save": []` disables saving entirely and the `save` hint disappears from
the hint bar.
**cyrup** — no keybinding accessor exists on `HostServices`, and the panel does not need one.
`cyrup-config` is a direct dependency of both shipped native extension crates and owns
`<agent_dir>/keybindings.json` plus `migrate_keybindings_config` (the legacy-id rename table) and
`KEYBINDING_IDS`; `cyrup-mcp` reads that document itself, which is also the only way to see
`mcp.panel.save`, an id deliberately absent from the known table. Key-spec matching is reimplemented
inside `cyrup-mcp` against `Key::parse`/`Key::matches` semantics (`crates/cyrup-tui/src/keymap.rs`),
because `cyrup-mcp` must not depend on `cyrup-tui`. The residue is the **defaults** for the three
canonical ids, which live in `cyrup-tui`'s `SelectAction`: `cyrup-mcp` carries its own copy
(`up`/`down`/`return`, i.e. upstream's no-manager fallbacks) and they drift if `cyrup-tui`'s change —
MCP-363a is the decision about closing that.
**verify** — unit: the three `mcp.panel.save` cases (absent / `["ctrl+p"]` / `[]`) each produce the
right `save()` predicate and `saveLabel()`, and the hint bar reflects all three; a `keybindings.json`
rebinding `tui.select.up` to `ctrl+p` moves the cursor on `ctrl+p`.

**MCP-363a — Where the canonical select-key defaults live** · medium · S · `open-decision`
**upstream** — `panel-keys.ts`'s two questions to a host-supplied manager: `matches(data, id)` for the
three canonical ids, and `getUserBindings()` for the raw map. Upstream has one source of truth because
the manager owns both the defaults and the user document.
**behavior** — decides whether a user who never rebinds anything navigates the MCP panels with the
same keys as the rest of the TUI, permanently, or only until `cyrup-tui`'s defaults change.
**cyrup** — the resolved `action id → keys` map **already crosses into `cyrup-ext`**:
`ExtensionRegistry::resolve_shortcuts` takes `&[(String, Vec<String>)]` and `ExtensionHost::resolve_shortcuts`
is its public front door — but it consumes the argument to build a `key → owner` map for the
editor-global dispatch, stores nothing, and has no production caller supplying the map today. Three
options: **(a)** `cyrup-mcp` keeps its own default copy (MCP-363's shape) and a test asserts it equals
`cyrup-tui`'s — cheapest, one test, no seam change; **(b)** the host stores the resolved map once and
`HostServices` grows `keybindings(&self) -> Vec<(String, Vec<String>)>`, with `resolve_shortcuts`
becoming a consumer of the stored copy — one map in the tree, three crates touched; **(c)** promote the
canonical select defaults into `cyrup-config` beside `KEYBINDING_IDS`, where both readers already look.
Recommendation: **(a)** now, **(c)** when a second extension needs the same answer.
**verify** — unit: a cross-crate test asserting `cyrup-mcp`'s default select keys equal `cyrup-tui`'s
`SelectAction` defaults, which fails the day either side moves.

**MCP-364 — The terminal-injection sanitizers** · critical · M · `hand-written`
**upstream** — `mcp-panel.ts`'s `sanitizeDisplayText` and `sanitizeRowContent` over `utils.ts`'s
`stripOscSequences` and `sanitizeTerminalText`. §1.10.
**behavior** — a malicious MCP server whose tool is named `search\u{7}issues`, or whose description
embeds `OSC 8` hyperlinks and SGR codes, cannot repaint the terminal, hide text, plant a clickable
link, or break a row onto multiple lines. Without this the frame shows content the panel did not
author, which is exactly clause 2 — silent wrong output — with a hostile author.
**cyrup** — absent for this purpose. `visible_width`'s `strip_ansi_sequences` helper
(`crates/cyrup-intercom/src/ui/mod.rs`) is escape-stripping for *measurement*, not this sanitizer, and
lives in a crate `cyrup-mcp` should not depend on. Hand-write a scanner, not a regex: the OSC rule
("consume to `BEL`/`ST`/`ESC \`, or to end of input if none arrives") depends on the loop for exactly
that. The CSI/two-byte pattern `(?:\x1b\[[0-?]*[ -/]*[@-~]|\x1b[@-Z\\-_])` maps to a byte-class walk;
control detection is `c.is_control() || ('\u{7f}'..='\u{9f}').contains(&c)`.
**verify** — unit: port all four sanitation assertions from `__tests__/mcp-panel-rendering.test.ts`
verbatim — the `\u{7}`-in-name case, the SGR/OSC/`\t`/`\0`-in-description case, the notice-line OSC
case, and the **unterminated** OSC case that must not leak `https://secret.invalid/truncated` — plus
the whole-frame invariant that no emitted line matches any control character.

**MCP-365 — `estimateTokens` and the footer statistics** · low · S · `hand-written`
**upstream** — `mcp-panel.ts`'s `estimateTokens`, the per-server `~N` in `renderServerRow`, and the
`{N} direct  ~{T} tokens` / `no direct tools` status line. §1.1, §1.11 step 8.
**behavior** — the user sees roughly what enabling a set of direct tools costs in context before
saving.
**cyrup** — absent. `serde_json::to_string` over a **key-order-preserving** value
(`serde_json/preserve_order`); a `BTreeMap`-backed value sorts keys and changes the byte count, hence
every estimate. `toLocaleString()` becomes explicit `,` thousands grouping — do not reach for a locale
crate to reproduce a Node default.
**verify** — unit: a tool with a known schema produces the exact integer JS produces, with the
expected value computed from the literal formula in the test rather than a golden file.

**MCP-366 — The panel frame layout** · medium · L · `hand-written`
**upstream** — `mcp-panel.ts`'s `render`: title centring, the three-form search row, notice rows, the
12-row viewport and its `startIdx` formula, the inline failure-message block on the cursor row, the
10-dot rainbow progress with its `n/total` counter, the import/auth notice rows, the status line
(discard buttons / `authOnly` hint / direct-tool stats) and the greedy hint-bar wrap. §1.11.
**behavior** — the panel is a framed box whose visible window follows the cursor, whose progress
indicator says how far down a long list the user is, and whose hint bar never overflows the frame.
**cyrup** — the host side exists and is reused unchanged (`ExtensionOverlay::render`); the box glyphs,
`Clear` and cell painting are the host's and are the only genuinely substrate-excluded part. The panel
emits `Vec<OverlayLine>` directly. `truncateToWidth(text, w, "…", /*pad*/ true)` must be ported **with
the pad flag** — pi's signature is `truncateToWidth(text, maxWidth, ellipsis = "...", pad = false)`
(`pi/packages/tui/src/utils.ts` @v0.84.1) and `pad` right-pads to exactly `maxWidth`, including the
empty-text arm returning `" ".repeat(maxWidth)`. `truncate_to_width` (`crates/cyrup-intercom/src/ui/mod.rs`)
has the pending-ANSI algorithm but no `ellipsis` and no `pad` — a starting point, not a drop-in.
`visibleWidth` needs `unicode-width` (in the tree for `cyrup-intercom` but **not** in
`[workspace.dependencies]`; promoting it is part of this item) plus grapheme clustering. One measured
divergence to carry into any shared helper: pi's `visibleWidth` normalises `\t` to **three spaces**
before measuring, which the in-tree port does not — unreachable from these call sites, since every
string reaching `row()` has had its C0 controls collapsed, but a shared helper must not lose it for
other callers.
**verify** — unit: golden-frame tests at widths 60/82/120 for the empty, short-list, long-list
(`total > 12`), discard-modal and `authOnly` states, asserting every emitted row is exactly `width`
columns wide. **Not done until run in a real terminal** at more than one width: a `TestBackend` frame
cannot show a box the host paints one column narrower than the panel assumed.

**MCP-367 — The row renderers, status labels and word wrap** · medium · M · `hand-written`
**upstream** — `mcp-panel.ts`'s `renderServerRow` (including the `(not cached)` branch and the
`"import"` vs `"external"` label fallbacks), `renderConnectionStatus`'s eight-way first-match ladder,
`renderToolRow`'s `prefixLen = 7 + width(name)` / `maxDescLen = innerW - prefixLen - 8` / `> 5`
description budget, and `wrapText`'s greedy wrap with hard-split. §1.12.
**behavior** — each row shows the right icon, the right counts and the right status word, and a long
failure message wraps under its server rather than being truncated: the rendering test asserts every
word of an 84-character docker error appears, across more than one line, with no `…`.
**cyrup** — absent. Straight port into `OverlaySpan` values per §1.11's theme table. `wrapText` here is
a **different function** from `mcp-setup-panel.ts`'s, which does not hard-split; both must exist.
**verify** — unit: port `createFailedPanel` from `__tests__/mcp-panel-rendering.test.ts` and both of
its assertions (the wrapping case and the not-failed negative), plus one row at each of the eight
status-ladder conditions.

**MCP-368 — Overlay geometry: the requested column counts, and the silent height clip (HA-3)** · low · M · `host-addition` + `hand-written`
**upstream** — `commands.ts`'s three `ctx.ui.custom` call sites: `{anchor:"center", width:92}` for the
setup panel and `{anchor:"center", width:82}` for both MCP-panel entry points. The MCP panel's whole
layout — `MAX_VISIBLE = 12`, the `prefixLen + 8` description budget, the hint wrap at `innerW - 2` —
is designed around 82.
**behavior** — at any terminal size the MCP panel is 82 columns and the setup panel 92, centred.
**cyrup** — this is the section's one genuine host addition, and the audit is right that it is
**cosmetic**: `InteractiveOverlay` carries no geometry method, `open_overlay` takes no options bag, and
`ExtensionOverlay` (`crates/cyrup-tui/src/overlay.rs`) hardcodes `OVERLAY_WIDTH_PCT = 95`,
`OVERLAY_MIN_WIDTH = 60`, `OVERLAY_MAX_HEIGHT_PCT = 85`, `OVERLAY_MARGIN = 1` — pi-subagents' numbers,
adopted as *the* geometry when it was the only consumer (its own doc says so). `box_rect` resolves
them, the panel is handed exactly the width it will be painted at, and both panels render correctly at
any width; the loss is an 82-column design painted at 190. Options: **(a)** add
`OverlayOptions { anchor, width, min_width, max_height, margin }` to `open_overlay`, plumbed through
`OverlayRequest` into `ExtensionOverlay` with the current constants as `Default`, so `FleetOverlay` is
untouched — the literal mechanism, since upstream *has* an options bag; **(b)** render at whatever
width is given; **(c)** clamp inside the panel (`width.min(82)`) and pad, which leaves the box
mis-centred because the host centres the 95% rect, not the content. Recommendation: **(a)**.
   **The height half is not the host's and does not wait on (a).** `render` clips with
`.take(rect.height)` and the seam's contract states that returning more rows than the host can show is
"normal and lossless-by-design, not an error". The setup panel's preview block is unbounded (§3.6), so
on a short terminal its hint row and bottom border simply vanish. The panel must window its own body:
that work belongs to MCP-366 (the MCP panel already windows to 12 rows) and MCP-377 (the setup panel,
whose action list is unwindowed above `innerW >= 60` and whose preview is never windowed).
**verify** — live: open both panels in a real terminal at 100 and at 200 columns and assert the box
width the user sees. Unit: a setup-panel frame with a 40-line preview at a 20-row terminal must still
emit the hint row inside the first 20 lines.

**MCP-369 — `McpPanelResult` escaping an `open_overlay` that returns only `bool`** · critical · S · `host-verb`
**upstream** — `createMcpPanel(..., done: (result: McpPanelResult) => void, ...)` and `openMcpPanel`'s
consumption of it: when not cancelled and non-empty, `writeDirectToolsConfig` →
`onDirectToolsConfigChanged` → `notify("Direct tools updated for this session.")`.
**behavior** — closing the panel with `ctrl+s` persists the user's direct-tool selection; closing with
Esc, `ctrl+c` or the inactivity timer does not. Losing the result silently discards every toggle the
user made — clause 1.
**cyrup** — `HostServices::open_overlay` reports only whether a host took the overlay, and
`LiveHostServices::open_overlay` blocks until teardown and returns `true`. The shipped pattern is
`PermissionSystemSettingsOverlay`: `open_overlay` consumes the box, so the outcome is read off an
`Arc`-shared object the overlay wrote through — there, `ConfigController::take_last_error` after the
call returns. Here: construct an `Arc<Mutex<Option<McpPanelResult>>>` in `commands.rs`, clone it into
the overlay, write it on the `Close` path, read it after `open_overlay` returns. Do **not** add a
result type to the seam — the block-until-closed contract already gives the happens-before, and a
typed return would have to be `Value`-erased. A `false` return is pi's `if (!ctx.hasUI)` branch: fall
back to `showStatus`, never an error.
**verify** — unit: a stub `HostServices` that drives the overlay to `Close` and asserts the caller
observes the same `McpPanelResult` the model built, plus a `false`-returning stub that takes the
headless branch. Live: toggle two tools in a real terminal, save, and assert the config file changed.

**MCP-370 — Tool/resource/prompt name formatting versus the in-tree consumer** · critical · M · `open-decision`
**upstream** — `types.ts`'s `sanitizeServerPrefix` (**preserves hyphens**; hex-escapes anything outside
`[A-Za-z0-9_-]` as `_{hex}_`), `ToolPrefix` with **four** modes (`server|none|short|mcp`),
`getServerPrefix` (`"mcp"` mode gives `mcp__{sanitized}`), `getToolNameCandidates` (**18** expressions,
not 17), and `resource-tools.ts`'s `resourceNameToToolName`; resource tools are **`read_<name>`** at
all thirteen upstream sites.
**behavior** — the name a direct tool is registered under, the name `excludeTools` must match, and the
name a permission rule targets are all this function's output. A mismatch means the panel lists a tool
the model cannot call, or a permission rule that silently fails to bind.
**cyrup** — `crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs` disagrees on four points:
`get_server_prefix` replaces `-` with `_` where upstream preserves it; `ToolPrefix` has three variants
and `get_tool_prefix` folds every unknown value — `"mcp"` included — into `Server`; resource tools are
`get_<name>`, not `read_<name>`; and `is_tool_excluded` builds 4 candidates against upstream's 18, with
no `includeTools` and no globs. That file is the **only** `ToolPrefix` in the tree, so upstream's
`"mcp"` mode has no representation anywhere in cyrup. Options: **(a)** `cyrup-mcp` implements upstream
exactly and `mcp_direct_tools.rs` is upgraded in the same change — the only outcome that leaves the
tree self-consistent *and* upstream-faithful, and it edits a file outside `cyrup-mcp`; **(b)**
`cyrup-mcp` matches the in-tree consumer, silently renaming every hyphenated server's tools and
dropping `toolPrefix: "mcp"`; **(c)** ship both and translate — no. Recommendation: **(a)**, together
with the cache-hash and config-source reconciliations, as one change. `critical` because **(b)** is the
default outcome of not deciding, and its failure mode is a tool the panel lists, the config names, and
the model cannot call.
**verify** — unit: one shared table of `(server, tool, mode) -> name` covering hyphens, non-ASCII, the
`-mcp` suffix and all four modes, asserted by **both** `cyrup-mcp` and `cyrup-ext-subagents` so the two
cannot drift again. Conformance: a fixture cache written by `cyrup-mcp` resolves to the same names
through `resolve_mcp_direct_tool_names`, using its hermetic `..._in(…, &McpDirs)` variant for the
fixture dirs.

**MCP-371 — `McpSetupPanel`'s screen model and dynamic action list** · medium · M · `hand-written`
**upstream** — `mcp-setup-panel.ts`'s constructor, `getActions`, `getDetectedPaths` and `handleInput`:
four screens, an action list rebuilt on every call whose membership depends on `screen`,
`discovery.imports`, the presence of a `shared-project` source, `getDetectedPaths()` and the RepoPrompt
five-field guard; the ordered dispatch of §3.2, including `escape` returning to
`hasAnyConfig ? "setup" : "empty"` rather than to the previous screen, and `busy` being checked
**after** `ctrl+c`/`escape`.
**behavior** — `/mcp setup`, or `/mcp` with zero servers, opens a menu whose entries reflect what is
actually on this machine, and Esc always gets the user out.
**cyrup** — absent. `fn actions(&self) -> Vec<Action>` rebuilt per call, exactly as upstream — do
**not** cache it into a field, because `run-setup` mutates `screen` and therefore the list's length,
and the cursor index is interpreted against the *current* list. **Hazard to record:** `escape` during
`busy` closes the panel while a write is still running. In JS the settled promise then writes to a dead
object harmlessly; in Rust the overlay is dropped, so the spawned task must own everything it touches
(an `Arc` to the callback bundle, never a borrow of the panel) and must discard its result when the
receiver is gone.
**verify** — unit: a discovery fixture with 0/1/N imports, with and without a project config, with and
without RepoPrompt, asserting the exact action id sequence for `screen == "empty"` and `"setup"`; plus
`escape` during `busy` closing the panel without a panic.

**MCP-372 — The imports and paths sub-screens** · medium · M · `hand-written` + `extension-owned`
**upstream** — `mcp-setup-panel.ts`'s `handleImportsInput` / `renderImports` / `applySelectedImports`
and `handlePathsInput` / `renderPaths`, plus `utils.ts`'s `openPath` over `execOpen`. §3.3, §3.4.
**behavior** — a user picks which host-specific MCP configs to adopt, sees the exact diff that would be
written before committing, and can open any detected config file.
**cyrup** — `selectedImports` is a `HashSet<ImportKind>` **pre-seeded with every detected import**, but
`applySelectedImports` takes them in `discovery.imports` order, not selection order — the set is a
membership test only and the order comes from the discovery vector. `openPath` needs no host verb: a
native crate spawns its own children, as `cyrup-ext-subagents` does for its subagent processes. Port
the dispatch literally (`open <path>` on macOS, `cmd /c start "" <path>` on Windows, `xdg-open <path>`
elsewhere, error text `stderr || "Failed to open path (exit code {code})"`) and **not** the `opener`
crate: `execOpen`'s `$BROWSER`/`browser` override and abort support are a literal mechanism `opener`
would collapse, and this call site shares the function with `openUrl`.
**verify** — unit: toggling and confirming produces the expected `adoptImports` argument and both
notice variants; a stub spawner records the exact argv per platform; the empty-selection warning fires
without calling `adoptImports`.

**MCP-374 — `runAction`, the busy latch and the notice model** · medium · M · `hand-written`
**upstream** — `mcp-setup-panel.ts`'s `runAction` and `runBusy`: the eight-way dispatch,
`markSetupCompleted()` after **every** successful write, the three identical success-notice texts, the
muted "Review the details below…" fallback for read-only actions, and the
`Working...` → result/error → `busy = false` cycle. §3.5.
**behavior** — pressing Enter on a writing action shows `Working...`, then either
`Added X to <path>. Pi will reload after this panel closes.` or the error message, and the panel
refuses further input meanwhile.
**cyrup** — absent; same spawn/drain shape as MCP-359, with `FleetOverlay::drain_jobs` as the
precedent — including the `Closed`-receiver arm, which must still clear the latch or the panel refuses
every later action until it is closed. `runBusy` becomes "set `busy`, set the `Working...` notice,
spawn, return `Redraw`", with the settle in `tick`; the error arm maps `Error::to_string()` to the
`warning` tone.
**verify** — unit: each of the four writing actions produces the exact notice text on success and the
error's message on failure; a key pressed while `busy` is ignored except `ctrl+c`/`escape`; a dropped
sender clears `busy`.

**MCP-375 — The per-action preview builders** · medium · M · `hand-written`
**upstream** — `mcp-setup-panel.ts`'s `getActionPreview`: the nine preview bodies of §3.7, including
the eleven literal `view-example` lines, the seven numbered precedence paths, the first-eight conflict
lines, and the RepoPrompt intro's `?? "not found"` / `"n/a"` / `"repoprompt"` fallbacks.
**behavior** — the selected action's consequences are shown before it is taken; `show-precedence` is
the only place the config read order is documented to the user.
**cyrup** — absent; literal string tables. Two things to carry. **The frame-time filesystem read:**
`getActionPreview` runs inside `render`, and `renderImports` calls `previewImports` from `render` too,
so four preview functions hit the filesystem on **every frame**. Upstream is synchronous `readFileSync`
and cyrup's `render` is likewise synchronous on the run loop's task, so the literal mechanism ports
as-is — but coupled with a non-zero `refresh_ms` (the framing's consequence 1), which upstream does not have,
an *idle* panel re-reads the config four times a second where upstream re-reads it only on a keystroke.
Port the literal read; the amplification is the poll-repaint residue of MCP-362 and the framing's consequence 1, not a licence to cache
here. If a cache is added anyway (keyed on `(screen, actionCursor, selectedImports)`), the residue is
that an external edit stops showing until a keystroke — file it, do not call it accepted. **The
precedence text** names pi's paths including `.pi/mcp.json`; cyrup renames the last to
`.cyrup/mcp.json`, so that literal must be updated in step with the config port, not copied.
**verify** — unit: golden text for each of the nine previews at width 74 and at width 20.

**MCP-376 — `formatWritePreview` and `formatPreview`** · medium · S · `hand-written`
**upstream** — `mcp-setup-panel.ts`'s `formatWritePreview` and `formatPreview`: intro lines, a blank
line when intro was non-empty, `{title}: {path}`, the existed/new sentence, a blank line, the diff
capped at **18** lines, and `… {N} more diff line{s}` (singular only at 1). §3.7.
**behavior** — every write action shows a real before/after diff of the file it would touch, bounded so
it cannot push the rest of the panel off screen.
**cyrup** — absent. `ConfigWritePreview { path, existed, changed, beforeText, afterText, diffText }`
comes from the config module, which is another section's port. Use `mcp-setup-panel.ts`'s **own**
`wrapText` — the non-hard-splitting variant — so a diff line longer than the width is emitted whole and
then truncated by `padLine`, which is why a long JSON line shows as `…` rather than wrapping.
**verify** — unit: a 25-line diff at width 74 yields 18 lines plus `… 7 more diff lines`; a 19-line
diff yields 18 plus `… 1 more diff line`.

**MCP-377 — The compact-width action window** · low · S · `hand-written`
**upstream** — `mcp-setup-panel.ts`'s `COMPACT_WIDTH`/`COMPACT_ACTION_ROWS`, `renderActions` and
`visibleActionRange`: `compact = innerW < 60`; when compact, window to 7 rows with `half = 3` and the
standard clamp, emitting `… N more above` / `… N more below`; the hint row also switches to
`Enter select · Esc back`. §3.6.
**behavior** — on a narrow terminal the action list scrolls instead of overflowing, and the hint fits.
**cyrup** — absent. The compact branch **is** reachable: `OVERLAY_MIN_WIDTH = 60` means `innerW` is 58
at minimum, so any terminal narrower than about 63 columns takes it. The "Add a known server" heading
interacts with windowing: it is emitted when the preset is the first *visible* row, not only the first
row overall. This unit also owns the setup panel's half of MCP-368's height problem — above
`innerW >= 60` the list is unwindowed and the preview never is, so a long list plus a long diff pushes
the hint row and bottom border past the host's clip.
**verify** — unit: a 13-action list at `innerW = 58` with the cursor at index 9 emits `… 6 more above`,
seven rows and no `more below`. Live: a real terminal at 60 columns and at 24 rows, both panels opened.

**MCP-378 — The two summary lines** · low · S · `hand-written`
**upstream** — `mcp-setup-panel.ts`'s `discoverySummaryLine` (three branches, the first varying on
`setupCompleted`) and `secondarySummaryLine` (three branches plus the `hostNote`/`conflictNote`
suffixes). §3.6.
**behavior** — the top of the setup panel says in one sentence what was found and in a second what to
do about it, and the wording changes once setup has been completed at least once.
**cyrup** — absent; literal string tables. Three different pluralisation rules are in play: `source{s}`
keyed on `shared + piOwned == 1`, `host source{s}` on `hostConfigs.len() == 1`, `conflict{s}` on
`conflicts.len() == 1` — and `configured servers` is never singularised.
**verify** — unit: a table over `(hasAnyConfig, totalServerCount, imports.len(), repoPrompt.executablePath, setupCompleted)`
asserting the exact sentence.

**MCP-379 — `KNOWN_SERVER_PRESETS`** · medium · S · `hand-written`
**upstream** — `config.ts`'s `KNOWN_SERVER_PRESETS` and the `KnownServerPreset` interface: five presets
in order (`deepwiki`, `context7`, `notion`, `github`, `chrome-devtools`) with their exact `name`,
`summary` and `entry`; **four** carry `protocolVersion: "auto"` and two of those also `auth: "oauth"`;
`chrome-devtools` alone is a stdio `npx` entry with no `protocolVersion`. §3.1.
**behavior** — a new user adds a working MCP server from the setup panel without writing JSON.
**cyrup** — absent; a `const [KnownServerPreset; 5]`. Two details that are easy to lose: the write key
is `preset.id` while the success notice reports `preset.name`, and `previewKnownServer`/`addKnownServer`
both target the **project** `.mcp.json` via `getProjectConfigPath(cwd)`, never the global one.
`protocolVersion: "auto"` reaches rmcp's negotiation policy, which is *probable-supported, verify on
first use* — adding a preset is the first thing that exercises it.
**verify** — unit: each preset's serialised entry is byte-equal to the upstream literal; adding
`chrome-devtools` writes the key `"chrome-devtools"` and notices `Added Chrome DevTools to …`.

**MCP-380 — The onboarding-state file** · low · S · `hand-written`
**upstream** — `onboarding-state.ts` in full: `<agentDir>/mcp-onboarding.json`,
`{version: 1, sharedConfigHintShown, setupCompleted, lastDiscoveryFingerprint?}`, a total-fallback
loader, and a write-to-`"{path}.{pid}.tmp"`-then-`rename` save. §3.8.
**behavior** — the shared-config hint is shown once ever, and the setup panel's opening sentence
changes after the first completed setup. Deleting the file resets both.
**cyrup** — absent. `ConfigDirs` (`crates/cyrup-config/src/env.rs`) supplies `agent_dir`;
`crates/cyrup-permission-system/src/ext_config.rs` is the in-tree precedent for an extension owning its
own JSON file. The temp-file name is a **literal mechanism**: `"{path}.{pid}.tmp"` is what keeps two
concurrent processes from clobbering each other's temp, and `tempfile::NamedTempFile` would use a
random name instead — keep the pid suffix and the `rename`. Reproduce the loader's total fallback
(missing / unparseable / non-object all give the default, never an error) and the strict `== true`
boolean reads; `version` is written but never validated, so do not add a check upstream does not have.
There is **no file lock** and two racing processes lose one update: upstream accepts that, so record it
rather than add `fs4`.
**verify** — unit: a corrupt file loads as the default; a save produces exactly two-space-indented JSON
with a trailing newline; a concurrent-save test asserts no partial file is ever observable.

---

### Behavioural specification — slash commands and prompts

Section numbering continues the panels' specification above: §1–§3 are `mcp-panel.ts`,
`panel-keys.ts` and `mcp-setup-panel.ts`; §4 is `commands.ts` with the two registrations `index.ts`
owns, and §5 is `prompts.ts`. The units for §4 and §5 follow in their own block.

#### 4. Slash commands (`commands.ts`, `index.ts`'s two registrations)

##### 4.1 `/mcp` — registration, prologue, argument split

`pi.registerCommand("mcp", { description: "Show MCP server status", getArgumentCompletions, handler })`,
registered unconditionally at extension **load**, before any server connects.

**Argument completions** (`getArgumentCompletions(prefix)`):

```
normalized = prefix.trimStart()
if !normalized.matches(/^(\S+)\s+(.*)$/):
    return the 8 subcommand items whose `value` starts with `normalized`, or null when none match
(_, subcommand, argumentPrefix) = the match
if subcommand not in {reconnect, logout, disable, enable} || argumentPrefix is undefined || !state:
    return null
return Object.keys(state.config.mcpServers)
         .filter(|n| n.starts_with(argumentPrefix.trimStart()))
         .map(|n| { value: "{subcommand} {n}", label: n })
       or null when empty
```

The eight items, in this order, with these exact labels (em dashes):

| value | label |
|---|---|
| `reconnect` | `reconnect — Reconnect servers` |
| `tools` | `tools — List all tools` |
| `prompts` | `prompts — List all MCP prompts` |
| `setup` | `setup — Configure MCP servers` |
| `logout` | `logout — Clear server credentials` |
| `disable` | `disable — Disable a server` |
| `enable` | `enable — Enable a server` |
| `status` | `status — Show server status` |

`status` is listed but is handled by the same switch arm as the empty subcommand. The server list is
read from **live** state at completion time, and returns `null` (not `[]`) when nothing matches.

**Handler prologue**, identical in shape for `/mcp-auth`:

1. capture `commandOwner = currentOwner` and `commandReload = ctx.reload.bind(ctx)` when it is a
   function, else an async no-op;
2. build a synthetic `commandCtx` from `{hasUI, ui: hasUI ? createOwnedUi(ctx.ui, owner) : undefined,
   cwd, mode, signal: owner?.signal ?? ctx.signal}` — every field snapshotted **before the first
   await**, the UI wrapped in the owner's inert-when-stale proxy;
3. `!state && initPromise` gives `await initPromise`, `owner.throwIfInactive()`, assign; on error
   `notify("MCP initialization failed: {msg}", error)` and return;
4. still `!state` gives `notify("MCP not initialized", error)` and return.

`owner.throwIfInactive()` is then re-checked before **every** side effect in the switch, not once at
entry.

**Argument split**: `parts = args?.trim()?.split(/\s+/) ?? []`; `subcommand = parts[0] ?? ""`;
`targetServer = parts[1]`; `rest = parts.slice(1).join(" ")`. `"".split(/\s+/)` yields `[""]`, so the
no-argument case has `subcommand == ""`. **`reconnect` uses `targetServer` (parts[1] only) while
`logout`, `disable` and `enable` use `rest`** — so `/mcp logout my server` targets `"my server"` and
`/mcp reconnect a b` targets `"a"`.

##### 4.2 The eight-way switch, arm by arm

| arm | behaviour |
|---|---|
| `reconnect` | `throwIfInactive()`; `await reconnectServers(state, ctx, targetServer)`; then `if directToolsFrozen { syncToolSurface(ctx) }`; `break` |
| `tools` | `await showTools(state, ctx)`; `break` |
| `prompts` | `await showPrompts(state, ctx)`; `break` |
| `setup` | `throwIfInactive()`; `programmaticConfig` gives `notify("MCP setup is unavailable when config is supplied by createMcpAdapter().", info)` + `break`; else `openMcpSetup(state, pi, ctx, earlyConfigPath, "setup")`, and `if configChanged { throwIfInactive(); await commandReload(); return }` — an early **return**, not a break |
| `logout` | empty `rest` gives `notify("Usage: /mcp logout <server>", error)` and **return**; else `throwIfInactive()`; `await logoutServer(rest, state, ctx)`; `break` |
| `disable` / `enable` | one shared arm. `programmaticConfig` gives `notify("/mcp {sub} is unavailable when config is supplied by createMcpAdapter().", info)`; empty `rest` gives `notify("Usage: /mcp {sub} <server>", error)`; a name absent from `state.config.mcpServers` gives `notify("Server \"{name}\" not found in effective config", error)` — each of the three `break`s. Otherwise `throwIfInactive()` and `writeProjectServerDisabledOverride(earlyConfigPath, cwd, name, sub == "disable")`, then `notify(result.changed ? "{Disabled\|Enabled} server \"{name}\" in {result.path} — run /reload to apply" : "Server \"{name}\" is already {disabled\|enabled}", info)` |
| `status`, `""`, **anything unrecognised** | with UI: `throwIfInactive()`; `programmaticConfig` gives `notify("MCP status is shown from the in-memory SDK config; configuration discovery is unavailable.", info)` + `showStatus` + `break`; else `openMcpPanel(state, pi, ctx, earlyConfigPath, onDirectToolsConfigChanged)` and `if configChanged { throwIfInactive(); await commandReload(); return }`. Without UI: `showStatus` |

The `default` label shares the `status`/`""` arm, so `/mcp wibble` opens the panel rather than
erroring. `disable`/`enable` is the **only** pair that tells the user to run `/reload` themselves;
`setup` and the panel path call `reload` for them.

##### 4.3 `/mcp-auth`

`description: "Authenticate with an MCP server (OAuth)"`, and **no `getArgumentCompletions`** — an
upstream asymmetry with `/mcp`, kept. `serverName = args?.trim()`. An empty `serverName` with
`!hasUI` returns **silently**, before the init prologue. Then the same prologue. Then: empty name
with `programmaticConfig` gives `notify("Use /mcp-auth <server> to authenticate a server from the
in-memory SDK config.", info)`; empty name otherwise opens `openMcpAuthPanel`; a non-empty name runs
`authenticateServer(name, state.config, ctx, ctx.signal, state.oauthRuntime)` and, on `ok`,
`throwIfInactive()` then `reconnectServer(state, ctx, name)`.

##### 4.4 `showStatus`

Returns immediately when `!ctx.hasUI`. Builds `["MCP Server Status:", ""]` then one row per server in
`Object.keys(config.mcpServers)` order, and emits the whole block as a **single**
`ui.notify(lines.join("\n"), "info")`.

- disabled gives `⊘ {name}: disabled (run /mcp enable {name}, then /reload)` and `continue` — no tool
  suffix.
- otherwise the ladder, **first match wins**:

| condition | status | icon | `failed` |
|---|---|---|---|
| `connection?.status == "connected"` | `connected` | `✓` | — |
| `connection?.status == "needs-auth"` | `needs auth` | `⚠` | — |
| `getFailureAgeSeconds(state, name) != null` | `failed {N}s ago — {sanitizeTerminalText(reason)}`, or `failed {N}s ago` when the reason is empty | `✗` | yes |
| `metadata !== undefined` | `cached` | `○` | — |
| default | `not connected` | `○` | — |

- `toolSuffix = failed ? "" : " ({toolCount} tools{status == "cached" ? ", cached" : ""})"`, where
  `toolCount = state.toolMetadata.get(name)?.length ?? 0`. **`tools` is never singularised.**
- row = `{icon} {name}: {status}{toolSuffix}`.

An empty `mcpServers` appends `No MCP servers configured` and
`Run /mcp setup to adopt imports or scaffold a starter .mcp.json`. The failure arm is tested **before**
the metadata arm, so a failed server never reports `cached` even when its metadata is present.

##### 4.5 `showTools`

`allTools` = every `toolMetadata` entry whose server is not disabled, flat-mapped to `m.name` — the
**prefixed, registered** names, in map-iteration order. Empty gives `notify("No MCP tools available",
info)`. Otherwise `["MCP Tools:", "", ...names.map(n => "  " + n), "", "Total: {N} tools"]`, one
`info` notify. `tools` is not singularised here either: one tool reports `Total: 1 tools`.

##### 4.6 `showPrompts`

`allPrompts` = every value of `state.promptMetadata` flattened. `failedPromptServers` = the names of
connections that are `connected` **and** carry `promptDiscoveryFailed`, sorted.

Empty case: one `info` notify of ``No MCP prompts available. Prompts are discovered when servers with
the `prompts` capability connect.`` plus, when `failedPromptServers` is non-empty,
` Prompt discovery failed for: {names.join(", ")}.` — appended to the same string, leading space
included.

Otherwise `["MCP Prompts:", ""]`, then group by `serverName` (insertion order into the `Map`) and
iterate groups sorted by `a.localeCompare(b)`. Per group:

- **a header row `{serverName}:`** — unindented, no icon, plain colon, not sanitized (the name comes
  from the user's own config);
- prompts sorted **in place** by `commandName.localeCompare`;
- per prompt `args = arguments.map(a => a.required ? "<{a.name}>" : "[{a.name}]").join(" ")` and a row
  `  /{commandName}{args ? " " + args : ""}` (two-space indent);
- when `description` is non-empty, a second row of **six** spaces + the description;
- a blank line after the group.

Then `Total: {N} prompt{s}` (singular only at 1), and when `failedPromptServers` is non-empty a final
`Prompt discovery failed for: {names.join(", ")}. Cached prompt metadata may be stale.`

##### 4.7 `reconnectServer` and `reconnectServers`

`reconnectServers(state, ctx, target?)`: an unknown `target` gives `notify("Server \"{target}\" not
found in config", error)` and returns. Otherwise the target, or **every** configured server in key
order, is reconnected **sequentially** (`for … await`, not a join), then `updateStatusBar(state)`.

`reconnectServer(state, ctx, name) -> bool`:

1. missing definition: `notify("Server \"{name}\" not found in config", error)`, `false`.
2. disabled: `notify("MCP: {name} is disabled. Run /mcp enable {name}, then /reload.", warning)`, `false`.
3. `manager.close(name)`; `throwIfInactive()`; `manager.connect(name, definition, signal?)`; `throwIfInactive()`.
4. `connection.status == "needs-auth"`: `notify("MCP: {name} requires OAuth. Run /mcp-auth {name} first.", warning)`, `updateStatusBar`, `false`.
5. `prefix = settings.toolPrefix ?? "server"`; `buildToolMetadata(connection.tools, connection.resources, definition, name, prefix, config.mcpServers, state.toolMetadata)` gives `{metadata, failedTools}`; `state.toolMetadata.set(name, metadata)`.
6. when `!connection.promptDiscoveryFailed`: `promptMetadata.set(name, reconstructPromptMetadata(name, connection.prompts ?? [], prefix, definition))` and `promptMetadataLive.add(name)`. **A failed prompt discovery leaves cached prompt metadata untouched and does not raise the `live` flag** — that flag is what `createPromptCommand` uses to decide whether a missing prompt is authoritative (§5.5).
7. `connection.instructions` present gives `serverInstructions.set(name, …)`, absent gives `.delete(name)`.
8. `updateMetadataCache(state, name)`; `notifyToolMetadataUpdated(state, name, "command-reconnect")`; `markKeepAliveAfterConnect(state, name)`; `clearFailure(state, name)`.
9. `notify("MCP: Reconnected to {name} ({T} tools, {R} resources)", info)` where `T`/`R` are the **raw** `connection.tools.length` / `connection.resources.length`, *not* the filtered `metadata.length` from step 5 — so the count the user is told can exceed the number of tools actually registered. When `failedTools` is non-empty, a second `notify("MCP: {name} - {N} tools skipped", warning)` (hyphen, not em dash).
10. `updateStatusBar(state)`; `true`.

On a throw: `isAbortError(error, signal)` rethrows; otherwise `recordFailure(state, name, message)`,
`notify("MCP: Failed to reconnect to {name}: {sanitized}", error)`, `updateStatusBar`, `false`.

##### 4.8 `authenticateServer` — the interactive OAuth flow

`signal ??= ctx.signal`. `!ctx.hasUI` returns `{ok:false, message:"OAuth authentication requires an
interactive session."}` with **no** notify. Then four guards, each notifying and returning
`{ok:false, message}`:

| guard | notified text | level |
|---|---|---|
| missing definition | `Server "{name}" not found in config` | error |
| disabled | `Server "{name}" is disabled. Run /mcp enable {name}, then /reload.` | warning |
| `!supportsOAuth(definition)` | the same sentence as the returned message but **split across two lines with a `\n`** | error |
| `resolveServerUrl(definition)` falsy | `Server "{name}" has no URL configured (OAuth requires HTTP transport)` | error |

The `supportsOAuth` arm's **returned** message is
`Server "{name}" does not use OAuth authentication. Set "auth": "oauth" or omit auth for auto-detection.`
on one line, while the **notified** text is the same words broken by a newline. Two different
strings, deliberately; both port.

Then `ui.setStatus("mcp-auth", "Authenticating {name}...")`,
`authStorageOptions = getAuthStorageOptions(settings.oauthDir, cwd)`, and `authenticate(…)` with two
callbacks:

- `onAuthorizationUrl(url)`: one `info` notify of
  `Open this URL to authenticate {name}:\n\n{terminalHyperlink(url, url)}\n\nAfter approving, Pi will complete automatically if the browser can reach its localhost callback. On a remote machine, copy the full localhost URL from the browser address bar and paste it into Pi.`
- `onAuthorizationInput(url, inputSignal)`:
  `ui.confirm("Authorize {name}", "Open this link in your browser:\n{terminalHyperlink(url,url)}\n\nAfter approving access, select Yes to paste the callback URL.", {signal: inputSignal})`;
  a false answer or an aborted signal returns `undefined`; otherwise
  `ui.input("Complete {name} OAuth", "Paste the full callback URL", {signal: inputSignal})`.

`terminalHyperlink(label, url)` emits an OSC-8 hyperlink:
`ESC ] 8 ;; {sanitizeTerminalText(url)} ESC \ {sanitizeTerminalText(label)} ESC ] 8 ;; ESC \`. This is
the one place the adapter *emits* OSC while the panels *strip* it; the two are not in conflict
(notification body vs. panel row), but a port that routes notifications through the panel sanitizer
destroys the link.

Outcome: `status == "authenticated"` gives `notify("OAuth authentication successful for \"{name}\".",
info)` and `{ok:true}`; otherwise `notify("OAuth authentication failed for \"{name}\".", error)` and
`{ok:false}`. A throw with `signal.aborted` rethrows; otherwise
`notify("Failed to authenticate \"{name}\": {msg}", error)`. The `finally` clears the `mcp-auth`
status key **only when the signal is not aborted** — an aborted flow deliberately leaves it on the
footer for the teardown path.

##### 4.9 `logoutServer`

1. missing definition: `notify("Server \"{name}\" not found in config", error)`, `{ok:false}`.
2. `removeAuth(name, {authStorageOptions, signal, runtime})`; on throw, rethrow abort errors, else
   `notify("Failed to clear OAuth credentials for \"{name}\": {sanitized}", error)`, `{ok:false}`.
3. `throwIfInactive()`; `manager.close(name)`; on throw, rethrow abort errors, else
   `notify("OAuth credentials were cleared for \"{name}\", but its connection could not be closed: {sanitized}", error)`, `{ok:false}`.
4. `throwIfInactive()`; `updateStatusBar(state)`; `notify("OAuth credentials cleared for \"{name}\". Run /mcp-auth {name} to authenticate again.", info)`, `{ok:true}`.

Step 3's message is the load-bearing one: the credentials **are** gone even though the call reports
failure, and the text says so.

##### 4.10 The three panel entry points

**`openMcpSetup(state, pi, ctx, configOverridePath?, mode = "setup", {includeHostConfigs}?)`** —
`!hasUI` gives `{configChanged:false}`; `programmaticConfig` notifies
`MCP setup is unavailable when config is supplied by createMcpAdapter().` (info) and returns. Else it
computes `discovery = getMcpDiscoverySummary(configOverridePath, cwd, options)` and
`onboardingState = loadOnboardingState()` **once**, builds the ten callbacks — each of the four
writers sets the closed-over `configChanged = true`, `adoptImports` only when `result.added.length >
0` — and opens the panel at `width: 92`, resolving `{configChanged}` from the panel's `done()`.
`markSetupCompleted` persists `discovery.fingerprint`, the value captured **at open time**.

**`openMcpPanel(state, pi, ctx, configOverridePath?, onDirectToolsConfigChanged?)`** —

1. `programmaticConfig`: with UI, notify `MCP status is shown from the in-memory SDK config;
   configuration discovery is unavailable.` and run `showStatus`; return `{configChanged:false}`.
2. **zero configured servers**: delegate to `openMcpSetup(…, "empty", {includeHostConfigs:false})` and
   return its result — this is how the browser becomes the onboarding flow.
3. `cache = loadMetadataCache()`; `configPath = pi.getFlag("mcp-config") ?? configOverridePath`;
   `provenanceMap = getServerProvenance(configPath, cwd)`;
   `{lines: noticeLines, fingerprint} = buildSharedConfigNoticeLines(configPath, cwd)`.
4. open at `width: 82` with `{noticeLines, keybindings}`.
5. in `done`: when `!result.cancelled && result.changes.size > 0`, run
   `writeDirectToolsConfig(changes, provenanceMap, config)`, then
   `await onDirectToolsConfigChanged?.(changes)`, then `notify("Direct tools updated for this session.", info)`.
   A throw anywhere in that block gives `notify("Direct tools updated, but live refresh failed: {msg}", error)`
   **and sets `configChanged = true`**. So a *successful* save does not reload, while a *failed live
   refresh* does — the reload is the fallback for the in-session refresh having failed.
6. after the panel closes: `if noticeLines.length > 0 && fingerprint { markSharedConfigHintShown(fingerprint) }`.

**`openMcpAuthPanel(state, pi, ctx, configOverridePath?)`** — `!hasUI` returns; `programmaticConfig`
notifies `Use /mcp-auth <server> to authenticate a server from the in-memory SDK config.`; zero
non-disabled OAuth-capable servers notifies `No OAuth-capable MCP servers are configured.` (warning).
Otherwise the **same** panel factory at `width: 82` with `{authOnly: true, keybindings, noticeLines:
["Select an OAuth MCP server and press Enter or ctrl+a to authenticate."]}`. Always returns
`{configChanged: false}`.

##### 4.11 `buildMcpPanelCallbacks` and `buildSharedConfigNoticeLines`

`buildMcpPanelCallbacks` is a per-open closure holding `authStatusFailures: Map<string, string>` —
panel-only diagnostics, so inspecting a credential store cannot pollute real connection-failure
state. `getConnectionStatus(name)`, in order:

1. `authStatusFailures.delete(name)`
2. `isServerDisabled(definition)` gives `disabled`
3. `resolveServerUrl(definition)` **throws** (a missing `${VAR}` or an unparseable URL) gives `failed`
4. when `definition.auth == "oauth" && serverUrl && definition.oauth !== false &&
   definition.oauth?.grantType != "client_credentials"`: `inspectAuthForUrl(name, serverUrl,
   authStorageOptions)`; `unavailable` records the message in `authStatusFailures` and gives `failed`;
   `absent` or `!entry.tokens` gives `needs-auth`
5. `connection?.status == "needs-auth"` gives `needs-auth`
6. `connection?.status == "connected"` gives `connected`
7. `getFailureAgeSeconds(state, name) != null` gives `failed`
8. otherwise `idle`

`getFailureMessage(name) = authStatusFailures.get(name) ?? getFailureMessage(state, name)`.
`canAuthenticate(name) = !isServerDisabled(def) && supportsOAuth(def)`, `false` when the definition is
absent. `reconnect` is §4.7's `reconnectServer`; `authenticate` is §4.8's `authenticateServer`.
`refreshCacheAfterReconnect(name) = loadMetadataCache()?.servers?.[name] ?? null` — **the whole cache
file is re-read from disk after every reconnect**, deliberately, so the panel observes whatever
`updateMetadataCache` just flushed.

`buildSharedConfigNoticeLines(configOverridePath, cwd)`: `discovery =
getMcpStandardConfigSummary(...)`, `onboardingState = loadOnboardingState()`. When
`!discovery.hasSharedServers` **or** `onboardingState.sharedConfigHintShown`, returns `{lines: [],
fingerprint: null}`. Otherwise the two lines

```
Using standard MCP config from {sources with kind=="shared" && serverCount>0, paths joined ", "}.
Pi only writes compatibility imports and adapter-specific overrides into Pi-owned files when needed.
```

plus `discovery.fingerprint`. The hint is marked shown only **after** the panel closes, so a panel
that never opens does not consume the one-shot.

#### 5. MCP prompts (`prompts.ts`)

An MCP **prompt** is a server-provided prompt template with a name, a description and a list of
declared arguments. The adapter surfaces each one as **its own slash command**: a server named `demo`
offering `brief` becomes `/mcp__demo__brief`. Invoking it fetches the rendered prompt from the server
and pushes the result into the session as a user turn.

##### 5.1 How a server prompt becomes invocable

Two paths, and only one of them is a live one:

- **cache-time** — `resolveCachedPrompts(config)` runs at extension **load**, before any connection:
  `loadMetadataCache()`, then for each `(serverName, entry)` in `cache.servers`, skip when the server
  is absent from config or `isServerDisabled`, skip when `entry.prompts` is empty or
  `!isServerCacheValid(entry, definition)`, else
  `reconstructPromptMetadata(serverName, entry.prompts, prefix, definition)`. `prefix =
  config.settings?.toolPrefix ?? "server"`. This is a synchronous file read, so every prompt it
  returns is registrable during `init`.
- **live** — `syncPromptCommands()` re-runs `registerPromptCommands` on **every** tool-metadata
  update and once after init, for the whole session. `registeredPromptCommands: Set<string>`
  de-duplicates by `commandName`, logging
  ``MCP: prompt "{originalName}" on {serverName} skipped; /{commandName} is already registered`` and
  skipping; otherwise `pi.registerCommand(commandName, createPromptCommand(pi, () => state, spec))`.

`reconstructPromptMetadata` resolves `effectivePrefix = resolveToolPrefix(definition, prefix)`, drops
prompts with a falsy `name`, drops arguments with a falsy `name`, keeps `{name, description?,
required?}` per argument, defaults `description` to `""`, and carries `title` only when present.

##### 5.2 Command naming — `sanitizePromptName` and `formatPromptCommandName`

Both are **pure functions of their inputs** with no host dependency, no I/O and no state:

```
formatPromptCommandName(promptName, serverName, prefix)
  = "mcp__" + serverPart + "__" + sanitizePromptName(promptName)
serverPart = getServerPrefix(serverName, prefix) || sanitizeServerPrefix(serverName) || "server"
```

- `sanitizePromptName(n)`: replace runs of `[^A-Za-z0-9_-]` with `_`, strip leading and trailing
  `[_-]` runs, empty gives `"prompt"`, a leading digit gets an `_` prefix.
- `sanitizeServerPrefix(n, preserveProviderValid = true)`: keep each character matching
  `[A-Za-z0-9_-]` (or `[A-Za-z0-9]` when the flag is false), else replace it with `_{hex codepoint}_`
  — `a b` gives `a_20_b`. **Hyphens are preserved** under the default flag.
- `getServerPrefix(name, mode)`: `none` gives `""`; `short` gives
  `sanitizeServerPrefix(name.replace(/-?mcp$/i, ""))` or `"mcp"` when that is empty; `mcp` gives
  `"mcp__" + sanitizeServerPrefix(name)`; `server` gives `sanitizeServerPrefix(name)`.

Because of the `||` chain a prompt command **always carries the server name**, even under
`toolPrefix: "none"`; under `toolPrefix: "mcp"` the name doubles up as `mcp__mcp__{server}__{prompt}`.

##### 5.3 Argument parsing — `parsePromptArgs` over `tokenizeArgs`

```
tokens = []; current = ""; quote: Option<char> = None; escaped = false
for ch in input.chars():
    if escaped { current.push(ch); escaped = false; continue }
    if ch == '\\' && quote != Some('\'') { escaped = true; continue }   # backslash is literal in ''
    if let Some(q) = quote { current.push(ch); if ch == q { quote = None }; continue }
    if ch == '"' || ch == '\'' { quote = Some(ch); current.push(ch); continue }
    if ch.is_whitespace() { if !current.is_empty() { tokens.push(current); current = "" }; continue }
    current.push(ch)
if !current.is_empty() { tokens.push(current) }
```

**The quote characters stay in the token**; they are removed later by `stripQuotes`. An unterminated
quote runs to end of input. Per token, `eq = findUnquotedEquals(token)` (the index of the first `=`
outside quotes, `-1` when none): when `eq > 0` **strictly**, `key = token[..eq].trim()` and
`value = stripQuotes(token[eq+1..].trim())`, and a non-empty `key` consumes the token as
`named[key] = value`; otherwise `positional.push(stripQuotes(token))`. `stripQuotes(v)` strips exactly
one layer when `v.len() >= 2` and `v` starts and ends with the *same* quote character.

##### 5.4 Argument binding — `resolvePromptArgs`

```
args = {}; positionalIndex = 0
for argDef in metadata.arguments:                    # declaration order
    value = parsed.named[argDef.name] ?? parsed.positional[positionalIndex++]
    if value is Some(v) && v != "" { args[argDef.name] = v }
for (k, v) in parsed.named:                          # undeclared named args are preserved
    if !args.contains_key(k) { args[k] = v }
missing = declared.filter(a => a.required && args[a.name] is None or "")
if !missing.is_empty() { return Err(buildUsageMessage(metadata, missing)) }
```

Three subtleties. **`positionalIndex` advances only when the named lookup missed** — JS `??`
short-circuits before evaluating `positional[positionalIndex++]` — so a named hit consumes no
positional slot. **Undeclared named arguments are forwarded to the server**, cited upstream to the
MCP spec's allowance of arbitrary string key/values in `prompts/get` params. And the loops interact:
an explicitly empty named value for a *declared* argument (`topic=`) is rejected by loop 1's
`value !== ""` guard and then re-added by loop 2's `if (!(key in args))`, so `args["topic"] = ""`
**is** sent for a declared *optional* argument, while a declared *required* one still fails the
`missing` filter and produces the usage error.

`buildUsageMessage`: `Missing required argument{s}: {names.join(", ")}.\nUsage: /{commandName} {usage}`,
trimmed, where `usage` is every declared argument as `<name>` (required) or `[name]` (optional) joined
by spaces, and `{s}` appears only when more than one is missing.

##### 5.5 The command handler — `createPromptCommand`

`description = truncateAtWord("MCP: " + (description || title || "MCP prompt from {serverName}"), 120)`,
with a final fallback of `MCP prompt from {serverName}` when that comes out empty. `truncateAtWord`
returns the text unchanged at or under the target; otherwise it cuts at the target, and if the last
space in the cut lies beyond `target * 0.6` it cuts there instead, appending `"..."` either way.

Handler sequence — every failure is a `notify(…, "error")` when `ctx.hasUI` and a silent return
otherwise:

1. `state` is `None` gives `MCP not initialized`.
2. `liveMetadata = findLivePromptMetadata(state, serverName, originalName)` (a linear search of
   `promptMetadata[serverName]` by `originalName`). When `promptMetadataLive.has(serverName) &&
   liveMetadata is None`:
   ``MCP prompt "{originalName}" is no longer advertised by server "{serverName}". Run /mcp reconnect to refresh.``
   The `promptMetadataLive` guard is what stops a **cache-only** command from being refused before its
   server has ever been contacted.
3. `live = liveMetadata ?? metadata`; parse and resolve arguments; a resolution error gives
   `resolved.error ?? "Invalid prompt arguments"`.
4. `state.config.mcpServers` missing `serverName` gives
   ``MCP prompt "{live.originalName}" is no longer configured. Run /mcp reconnect to refresh.``
5. `lazyConnect(state, serverName, ctx.signal)`; on `false`, `needs-auth` gives
   ``MCP server "{serverName}" needs authentication. Run /mcp-auth {serverName}.`` and anything else
   ``MCP server "{serverName}" is not available. Run /mcp reconnect {serverName}.``
6. **re-check** for the prompt after connecting, with the identical `promptMetadataLive` guard and the
   identical message as step 2 — this is the check that catches a prompt that disappeared *during*
   the connect.
7. `dispatchMetadata = refreshed ?? live`; `manager.getPrompt(serverName,
   dispatchMetadata.originalName, args.is_empty() ? undefined : args, ctx.signal)`. On throw:
   ``logger.debug("MCP prompt \"{live.originalName}\" on {serverName} failed: {msg}")`` **and**
   ``notify("MCP prompt \"{live.originalName}\" failed: {msg}", error)``. The **original** name goes
   to the server, never the slash name; the *refreshed* metadata's name is dispatched while the
   *pre-connect* name is what every error quotes.
8. `text = formatPromptResult(result)`; empty gives
   ``notify("MCP prompt \"{live.originalName}\" returned no text content.", warning)``.
9. `pi.sendUserMessage(text)` — a single user turn, no options.

##### 5.6 Result flattening — `formatPromptResult` and `extractMessageText`

```
lines = []
for message in result.messages:
    text = extractMessageText(message)
    if text.is_empty() { continue }
    if message.role == "user" && result.messages.len() == 1 { lines.push(text) }
    else { lines.push("[{role}] {text}") }
return lines.join("\n\n").trim()
```

The single-user-message special case is what makes the common one-message prompt arrive verbatim;
everything else keeps inline role markers because pi offers no multi-message replay API — a
limitation cyrup shares, since `ControlOp::SendUserMessage` also takes one string.

`extractMessageText` requires `content` to be a non-null object, then switches on `type`:

| type | text |
|---|---|
| `text` | `content.text ?? ""` |
| `resource` | `[resource {uri}]\n{text}` when the resource has a string `text`, else `[resource {uri}]`; a missing `resource` gives `""` |
| `resource_link` | `[resource_link {uri ?? ""}{name ? " — {name}" : ""}]` (em dash) |
| `image` | `[image {mimeType ?? "unknown"}{data ? " (embedded)" : ""}]` |
| `audio` | `[audio {mimeType ?? "unknown"}]` |
| anything else | `""` |

`listAllPromptMetadata(state)` flattens `promptMetadata` and sorts by `commandName.localeCompare`; it
is the `/mcp prompts` helper (§4.6).

### Port units — `commands.ts` and `prompts.ts`

Ids `MCP-381`…`MCP-399` cover the two command files; the letter-suffixed ids (`MCP-385a`, `MCP-394a`,
`MCP-395a`, `MCP-397a`) are surfaces found after the id range was allocated and are filed inside it so
every id already assigned still points at the same unit. `MCP-373` is **retired as a unit**:
`glimpse-ui.ts` is MCP-UI and falls under Cut 2, recorded in *Out of scope*.

**MCP-381 — `/mcp`: registration, the owner-fenced prologue and the eight-way switch** · high · M · `hand-written`
**upstream** — `index.ts`'s `pi.registerCommand("mcp", …)`: the `getArgumentCompletions` closure, the
synthetic owner-fenced `commandCtx`, the init-await prologue, the `split(/\s+/)` argument split with
`targetServer` vs `rest`, and the eight-arm switch of §4.1–§4.2.
**behavior** — `/mcp` opens the browser panel (or prints status headlessly); `/mcp <sub> [server]`
runs the named operation; an unrecognised subcommand falls through to the panel rather than erroring;
`logout` with no argument returns instead of breaking, so nothing after it runs.
**cyrup** — `InitApi::register_command(name, CommandDescriptor { description, completions })`
(`crates/cyrup-ext/src/native.rs`, `crates/cyrup-ext/src/registry.rs`) in `init`, dispatched by
`NativeExtension::execute_command`. That returns `Result<Option<String>, ExtError>` and its contract
documents the `String` channel as Info-only, so every arm here calls `HostServices::notify` at its own
level and returns `Ok(None)` — returning both would print the message twice. `ctx.reload()` is
`HostServices::control(ControlOp::Reload)` on the stashed `Arc`; `execute_command` runs at
`CtxTier::Command`, but the deadlock guard is applied by the WASM path, so a native calls
`HostCtx::require_command_tier` itself. The owner fence maps to a `cyrup_core::CancelToken` captured
with the services handle: cyrup's `HostServices` methods are no-ops by default so the inert-proxy
trick is unnecessary, but the **generation check before each side effect** is not — it must be a real
check, not an entry-time one.
**cyrup** *(blocking-ness, not severity)* — nothing else in this section is reachable until `/mcp`
dispatches, but "the command does not exist yet" is a feature gap, not data loss, silent wrong
output, a permission bypass or a crash.
**verify** — unit: each of the eight arms plus `""` and an unknown subcommand routes to the right
handler with the right argument — `/mcp logout my server` targets `"my server"`, `/mcp reconnect a b`
targets `"a"`, `/mcp wibble` opens the panel; and `logout` with no argument performs no further work.

**MCP-382 — HA-2: `/mcp`'s dynamic argument completions have no native path, no label and no consumer** · medium · M · `host-addition`
**upstream** — `index.ts`'s `getArgumentCompletions(prefix)`: the eight labelled subcommand items
filtered by prefix, then, for `reconnect`/`logout`/`disable`/`enable`, the **live**
`Object.keys(state.config.mcpServers)` filtered by the argument prefix as `{value: "{sub} {name}",
label: name}`, or `null` when nothing matches.
**behavior** — `/mcp <TAB>` lists the subcommands with their descriptions; `/mcp reconnect <TAB>`
lists the user's actually-configured servers.
**cyrup** — **three legs, all real.** (1) `CommandDescriptor.completions` is a `Vec<String>` fixed at
`init`, and `ExtensionHost::command_completions` — the only dynamic path — is `#[cfg(feature =
"wasm-host")]` and resolves its owner through `live_for_command`, a lookup in the **live-WASM** map;
a native built-in is absent from that map, so the call errors `command \`X\` has no live owner`.
(2) There is no `NativeExtension::argument_completions` at all. (3) There are **zero** consumers of
`command_completions` in `cyrup-tui`, `cyrup-modes` or `cyrup-session-svc` — the only caller in the
tree is a `cyrup-it` test. The live TUI path is `Autocomplete::compute` → `slash_context`
(`crates/cyrup-tui/src/autocomplete.rs`), which returns `None` the moment the buffer contains
whitespace, so there is no argument-completion context to hook: `CompletionContext` is
`Slash | Path | Mention`. `SlashCommand` carries `argument_hint` and `has_arg_completion`
(`crates/cyrup-tui/src/commands.rs`) but nothing reads the latter, and
`dynamic_commands_from_catalog_gated` hardcodes both off for every dynamic row. Even the WASM path is
**shape-lossy**: it returns `Result<Vec<String>, ExtError>` (`ExtensionHost::command_completions`,
`LiveExtension::argument_completions` in `crates/cyrup-ext/src/host/live.rs`, guest side
`crates/cyrup-ext-sdk/src/api.rs`) — a bare string list with no `label`, so upstream's
`reconnect — Reconnect servers` is unexpressible on the one path that exists.
**cyrup** *(sizing)* — this is **not** a small hook. It is a `{value, label}` completion item type, a
`NativeExtension` method, a widened WIT return type, an argument-completion context in the TUI's
autocomplete engine, and a catalog field to carry `has_arg_completion` through
`dynamic_commands_from_catalog_gated` — a from-zero design across `cyrup-ext`, `cyrup-ext-sdk`,
`cyrup-session-svc` and `cyrup-tui`. It is the **same host addition section 01 files as MCP-041**
(HA-2), with a second consumer already waiting in
`crates/cyrup-ext-subagents/src/registration/slash_commands.rs`, whose own comment states that a
static list "cannot express per-invocation dynamic completions". One addition, three consumers — not
a second addition. Interim: populate the static `completions` with the eight subcommand *values*,
losing both the labels and the live server names, and file it as partial.
**verify** — cyrup-it: with two configured servers, `/mcp reconnect ` + Tab in a live pty offers both
names; and `/mcp ` + Tab offers the eight subcommands with their labels. Today the first assertion
fails at the autocomplete engine, not at the extension.

**MCP-383 — Port `showStatus`** · medium · S · `hand-written`
**upstream** — `commands.ts` `showStatus`: the five-way ladder of §4.4 with its exact icons
(`⊘ ✓ ⚠ ✗ ○`), the `failed {N}s ago — {reason}` form, the `(N tools[, cached])` suffix suppressed on
failure, and the two-line empty-config message.
**behavior** — `/mcp status`, and `/mcp` in a headless mode, print one line per server saying exactly
why it is or is not usable. This is the only MCP surface a non-TUI session has.
**cyrup** — one multi-line `HostServices::notify(&str, NotifyKind::Info)`. `ctx.hasUI` is
`HostCtx::has_ui`; the headless branch is selected on that, not on whether `open_overlay` returned
`false`.
**verify** — unit: a fixture with one server in each of the six states (disabled, connected,
needs-auth, failed-with-reason, failed-without-reason, cached, not-connected) produces the exact
lines, including that the failed server carries **no** tool suffix and never reports `cached`.

**MCP-384 — Port `showTools`** · low · S · `hand-written`
**upstream** — `commands.ts` `showTools`: flat-maps `toolMetadata` over non-disabled servers to the
**prefixed** registered names in map order; `No MCP tools available` when empty; otherwise
`MCP Tools:` / blank / two-space-indented names / blank / `Total: {N} tools`.
**behavior** — `/mcp tools` lists every MCP tool the model can currently see, under the names it must
call them by — so the names must be the registered ones, not the raw server-side ones.
**cyrup** — the same `notify` seam; the names come from the tool-metadata port, whose prefixing is
MCP-370's reconciliation with `crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs`.
**verify** — unit: one disabled and one enabled server produce only the enabled server's names, in
insertion order; a single tool reports `Total: 1 tools` (never singularised).

**MCP-385 — Port `showPrompts`** · medium · S · `hand-written`
**upstream** — `commands.ts` `showPrompts`: grouped by server with servers sorted by `localeCompare`
and prompts sorted **in place** by `commandName.localeCompare`, the `<required>` / `[optional]` usage
rendering, the two-space `/{commandName}` row, the six-space description row, the per-group blank
line, `Total: {N} prompt{s}`, and the two distinct `promptDiscoveryFailed` notes (one appended to the
empty-case sentence, one as a trailing line).
**behavior** — `/mcp prompts` shows every invokable MCP prompt under its server's name with its slash
name and argument shape, and warns when a *connected* server refused prompt discovery — the case
where the list is stale rather than empty.
**cyrup** — the same `notify` seam. **Divergence to state:** `String.localeCompare` with no locale is
ICU root collation; Rust's `str::cmp` is byte order. They agree for ASCII-lowercase names and
disagree on mixed case (`Foo` vs `bar`). Either take a collation crate or use `str::cmp` and say so
in the ported comment; do not leave it unstated. The in-place sort mutates the grouped vector as a
side effect of rendering — harmless because the grouping is rebuilt per call, so do not "optimise" it
into a shared cache.
**verify** — unit: two servers with three prompts each, one prompt carrying two required and one
optional argument, produce the exact block **including both `{server}:` header rows**; a
`promptDiscoveryFailed` server adds the exact trailer; the empty case with a failed server produces
the single-sentence variant with its leading space.

**MCP-385a — `/mcp prompts` opens each group with a `{serverName}:` header row** · low · S · `hand-written`
**upstream** — `commands.ts` `showPrompts` pushes `` `${serverName}:` `` once per group, immediately
after the group iteration begins and before any prompt row.
**behavior** — the prompt list is grouped **and labelled**. Without it, two servers print one flat run
of `/mcp__a__x` / `/mcp__b__y` rows separated by unexplained blank lines, with nothing saying which
server owns what.
**cyrup** — absent.
**verify** — unit: the golden block for two servers contains `alpha:` and `beta:` on their own lines,
unindented, with no icon. Filed separately because a golden-text test written from a spec that omits
the row cannot catch its absence.

**MCP-386 — Port `reconnectServer` / `reconnectServers`** · high · M · `hand-written`
**upstream** — `commands.ts` `reconnectServer` and `reconnectServers`: the ten steps of §4.7 with
their six exact messages, the `promptDiscoveryFailed` guard that leaves cached prompt metadata alone
and does **not** raise `promptMetadataLive`, the `instructions` set/delete pair, and the sequential
all-servers loop.
**behavior** — `/mcp reconnect [server]` closes and re-opens connections, refreshes tool/prompt/
resource metadata and the on-disk cache, and reports tool and resource counts plus any skipped tools.
The counts reported are the **raw** connection counts, so they can exceed the number of tools
actually registered — reproduce that rather than "fixing" it.
**cyrup** — absent. `isAbortError(error, signal)` matches `signal.aborted`, an `AbortError` name, or
the literal message `"MCP extension runtime stopped"`; in Rust the first two are a `CancelToken`
check plus a typed error variant, and the literal-message arm exists only because JS has no typed
errors — it does not port, and the port says so rather than dropping it silently.
**cyrup** *(sequencing)* — the all-servers loop is `for … await`, not a join: a server that takes 30 s
to fail blocks every server after it. That is upstream's behaviour and it is observable.
**verify** — unit: each of the six message paths; a two-server fixture proves sequencing (B's connect
starts only after A's settles); a `promptDiscoveryFailed` connection leaves the previous prompt
metadata intact and `promptMetadataLive` unset for that server.

**MCP-387 — Port `/mcp setup` and the reload-after-write flow** · high · M · `hand-written`
**upstream** — `index.ts`'s `setup` arm and `commands.ts` `openMcpSetup`: the `programmaticConfig`
refusal, the once-only `discovery` + `onboardingState` computation, the ten callbacks whose four
writers set `configChanged` (`adoptImports` only when `added` is non-empty), and the
`if (configChanged) { throwIfInactive(); await ctx.reload(); return }` early return.
**behavior** — writing a config from the setup panel reloads the session so the new servers take
effect immediately, and nothing after the reload in the handler runs.
**cyrup** — `HostServices::control(ControlOp::Reload)`, Command-tier, with the explicit
`HostCtx::require_command_tier` guard (MCP-381). The ten callbacks become a `SetupCallbacks` struct of
boxed closures or one trait object.
**cyrup** *(ordering)* — `discovery` and `onboardingState` are computed **at open time** and
`markSetupCompleted` persists that pre-write fingerprint; that is what lets a later discovery change
re-arm the hint. Recomputing after the write silently disables the re-arm.
**verify** — cyrup-it: scaffolding a project config from the panel issues exactly one
`ControlOp::Reload` and performs no further handler work; unit: `adoptImports` returning zero added
servers leaves `configChanged` false.

**MCP-388 — Port `logoutServer`** · high · S · `hand-written`
**upstream** — `commands.ts` `logoutServer` and `index.ts`'s `logout` arm: the usage error, then
`removeAuth` → `manager.close` → `updateStatusBar`, with a distinct message at each failure point.
**behavior** — `/mcp logout <server>` deletes the stored OAuth credential and drops the live
connection, and tells the user precisely which half succeeded when one fails.
**cyrup** — absent; the credential store itself is the keyring section's (`keyring`, linked directly
by the native crate).
**cyrup** *(the load-bearing string)* — `OAuth credentials were cleared for "{name}", but its
connection could not be closed: {msg}` must survive intact. A port that collapses the two failure arms
into one "logout failed" tells the user their credentials are intact when they are gone.
**verify** — unit: a failing `close` after a succeeding `removeAuth` produces that exact text and
`{ok:false}`; a failing `removeAuth` produces the other text and never calls `close`.

**MCP-389 — Port `/mcp disable` and `/mcp enable`** · medium · S · `hand-written`
**upstream** — `index.ts`'s shared `disable`/`enable` arm: the `programmaticConfig` refusal, the usage
error, the `not found in effective config` error, then `writeProjectServerDisabledOverride(…, sub ==
"disable")` and one of two notices depending on `result.changed`.
**behavior** — a user turns a server off without editing JSON, and is told which file changed and that
`/reload` is needed. This is the **only** subcommand that asks the user to reload rather than
reloading for them.
**cyrup** — absent; `writeProjectServerDisabledOverride` belongs to the config-writer port.
`isServerDisabled` is strict — only the literal boolean `true` disables.
**verify** — unit: the four message paths, and that a no-op re-disable reports `already disabled` and
writes nothing.

**MCP-390 — Port `authenticateServer` and `/mcp-auth`** · high · L · `host-verb`
**upstream** — `commands.ts` `terminalHyperlink` and `authenticateServer`, plus `index.ts`'s
`pi.registerCommand("mcp-auth", …)`: the four guards with their exact texts (including the
deliberately different returned vs. notified `supportsOAuth` message), `setStatus("mcp-auth",
"Authenticating {name}...")`, the OSC-8 hyperlink, the `onAuthorizationUrl` notice, the
`onAuthorizationInput` confirm-then-input pair carrying the flow's own `inputSignal`, and the
`finally` that clears the status key **only when the signal is not aborted**.
**behavior** — `/mcp-auth <server>` opens the browser flow and, on a remote machine, falls back to
"paste the callback URL", with the footer reading `Authenticating <server>...` throughout and a
successful flow chaining into a reconnect.
**cyrup** — `HostServices::{confirm, input, set_status, notify}` are a 1:1 match. Three things to get
right: (i) `HostCtx::begin_human_wait` must wrap the `confirm`/`input` pair or the dispatcher's
invocation budget can expire mid-dialog — that gate exists for exactly this; (ii) `DialogOptions`
carries `signal_id: Option<String>`, a host-registry **key**, not a signal, and upstream passes the
*authorization flow's own* signal (distinct from `ctx.signal`) to both dialogs and checks it between
them, so the port registers that inner signal and threads its id; (iii) the OSC-8 escape goes into a
**notification body**, while the panels deliberately strip OSC from panel rows — one global sanitizer
over both surfaces would be wrong. The OAuth machinery itself is section 07's
(`cyrup_provider::auth::oauth`).
**cyrup** *(residue)* — whether cyrup's notification renderer passes the OSC-8 bytes through is
unobserved. If it does not, the link degrades to visible text; that is a residue to file with its
consequence, not an outcome to accept.
**verify** — unit: each guard's exact text, and that the returned and notified `supportsOAuth`
messages differ; cyrup-it: a live pty shows `Authenticating <server>...` in the footer, cleared on
completion but **not** on abort; cyrup-it: the notification body still contains the `ESC ] 8` bytes
when it reaches the terminal.

**MCP-391 — Port `openMcpAuthPanel`** · medium · S · `host-verb`
**upstream** — `commands.ts` `openMcpAuthPanel`: the `!hasUI` / `programmaticConfig` /
zero-OAuth-servers guards with their exact texts, then the **same** panel factory at width 82 with
`{authOnly: true, noticeLines: ["Select an OAuth MCP server and press Enter or ctrl+a to
authenticate."]}`; always returns `configChanged: false`.
**behavior** — bare `/mcp-auth` opens a picker listing only OAuth-capable servers with their auth
state; Enter or `ctrl+a` starts the flow.
**cyrup** — `HostServices::open_overlay` with the panel type from the panel units carrying an
`auth_only` flag — **one type, not two**. Upstream threads `authOnly` through construction, the
visible-item rebuild, three key handlers, `toggleItem`, the title, the empty message, the status line,
the hint list and the connection-status renderer; splitting the type duplicates ten call sites.
**verify** — unit: an `authOnly` panel over a mixed config lists only the OAuth servers, shows no
tools, and renders the reduced hint bar; the entry point returns `configChanged: false` even after a
successful authentication.

**MCP-392 — Port `buildMcpPanelCallbacks`'s connection-status derivation** · high · M · `hand-written`
**upstream** — `commands.ts` `buildMcpPanelCallbacks`: the eight-step ladder of §4.11, the per-open
`authStatusFailures` map, the `resolveServerUrl`-throws-gives-`failed` arm, the four-condition OAuth
guard, and `refreshCacheAfterReconnect` re-reading the whole cache file.
**behavior** — the panel shows `needs auth` for a server whose credential is missing **before** ever
contacting it, and `failed` with a readable reason when the credential store itself cannot be read —
so the user is told to authenticate rather than watching a connect fail.
**cyrup** — absent. The map is per-panel-open state, never session state; that separation is the
documented point, and merging it into the real failure tracker makes a credential-store read look
like a connection failure for the rest of the session.
**verify** — unit: each of the eight rungs, including `inspectAuthForUrl` returning `unavailable`
producing both `failed` **and** a recoverable `getFailureMessage`; and that a second open starts with
an empty `authStatusFailures`.

**MCP-393 — Port the shared-config notice and its one-shot state** · low · S · `hand-written`
**upstream** — `commands.ts` `buildSharedConfigNoticeLines` and `openMcpPanel`'s post-close
`markSharedConfigHintShown(fingerprint)`.
**behavior** — the first time a user opens `/mcp` with servers coming from a standard shared config,
the panel explains that Pi only writes into Pi-owned files; a panel that never opens does not consume
the one-shot.
**cyrup** — absent; the onboarding-state file port is the setup-panel units'.
**verify** — unit: opening twice shows the notice once; a changed fingerprint does **not** re-arm it —
the flag is a plain boolean and the fingerprint is stored but never compared.

**MCP-394 — Port `openMcpPanel`'s orchestration and the direct-tools write-back** · critical · M · `hand-written`
**upstream** — `commands.ts` `openMcpPanel`, `index.ts`'s panel arm and `config.ts`'s
`writeDirectToolsConfig`: the `programmaticConfig` branch, the **zero-servers-delegates-to-setup**
path, the flag/override config-path resolution, and the `done` chain
`writeDirectToolsConfig` → `onDirectToolsConfigChanged` → notify whose **error** arm sets
`configChanged = true`.
**behavior** — `/mcp` on a fresh machine opens onboarding instead of an empty box; saving toggles
rewrites the right config files and refreshes the live tool surface without a reload; a *failed* live
refresh falls back to a reload while a *successful* save does not.
**cyrup** — **critical because the result must escape a `bool`.** `HostServices::open_overlay` returns
`bool`, so `McpPanelResult` leaves through shared state the caller constructed and handed in — an
`Arc<Mutex<Option<_>>>` written on the model's close path and read after `open_overlay` returns. A
naive port silently discards the user's toggles after they pressed save: data loss with a success
notice on screen. The in-session refresh half needs `ExtensionHost::register_late_tool`
(`crates/cyrup-ext/src/facade.rs`) reachable from a native — HA-1's tool leg, section 01's MCP-037 —
so until that lands **every** save must take the `configChanged` reload path; state which of the two
ships first. `writeDirectToolsConfig` groups changes by `provenance.path` and, for `kind ==
"import"` servers, writes the **whole resolved definition** plus `directTools` into the target file
(which is what the panel's "will copy to user config on save" notice warns about); every other kind is
a merge onto the existing entry.
**verify** — unit: a zero-server config routes `/mcp` to the setup panel; a save with changes writes
the expected JSON per provenance path; a throwing `onDirectToolsConfigChanged` produces the exact
error notice **and** sets `configChanged`; cyrup-it: toggling a tool, saving, and reopening the panel
shows the toggle persisted.

**MCP-394a — A change for a server with no provenance entry is silently dropped** · medium · S · `hand-written`
**upstream** — `config.ts` `writeDirectToolsConfig` iterates `changes` and does
`const prov = provenance.get(serverName); if (!prov) continue;` before bucketing by path. A server in
the panel's `changes` map but absent from `provenanceMap` is written nowhere and reported nowhere,
while `openMcpPanel` emits `Direct tools updated for this session.` unconditionally whenever
`changes.size > 0`.
**behavior** — the user toggles a tool, presses ctrl+s, sees a success notice, and the setting is not
persisted. The one path in this section where the notice and the on-disk result can disagree.
**cyrup** — absent. Reproduce the skip exactly — in practice the two maps agree, because the panel is
built from the same `getServerProvenance` call — but write it as an explicit named branch
(`None => { /* upstream drops it */ }`) rather than a `filter_map` that reads like a bug.
**verify** — unit: a `changes` map containing a server missing from `provenanceMap` writes no file and
still produces the success notify, asserting upstream's behaviour so a later change to it is visible
in the diff.

**MCP-395 — HA-1's command leg: MCP prompts are slash commands, and there is no late command registration** · high · L · `host-addition`
**upstream** — `prompts.ts` `resolveCachedPrompts` and `index.ts`'s `registerPromptCommands` /
`syncPromptCommands`: one `pi.registerCommand(commandName, createPromptCommand(…))` per prompt,
de-duplicated through `registeredPromptCommands`, re-run on **every** tool-metadata update for the
whole session.
**behavior** — a server offering `brief` gives the user `/mcp__demo__brief`, available from the first
frame when it is in the cache and appearing mid-session when it is not.
**cyrup** — the **cache-backed half ports cleanly** (MCP-395a); the live half has nothing to land on.
`InitApi::register_command` is reachable only from `NativeExtension::init` and the `InitApi` is
consumed there. `ExtensionHost::register_late_tool` exists for **tools** and is paired with
`ExtensionRegistry::{mark_tools_dirty, take_tools_dirty}` and `ExtensionHost::refresh_tools`; there is
**no `register_late_command`, no command dirty flag and no command refresh path**. On the TUI side the
`/` menu's registry is built from `AgentSession::slash_command_catalog`
(`crates/cyrup-session-svc/src/session.rs`) through `dynamic_commands_from_catalog_gated` and
installed at exactly three points — initial install, session swap, and the `enableSkillCommands`
setting toggle — with no mid-session rebuild signal. So the command leg is **three additions, not
one**: a post-`init` registry write, a dirty/refresh pair mirroring the tool side, and a catalog
rebuild the TUI acts on. Calling it a sibling of `register_late_tool` understates it by two of the
three.
**cyrup** *(a second, smaller delta)* — `slash_command_catalog` emits `source: "extension"` for every
registered extension command, and `CommandSource::Prompt` is reserved for filesystem prompt
templates, so MCP prompt commands surface labelled `Extension` in the `/` menu unless the catalog row
learns to carry the right source.
**cyrup** *(interim)* — register only cached prompts and accept that a first-ever connection to a
prompt-bearing server yields no commands until the next session. That is a real, user-visible loss and
is filed as partial, not done. A single `/mcp-prompt <name> [args]` dispatcher would preserve
invocability with one static registration but is a **different user-facing surface** — every prompt
loses its `/` menu entry and its own description — so it is a redesign, not a port.
**verify** — cyrup-it: with a populated cache, `/mcp__demo__brief` is in the `/` menu on the first
frame of a fresh session; with an empty cache, connect a prompt-bearing server and assert whether the
command appears — the assertion encodes whichever option shipped.

**MCP-395a — Cache-time prompt resolution and command naming** · medium · S · `hand-written`
**upstream** — `prompts.ts` `resolveCachedPrompts`; `metadata-cache.ts` `reconstructPromptMetadata`;
`types.ts` `sanitizePromptName`, `formatPromptCommandName`, `getServerPrefix`, `sanitizeServerPrefix`
(§5.1–§5.2).
**behavior** — every prompt in a *valid* cache entry for an enabled, configured server becomes a
slash command before any connection exists; the name is deterministic, always carries the server, and
is stable across runs.
**cyrup** — pure functions with no host dependency: a synchronous cache read plus string
transformation, registrable during `init` through `InitApi::register_command`. The prefix modes are
where cyrup and upstream currently disagree — the in-tree `ToolPrefix`
(`crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs`) has three variants and folds `"mcp"` into
`Server`, and its `get_server_prefix` replaces hyphens where upstream's `sanitizeServerPrefix`
preserves them — which is MCP-370's reconciliation. `formatPromptCommandName` reaches
`getServerPrefix(name, "mcp")` whenever `settings.toolPrefix == "mcp"`, so the missing fourth variant
is load-bearing here and not only for tools.
**verify** — unit: a golden table over `{prompt name, server name, prefix mode}` covering a leading
digit, an empty sanitised result, a space (`a_20_b`), a preserved hyphen, `toolPrefix: "none"` still
carrying the server, and `toolPrefix: "mcp"` producing `mcp__mcp__{server}__{prompt}`; plus a cache
fixture where one server is disabled, one has an invalid hash and one is absent from config,
yielding only the remaining server's prompts.

**MCP-396 — Port `parsePromptArgs`'s bash-style tokenizer** · medium · S · `hand-written`
**upstream** — `prompts.ts` `tokenizeArgs`, `findUnquotedEquals`, `stripQuotes`, `parsePromptArgs`
(§5.3).
**behavior** — `/mcp__demo__brief today "important tasks"` and
`/mcp__demo__brief day=today topic="important tasks"` both work, and a value containing spaces
survives to the server.
**cyrup** — absent, and **not** substitutable: `crates/cyrup-tools` has shell-word handling for the
bash tool, but that is a POSIX grammar; this one *retains* the quote characters in the token and
strips them later, which POSIX splitting does not. Three details a reasonable rewrite gets wrong:
(i) both the opening **and** closing quote go into the token, which is the only reason `stripQuotes`
exists; (ii) `eq > 0` is strict, so a token beginning with `=` is positional, not a named argument
with an empty key; (iii) `escaped` is consumed across iterations, so a trailing lone backslash is
dropped.
**verify** — unit: a table over `a b`, `a="x y"`, `'a=b'`, `=x`, `a\ b`, `"unterminated`, a trailing
backslash, and empty input.

**MCP-397 — Port `resolvePromptArgs` and the usage message** · medium · S · `hand-written`
**upstream** — `prompts.ts` `resolvePromptArgs` and `buildUsageMessage` (§5.4).
**behavior** — `topic=x today` binds `topic` by name and `today` to the first *unbound* declared
argument; a missing required argument produces a usage line locally rather than a server error.
**cyrup** — absent. The `positionalIndex` subtlety is a JS `??` short-circuit; in Rust write it
explicitly as "look up by name; only if that is `None`, take and advance the positional cursor".
Getting it wrong shifts every subsequent positional argument by one — silent wrong output, not an
error. The undeclared-named passthrough is deliberate and cited to the MCP spec; do not filter it to
the declared set.
**verify** — unit: a two-required/one-optional metadata over the six named/positional/missing
combinations, plus an undeclared named argument surviving into the output, plus the exact
`Missing required arguments: a, b.` plural form.

**MCP-397a — An explicit empty named value for a declared optional argument is still sent** · low · S · `hand-written`
**upstream** — `prompts.ts` `resolvePromptArgs`: loop 1 refuses to bind an empty value, but loop 2
re-adds **any** named key not already in `args`, and a key loop 1 rejected is by definition not in
`args`. So `topic=` puts `args["topic"] = ""` on the wire for a declared *optional* `topic`, while a
declared *required* `topic` still fails the `missing` filter and produces the usage error.
**behavior** — a user can explicitly clear an optional prompt argument by passing `name=`, and the
server receives the empty string rather than the key being omitted. Whether "absent" and "empty"
differ is the server's business; the adapter passes what was typed.
**cyrup** — absent. Write the two loops in upstream's order and do **not** add an `is_empty()` guard
to loop 2. The natural Rust rewrite — filter empties in loop 1, then extend only unknown non-empty
keys — silently drops the argument, and is exactly the shape a reviewer will suggest.
**verify** — unit: metadata with one optional `topic` and input `topic=` yields `{"topic": ""}`;
the same input against a *required* `topic` yields `Missing required argument: topic.` plus the usage
line.

**MCP-398 — Port the prompt command handler** · high · M · `host-verb`
**upstream** — `prompts.ts` `createPromptCommand`, `findLivePromptMetadata` and
`buildCommandDescription` (§5.5): the `promptMetadataLive`-guarded staleness check **before and
after** `lazyConnect`, the un-configured-server check, the two connect-failure messages keyed on
`needs-auth`, dispatching the *refreshed* `originalName` while quoting the *pre-connect* name in
errors, the `logger.debug` alongside the user-facing error, the empty-text warning, and
`pi.sendUserMessage(text)`.
**behavior** — invoking a prompt command connects the server on demand, fetches the template, and
pushes the rendered text into the conversation as a user turn — or explains precisely why it could
not.
**cyrup** — `HostServices::control(ControlOp::SendUserMessage { content, opts })`, Command-tier,
satisfied by `execute_command` with the explicit `HostCtx::require_command_tier` guard. The
`promptMetadataLive` set is the load-bearing guard: without it a **cache-only** command is refused
before its server has ever been contacted, because the live lookup returns `None` for any server not
yet connected. The double check is not redundant — the second catches a prompt that disappeared
*during* the connect. `truncateAtWord` cuts at 120 **JS string-length** units and falls back to the
last space only beyond `120 * 0.6`; port the ratio literally, and decide whether the unit is chars or
UTF-16 code units for a non-ASCII description.
**verify** — unit: each of the seven failure messages, that the *original* name reaches
`getPrompt` while the *pre-connect* name appears in the error text, and that a successful invocation
issues exactly one `SendUserMessage` carrying the flattened text.

**MCP-399 — Port `formatPromptResult` and `extractMessageText`** · medium · S · `hand-written`
**upstream** — `prompts.ts` `formatPromptResult` and `extractMessageText` (§5.6): a lone `user`
message is emitted bare, every other message is prefixed `[{role}] `, empty extractions are skipped,
the whole is joined by `\n\n` and trimmed; the five content kinds each have an exact placeholder.
**behavior** — a one-message prompt arrives exactly as the server wrote it; a multi-message prompt
keeps its conversational shape as inline role markers; non-text content degrades to a readable
placeholder rather than vanishing.
**cyrup** — a `match` over `rmcp::model::PromptMessage`'s content enum, fed by
`rmcp::model::GetPromptResult`. The `[role]` fallback is justified upstream by pi having no
multi-message replay API; cyrup has the same limitation, so the rationale ports unchanged. Exact
strings: `[resource {uri}]` optionally followed by a newline and the text; `[resource_link {uri}]` or
`[resource_link {uri} — {name}]` (em dash); `[image {mime|unknown}]` or
`[image {mime|unknown} (embedded)]`; `[audio {mime|unknown}]`.
**verify** — unit: a single user text message round-trips verbatim; a two-message result produces
`[user] …\n\n[assistant] …`; a single **assistant** message still gets its `[assistant] ` prefix; each
of the five content kinds produces its exact placeholder and an unknown kind contributes nothing.

---

### Out of scope

These are decided, not deferred. Recorded with reasons so a later pass does not re-file them as gaps.

* **MCP Apps / the UI extension — and with it `glimpse-ui.ts`.** Cut entirely. `glimpse-ui.ts` is not
  a TUI file: it is macOS-only detection and launch of the `glimpseui` npm package's native webview
  window (`isGlimpseAvailable`, `openGlimpseWindow`), and its only production consumer is
  `ui-session.ts`'s `useGlimpse` branch, which builds an iframe wrapper and sets `viewer =
  "glimpse"`. No panel, command or prompt touches it. **Consequence in this section:** nothing — the
  panels and commands are complete without it, and `MCP-373` is retired as a unit id rather than
  filed as a gap. Two facts are recorded so the decision is auditable rather than inherited:
  `glimpseui` is **not a declared dependency** (the only `glimpse` entry in `package.json` is the
  published-file manifest line for `glimpse-ui.ts` itself), so it is discovered at runtime or not at
  all; and `openGlimpseWindow` does `await import(modulePath)` **in-process** and then subscribes to
  a `"closed"` event on the returned handle, which a Rust binary cannot do at all — that arm would
  have been a subprocess-with-IPC design or nothing, which is a second reason it does not belong to
  a TUI panel port.
* **`mcpScript` and the JavaScript worker.** Cut entirely. **Consequence in this section:** none of
  the panels, commands or prompt handlers reaches a script origin, so no arm disappears — but the cut
  is restated here because `glimpse-ui.ts` above is the only file in this set that would otherwise
  invite an in-process JS runtime, and it does not get one either.
* **The legacy HTTP+SSE transport.** rmcp ships no SSE client transport; supported transports are
  `stdio` and streamable HTTP. **Consequence in this section:** `/mcp-auth` and the auth panel apply
  only to `url`-configured streamable-HTTP servers, which is already `supportsOAuth`'s rule; a server
  declaring `httpTransport: "sse"` is rejected at config load with a named diagnostic rather than
  appearing in the picker and then never connecting.
* **The raw unix-socket transport.** rmcp's UDS support is streamable-HTTP-over-UDS, a different wire
  shape. **Consequence in this section:** a `socket` server has no `url`, so it never reaches an OAuth
  path; it still appears in the panel's server list and in `showStatus` under whatever the config
  loader's diagnostic leaves it as.

---

### What does not fit cleanly

Two host additions are load-bearing here, one is cosmetic, and four decisions are genuinely open.
Everything else in the section is `hand-written` policy over host verbs that already exist.

**1 · HA-2 — labelled, dynamic argument completions for a native command (MCP-382).** The same
addition section 01 files as MCP-041; this section is its second consumer and
`crates/cyrup-ext-subagents/src/registration/slash_commands.rs` is its third. Three legs:
`ExtensionHost::command_completions` is WASM-only and resolves owners in the live-guest map, so a
native errors; `NativeExtension` has no `argument_completions`; and the TUI has no
argument-completion context to consume one — `Autocomplete::compute` offers `Slash | Path | Mention`,
`slash_context` returns `None` as soon as the buffer contains whitespace, and nothing reads
`SlashCommand::has_arg_completion`. The one path that exists is also label-less. **Recommendation:**
add a `{value, label}` completion item, a `NativeExtension::argument_completions` default-empty
method, widen the guest return type, and add the argument context to the autocomplete engine —
sized as a design, not a hook. Interim: eight static subcommand values, filed as partial.

**2 · HA-1's command leg — late registration of prompt slash commands (MCP-395).** The tool side has
`ExtensionHost::register_late_tool` plus a dirty flag and `refresh_tools`; the command side has none
of the three, and the TUI's `/` registry is rebuilt only on initial install, session swap, or an
`enableSkillCommands` toggle. This is not a sibling of the tool path — it is a registry write, a
dirty/refresh pair and a rebuild signal. **Recommendation:** solve it with HA-1's shape so one
mechanism serves tools and commands, and until it lands, ship the cache-backed half and name the loss
(a first-ever connection to a prompt-bearing server yields no commands until the next session).

**3 · HA-3 — overlay geometry, plus the clipping the host declines to own (MCP-368).** Upstream asks
for fixed 82 / 92 columns; `ExtensionOverlay` hardcodes a width percentage, a minimum width, a
maximum height percentage and a margin, and `open_overlay` takes no options bag. The width half is
cosmetic — the host paints no border, so content self-centres and every panel row is computed against
the width it is handed. The **height** half is not cosmetic: overflow is a `take` on the rendered rows
and `InteractiveOverlay`'s own contract calls that lossless-by-design, so the panel must window its
own body or its hint row and bottom border vanish on a short terminal with no indication.
**Recommendation:** panel-side windowing regardless; the geometry argument to `open_overlay` only if
the 82/92 design is judged worth a host change.

**4 · Panel keybindings (MCP-363 / MCP-363a).** `panel-keys.ts` resolves `tui.select.up`/`down`/
`confirm` and the adapter-defined `mcp.panel.save` against the host's keybinding manager. cyrup has
the ids (`crates/cyrup-config/src/keybindings.rs`, resolved by `crates/cyrup-tui/src/keymap.rs`) and
already passes the resolved `action id → keys` map **into `cyrup-ext`** as
`ExtensionRegistry::resolve_shortcuts`' parameter — but nothing stores it, nothing answers "what is
`tui.select.up` bound to?", and no adapter-defined id can be declared. Options: expose a
`HostServices` read of the stored map (one method, and it also gives `resolve_shortcuts` a source of
truth it currently lacks), or take upstream's own no-manager fallbacks (arrows / Enter / `ctrl+s`) and
lose user rebinding inside the panel. **Recommendation:** the read; it is the same decision either
way and leaves one map rather than two.

**5 · `ctrl+y` copies a server's failure message (MCP-361).** No clipboard method on `HostServices`.
What exists in `cyrup-tui` is a `#[cfg(unix)]` subprocess spawn of `pbcopy` / `wl-copy` /
`xclip -selection clipboard` that swallows every failure and is a no-op stub elsewhere, plus
`arboard` used only for clipboard *image read*. So no thread affinity is required and no new
dependency is needed, but the in-tree helper cannot produce upstream's failure notice.
**Recommendation:** a host verb that returns success, since a silent copy is exactly the failure mode
upstream's message exists to prevent.

**6 · Collation (MCP-385).** `showPrompts` and `listAllPromptMetadata` sort by ICU root collation via
`localeCompare`; `str::cmp` is byte order. Identical for ASCII-lowercase names, different for mixed
case. **Recommendation:** `str::cmp` with the divergence named in the ported comment, unless a
collation crate is already in the tree for another reason.

**7 · Poll-repaint replaces push-repaint (MCP-350).** Upstream repaints on `requestRender()`; cyrup's
`InteractiveOverlay` is pulled by `tick()` at the overlay's own `refresh_ms()`, which defaults to
"never". The panels must opt into a cadence, and the cost is up to one tick of visible staleness on
every async settle plus a wakeup per tick for the life of the panel. That is a residue filed as work —
the honest close is a push channel from extension to host, which does not exist.

---

### Coverage

**Read**

*Upstream, at `v2.25.0`, in full:* `mcp-panel.ts`, `mcp-setup-panel.ts`, `commands.ts`, `prompts.ts`,
`panel-keys.ts`, `glimpse-ui.ts`, `onboarding-state.ts`, `ui-tool-visibility.ts`,
`__tests__/mcp-panel-rendering.test.ts`.

*Upstream, in the regions this section depends on:* `index.ts`'s prompt-command registration and the
`/mcp` + `/mcp-auth` registrations; `types.ts`'s `isServerDisabled`, `ToolPrefix`, `PromptMetadata`,
`ServerProvenance`, `McpPanelCallbacks` / `McpPanelResult`, and the whole name-formatting and
include/exclude engine (`sanitizePromptName`, `formatPromptCommandName`, `getServerPrefix`,
`sanitizeServerPrefix`, `getLegacyServerPrefix`, `getToolNameCandidates`); `config.ts`'s
`KNOWN_SERVER_PRESETS`, `IMPORT_PATHS`, the discovery/preview types, `writeDirectToolsConfig`;
`metadata-cache.ts`'s `reconstructPromptMetadata`; `utils.ts`'s `stripOscSequences`,
`sanitizeTerminalText`, `truncateAtWord`, `openPath`; `resource-tools.ts`'s `resourceNameToToolName`;
`__tests__/mcp-panel-keybindings.test.ts`'s fixture head (the three-way `mcp.panel.save` semantics and
the two-server OAuth config). Plus a `git grep` at the tag for resource-tool base-name construction
(13 sites) and for `glimpse` consumers.

*pi, at `v0.84.1`:* `packages/tui/src/utils.ts`'s `truncateToWidth` signature and `pad` semantics, and
`visibleWidth`'s head including its tab→3-spaces normalisation — both are what the panel's row
renderer depends on.

*cyrup, branch `david/cyrup`, by symbol:* `cyrup_ext::host::overlay`'s `InteractiveOverlay`,
`OverlayLine`, `OverlaySpan`, `OverlayKey`, `OverlayOutcome`; `cyrup_ext::host::services`'s
`HostServices` (`open_overlay`, `notify`, `set_status`, `confirm`, `input`, `select`, `control`,
`human_interaction_lock`, `is_run_cancelled`), `DialogOptions`, `ControlOp`, `NotifyKind`;
`cyrup_ext::native`'s `NativeExtension` (`init`, `execute_command`, `set_host_services`), `InitApi`
(`register_command`, `register_tool`, `register_shortcut`, `add_autocomplete`), `HostCtx`
(`begin_human_wait`, `require_command_tier`), `ExtMode`; `cyrup_ext::facade`'s `ExtensionHost`
(`load_native_with_services`, `register_late_tool`, `refresh_tools`, `execute_command`,
`command_completions`, `live_for_command`, `resolve_shortcuts`); `cyrup_ext::registry`'s
`CommandDescriptor`, `ResolvedCommand`, `ExtensionRegistry` (`register_command`, `resolved_commands`,
`mark_tools_dirty`, `take_tools_dirty`, `resolve_shortcuts`); `cyrup_ext::host::live`'s
`LiveExtension::argument_completions`; the `wasm-host` gate on `cyrup_ext::host`;
`crates/cyrup-tui/src/overlay.rs`'s `ExtensionOverlay`, its geometry constants, `box_rect`, `Drop`,
`to_ratatui_line`/`span`/`color`, `to_overlay_key`; `crates/cyrup-tui/src/app/extension_ui.rs`'s overlay input
routing, `handle_overlay_key`, `/copy`, `copy_to_clipboard`, `read_clipboard_image_to_temp`, and the
three registry-install sites; `crates/cyrup-tui/src/commands.rs`'s `SlashCommand`,
`CommandRegistry::with_dynamic`, `dynamic_commands_from_catalog_gated`;
`crates/cyrup-tui/src/autocomplete.rs`'s `Autocomplete::compute`, `slash_context`, `path_context`,
`CompletionContext`; `crates/cyrup-tui/src/keymap.rs`'s `SelectAction`, `Key::{parse, matches}`;
`crates/cyrup-config/src/keybindings.rs`'s `tui.select.*` ids;
`crates/cyrup-session-svc/src/host_services.rs`'s `LiveHostServices::open_overlay`, `OverlayRequest`,
`OverlaySink`; `crates/cyrup-session-svc/src/session.rs`'s `AgentSession::slash_command_catalog`;
`crates/cyrup-ext-subagents/src/tui/fleet_overlay.rs`'s async-in-sync-overlay pattern and its
`fleet.rs` refresh cadence; `crates/cyrup-ext-subagents/src/extension.rs`'s host-services stash and
`open_overlay` call site; `crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs`'s `ToolPrefix`,
`resolve_mcp_direct_tool_names`, `get_tool_prefix`, `get_server_prefix`, `format_tool_name`;
`crates/cyrup-ext-subagents/src/registration/slash_commands.rs`'s static-completions note;
`crates/cyrup-intercom/src/ui/mod.rs`'s `visible_width` / `truncate_to_width`;
`docs/adr/ADR-0001-tui-substrate.md`'s six rules, three commitments and per-item consequences.

*rmcp, at `rmcp-v3.1.2-7-gf713ebd`:* `rmcp::model`'s `GetPromptResult`, `PromptMessage` and the
content variants MCP-399 matches on.

**Excluded**

Under ADR-0001 rule 2 — "does this line draw? if it does not draw, it is in scope" — applied per
region, and recorded here so a later pass cannot re-raise a defence rule 2 already refuses:

* `mcp-panel.ts`'s `fg(code, text)` helper and every raw `\x1b[…m` literal in its `bold` / `italic` /
  `inverse` wrappers — **computes cells; the host now computes them.** The *colours* are **not**
  excluded: which slot is dim and which is cyan is transcribed into `OverlaySpan` fields, because it
  is what the user sees.
* The box-glyph assembly and padding arithmetic inside `mcp-panel.ts`'s and `mcp-setup-panel.ts`'s
  `row()` / `padLine()` — the glyph emission only. `ExtensionOverlay` paints a `Clear`ed rect and a
  `Paragraph`, so the border is the host's. **The width arithmetic is not excluded**: `innerW`,
  `contentW`, `previewW` and the `pad` flag decide what fits and are specified and ported.
* Nothing else. Explicitly and deliberately **in scope**: `fuzzyScore`, `rebuildVisibleItems`, the
  visible-window start formula, the hint-bar wrap policy, both `wrapText`s, `estimateTokens`,
  `visibleActionRange`, `sanitizeRowContent`, `sanitizeDisplayText`, every key binding, every state
  transition, every sort order, and every string. Rule 2 names input sanitation and fuzzy ranking as
  in scope with no further argument needed, which settles the two items most often defended.

Owned by other sections, read only far enough to cite: `config.ts`'s discovery / preview / write
machinery; `mcp-auth.ts` and `mcp-auth-flow.ts` (`supportsOAuth`, `authenticate`, `removeAuth`,
`getAuthStorageOptions`, `inspectAuthForUrl`) — §4.8 and §4.11 specify only the UI contract they must
satisfy; `init.ts`'s `lazyConnect`, `updateStatusBar`, `updateMetadataCache`,
`notifyToolMetadataUpdated`, `markKeepAliveAfterConnect` and the failure-tracking quartet;
`metadata-cache.ts`'s `computeServerHash` / `isServerCacheValid`; `tool-metadata.ts`'s
`buildToolMetadata`; `direct-tools.ts`, `state.ts`, `runtime-owner.ts` at their call sites only.

Not read at all, and why: nine upstream test files —
`mcp-panel-auth`, `mcp-panel-copy-error`, `mcp-panel-exclude-tools`, `commands-auth`,
`commands-onboarding`, `commands-panel-auth-storage`, `commands-status-failure`, `prompts`,
`prompts-regressions`, `prompts-sdk-integration` and the prompts fixture server. Only the rendering
test was assigned. They are the natural independent source for the `verify` lines of MCP-383,
MCP-390, MCP-392, MCP-393 and MCP-396–MCP-399. **Unverified, not cleared.**

**Negative results**

* `ExtensionHost::command_completions` is `#[cfg(feature = "wasm-host")]` and resolves its owner
  through `live_for_command`, which looks the owner up in the **live-WASM** map; a native built-in is
  absent from it and the call fails with `command \`X\` has no live owner`. There is no
  `NativeExtension::argument_completions`. The only caller of `command_completions` in the whole tree
  is a `cyrup-it` test — **zero** production consumers in `cyrup-tui`, `cyrup-modes` or
  `cyrup-session-svc`.
* `Autocomplete::compute` has exactly three contexts (`Slash`, `Path`, `Mention`), and
  `slash_context` returns `None` as soon as the buffer contains whitespace — so there is no
  argument-completion code path to consume completions even if one were produced. `SlashCommand`
  carries `argument_hint` and `has_arg_completion`, but `has_arg_completion` is read nowhere outside
  its own tests, and `dynamic_commands_from_catalog_gated` hardcodes both off for every dynamic row.
* `AgentSession::slash_command_catalog` emits `source: "extension"` for every registered extension
  command; the `"prompt"` rows it also emits come from the session's filesystem prompt templates, not
  from extensions. So an MCP prompt registered as an extension command surfaces in the `/` menu
  labelled `Extension` unless the catalog row learns a new source.
* The TUI's `/` registry is installed at exactly three points — initial install, session swap, and the
  `enableSkillCommands` setting toggle. There is no mid-session rebuild triggered by extension
  activity, and no command analogue of `ExtensionRegistry::{mark_tools_dirty, take_tools_dirty}` or
  of `ExtensionHost::refresh_tools`. `ExtensionHost::register_late_tool` has no command sibling.
* `CommandDescriptor` is `{ description: String, completions: Vec<String> }` — the completions are
  fixed at `init` and there is no per-invocation hook.
* `DialogOptions` is `{ timeout_ms: Option<u64>, signal_id: Option<String> }` — `signal_id` is a
  host-registry key, not an `AbortSignal`, so upstream's inner authorization signal must be
  registered and threaded by id.
* `NativeExtension::execute_command`'s default returns
  `native extension has no handler for command \`{name}\``, and `set_host_services`' default is a
  no-op — a native that does not override the latter can call nothing, because `HostCtx` carries only
  `mode`, `has_ui`, `cwd` and a tier check and exposes no services accessor.
* No clipboard and no keybinding accessor anywhere in the `HostServices` trait body. cyrup's clipboard
  **write** is a `#[cfg(unix)]` subprocess spawn (`pbcopy` / `wl-copy` / `xclip`) that swallows
  failures and is a no-op stub on other targets; `arboard` is used only for clipboard image *read*.
* `ExtensionRegistry::resolve_shortcuts` does take the host's resolved `action id → keys` map as
  `&[(String, Vec<String>)]` and `ExtensionHost::resolve_shortcuts` exposes it — but it resolves
  *extension-declared* shortcuts for editor-global dispatch, stores nothing queryable, and has no
  production caller. The data is already in the right crate in the right shape; only the
  extension-facing read is missing.
* `ExtensionOverlay`'s `box_rect` computes width from the drawing area alone and uses the row count
  only for height, so the width a panel is probed at always equals the width it is painted into. A
  panel that pads to the width it is given cannot be mis-padded at any row count.
* Only `Ctrl+Shift+D` is intercepted ahead of an open overlay; `ctrl+c`, `ctrl+a`, `ctrl+r`, `ctrl+s`,
  `ctrl+y`, `?`, `space` and every printable reach it, and `handle_overlay_key` swallows an unhandled
  key rather than leaking it to the editor — matching upstream's panels.
* `unicode-width` is not a workspace dependency; the only declaration is `cyrup-intercom`'s. Any
  width-measuring port must promote it. `cyrup-intercom`'s `truncate_to_width` takes no ellipsis and
  no pad flag, so it is not a drop-in for the panel's call, and its `visible_width` omits pi's
  tab normalisation.
* There is no MCP panel, MCP slash command or MCP prompt handling anywhere in `crates/` today, and
  `crates/cyrup-mcp` does not exist. The only MCP-shaped Rust is
  `crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs`, a *consumer* of the metadata cache these
  panels read; it is fully `pub`, so a cross-crate conformance test against it is feasible.

**Blind spots**

1. **Collation and locale formatting.** `localeCompare` (MCP-385) and `toLocaleString` (the panel's
   token counts) are ICU-default behaviours asserted from `en-*` defaults. A defect hides as "the
   prompt list is in a slightly different order" or "the count reads `12 345`", which no test catches
   unless it uses mixed-case names or a non-English ICU build.
2. **The nine unread test files.** The highest-density independent source of exact expected strings
   for this section; eight `verify` lines would be stronger written against them. Every string here
   was derived from the implementation rather than from an independent assertion. Unverified, not
   cleared.
3. **Grapheme semantics of `visibleWidth` / `truncateToWidth`.** pi's tab normalisation and `pad`
   contract are established; the segmentation body is not, nor is `cyrup-intercom`'s
   `grapheme_clusters`. If they disagree on ZWJ sequences or regional indicators, every panel row is
   a column off for names containing emoji — a ragged border, not a crash.
4. **What cyrup's notification renderer does to OSC-8** (MCP-390). ADR-0001 puts OSC-8 emission
   firmly in scope with the substrate defence explicitly withdrawn, so this is a measurement, not a
   negotiation — but the notification path was not read, and the defect hides as "the auth link is
   not clickable", which is degradation rather than failure and is therefore easy to ship.
5. **The practical reach of the `wasm-host` feature.** `HostServices` — and therefore this section's
   entire host surface — sits behind it. Which binaries and test profiles enable it, and whether a
   `cyrup-mcp` built without it should fail to compile or degrade to headless, was not traced. A
   defect hides as "MCP works in the dev build and silently has no panels in another profile".
6. **Whether an out-of-range panel cursor is reachable after a reconnect that shrinks the list.**
   Established that upstream's renderer and every key handler guard the `undefined`, so a Rust port
   using `Vec::get` everywhere is safe and one using `[]` panics where upstream no-ops. What is
   unmeasured is whether the state occurs in practice, which needs the live-pty test MCP-360 names.
