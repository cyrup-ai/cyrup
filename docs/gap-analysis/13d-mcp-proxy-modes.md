# 13d · Proxy modes and search ranking

> Part of **[13 — cyrup-mcp](13-cyrup-mcp.md)**, which holds the thesis, the seam map, the
> architecture and the one canonical table of every port unit. Method and phasing are in
> **[MCP-PORT-METHODOLOGY.md](MCP-PORT-METHODOLOGY.md)**.

This section owns the **one tool the model sees**. After the scope cuts, `pi-mcp-adapter`'s entire
model-facing surface reduces to a single registered tool named `mcp` whose *arguments* select one of
nine behaviours, plus the per-server direct tools another section owns. Everything else in the
adapter — transports, OAuth, the metadata cache, lifecycle — exists to keep that one tool answering.
The whole of it is `hand-written` Rust inside `crates/cyrup-mcp`, calling `rmcp` for the wire and
three existing cyrup verbs for the host: `InitApi::register_tool` to install the tool,
`HostServices::set_status` for the footer, and `ToolRenderKind` for the render shell. **Nothing in
cyrup's core changes to make this section work.**

Three properties a porter must internalise before writing a line. **First, the `mcp` tool's
description is data, not a literal.** `buildProxyDescription` (`direct-tools.ts`) regenerates it from
the current config and the on-disk metadata cache on every surface sync, and `syncProxyTool`
(`index.ts`) re-registers the whole tool whenever the generated text differs. The model learns which
servers exist, how many tools each has, which are disabled, a 150-character snippet of each server's
own instructions, and a nine-line usage cheatsheet — all from that regenerated string. A port that
hard-codes it ships a gateway the model cannot discover anything through. This is the section's one
contact point with the seam map's `HA-1`: the *mechanism* for late registration exists and works
(`ExtensionHost::register_late_tool` → `refresh_tools` → `AgentSession::refresh_extension_tools` →
`push_active_tools`, at every turn boundary of a live run); what a native extension lacks is the
**handle** to reach it. On a cold cache the degradation is graceful — the description is frozen at
the cold-cache text for one session — so this is scheduling information, not a severity story.

**Second, `executeCall` is not a dispatcher, it is a resolution state machine** with five entry
paths and five auto-auth retry points fenced by one function-scoped boolean. A bare tool name can
resolve against already-known metadata, against a server hint, by lazily connecting a server whose
prefix the name starts with, or by connecting and re-resolving after the handshake — and at five of
those points a `needs-auth` connection can trigger `attemptAutoAuth`, close, reconnect and resolve
again. `autoAuthAttempted` latches all of them, including the `withSessionRecovery` callback that
fires mid-request. Get the latch wrong and a misconfigured OAuth server opens a browser flow per
resolution attempt. Get the **ambiguity gate** wrong and a call silently reaches the wrong server's
same-named tool — that is this section's only `critical`.

**Third, `details.error` is the contract, not the text.** Every mode returns
`{ content: [{type:"text", text}], details: {…} }`, and `details.error` is a machine-readable code
that downstream code branches on: `error-signal.ts`'s `toolErrorOverride` re-flags exactly
`tool_error` and `call_failed` as `isError`, and nothing else. Port the prose loosely at your peril;
port the codes byte-exactly. Thirty-two codes survive the cuts, of which thirty-one are reachable.

The ranking half is smaller and purer. `search-ranking.ts` is 206 lines of allocation-free integer
scoring with no I/O: it backs `mcp({search})`, every "Did you mean:" suggestion in the package, and
nothing else. It ports as a leaf module with an executable specification already written — eleven
upstream conformance cases that transfer verbatim.

*Provenance: upstream is `pi-mcp-adapter` v2.25.0; cyrup is branch `david/cyrup`; rmcp is 3.1.2.*

---

### How it lands

| adapter capability | upstream mechanism | cyrup mechanism | verdict |
|---|---|---|---|
| register the `mcp` tool | `index.ts` `registerProxyTool` → `pi.registerTool` | `InitApi::register_tool(Arc<dyn Tool>)` during `NativeExtension::init` | **host-verb** |
| the tool's JSON Schema | TypeBox `Type.Object` + `optionalNumber` shim | `Tool::parameters() -> &serde_json::Value`, a `OnceLock` literal — the TypeBox/Gemini shim has no analogue | **hand-written** |
| regenerate + re-register the description | `syncProxyTool` re-registers on text change | `ExtensionHost::register_late_tool` exists and propagates; a native has no handle | **host-addition** (`HA-1`) |
| hide the tool (`disableProxyTool`) | `deactivateTools` — optional `pi.unregisterTool`, else `setActiveTools` minus the name | `HostServices::{active_tools, set_active_tools}` — upstream's own no-`unregisterTool` branch | **host-verb** |
| render shell / call / result binding | `renderShell`, `renderCall`, `renderResult` | `Tool::{render_kind, render_call, render_result}`, `ToolRenderKind::{Default, SelfRendered}` | **host-verb** |
| mode dispatch, args coercion, init gate | `index.ts` `execute` | `Tool::execute(call_id, params, cancel, on_update) -> Result<ToolResult, ToolError>` | **hand-written** |
| nine `execute*` modes | `proxy-modes.ts` | pure Rust over `cyrup-mcp`'s own state | **hand-written** |
| `tools/list` behind `connect` | SDK `client.listTools` paginated | `Peer::list_all_tools` | **rmcp** |
| `tools/call` | `client.callTool(...)` wrapped in `abortable(…, signal)` | `Peer::send_request_with_option(ClientRequest::CallToolRequest(CallToolRequestParams{…}), PeerRequestOptions{timeout, …})` → `RequestHandle::{await_response, cancel}`; or `RunningService::call_tool` for the MRTR helper | **rmcp** |
| `resources/read` behind a resource tool | `client.readResource({uri}, requestOptions)` | `Peer::read_resource` | **rmcp** |
| per-request timeout / cancellation | `manager.getRequestOptions(server, signal)` | `PeerRequestOptions { timeout, reset_timeout_on_progress, max_total_timeout }`; `RequestHandle::cancel(reason)` emits `notifications/cancelled` | **rmcp** |
| run/turn cancellation | `combineAbortSignals(owner.signal, signal)` | `tokio_util::sync::CancellationToken` child of the dispatch `CancelToken`; `HostServices::is_run_cancelled` for the run-scoped poll | **host-verb** + **extension-owned** |
| ranking, tokenisation, pagination, suggestions | `search-ranking.ts` | leaf module, integer arithmetic | **hand-written** |
| regex search + ReDoS gate | `new RegExp(q,"i")` + `recheck.checkSync` | `regex` (already in the lock file) with explicit `size_limit`/`dfa_size_limit`; linear-time matching makes the gate unnecessary | **hand-written** |
| insertion-ordered server/metadata maps | JS `Map` / object key order | `indexmap::IndexMap` inside `cyrup-mcp`, deserialised directly — no workspace change | **extension-owned** |
| footer status during connect | `state.ui.setStatus("mcp", …)` | `HostServices::set_status(key, Option<&str>)` — a default no-op, exactly upstream's `if (state.ui)` guard | **host-verb** |
| native-tool detection (`getPiTools`) | `pi.getAllTools()`, passed as an **optional** callback | `HostServices::all_tool_names() -> Option<Vec<String>>`; `None` == upstream's `getPiTools === undefined` branch | **host-verb** |
| approval gate | `ensureToolCallApproved` + broker event | `ensureToolCallApproved`'s **local** gate ports; the gate proper is `ExtHooks::before_tool_call` + `cyrup-permission-system` | **hand-written** + **host-verb** |
| browser open for auto-auth | npm `open` | `opener::open` called directly by the native crate | **extension-owned** |
| `mcpScript`, `tools` Proxy, JS sandbox | `mcp-code.ts`, `mcp-script-worker.mjs` | — | **cut** |
| `mcp({action:"ui-messages"})` | `executeUiMessages` | — | **cut** |

---

### Behavioural specification

#### 1. The registered tool

| field | value |
|---|---|
| `name` | `mcp` — **fixed by cross-crate contract**, see §13 |
| `label` | `MCP` |
| `description` | regenerated — §2 |
| `promptSnippet` | `MCP gateway — status, search, describe, auth, and single MCP tool calls` |
| `renderShell` | config-forked, below |
| `renderCall` | `createMcpProxyToolCallRenderer(toolRenderOptions)` |
| `renderResult` | `createMcpToolResultRenderer(toolRenderOptions)` |

**`renderShell` is not a constant.** `index.ts` computes
`toolRenderShell = toolRenderOptions.resultRendering === "compact" ? "self" : "default"`, and
`tool-result-renderer.ts`'s `resolveMcpToolRenderOptions` sets
`resultRendering = settings?.toolResultRendering === "boxed" ? "boxed" : "compact"`. So the shell is
`"self"` **by default** and `"default"` exactly when the user sets
`settings.toolResultRendering: "boxed"` — read from the *early* config at load time, so it never
changes within a session. In cyrup: `ToolRenderKind::SelfRendered` vs `ToolRenderKind::Default`
(`crates/cyrup-core/src/tool.rs`), chosen once at construction.

The JSON Schema handed to the provider. Every property is optional, so no `required` is emitted;
`args` is a union. Upstream's `optionalNumber` helper exists only to dodge a TypeBox 1.x artefact
(an enumerable `~optional` key that Gemini rejects with `400 INVALID_ARGUMENT`); both of its branches
serialise identically, and in Rust `Tool::parameters()` returns a raw JSON Schema, so the shim
evaporates. Emit this:

```json
{
  "type": "object",
  "properties": {
    "tool":           {"type":"string",  "description":"Tool name to call (e.g., 'xcodebuild_list_sims')"},
    "args":           {"anyOf":[
                         {"type":"string","description":"Arguments as a JSON string (e.g., '{\"key\": \"value\"}')"},
                         {"type":"object","additionalProperties":true,"description":"Arguments as a JSON object (e.g., { \"key\": \"value\" })"}],
                       "description":"Tool arguments as a JSON object, or as a JSON string encoding one"},
    "connect":        {"type":"string",  "description":"Server name to connect (lazy connect + metadata refresh)"},
    "describe":       {"type":"string",  "description":"Tool name to describe (shows parameters)"},
    "instructions":   {"type":"string",  "description":"Server name to show that server's usage instructions"},
    "search":         {"type":"string",  "description":"Search tools by name/description"},
    "regex":          {"type":"boolean", "description":"Treat search as regex (default: substring match)"},
    "includeSchemas": {"type":"boolean", "description":"Include parameter schemas in search results (default: true)"},
    "limit":          {"type":"number",  "minimum":1, "description":"Maximum search results to return (default: 12)"},
    "offset":         {"type":"number",  "minimum":0, "description":"Search result offset (default: 0)"},
    "server":         {"type":"string",  "description":"Filter to specific server (also disambiguates tool calls)"},
    "action":         {"type":"string",  "description":"Action: 'auth-start' or 'auth-complete'"}
  }
}
```

**One cut-driven edit**: `action`'s description upstream reads
`"Action: 'ui-messages', 'auth-start', or 'auth-complete'"`. With MCP Apps out of scope there are
exactly two legal values, and the description must say so — a model told about `ui-messages` will
call it and get a `mcp_status` fall-through with no explanation. **Twelve properties, all optional,
all keeping their upstream names**: renaming any of the five the permission system reads silently
changes which rules apply (§13).

Property order above is upstream source order. cyrup's `serde_json` is built without
`preserve_order`, so a `serde_json::Value` schema serialises keys alphabetically —
`action, args, connect, describe, includeSchemas, instructions, limit, offset, regex, search,
server, tool`. See MCP-194.

#### 2. `buildProxyDescription` — the regenerated description

Assembled in this exact order, each block appended only when non-empty.
`prefix = config.settings?.toolPrefix ?? "server"`; `INSTRUCTIONS_SNIPPET_LENGTH = 150`.

1. **Header, always**, ending in a newline. Upstream:
   `MCP gateway — server status, tool search/describe, auth, and single MCP tool calls. When one request needs several MCP calls with logic between them, use mcpScript. Non-MCP Pi tools should be called directly, not through mcp.\n`
   **Post-cut**, with the `mcpScript` sentence removed and the host renamed:
   `MCP gateway — server status, tool search/describe, auth, and single MCP tool calls. Non-MCP cyrup tools should be called directly, not through mcp.\n`
2. **Direct tools**, when ≥1 direct-tool spec exists — counts accumulated into a map keyed by server
   in `directSpecs` iteration order:
   `\nDirect tools available (call as normal tools): <server> (<n>), …\n`
3. **Servers** — for each key of `config.mcpServers` in insertion order:
   - skip a missing or disabled definition;
   - `entry` = the cache entry only when `isServerCacheValid(cachedEntry, definition)`, else
     `undefined` — a stale entry is *not* skipped here, it just yields zero counts;
   - `toolCount` = cached tools passing `isUiToolVisibleToModel(uiVisibility)` **and**
     `isToolAllowed(name, server, effectivePrefix, includeTools, excludeTools,
     getOtherCurrentCandidates(name))`;
   - `resourceCount` = cached resources passing the same `isToolAllowed` on
     `read_<resourceNameToToolName(name)>`, or 0 when `exposeResources === false`;
   - `totalItems === 0` ⇒ `continue` — this is how a stale or missing cache entry drops out;
   - `proxyCount = totalItems − directCount`, emitted only when `> 0`.

   Then, when ≥1 summary: `\nServers: <server> (<proxyCount> tools), …\n`.

   **`getOtherCurrentCandidates` is not a formality.** For every tool it builds the set of name
   candidates produced by *every other* cache-valid, enabled server (including
   `read_<resource>` names when `exposeResources !== false`), removes this tool's own candidates,
   and hands the remainder to `isToolAllowed` as its collision set. The counts the model reads are
   therefore an O(servers × tools) cross-server computation, not a per-server filter: adding an
   unrelated server can change a third server's advertised count. See MCP-198.

   **`isUiToolVisibleToModel` survives the MCP Apps cut** (seam map, Cut 2 seam): dropping it would
   expose to the model tools the server explicitly marked app-only.
4. **Disabled**, when ≥1:
   `\nDisabled servers (enable with /mcp enable <server> and /reload): <a>, <b>\n`
5. **Instructions**, when ≥1 enabled server has cached, cache-valid `instructions`:
   `\nServer instructions (truncated - full text via mcp({ instructions: "name" })):\n  <server>: <snippet>\n…\n`
   where `<snippet> = truncateAtWord(instructions.replace(/\s+/g," ").trim(), 150)` and the
   two-space indent is part of each summary line.
6. **Usage block, always**, byte-exact including the two-space indent and the arrow glyph `→`. The
   final `Mode:` line carries **no** trailing newline. Post-cut (the
   `mcp({ action: "ui-messages" })` line removed; every other line and the `Mode:` precedence line
   unchanged):

```
Usage:
  mcp({ })                              → Show server status
  mcp({ server: "name" })               → List tools from server
  mcp({ search: "query" })              → Search MCP tools by name/description
  mcp({ describe: "tool_name" })        → Show tool details and parameters
  mcp({ instructions: "name" })         → Show full server usage instructions
  mcp({ connect: "server-name" })       → Connect to a server and refresh metadata
  mcp({ tool: "name", args: { key: "value" } })         → Call a tool (object args; JSON string also accepted)
  mcp({ action: "auth-start", server: "name" })      → Start manual OAuth and get a browser URL
  mcp({ action: "auth-complete", server: "name", args: { redirectUrl: "..." } }) → Complete manual OAuth

Mode: action > tool (call) > connect > describe > instructions > search > server (list) > nothing (status)
```

`truncateAtWord(text, target)` (`utils.ts`): returns `text` unchanged when falsy or when
`text.length <= target`; else slices to `target`, and if the last space in that slice is at index
`> target * 0.6` returns `slice(0, lastSpace) + "..."`, otherwise `slice + "..."`. JS `.length` is
UTF-16 code units; a Rust port using `char_indices` diverges on astral-plane text (rare in tool
descriptions, but state the choice).

`syncProxyTool` also **hides** the tool: when `settings.disableProxyTool === true` **and**
`directSpecs.length > 0` **and** no configured direct-tool server is missing from the cache, it calls
`deactivateTools(["mcp"])`. Otherwise it registers, and — when already registered with an unchanged
description — re-adds `"mcp"` to the active set if it fell out.

#### 3. Mode dispatch

Runs before any mode, in this order:

1. **Args coercion.** Only when `params.args !== undefined && params.args !== ""`. A `string` is
   `JSON.parse`d; a `SyntaxError` is rethrown as `Invalid args JSON: <e.message>` with `cause`
   preserved; any other parse throw propagates. A non-string is taken as-is. The result must be a
   non-null, non-array object or the call throws
   ``Invalid args: expected a JSON object, got <gotType>`` where `gotType ∈ {array, null, string,
   number, boolean, …}`. **These two are thrown, not returned** — they surface as tool-execution
   errors (`Err(ToolError)` in cyrup), not as `details.error` codes.
2. **Init-wait gate.** When state is null but an init promise is live, race it against
   `INIT_WAIT_TIMEOUT_MS = 30_000` with an unref'd timer.
   - timeout → `{content:[{text:"MCP initialization is still in progress. Try again shortly."}], details:{error:"init_timeout", timeoutMs:30000}}`
   - rejection → rethrow if it is the owner's abort, else
     `{text:"MCP initialization failed: <msg>", details:{error:"init_failed", message}}`
   - still null → `{text:"MCP not initialized", details:{error:"not_initialized"}}`

   These three carry **no `mode` key**. (Upstream's `mcpScript` twin set `mode:"script"`; with the
   scripting cut there is no longer an asymmetry to reproduce.)
3. **Generation fence** — `executeOwner?.throwIfInactive()`, i.e. a stale lifecycle generation
   aborts rather than writing into a restarted session.
4. **Dispatch**, first match wins. Nine arms after the cut, in unchanged relative order:

| # | condition | mode |
|---|---|---|
| 1 | `action === "auth-start"` | requires `server`, else `{mode:"auth-start", error:"missing_server"}` with text ``auth-start requires `server`. Example: mcp({ action: "auth-start", server: "linear-server" })`` |
| 2 | `action === "auth-complete"` | requires `server` (`{mode:"auth-complete", error:"missing_server"}`, text ``auth-complete requires `server`.``); then `input = parsedArgs?.redirectUrl ?? parsedArgs?.code ?? parsedArgs?.input`; a non-string or blank input → `{mode:"auth-complete", error:"missing_input"}`, text ``auth-complete requires args with `redirectUrl`, `code`, or `input`.`` |
| 3 | `params.tool` truthy | `executeCall(state, tool, parsedArgs, params.server, getPiTools, signal)` — `origin` left unset |
| 4 | `params.connect` truthy | `executeConnect(…)` **then `syncToolSurface(ctx)`** before returning |
| 5 | `params.describe` truthy | `executeDescribe(state, describe)` |
| 6 | `params.instructions` truthy | `executeInstructions(state, instructions)` |
| 7 | `params.search !== undefined` | `executeSearch(state, search, regex, server, includeSchemas, limit, offset)` — **`!== undefined`**, so `search: ""` reaches the mode |
| 8 | `params.server` truthy | `executeList(state, server)` |
| 9 | otherwise | `executeStatus(state)` |

An unrecognised `action` value (`action:"frobnicate"`, and now also `action:"ui-messages"`) falls
through arms 1-2 and lands on whichever of 3-9 matches — it is **not** an error. With no other
argument present that is `executeStatus`.

#### 4. Discovery modes — `status`, `list`, `instructions`

**`executeStatus`.** Per server key of `config.mcpServers`, in insertion order, compute `status` by
this ladder: `disabled` (`isServerDisabled`) → `connected` → `needs-auth` → `failed` (when
`getFailureAgeSeconds` is non-null) → `cached` (metadata present) → `not connected`.
`toolCount = metadata?.length ?? 0`; `metadata`/`connection` are forced to `undefined` and
`failedAgo` to `null` for disabled servers.

Header: `MCP: <connectedCount>/<enabledCount> servers, <totalTools> tools`, plus
` (<n> disabled)` when `n > 0`, then `\n\n`. `totalTools` and `connectedCount` count **enabled
servers only**. Body, one line per server in the same order, byte-exact glyphs:

| status | line |
|---|---|
| disabled | `⊘ <name> (disabled)\n` |
| connected | `✓ <name> (<n> tools)\n` |
| needs-auth | `⚠ <name> (needs auth)\n` |
| cached | `○ <name> (<n> tools, cached)\n` |
| failed | `✗ <name> (failed <s>s ago)\n` — `failedAgo ?? 0` |
| not connected | `○ <name> (not connected)\n` |

Footer when ≥1 server: `\nmcp({ server: "name" }) to list tools, mcp({ search: "..." }) to search`.
Final text is `.trim()`ed.
`details = {mode:"status", servers, totalTools, connectedCount, disabledCount}` where each
`servers[i] = {name, status, toolCount, failedAgo, disabled?: true}` — the `disabled` key present
only when true.

**`executeList`.**
1. unknown server → `{mode:"list", server, tools:[], count:0, error:"not_found"}`, text
   `Server "<s>" not found. Use mcp({}) to see available servers.`
2. disabled → the shared `disabledResult("list", s)` (§12).
3. Build `instructionsText` from `state.serverInstructions` with
   `INSTRUCTIONS_PREVIEW_LENGTH = 300`: `\n\nServer instructions:\n<preview>`, plus
   `\nUse mcp({ instructions: "<s>" }) for the full text.` **only when the preview differs from the
   full text**.
4. Zero tools, three sub-cases:
   - connected → `Server "<s>" has no tools.<instructionsText>` · `{…count:0, hasInstructions}`
   - metadata present but not connected → `Server "<s>" has no cached tools (not connected).<instructionsText>` · `{…cached:true, hasInstructions}`
   - no metadata → `Server "<s>" is configured but not connected. Use mcp({ connect: "<s>" }) or /mcp reconnect <s> to retry.<instructionsText>` · `{…error:"not_connected", hasInstructions}`
5. Otherwise header `<s> (<n> tools<cachedNote>):\n\n` where `cachedNote` is
   ` (not connected, cached)` unless the connection is `connected`; then one line per tool in
   metadata order: `- <name>` plus ` - <truncateAtWord(desc, 50)>` when the truncated description is
   non-empty; then `instructionsText`; then `.trim()`.
   `details = {mode:"list", server, tools:[names], count, hasInstructions}`.

**`executeInstructions`.** `not_found` / `server_disabled` as above, then in this order: cached
instructions → `<s> instructions:\n\n<instructions>` with `{mode:"instructions", server, length}`;
connected but no instructions → `Server "<s>" does not provide instructions.` /
`error:"no_instructions"`; else
`No instructions cached for "<s>". Use mcp({ connect: "<s>" }) to connect and refresh.` /
`error:"not_connected"`. **Cached instructions win even for a disconnected server.**

#### 5. `executeDescribe`

1. **Ambiguity first.** `getEnabledToolMatches(state, name, exact=true)` — if >1, ambiguous. Else if
   0 exact and `getEnabledToolMatches(..., exact=false)` > 1, ambiguous.
   `ambiguousToolResult("describe", name)` → text and `details.message` =
   `Tool "<n>" matches multiple servers. Specify a server.`;
   `details = {mode:"describe", error:"ambiguous_tool", requestedTool, message}`.
2. Take the single exact match; if none, walk `state.toolMetadata` in insertion order with
   `findToolByName` (hyphen-normalised), remembering the **first** disabled hit (`??=`) and breaking
   on the first enabled hit.
3. No hit: if a disabled server matched → `disabledResult("describe", disabledMatch)`. Else
   `suggestions = rankSuggestions(state, name, 5)` and
   `Tool "<n>" not found. Use mcp({ search: "..." }) to search.` + ` Did you mean: <a, b, c>` when
   non-empty; `details = {mode:"describe", error:"tool_not_found", requestedTool, suggestions}`.
4. Render — note `formatSchema` is called here with **no indent argument**, unlike §6:

```
<toolMeta.name><" (requires approval)" if isToolCallApprovalRequired>
Server: <serverName>
[Type: Resource (reads from <resourceUri>)      ← only when resourceUri]

<description or "(no description)">

<one of:>
  Shape:
  <renderTsShape(inputSchema)>                  ← when inputSchema && !resourceUri && shape !== null
  Parameters:
  <formatSchema(inputSchema)>                   ← when shape === null
  No parameters required (resource tool).       ← when resourceUri
  No parameters defined.                        ← otherwise
```

then `.trim()`. `details = {mode:"describe", tool: <the whole ToolMetadata>, server}`.

#### 6. `executeSearch`

Signature defaults: `limit = 12`, `offset = 0`; `showSchemas = includeSchemas !== false` (so
`undefined` ⇒ true). A `server` that is disabled short-circuits to `disabledResult("search", server)`.

Three mutually exclusive selection paths.

**(a) `regex` truthy**, in this exact order:
1. `query.length > MAX_REGEX_SEARCH_QUERY_LENGTH` (= 256) → `Regex query is too long; maximum length is 256 characters.` / `{mode:"search", error:"query_too_long", query, maxLength:256}`.
2. Compile the pattern case-insensitively — a compile failure reaches the outer catch:
   `Invalid regex: <query>` / `{mode:"search", error:"invalid_pattern", query}`.
3. *(Upstream: the `recheck` ReDoS gate — out of scope, §Out of scope.)*
4. Scan every `(serverName, metadata)` of `state.toolMetadata` in insertion order, skipping disabled
   servers and (when `server` is set) non-matching servers; a tool matches when
   `pattern.test(tool.name) || pattern.test(tool.description) ||
   resolveSearchKeywords(definition, tool.originalName, serverName, globalPrefix).some(k => pattern.test(k))`,
   with `globalPrefix = settings?.toolPrefix ?? "server"`.
   **Every match gets `score: 0` and the list is never sorted** — output order is server-insertion
   order then per-server metadata order, and it is observable in `details.matches`.

**(b) `query.trim().length === 0`**: with no `server` → `Search query cannot be empty` /
`{mode:"search", error:"empty_query"}`. With a `server`, take all of that server's metadata,
`score: 0`, sorted by `a.tool.name.localeCompare(b.tool.name)`.

**(c) otherwise**: `rankToolMatches(state, query, server)` — §7, keyword-aware, sorted by score
descending then `localeCompare` ascending.

**Rendering.** `paginate(matches, offset, limit)`. **Zero results**: compute `connectingServers` —
when `server` is set, `[server]` iff it is configured **and** `manager.isConnecting(server)`;
otherwise every configured, non-disabled, connecting server sorted by `localeCompare`. Base message
`No tools matching "<q>" in "<server>"` or `No tools matching "<q>"`, suffixed with
` Server "<x>" is still connecting; retry in a moment.` (exactly 1) or
` Servers "<a>", "<b>" are still connecting; retry in a moment.` (≥2) or nothing.
`details = {mode:"search", matches:[], count:0, hasMore:false, nextOffset:null, query, connectingServers?}`
— the `connectingServers` key present only when non-empty.

**Non-zero**: header `Found <total> tool<s> matching "<q>":\n\n` (`tool` singular iff
`total === 1`), then per item:

- with schemas: `<name><approvalMarker>\n`, `  <description or "(no description)">\n`, then when
  `inputSchema && !resourceUri`: `\n  Shape:\n` + `renderTsShape` output with every line prefixed by
  4 spaces + `\n`, or `\n  Parameters:\n<formatSchema(schema, "    ")>\n` when the shape renderer
  returns null; when `resourceUri`: `  No parameters (resource tool).\n`. Then a blank line.
- without schemas: `- <name><approvalMarker>` plus ` - <truncateAtWord(description, 50)>` when a
  description exists, then `\n`.

`approvalMarker` is `" (requires approval)"` or `""`. Footer when `page.hasMore`:
`\n<page.items.length> of <total> — offset: <nextOffset> for more\n` (em-dash). Whole text
`.trim()`ed.
`details = {mode:"search", matches:[{server, tool:<prefixed name>, score}], count:<total>, hasMore, nextOffset, query}`.

#### 7. The ranking algorithm (`search-ranking.ts`)

**Normalisation.** `normalizeSearchText(v)` applies, in order:
1. `v.replace(/([a-z0-9])([A-Z])/g, "$1 $2")` — camelCase split, **before** lowercasing (so `ID`
   does not split; the pattern needs a lowercase or digit *before* the uppercase).
2. `.replace(/[_./:-]+/g, " ")` — the class is exactly `_ . / : -`, runs collapsed to one space.
3. `.toLowerCase()`.

`tokenize(v)` = `normalizeSearchText(v).split(/[^a-z0-9]+/).filter(Boolean)` — ASCII-only; any
non-`[a-z0-9]` byte is a separator, so non-ASCII identifiers tokenize to nothing.

**Constants.** `FIELD_WEIGHTS = { name: 12, originalName: 10, server: 8, description: 5,
keywords: 5 }`; `MIN_STEM_LENGTH = 4`.

**`scoreToolMatch(tool, server, query, keywords?) -> Option<i64>`:**

1. `normalizedQuery = normalizeSearchText(query).trim()`; `queryTokens = tokenize(query)`. Empty
   `queryTokens` ⇒ `None`.
2. Four fields in this order: `name`, `originalName`, `server`, `description`, each normalised but
   **not** trimmed — a leading space in a description defeats `starts_with`.
3. Per field, exactly one **phrase** bonus, first match wins:
   - `value == normalizedQuery` ⇒ `+weight*14`, sets `phraseMatched` **and** `wholeFieldExact`;
   - `value.starts_with(normalizedQuery)` ⇒ `+weight*9`, sets `phraseMatched`;
   - `value.contains(normalizedQuery)` ⇒ `+weight*6`, sets `phraseMatched`.
4. Per field, per query token, exactly one **token** bonus, first match wins:
   - token ∈ fieldTokens ⇒ `+weight*4`;
   - else some `fieldToken.starts_with(token)` **or** (`fieldToken.len() >= MIN_STEM_LENGTH` and
     `token.starts_with(fieldToken)`) ⇒ `+weight*2`;
   - else `value.contains(token)` ⇒ `+weight*1`.

   Any of the three adds the token to `matchedTokens`. **The stem rule is deliberately asymmetric**:
   `field.starts_with(token)` at any length, but `token.starts_with(field)` only when the field token
   is ≥ 4 characters. Real descriptions tokenize possessives into single letters (`"project's"` →
   `["project","s"]`), which would otherwise make every query starting with that letter a match.
5. **Keywords**, only when `Some` and non-empty. `phrases = keywords.map(normalizeSearchText).map(trim).filter(non-empty)`.
   The phrase bonus is a **max over phrases** (`phraseScore = max(phraseScore, …)`), added **once**
   — deliberately, so a query spanning two unrelated keywords cannot collect a phrase bonus twice.
   Token bonuses then run against `keywordTokens = phrases.flat_map(tokenize)` with the identical
   three-way ladder, except the weakest tier tests `phrases.iter().any(|p| p.contains(token))`.
   `wholeFieldExact` and `phraseMatched` can both be set from a keyword phrase.
6. **Coverage gate.** `coverage = matchedTokens.len() / queryTokens.len()`.
   `if !phraseMatched && (queryTokens.len() <= 2 ? coverage != 1.0 : coverage < 0.6) { return None }`
   — without a phrase match, a 1-2 token query must match **all** its tokens and a longer query must
   reach 0.6.
7. **Final bonuses.** `+25` when coverage is exactly 1, else `+round(coverage * 10)` (JS
   `Math.round` is half-away-from-zero on positives = Rust `f64::round`); `+8` when
   `tokenize(fields.name)` contains the **first** query token; `+20` when `wholeFieldExact`.

Compute "full coverage" as the integer comparison `matched == total`, not a float equality.

**`rankToolMatches(state, query, server?, includeKeywords = true)`:** walk `state.toolMetadata` in
insertion order, skip disabled servers and (when `server` is set) non-matching servers, compute
`hasKeywords = includeKeywords && definition?.searchKeywords !== undefined` (**an empty object still
counts as present**, which changes whether `keywords` is `Some([])` or `None`; `Some([])` is a no-op
by step 5's non-empty guard), score each tool, keep `Some` scores. Sort by score descending then
`a.tool.name.localeCompare(b.tool.name)`.

**`resolveSearchKeywords(definition, toolOriginalName, serverName, globalPrefix)`:** a missing,
non-object or array `searchKeywords` yields `[]`. Candidates =
`getToolNameCandidates(originalName, serverName, resolveToolPrefix(definition, globalPrefix))`. For
each `[pattern, values]` **in object insertion order**, skip non-array values, skip when
`matchesToolPattern(candidates, [pattern])` is false, then push each trimmed non-empty string value
not already seen. Result is the ordered, deduped union across all matching patterns. Keys match by
original name, prefixed name, and glob: `{"search_*": ["records","fuzzy lookup"],
"search_records_advanced": ["fuzzy lookup","legacy"]}` ⇒ `["records","fuzzy lookup","legacy"]`.

Configured keywords are searchable by ranked query **and** by regex, but never appear in schemas,
`describe` output, or the metadata cache — `searchKeywords` is read only by `search-ranking.ts` and
the `ServerEntry` type.

**`paginate(items, offset, limit)`:**
`safeOffset = offset.is_finite() ? max(0, trunc(offset)) : 0`;
`safeLimit = limit.is_finite() ? max(1, trunc(limit)) : 1`;
`page = items[safeOffset .. safeOffset+safeLimit]` (JS `slice` clamps; Rust must clamp both ends);
`nextOffsetRaw = safeOffset + page.len()`; `hasMore = nextOffsetRaw < total`;
`nextOffset = hasMore ? Some(nextOffsetRaw) : None`.
Pinned: `paginate(["a","b","c"], 1, 1)` ⇒ `{items:["b"], total:3, hasMore:true, nextOffset:2}`;
`paginate(["a","b","c"], 5, 1)` ⇒ `{items:[], total:3, hasMore:false, nextOffset:null}`.

**`rankSuggestions(state, name, limit)`:**

```
stripped = mcpServers.keys()
   .flat_map(|s| ["server","short","mcp"].map(|p| getServerPrefix(s, p)))
   .filter(|c| !c.is_empty() && name.starts_with(&format!("{c}_")))
   .sorted_by(|a,b| b.len().cmp(&a.len()))
   .map(|c| name[c.len()+1..].to_string());
query = stripped.first().cloned().unwrap_or(name);
rankToolMatches(state, &query, None, /*includeKeywords=*/false)[..limit].map(|m| m.tool.name)
```

Three prefix modes are probed — `server`, `short`, `mcp` — regardless of the configured mode, and
`none` is deliberately excluded (it yields an empty prefix). `includeKeywords = false`, so a
suggestion never comes from a configured alias. **`getServerPrefix` is the one place this section
collides with an existing cyrup implementation — see MCP-178.**

#### 8. `executeConnect` and the auto-auth ladder

**`attemptAutoAuth`** returns `{status:"skipped"} | {status:"success"} | {status:"failed", message}`:

1. `settings.autoAuth !== true` ⇒ **skipped**. Opt-in, not opt-out.
2. Missing / disabled / non-OAuth definition ⇒ skipped.
3. `resolveServerUrl(definition)` **throwing** (missing `${VAR}`, invalid URL) ⇒ **failed** with
   `getAuthFailedMessage`. A falsy URL ⇒ skipped. (Throw is a failure, not a skip — reproduce that.)
4. `grantType = definition.oauth?.grantType ?? "authorization_code"`. **No interactive UI and grant
   type is not `client_credentials`** ⇒ failed, message = `getAuthRequiredMessage(state, s, <the
   standard default>)` — so a configured `settings.authRequiredMessage` still wins over that
   default.
5. `authenticate(serverName, serverUrl, definition, opts)` where `opts` carries `authStorageOptions`
   when present, `signal` when present, and always `runtime: state.oauthRuntime`. Upstream's
   four-way branch exists only to avoid passing `undefined` keys; Rust builds one options struct.
6. Abort errors rethrow; anything else ⇒ failed with `getAuthFailedMessage`.

**`executeConnect`.** `ownedSignal = combineAbortSignals(state.owner?.signal, signal)`,
`throwIfAborted`. `not_found` → `{mode:"connect", error:"not_found", server}`; disabled →
`disabledResult("connect", s)`.

Inside a try: set the footer to `formatMcpStatus(config, "connecting to <s>...")` when an
interactive surface exists; **reconnect** if already connected, else **connect**. On `needs-auth`:
`attemptAutoAuth`; `failed` ⇒ `{mode:"connect", error:"auth_required", server, message}`; `success`
⇒ close, re-check abort, connect again; still `needs-auth` ⇒ `auth_required` with
`getAuthRequiredMessage`.

On success, in this order: compute `prefix`; `buildToolMetadata(...)` → store into
`state.toolMetadata`; when `!connection.promptDiscoveryFailed` store `reconstructPromptMetadata(...)`
and add the server to `promptMetadataLive`; **set or `delete`** `serverInstructions`;
`updateMetadataCache`; `notifyToolMetadataUpdated(state, s, "proxy-connect")`;
`markKeepAliveAfterConnect`; `clearFailure`; `updateStatusBar`; **return `executeList(state, s)`** —
so a successful connect renders as a list and `details.mode === "list"`.

Catch: `recordFailure` unless aborted, `updateStatusBar`, return
`Failed to connect to "<s>": <msg>` with `error: "aborted" | "connect_failed"`.

#### 9. `executeCall` — the resolution state machine

State: `serverName` (seeded from `serverOverride`), `toolMeta`, `autoAuthAttempted = false`,
`prefixMode = settings.toolPrefix ?? "server"`.

**Phase 1 — server hint given.** Unknown server ⇒
`{mode:"call", error:"server_not_found", server, requestedTool}`. Else `getSingleToolMatch` on that
server's metadata (ambiguous ⇒ ambiguous result), then a **disabled check after resolution** so the
error can name the resolved tool: `disabledCallResult` emits identity `{server, resourceUri}` for
resource tools, `{server, tool: originalName}` otherwise, or bare `{server, requestedTool}` when
nothing resolved.

**Phase 2 — no hint.** Ambiguity gate over all enabled servers (exact set, then fuzzy set; >1 in
either ⇒ ambiguous). Then two ordered scans of `state.toolMetadata`: first **exact name only**, then
`findToolByName` fuzzy; each remembers the first disabled hit (`??=`) and breaks on the first enabled
hit. **The fuzzy scan is guarded by `!toolMeta && !disabledMatch`** — an exact match on a *disabled*
server suppresses the fuzzy pass entirely, so a fuzzy-matching enabled server is never reached. If
only a disabled hit exists ⇒ `disabledCallResult`.

*This gate is the section's `critical`. `getSingleToolMatch` returns the sentinel `"ambiguous"` for
>1 match rather than picking the first; upstream's conformance suite calls this "fails closed for
duplicate unqualified proxy names". A port that resolves ambiguity by first-match routes the call to
a different server's same-named tool, silently.*

**Phase 3 — hinted server, tool still unknown:** `lazyConnect`. On success re-resolve via
`getSingleToolMatch`. On failure, if the connection is `needs-auth` and `!autoAuthAttempted`: latch,
`attemptAutoAuth`; `failed` ⇒ `{error:"auth_required", server, requestedTool, message}`; `success` ⇒
close, `clearFailure`, `lazyConnect` again, re-resolve, and if still unresolved return
`Tool "<t>" not found on "<s>" after reconnect.<suggestions>` with
`error:"tool_not_found_after_reconnect"`. Still `needs-auth` ⇒ `auth_required`. Still no tool and a
recorded failure ⇒ `Server "<s>" not available (last failed <n>s ago)` / `error:"server_backoff"`.

**Phase 4 — lazy prefix discovery**, only when there is **no** server and **no** tool and
`prefixMode !== "none"`. Candidates = every non-disabled configured server whose
`getServerPrefix(name, prefixMode)` is non-empty and for which `toolName.startsWith(prefix + "_")`,
**sorted by descending prefix length**. For each: skip when a recorded failure exists *and* the
connection is not `needs-auth`; `lazyConnect`; a `needs-auth` failure triggers the single-shot
auto-auth ladder again; record the first successfully-connected candidate in `prefixMatchedServer`
(used only for the error hint); collect exact matches into `lazyExactMatches` and single fuzzy
matches into `lazyFallbackMatches`, returning ambiguous the moment any single server yields >1 of
either. Finally: exacts win if any; `>1` ⇒ ambiguous; exactly 1 ⇒ adopt it.

**Phase 5 — unresolved.** When no `serverOverride` was given, consult `getPiTools()` for a
same-named non-`mcp` host tool; a hit returns
`"<t>" is a native Pi tool. Call <t> directly instead of using mcp({ tool: "<t>" }).` /
`error:"native_tool"` (host name to be substituted, MCP-163). Otherwise build `Tool "<t>" not
found.` plus either ` Server "<hint>" has: <a, b, c>` (when `getToolNames(state, hintServer)` is
non-empty, with `hintServer = serverName ?? prefixMatchedServer`) or
` Use mcp({ search: "..." }) to search.`, plus ` Did you mean: <…>` from `rankSuggestions(…, 5)`.
`details = {mode:"call", error:"tool_not_found", requestedTool, hintServer, suggestions}`.

**`getPiTools` is an optional callback upstream** (`getPiTools?: () => ToolInfo[]`, invoked as
`getPiTools?.()`). When absent, the native-tool check is skipped and the call falls through to
`tool_not_found`. cyrup's `HostServices::all_tool_names()` returning `None` — "no live session
backend attached" — is exactly that branch. No gap.

**Phase 6 — connection readiness.** `callIdentity` is fixed here and reused by every subsequent
result: `{server, resourceUri}` for resource tools, `{server, tool: originalName}` otherwise. A
`needs-auth` connection runs the single-shot ladder. A missing or non-connected connection: a
recorded failure ⇒ `server_backoff`; a missing definition ⇒ `Server "<s>" not connected` /
`server_not_connected`; else set the footer and connect, with the same ladder, then `clearFailure`,
`updateServerMetadata`, `updateMetadataCache`,
`notifyToolMetadataUpdated(state, s, "proxy-call-reconnect")`, `markKeepAliveAfterConnect`,
`updateStatusBar`, and re-resolve; failure to re-resolve returns
`Tool "<t>" not found on "<s>" after reconnect. <hint><suggestions>` where `hint` is
`Available tools on "<s>": <…>` or `Server "<s>" has no tools.`. A connect throw ⇒ `recordFailure`
unless aborted, `Failed to connect to "<s>": <msg>` / `aborted | connect_failed`.

**Phase 7 — post-connect disabled recheck.** The definition may have been swapped under a live
connection.

**Phase 8 — approval.** `ensureToolCallApproved(state, server, toolMeta, args, ownedSignal, origin ??
(toolMeta.resourceUri ? "resource" : "proxy"))`. `approval.reason === "denied"` ⇒
`The user declined approval to run MCP tool "<orig>" on server "<s>".` / `error:"approval_denied"`;
otherwise ⇒ `MCP tool "<orig>" on server "<s>" is approval-gated and requires an interactive
session.` / `error:"approval_required"`. Both carry `{mode:"call", server, tool: originalName}` —
**not** `callIdentity`, so a resource tool reports `tool` here rather than `resourceUri`.

The `origin` parameter keeps its `"proxy" | …` shape and its `"proxy"` default; the `"script"` call
site is gone with Cut 4, leaving `"proxy"` and the derived `"resource"`.

#### 10. `executeCall` — invocation and result shaping

`requestOptions = manager.getRequestOptions?.(server, ownedSignal) ?? (ownedSignal ? {signal} :
undefined)`. `outputGuardOptions = resolveMcpOutputGuardOptions(config.settings)`.
`recoverAuthConnection` is the `onNeedsAuth` callback handed to `withSessionRecovery`; it reuses the
same `autoAuthAttempted` latch and throws `SessionRecoveryAuthRequiredError` on failure.

Wrapped in try/finally with `manager.touch(server)` + `manager.incrementInFlight(server)` on entry
and `decrementInFlight` + `touch` in `finally`.

**Cancellation asymmetry to reproduce, not "fix":** the tool call is wrapped in
`abortable(conn.client.callTool(...), ownedSignal)`, but the resource read is **not** — a
`readResource` is cancellable only through `requestOptions.signal`. In rmcp that is: the tool call
goes through `Peer::send_request_with_option(...)` → `RequestHandle`, with a task that calls
`RequestHandle::cancel(reason)` when the owned token fires; the resource read simply awaits
`Peer::read_resource` with the same `PeerRequestOptions`.

**Three result paths** after the MCP Apps cut (upstream had four; the UI-enabled-tool path is gone):

1. **Resource tool**: `read_resource({uri: resourceUri})` under `withSessionRecovery`;
   `transformMcpResourceContents(result.contents ?? [], owner.signal)`; empty ⇒
   `[{type:"text", text:"(empty resource)"}]`; guard; return
   `{mode:"call", ...callIdentity, ...guardedMcpDetails(guarded)}`.
2. **Tool error** (`result.isError`): `transformMcpContent`, empty ⇒ `(empty result)`, guard with
   `prefix:"Error: "`, `suffix:"\n\nExpected parameters:\n<formatSchema>"` when an input schema
   exists, `emptyTextFallback:"Tool execution failed"`, `rawMcpResult: result`;
   `details.error = "tool_error"`, spread order `{mode, error, ...callIdentity, ...guarded}`.
3. **Tool success**: `resolveMcpResultContent`, empty ⇒ `(empty result)`, guard,
   `{mode:"call", ...guardedMcpDetails(guarded), ...callIdentity}` — note the spread order differs
   from paths 1 and 2, which matters only if a guard key ever collides with `server`/`tool`.

The call request itself is `{ name: originalName, arguments: args ?? {} }` — upstream additionally
passed `_meta: uiSession?.requestMeta`, which is gone with Cut 2, so `CallToolRequestParams.meta`
stays `None` unless `PeerRequestOptions::meta` is used for progress tokens.

**Catch**, in order:
- `SessionRecoveryAuthRequiredError` ⇒ `error.authMessage ?? getAuthRequiredMessage(...)`,
  `{error:"auth_required", ...callIdentity, message, autoAuthAttempted}`.
- `UrlElicitationRequiredError` ⇒ `manager.handleUrlElicitationRequired(server, error)` returns
  `accept | decline | cancel`; the message is
  `The original MCP tool did not run. Complete the opened browser interaction, then retry the tool.`
  for accept, else `The URL interaction was declined.` / `The URL interaction was cancelled.`;
  `{error:"url_elicitation_required", ...callIdentity, action}`.
- anything else ⇒ guard `[{text: message}]` with `prefix:"Failed to call tool: "` and the schema
  suffix; `details.message` becomes the literal string
  `output truncated; see outputGuard.fullOutputPath` when `guarded.outputGuard` is set, else the
  original message; `error: "aborted" | "call_failed"`.

(Upstream fired `uiSession?.sendToolCancelled(message)` on all three arms and closed a *reused* UI
session in `finally`; both go with Cut 2. The `finally` must still do `decrementInFlight` + `touch`.)

#### 11. Manual OAuth modes

**`executeAuthStart`**: `not_found` / `server_disabled` as usual. Then `resolveServerUrl`; a falsy
URL or `!supportsOAuth(definition)` ⇒ `Server "<s>" is not configured for OAuth over HTTP.` /
`error:"oauth_not_supported"`. `startAuth` returning no `authorizationUrl` means the flow completed
synchronously (client-credentials) ⇒ `OAuth authentication successful for "<s>".` /
`{mode:"auth-start", server, authenticated:true}`. Otherwise render
`formatManualAuthInstructions`, byte-exact: an array of literals joined by `\n`, with empty strings
preserved and the final `portNote` dropped when empty via `.filter(Boolean)`. **`portNote` itself
begins with `\n`**, so when a port is parseable the rendered text has a **blank line before it**:

```
MCP OAuth required for "<s>".

Open this URL in your local browser:

<authorizationUrl>

After approving, copy the full redirected localhost URL from your browser address bar and send it back with:
mcp({ action: "auth-complete", server: "<s>", args: { redirectUrl: "PASTE_REDIRECT_URL_HERE" } })

You can also pass just the `code` query parameter as `args: { code: "PASTE_CODE_HERE" }`. JSON-string args remain supported.

The redirect URL will use local port <p>. On a remote server it is expected for that localhost page to fail locally; copy the address bar URL anyway.
```

(the last two lines are absent entirely when `getRedirectPort` yields nothing).
`details = {mode:"auth-start", server, authorizationUrl}`. Throw ⇒
`Failed to start OAuth for "<s>": <msg>` / `error:"auth_start_failed"`.

`getRedirectPort(authUrl)`: parse the `redirect_uri` query param, then
`Number.parseInt(new URL(redirectUri).port, 10)`, yielding nothing unless the result is an integer;
any parse failure yields nothing.

**`executeAuthComplete`**: `not_found` / `server_disabled`, then
`completeAuthFromInput(serverName, input, opts)`; a status other than `"authenticated"` ⇒
`OAuth authentication did not complete for "<s>".` / `{error:"not_authenticated", status}`. Success
⇒ `manager.close(server)`, `clearFailure`, `updateStatusBar`, then
`OAuth authentication successful for "<s>". Run mcp({ connect: "<s>" }) to connect with the new token.`
/ `{mode:"auth-complete", server, authenticated:true}`. Throw ⇒
`Failed to complete OAuth for "<s>": <msg>` / `error:"auth_complete_failed"`.

#### 12. Shared result helpers

| helper | behaviour |
|---|---|
| `getToolMatches(metadata, name, exact)` | `exact` ⇒ `tool.name === name`; else compare with all `-` replaced by `_` on **both** sides |
| `getEnabledToolMatches(state, name, exact)` | flat-maps `getToolMatches` over non-disabled servers, `state.toolMetadata` insertion order |
| `getSingleToolMatch(metadata, name)` | exact matches if any, else fuzzy; `>1` ⇒ the sentinel `"ambiguous"`, else the first or nothing |
| `ambiguousToolResult(mode, name)` | `Tool "<n>" matches multiple servers. Specify a server.` |
| `disabledResult(mode, server)` | `Server "<s>" is disabled. Run /mcp enable <s> and /reload to enable it.` · `{mode, error:"server_disabled", server, message}` |
| `getAuthRequiredMessage(state, s, default?)` | `formatAuthRequiredMessage(config, s, default)`; the default is ``Server "<s>" requires OAuth authentication. Run mcp({ action: "auth-start", server: "<s>" }) to get a browser URL, or /mcp-auth <s> in an interactive local session.`` |
| `getAuthFailedMessage(state, s, msg)` | with a custom `settings.authRequiredMessage`: `OAuth authentication failed for "<s>": <msg>. <getAuthRequiredMessage(...)>`; without: `OAuth authentication failed for "<s>": <msg>. Run mcp({ action: "auth-start", server: "<s>" }) to get a browser URL, or /mcp-auth <s> in an interactive local session.` |

#### 13. The `details.error` vocabulary and the permission contract

**Thirty-two codes survive the scope cuts** (upstream has 35; `timeout`, `script_error` and
`invalid_tool_path` were `mcpScript`-only). Of the 32, **`unsafe_pattern` has no producer** once the
`recheck` gate goes — 31 are reachable.

| code | modes |
|---|---|
| `init_timeout`, `init_failed`, `not_initialized` | dispatch preamble (**no `mode` key**) |
| `missing_server` | auth-start, auth-complete |
| `missing_input` | auth-complete |
| `server_disabled` | every mode |
| `not_found` | auth-start, auth-complete, list, instructions, connect |
| `oauth_not_supported`, `auth_start_failed` | auth-start |
| `not_authenticated`, `auth_complete_failed` | auth-complete |
| `ambiguous_tool`, `tool_not_found` | describe, call |
| `query_too_long`, `invalid_pattern`, `empty_query` | search |
| `unsafe_pattern` | search — **no producer post-cut** |
| `not_connected` | list, instructions |
| `no_instructions` | instructions |
| `auth_required` | connect, call |
| `connect_failed`, `aborted` | connect, call |
| `server_not_found`, `tool_not_found_after_reconnect`, `server_backoff`, `server_not_connected`, `native_tool` | call |
| `approval_denied`, `approval_required` | call |
| `tool_error`, `url_elicitation_required`, `call_failed` | call |

`error-signal.ts`'s `toolErrorOverride` maps **exactly** `tool_error` and `call_failed` to
`{isError: true}` — not `auth_required`, not `approval_denied`, not `connect_failed`. Its own comment
states why: pi never reads a result-level `isError`, so without the override a failed MCP call is
recorded as a success. In cyrup that lands as an `EventPatch::ToolResult` with only `is_error` set,
merged field-by-field, delivered under `EventKind::ToolResult`.

**Three cross-crate contracts this section must satisfy** (all pre-existing in cyrup; see the seam
map §8):

1. **The tool must literally be named `mcp`.** `cyrup_permission_system::manager` branches on
   `normalized == "mcp"`, and its `BUILT_IN_TOOL_NAMES` does not contain it, so it must arrive from
   the extension registry. `gate.rs` addresses it by name in the no-UI denial text
   (`Using tool 'mcp' requires approval, but no interactive UI is available.`).
2. **The parameter names `{tool, server, connect, describe, search}` are read by
   `create_mcp_permission_targets`**, in that precedence, falling through to the `mcp_status`
   baseline. Derived targets are `<server>_<tool>`, `<server>:<tool>`, `<server>`, `<tool>`, plus
   `mcp_call` / `mcp_connect_<name>` / `mcp_server_<name>`; the five baseline targets are
   `mcp_status`, `mcp_list`, `mcp_search`, `mcp_describe`, `mcp_connect`. **Renaming a parameter
   silently changes which permission rules apply.** The other seven properties (`args`, `regex`,
   `includeSchemas`, `limit`, `offset`, `instructions`, `action`) are not read by the derivation and
   are safe — with the consequence recorded as MCP-191.
3. **`Tool::prompt_guidelines()` returns `Vec<&str>`**, and `McpTool`'s vector must contain a string
   that normalises to `use mcp for mcp discovery first: search by capability, describe one exact tool
   name, then call it.` — normalisation being bullet-strip + whitespace-collapse + lowercase
   (`cyrup-permission-system`'s `sanitize/tools.rs`), so the source string need not itself be
   lowercase. **The failure mode is the opposite of "the guideline disappears":**
   `should_keep_guideline` is `guideline_keep_rule(...).unwrap_or(true)`, so an unmatched bullet is
   **always kept** — a mismatched string silently defeats the gating and leaves the "use mcp …"
   bullet in the system prompt even when `mcp` is not exposed.

One near-miss worth naming so nobody reaches for it: `ToolResult::added_tool_names`
(`crates/cyrup-core/src/tool.rs`) is a **cache-placement record** for provider adapters with native
deferred tool loading — it explicitly does not change the active tool set. It is not a substitute for
`HA-1`.

---

### Port units

**MCP-151 — Register the `mcp` tool with the exact JSON Schema** · high · M · **host-verb**
**upstream** — `index.ts` `registerProxyTool`: name `mcp`, label `MCP`, `promptSnippet`, twelve
optional properties, `args` as a `string | object` union.
**behavior** — the model sees one tool named `mcp` whose parameter names and descriptions are exactly
§1. A renamed or reshaped property silently breaks prompts, permission rules (§13.2) and the
guideline sanitizer.
**cyrup** — `McpTool` implementing `cyrup_core::Tool`, installed with `InitApi::register_tool` during
`NativeExtension::init`. `parameters()` returns `&serde_json::Value`, so the schema lives in a
`OnceLock`/field, not built per call. The TypeBox `~optional`/Gemini shim has no analogue and is
dropped — both of `optionalNumber`'s branches serialise identically. **One cut-driven edit**:
`action`'s description narrows to `"Action: 'auth-start' or 'auth-complete'"`.
**verify** — unit: snapshot `parameters()` against §1 including `anyOf` for `args`, and assert
`action`'s description names exactly two values; cyrup-it: the tool appears in the session's tool
catalogue after `init`.

**MCP-152 — Port `buildProxyDescription` and re-register on change** · high · M · **hand-written** + **host-addition (`HA-1`)**
**upstream** — `direct-tools.ts` `buildProxyDescription`, driven by `index.ts` `syncProxyTool`; six
conditional blocks (§2); `INSTRUCTIONS_SNIPPET_LENGTH = 150`.
**behavior** — the model learns the server inventory, per-server tool counts, disabled servers,
150-char instruction snippets and the usage cheatsheet **from the tool description**, regenerated
whenever config or cache changes. `syncProxyTool` re-registers when the text differs, and hides the
tool (`setActiveTools` minus `"mcp"`) when `disableProxyTool === true` and direct tools cover every
configured server.
**cyrup** — build the string with `write!`; store the last-emitted text; on change, re-register
through `HA-1`. The hide half already works: `HostServices::{active_tools, set_active_tools}` is
upstream's own documented `unregisterTool === undefined` fallback branch. `truncateAtWord` must
declare UTF-16-vs-char semantics. Counts require MCP-198.
**Without `HA-1`** the description is frozen at the cold-cache text for the session — the tool still
works, `mcp({search})` and `mcp({tool})` still resolve, and the next session is correct. That is
scheduling, not severity; but it also means `settings.disableProxyTool` must be treated as
unsupported until `HA-1` lands, because hiding a tool you cannot re-register is one-way.
**verify** — unit: golden-file the description for a 3-server fixture (1 disabled, 1 with
instructions, 1 with direct tools), asserting the post-cut header and nine-line usage block;
cyrup-it: mutate `mcp.json`, assert the registered description changed.

**MCP-153 — Port mode dispatch: precedence, args coercion, init gate** · high · M · **hand-written**
**upstream** — `index.ts` `execute`; `INIT_WAIT_TIMEOUT_MS = 30_000`, `awaitWithTimeout`.
**behavior** — §3: nine-arm first-match dispatch in a fixed order; `args` accepted as object **or**
JSON string with two *thrown* (not returned) error messages; a 30 s bounded wait on initialisation
returning `init_timeout`/`init_failed`/`not_initialized` with **no `mode` key**; `search` dispatches
on `!== undefined` so `search: ""` reaches the mode; `connect` calls `syncToolSurface` after the mode
returns; an unrecognised `action` falls through rather than erroring.
**cyrup** — `match` on the deserialised params in the documented order inside
`Tool::execute(call_id, params, cancel, on_update)`. `Invalid args JSON: …` and
`Invalid args: expected a JSON object, got …` become `Err(ToolError)`; every other outcome is
`Ok(ToolResult)`. The init wait is `tokio::time::timeout` over a shared `OnceCell`/watch.
**verify** — unit: table-drive all nine arms plus `action:"frobnicate"` and `action:"ui-messages"`
both falling through to status; unit: `args` as `"{\"a\":1}"`, `[]`, `"null"`, `""`.

**MCP-154 — Port `executeStatus`** · medium · S · **hand-written**
**upstream** — `proxy-modes.ts` `executeStatus`.
**behavior** — §4: six-rung status ladder, header counting enabled servers only, five distinct glyph
lines (`⊘ ✓ ⚠ ○ ✗`), a trailing usage hint, and a `details.servers[]` array whose `disabled` key
appears only when true.
**cyrup** — direct translation; the glyphs are literal `char`s and must not be substituted. Server
iteration must be insertion-ordered (MCP-170).
**verify** — unit: golden text for a fixture covering all six statuses at once; conformance:
`disabled-server.test.ts`'s "keeps no-theme status usable and reports disabled count".

**MCP-155 — Port `executeList`** · medium · S · **hand-written**
**upstream** — `proxy-modes.ts` `executeList`; `INSTRUCTIONS_PREVIEW_LENGTH = 300`.
**behavior** — §4: five outcomes, three of them for the zero-tool case, each with a distinct
`details` shape; the `Use mcp({ instructions: … }) for the full text.` hint appears only when the
300-char preview actually truncated.
**cyrup** — direct translation; `truncateAtWord(desc, 50)` per row.
**verify** — conformance: `proxy-modes-instructions.test.ts`'s four listing cases (short instructions
in full; truncation + pointer; unchanged when absent; connected/cached with no visible tools); unit:
the three zero-tool branches.

**MCP-156 — Port `executeInstructions`** · low · S · **hand-written**
**upstream** — `proxy-modes.ts` `executeInstructions`.
**behavior** — §4; five outcomes (`not_found`, `server_disabled`, present, `no_instructions`,
`not_connected`) checked in that order — cached instructions win even for a disconnected server.
**cyrup** — direct translation.
**verify** — conformance: `proxy-modes-instructions.test.ts`'s four instruction cases.

**MCP-157 — Port `executeDescribe`** · medium · M · **hand-written**
**upstream** — `proxy-modes.ts` `executeDescribe`.
**behavior** — §5: ambiguity checked before resolution (exact set, then fuzzy set); a disabled-server
match reported as `server_disabled` rather than `tool_not_found`; the `Shape:` / `Parameters:` fork;
the `" (requires approval)"` marker; ranked suggestions on a miss.
**cyrup** — direct translation. `renderTsShape` and `formatSchema` are dependencies owned by the
tool-metadata section; call them. Note `formatSchema` takes **no** indent argument here, unlike §6's
`"    "`.
**verify** — conformance: `proxy-modes-discovery.test.ts`'s "keeps keywords out of search and
describe output", "suggests the matching tool for a prefix-mangled describe name", "prefers an exact
describe name over an earlier normalized fallback"; unit: only-match-is-disabled; unit: a resource
tool renders `Type: Resource (reads from …)` and `No parameters required (resource tool).`.

**MCP-158 — Port `executeSearch` match selection** · high · M · **hand-written**
**upstream** — `proxy-modes.ts` `executeSearch` (selection half).
**behavior** — §6: three mutually exclusive paths; the regex path assigns `score: 0` to every match
and **never sorts** (observable in `details.matches` ordering); the empty-query-with-server path
sorts by `localeCompare`; a disabled `server` filter short-circuits.
**cyrup** — direct translation. The unsorted regex output must be reproduced.
**verify** — conformance: `proxy-modes-discovery.test.ts`'s "searches MCP tools only", "rejects regex
queries longer than the safety cap", "reports malformed regex queries separately", "accepts safe
regex queries", "keeps non-regex searches unaffected by the regex length cap", "returns ranked paged
search details", "paginates regex search results without changing their order"; unit: `search:""`
with and without `server`.

**MCP-159 — Port the regex search path onto a linear-time engine** · medium · S · **hand-written**
**upstream** — `proxy-modes.ts` `executeSearch`'s regex branch: `MAX_REGEX_SEARCH_QUERY_LENGTH = 256`,
`REGEX_SAFETY_CHECK_PARAMS = {attackTimeout:50, incubationTimeout:50, timeout:250}`,
`recheck.checkSync(query,"i",params)`, two distinct rejection codes.
**behavior** — a model- or user-supplied regex is compiled case-insensitively and run against tool
names, descriptions and configured keywords; over-long queries are rejected with `query_too_long`,
malformed ones with `invalid_pattern`.
**cyrup** — `regex` (1.12.4, already resolved in `Cargo.lock`; the native crate depends on it
directly). The crate compiles to a finite automaton with a **linear-time matching guarantee**, so
catastrophic backtracking is structurally impossible and the `recheck` gate has nothing to prevent.
Set `RegexBuilder::size_limit` and `dfa_size_limit` **explicitly** rather than relying on defaults,
and surface a compile or size-limit error as `invalid_pattern`. The 256-char cap and the `"i"` flag
port unchanged.
**Residual, stated precisely.** (a) The `unsafe_pattern` *diagnostic* disappears: upstream tells the
model `Regex query rejected as unsafe (<status>).`; a nested-quantifier pattern now simply runs, in
linear time. A model that relied on the rejection to learn its pattern was bad now gets results
instead. (b) JS `RegExp` accepts constructs `regex` rejects — backreferences, lookaround — which
become `invalid_pattern` where upstream compiled them. Name this in the `/mcp` help text rather than
pretending it is the same dialect.
**verify** — the upstream case "rejects catastrophic-backtracking regex queries" is **re-specified,
not ported**: assert `(a+)+$` compiles, returns results, and completes under a wall-clock bound, with
the divergence written into the test name; unit: 257-char query ⇒ `query_too_long`; unit: a
backreference pattern ⇒ `invalid_pattern`; unit: a non-regex search is unaffected by the cap.

**MCP-160 — Port `executeSearch` rendering, pagination footer and connecting hint** · medium · M · **hand-written**
**upstream** — `proxy-modes.ts` `executeSearch` (rendering half).
**behavior** — §6: the zero-result message plus a singular/plural "still connecting" hint listing
servers sorted by `localeCompare`; the with-schemas layout with 4-space-indented `Shape:` lines; the
no-schemas one-liner with a 50-char description; the `<n> of <total> — offset: <k> for more` footer
(em-dash).
**cyrup** — direct translation; the em-dash and the exact indents are model-visible.
**verify** — conformance: `proxy-modes-discovery.test.ts`'s "reports only the filtered server that is
still connecting" and "reports all enabled servers that are still connecting"; unit: golden text with
`includeSchemas` true and false.

**MCP-161 — Port `executeConnect`** · high · M · **hand-written** + **host-verb**
**upstream** — `proxy-modes.ts` `executeConnect`.
**behavior** — §8: reconnect-if-connected; one auto-auth retry; on success an eight-step metadata
commit (metadata, prompts iff discovery succeeded, instructions set-or-**delete**, cache write, notify
with reason `"proxy-connect"`, keep-alive mark, clear failure, status bar) and then **`executeList`'s
output** — a successful connect reports `details.mode === "list"`.
**cyrup** — `state.ui.setStatus("mcp", …)` is `HostServices::set_status(key, Option<&str>)`, a keyed
segment cleared with `None`. Its default impl is a no-op, which degrades exactly the way upstream's
`if (state.ui)` guard does — no gap. `Peer::list_all_tools` / `list_all_prompts` /
`list_all_resources` supply the post-handshake metadata (owned by the server-manager section; this
mode consumes it).
**verify** — conformance: `proxy-modes-auto-auth.test.ts`'s "refreshes an already connected server
instead of reusing stale metadata", "ignores stale same-server metadata during executeConnect", "uses
known metadata during executeConnect filtering", "auto-authenticates and retries executeConnect
once"; cyrup-it: connect a fixture stdio server, assert `details.mode === "list"`.

**MCP-162 — Port `attemptAutoAuth` and the single-shot latch** · high · M · **hand-written** + **extension-owned**
**upstream** — `proxy-modes.ts` `attemptAutoAuth`, latched by `autoAuthAttempted` and read/set at
five sites including `recoverAuthConnection`.
**behavior** — §8. Opt-in via `settings.autoAuth === true`. Headless sessions fail for every grant
type except `client_credentials`. **At most one auto-auth per `executeCall` invocation**, across all
five call sites — including `recoverAuthConnection`, which fires from inside `withSessionRecovery`
*after* the request started. The latch is the defence against a browser-flow loop on a misconfigured
server. `resolveServerUrl` throwing is a *failure*, not a skip.
**cyrup** — a `bool` on the call's local state, not on shared state; the five call sites read and set
it. Browser opening is `opener::open` called directly by the native crate — there is no host verb
for it and none is wanted, matching upstream's direct `open` dependency. The headless default message
still routes through `getAuthRequiredMessage`, so a configured `settings.authRequiredMessage`
overrides it.
**verify** — conformance: `proxy-modes-auto-auth.test.ts`'s "fails fast for non-ui browser auth when
autoAuth is enabled", "uses custom authRequiredMessage for non-ui autoAuth failures",
"auto-authenticates and retries executeCall once"; unit: a call traversing phases 3, 4 and 6 with a
`needs-auth` server invokes `authenticate` exactly once; unit: `autoAuth` unset ⇒ zero invocations.

**MCP-163 — Port `executeCall`'s resolution state machine (phases 1-5)** · **critical** · L · **hand-written**
**upstream** — `proxy-modes.ts` `executeCall`, resolution half.
**behavior** — §9 phases 1-5. Five ordered resolution paths; ambiguity computed over *enabled*
servers before any lazy connection and **failing closed** via the `"ambiguous"` sentinel; the first
*disabled* match remembered so the error names the right server; **the fuzzy scan skipped entirely
when the exact scan found a disabled match**; lazy prefix discovery over candidates sorted by
**descending prefix length**, skipping servers in failure backoff unless they are `needs-auth`; a
fall-through that distinguishes a native host tool from an unknown one.
**Why critical**: `getSingleToolMatch`/`getEnabledToolMatches` exist to refuse rather than guess. A
port that resolves by first-match sends `create_issue` to whichever server happens to be first in the
map — a silently wrong tool call against a live external system, on a normal path.
**cyrup** — a `resolve()` returning an enum
`{Resolved{server, meta}, Ambiguous, Disabled{server, meta:Option}, NotFound{hint_server:Option}}`.
Insertion-ordered iteration is mandatory (MCP-170). `getPiTools()` is
`HostServices::all_tool_names()`; `None` reproduces upstream's optional-callback branch exactly
(§9 Phase 5). `getServerPrefix` for phase 4 is MCP-178.
**Naming decision** — two model-facing strings name the host: the description header (`Non-MCP Pi
tools should be called directly…`, §2) and the `native_tool` message (`"<t>" is a native Pi tool.
Call <t> directly instead of using mcp({ tool: "<t>" }).`). Substituting `Pi` → `cyrup` is
recommended and is the only text this port changes for reasons other than the scope cuts.
**verify** — conformance: `proxy-modes-discovery.test.ts`'s "fails closed for duplicate unqualified
proxy names", "fails closed for same-server normalized fallback collisions", "tells callers to invoke
native … tools directly"; `proxy-modes-auto-auth.test.ts`'s "fails closed when lazy metadata has
duplicate exact tool names", "prefers a lazy exact match over a normalized fallback", "fails closed
when lazy metadata has duplicate normalized tool names"; unit: two servers with prefixes `foo` and
`foo-bar` — the longer wins.

**MCP-164 — Port `executeCall`'s invocation paths and result shaping** · high · L · **hand-written** + **rmcp**
**upstream** — `proxy-modes.ts` `executeCall`, invocation half.
**behavior** — §10. **Three** result paths after the MCP Apps cut (resource read, tool error, tool
success); `callIdentity` fixed once in phase 6 and spread into every subsequent `details`; guard
options carrying `prefix:"Error: "`, the `Expected parameters:` schema suffix and
`emptyTextFallback:"Tool execution failed"` on the error path; `touch`/`incrementInFlight` bracketing
with `decrementInFlight`/`touch` in `finally`.
**cyrup** — the call is `Peer::send_request_with_option(ClientRequest::CallToolRequest(
CallToolRequestParams{ name, arguments, .. }), PeerRequestOptions{ timeout, .. })` →
`RequestHandle::await_response()`, with a task calling `RequestHandle::cancel(reason)` when the owned
`CancellationToken` fires; the resource read is `Peer::read_resource` with the same options and **no**
cancellation wrapper, reproducing upstream's asymmetry rather than "fixing" it. `CallToolResult`'s
`content`/`structured_content`/`is_error` feed the transform owned by the tool-registrar section.
**verify** — conformance: `proxy-modes-auto-auth.test.ts`'s "surfaces aborted proxy tool calls via
the forwarded AbortSignal" and "preserves owner cancellation during a proxy tool call"; unit: a
fixture returning `isError:true` yields `error:"tool_error"` and no thrown error; unit: the in-flight
counter returns to zero on every path.

**MCP-165 — Port `executeCall`'s error taxonomy** · medium · M · **hand-written**
**upstream** — `proxy-modes.ts` `executeCall`'s catch block.
**behavior** — §10: `SessionRecoveryAuthRequiredError` → `auth_required` (+ `autoAuthAttempted` in
details); `UrlElicitationRequiredError` → `url_elicitation_required` with the three action-specific
messages; anything else → `aborted` or `call_failed`, with `details.message` replaced by the literal
`output truncated; see outputGuard.fullOutputPath` when `guarded.outputGuard` is set.
**cyrup** — a Rust error enum with these three arms. rmcp surfaces the URL-elicitation error; the
batch `handleUrlElicitationRequired` loop is adapter policy owned by the elicitation section. The
`uiSession?.sendToolCancelled` calls that upstream fired on all three arms go with Cut 2.
**verify** — conformance: `proxy-modes-auto-auth.test.ts`'s "runs URL elicitations returned by proxy
tool calls" and "rethrows proxy auto-auth cancellation"; unit: each arm's exact text and code; unit:
a guard-spilled path substitutes the truncation message.

**MCP-167 — Port `executeAuthStart` and `formatManualAuthInstructions`** · medium · M · **hand-written**
**upstream** — `proxy-modes.ts` `formatManualAuthInstructions`, `getRedirectPort`, `executeAuthStart`.
**behavior** — §11. The manual-OAuth block is a copy-paste protocol for remote/headless sessions: the
model prints an authorization URL and instructs the human to paste back the redirect URL. The port
note appears only when a numeric port parses out of `redirect_uri`, and **is preceded by a blank
line** because `portNote` itself starts with `\n` before the `join("\n")`.
**cyrup** — string assembly with `.filter(Boolean)` semantics — build a `Vec<String>` and drop the
trailing note when empty. `getRedirectPort` is `url::Url` parsing with integer-only acceptance.
`startAuth`/`supportsOAuth` are the OAuth section's, backed by rmcp's `OAuthState::start_authorization`
+ `get_authorization_url`.
**verify** — conformance: `proxy-modes-manual-auth.test.ts`'s "returns copyable instructions and
authorization URL" and "rejects auth-start for non-OAuth servers"; unit: golden text with and without
a parseable port, including the blank line.

**MCP-168 — Port `executeAuthComplete`** · medium · S · **hand-written**
**upstream** — `proxy-modes.ts` `executeAuthComplete`; input selection in `index.ts`'s dispatch.
**behavior** — §11. `input = args.redirectUrl ?? args.code ?? args.input`, blank rejected as
`missing_input`; on success the connection is **closed** and the failure record cleared so the next
`connect` uses the new token.
**cyrup** — direct translation over the OAuth section's `completeAuthFromInput`.
**verify** — conformance: `proxy-modes-manual-auth.test.ts`'s "completes auth from a copied redirect
URL and resets connection state"; unit: all three input keys accepted; unit: a non-`"authenticated"`
status yields `not_authenticated` with the status echoed.

**MCP-169 — Freeze the `details.error` vocabulary as a conformance table** · high · S · **hand-written**
**upstream** — 32 surviving codes across `index.ts` and `proxy-modes.ts` (§13), consumed by
`error-signal.ts`'s `toolErrorOverride`.
**behavior** — `details.error` is the machine-readable contract: only `tool_error` and `call_failed`
re-flag a result as `isError`. Missing the override records a failed MCP call as a success in the
transcript; renaming a code silently disables downstream behaviour.
**cyrup** — a single `#[non_exhaustive] enum McpErrorCode` with `#[serde(rename_all = "snake_case")]`
and a `const ALL`; a test asserts the serialised set equals §13. Keep `unsafe_pattern` in the enum as
a documented no-producer variant rather than deleting it, so a future engine change does not have to
reintroduce vocabulary. The consumer side is `EventPatch::ToolResult` with only `is_error` set,
merged field-by-field under `EventKind::ToolResult`.
**verify** — unit: the enum's serialised names equal the §13 table exactly, all 32; unit: exactly two
codes map to `is_error = true`.

**MCP-170 — Use insertion-ordered maps for servers and metadata** · high · S · **extension-owned**
**upstream** — `state.ts`'s `toolMetadata: Map<string, ToolMetadata[]>` and `config.mcpServers` key
order. A JS `Map` preserves insertion order, and every iteration site depends on it:
`getEnabledToolMatches`, `executeDescribe`'s fallback scan, `executeSearch`'s regex scan,
`executeCall`'s two scans, `rankToolMatches`.
**behavior** — insertion order decides which server wins a fuzzy tool-name match, which disabled
server is named in an error, and the output order of the unsorted regex search path. `mcpServers` key
order likewise drives `executeStatus`, `buildProxyDescription` and lazy-prefix candidate collection.
**cyrup** — `indexmap::IndexMap` (2.14.0, already resolved in `Cargo.lock`) for both maps inside
`cyrup-mcp`, deserialised **directly into typed structs** rather than through `serde_json::Value` —
so cyrup's `serde_json` lacking `preserve_order` is irrelevant here and no workspace change is
needed. **Do not** use `BTreeMap`: it changes observable behaviour whenever server names are not
already alphabetical.
**verify** — unit: two servers `zeta` and `alpha` inserted in that order, both exposing tool `t`, one
disabled — the error names `zeta` first.

**MCP-171 — Decide the `localeCompare` tie-break** · low · M · **open-decision**
**upstream** — the rank tie-break in `rankToolMatches`, the empty-query sort in `executeSearch`, and
the connecting-server list.
**behavior** — `String.prototype.localeCompare` with no locale uses ICU default collation:
case-insensitive at the primary level (`apple < Banana`), digits before letters, most punctuation
with low primary weight. Rust's `str::cmp` is byte order, giving `Banana < apple`. Tool and server
names may contain uppercase.
**cyrup** — **Options:** (a) `icu_collator` — exact, but a large new dependency tree for a display
tie-break; (b) a hand-written ASCII comparator (primary: case-folded alphanumeric; tertiary:
lowercase before uppercase; punctuation before alphanumerics) — exact for realistic MCP names, wrong
for non-ASCII; (c) `str::cmp` and accept the divergence. **Recommendation: (b)** with a documented
ASCII-only precondition — (a) is disproportionate and (c) is visibly wrong the first time a server is
named `GitHub`. This only orders equal-score results and a hint list; it never changes which tools
match.
**verify** — unit: `["Zeta","alpha"]` sorts as `["alpha","Zeta"]` under whichever comparator is
chosen, and the choice is named in the test.

**MCP-172 — Port `normalizeSearchText` and `tokenize`** · high · S · **hand-written**
**upstream** — `search-ranking.ts` `normalizeSearchText`, `tokenize`.
**behavior** — §7: camelCase split **before** lowercasing; separator class exactly `_ . / : -` with
runs collapsed; tokens split on any non-`[a-z0-9]`; empty tokens dropped. Non-ASCII text tokenizes to
nothing.
**cyrup** — a hand-written char scanner, not `regex` — both patterns are trivial and a scanner keeps
the ranking path allocation-light. `([a-z0-9])([A-Z])` inserts a space between a lowercase/digit and
a following uppercase, ASCII only.
**verify** — unit: `normalize("getUserID_v2/foo")` → `"get user id v2 foo"` (note `ID` does not
split); unit: `tokenize("")` → `[]`.

**MCP-173 — Port `scoreToolMatch` field scoring** · high · M · **hand-written**
**upstream** — `search-ranking.ts` `scoreToolMatch`; `FIELD_WEIGHTS`, `MIN_STEM_LENGTH = 4`.
**behavior** — §7 steps 2-4. Four fields in a fixed order with weights 12/10/8/5; one phrase bonus
per field (×14 exact, ×9 prefix, ×6 substring, first match wins); one token bonus per (field, query
token) (×4 exact token, ×2 stem, ×1 raw substring, first match wins); `phraseMatched`,
`wholeFieldExact` and `matchedTokens` accumulate across fields.
**cyrup** — integer arithmetic throughout (`i64`); fields are normalised but **not** trimmed. Keep
the asymmetric stem rule: `field.starts_with(token)` at any length, `token.starts_with(field)` only
when the field token is ≥ 4 characters.
**verify** — unit: port `search-ranking.test.ts`'s "ranks an exact name above a description match",
"drops partial two-token matches", "ignores single-letter possessive tokens instead of stem-matching
them" verbatim.

**MCP-174 — Port keyword scoring and `resolveSearchKeywords`** · medium · M · **hand-written**
**upstream** — `search-ranking.ts` `resolveSearchKeywords` and the keyword block of `scoreToolMatch`.
**behavior** — §7 step 5. Keyword phrase bonuses are a **max across phrases** added once, so a query
spanning two unrelated keywords cannot collect the phrase bonus twice; keyword token bonuses then run
with the same three-way ladder, its weakest tier testing `phrases.any(|p| p.contains(token))`.
`resolveSearchKeywords` unions and dedupes values across every matching pattern key, in key insertion
order, dropping non-arrays, non-strings and blank strings.
**cyrup** — direct translation. Pattern matching reuses `getToolNameCandidates` +
`matchesToolPattern` from the types section — do not reimplement glob matching here.
**verify** — unit: port `search-ranking.test.ts`'s four keyword-scoring cases (including "scores an
exact alias above incidental cross-phrase token matches" and "does not change scoring when the
keyword list is empty") and its three `resolveSearchKeywords` cases verbatim; conformance:
`proxy-modes-discovery.test.ts`'s "finds tools through configured search keywords" and "matches
keyword keys by prefixed name and glob".

**MCP-175 — Port the coverage gate and final bonuses** · high · S · **hand-written**
**upstream** — `search-ranking.ts` `scoreToolMatch`'s tail.
**behavior** — §7 steps 6-7. Without a phrase match, a query of 1-2 tokens must match **all** its
tokens and a longer query must reach 0.6 coverage, else the tool is dropped entirely. Then `+25` at
full coverage or `+round(coverage*10)`, `+8` when the first query token is a token of the normalised
prefixed name, `+20` for `wholeFieldExact`.
**cyrup** — `coverage` is `f64`; `Math.round` on a positive value is Rust's `f64::round`. Compute
full coverage as the integer comparison `matched == total`, never a float equality.
**verify** — unit: a 3-token query matching 2 tokens (0.667) survives; matching 1 (0.333) does not; a
2-token query matching 1 does not.

**MCP-176 — Port `rankToolMatches` and `paginate`** · high · S · **hand-written**
**upstream** — `search-ranking.ts` `rankToolMatches`, `paginate`.
**behavior** — §7. `hasKeywords` is `includeKeywords && definition.searchKeywords.is_some()` — an
empty object still counts as present, which changes whether `keywords` is `Some([])` or `None`
(`Some([])` is a no-op). `paginate` clamps offset ≥ 0 and limit ≥ 1 and returns no `nextOffset` when
the page reaches the end.
**cyrup** — `sort_by` with `b.score.cmp(&a.score).then(collate(&a.name, &b.name))` (MCP-171);
slicing must clamp both bounds, since JS `slice` never panics.
**verify** — unit: `search-ranking.test.ts`'s "paginates including offsets beyond the result set",
both cases.

**MCP-177 — Port keyword resolution inside the regex search path** · low · S · **hand-written**
**upstream** — `proxy-modes.ts` `executeSearch`'s regex scan tests the pattern against each resolved
keyword in addition to name and description.
**behavior** — configured `searchKeywords` are searchable by regex as well as by ranked query, but
never appear in schemas, `describe` output or the metadata cache — `searchKeywords` is read only by
`search-ranking.ts` and declared on `ServerEntry`.
**cyrup** — call the same resolver; the global prefix is `settings.toolPrefix ?? "server"`, not the
per-server override (the per-server override is applied inside `resolveSearchKeywords` via
`resolveToolPrefix`).
**verify** — conformance: `proxy-modes-discovery.test.ts`'s "matches keywords in regex search mode"
and "keeps keywords out of search and describe output" — a keyword-only regex match returns with
`score: 0`.

**MCP-178 — Port `rankSuggestions`, and settle the `getServerPrefix` conflict** · high · M · **open-decision**
**upstream** — `search-ranking.ts` `rankSuggestions`, using `getServerPrefix` from `types.ts` with
`sanitizeServerPrefix` and **four** `ToolPrefix` modes (`server | none | short | mcp`).
`sanitizeServerPrefix` with the default `preserveProviderValid` keeps `[A-Za-z0-9_-]` verbatim — so
`-` **survives** — and hex-escapes anything else as `_<codepoint-hex>_`; the `mcp` mode produces
`mcp__<sanitized>`.
**behavior** — §7. Suggestions strip the longest matching server prefix (probing modes `server`,
`short`, `mcp`) and re-rank the remainder with keywords disabled. The same `getServerPrefix` drives
`executeCall` phase 4's lazy prefix discovery.
**cyrup** — `cyrup_ext_subagents::exec::mcp_direct_tools`'s `get_server_prefix` has **three** modes
(`ToolPrefix::{Server, None, Short}`) and does `server_name.replace('-', "_")`. For a server named
`linear-server` upstream yields the prefix `linear-server` and cyrup yields `linear_server`; both
`rankSuggestions` and phase 4 test `toolName.starts_with(prefix + "_")`, so **every hyphenated server
name silently stops matching**. This is not a cyrup defect: `cyrup-ext-subagents` is a faithful port
of `pi-subagents`' `mcp-direct-tool-allowlist.ts`, which is itself the three-mode hyphen-replacing
form; `pi-subagents` has drifted from `pi-mcp-adapter` v2.25.0.
**Options:** (a) `cyrup-mcp` implements the adapter's four-mode `sanitizeServerPrefix` and leaves
`mcp_direct_tools` alone — the two then disagree about the *same* tool names and the subagent
allowlist stops matching; (b) upgrade `mcp_direct_tools` in the same change so both use the adapter's
rule — self-consistent and upstream-faithful, but edits a crate outside `cyrup-mcp` and changes what
`pi-subagents`-derived allowlists resolve; (c) reproduce cyrup's rule in `cyrup-mcp` and file the
divergence upstream. **Recommendation: (b)** — one tool-name grammar per process is the only
self-consistent option; record the allowlist behaviour change explicitly.
**verify** — unit: `rankSuggestions` on `linear-server_isues` with a server named `linear-server`
returns `linear-server_issues`; cyrup-it: the subagent allowlist and the adapter agree on the emitted
name for a hyphenated server.

**MCP-191 — `auth-start` / `auth-complete` derive no distinct permission targets** · high · M · **open-decision**
**upstream** — the `mcp` tool's nine dispatch arms versus `pi-permission-system`'s
`createMcpPermissionTargets`, which derives targets from only five argument shapes.
**behavior** — `mcp({action:"auth-start", server:"x"})` carries `server` and no `tool`/`connect`/
`describe`/`search`, so it matches the `server` arm and derives `["mcp_server_x", "x", "mcp_list"]`.
`mcp_list` is one of the five `MCP_BASELINE_TARGETS`, which auto-allow whenever any mcp rule allows
or the mcp default is `Allow`. **Starting an OAuth flow — which opens a browser and binds a loopback
callback listener — is therefore gated identically to listing a server's tools.** `auth-complete` is
the same shape.
**cyrup** — `cyrup_permission_system::manager`'s `create_mcp_permission_targets` has the identical
five arms and the identical baseline set; it is a faithful port and the gap is upstream's. *The MCP
Apps cut removes the worse half of this finding: `mcp({action:"ui-messages"})` used to fall to the
`mcp_status` baseline while **mutating** state (clearing `completedUiSessions`). With that mode gone,
every remaining baseline fall-through (`instructions`, bare status) is read-only.*
**Options:** (a) reproduce upstream exactly and document the hazard (strict parity); (b) add
`mcp_auth_start` / `mcp_auth_complete` arms to `create_mcp_permission_targets` and **exclude them
from the baseline** — a deliberate divergence that closes the hole; (c) reproduce and file the bug
against `pi-permission-system`. **Recommendation: (a) + (c)** for this port, with (b) as a follow-up
behind its own sign-off, because silently hardening the gate makes cyrup's policy files behave
differently from pi's for the same rules.
**verify** — unit: assert the current target vectors for all nine mode shapes, so the behaviour is
pinned whichever way the ruling goes.

**MCP-192 — Satisfy the permission system's contracts on the `mcp` tool** · medium · S · **host-verb**
**upstream** — `index.ts` registers the tool as literally `mcp`.
**behavior** — the permission gate, the guideline sanitizer and the no-UI denial text all address the
tool by name.
**cyrup** — `Tool::name()` returns `"mcp"`. `Tool::prompt_guidelines()` returns `Vec<&str>`; one
entry must normalise — bullet-strip, whitespace-collapse, lowercase — to
`use mcp for mcp discovery first: search by capability, describe one exact tool name, then call it.`
**The failure mode is inverted from the obvious guess:** `should_keep_guideline` is
`guideline_keep_rule(...).unwrap_or(true)`, so a bullet matching no rule is **always kept**. A
mismatched string therefore does not delete guidance — it silently disables the gating, leaving
"use mcp …" in the system prompt after the `mcp` tool has been taken away.
**verify** — unit: `normalize_guideline_text(prompt_guidelines()[0])` string-compares to the
sanitizer's key; cyrup-it: with `mcp` **denied**, the guideline is removed from the system prompt —
the assertion that actually catches a mismatch.

**MCP-193 — Reach `register_late_tool` from a native extension** · medium · M · **host-addition (`HA-1`)**
**upstream** — `index.ts` `syncProxyTool` re-registers `mcp` whenever `buildProxyDescription`'s output
differs, and hides it through `deactivateTools`'s `setActiveTools` fallback when it should disappear.
pi has no `unregisterTool`, so upstream itself always takes the fallback branch.
**behavior** — after a server connects, the model's next turn should see an `mcp` description naming
that server and its tool count. Without re-registration the description is frozen at whatever the
cold cache said.
**cyrup** — **the mechanism is complete and live.** `ExtensionHost::register_late_tool`
(`crates/cyrup-ext/src/facade.rs`) writes into `ExtensionRegistry` and raises the dirty flag;
`ExtensionHost::{refresh_tools, active_tools}` re-materialise;
`AgentSession::{refresh_extension_tools, next_turn_tools, push_active_tools}`
(`crates/cyrup-session-svc/src/session.rs`) merge into `DynamicToolState`, auto-activate new names,
rebuild the system prompt and push to the live agent at every turn boundary. **What is missing is the
handle**: a native extension holds only the `Arc<dyn HostServices>` late-bound by
`NativeExtension::set_host_services` and the per-dispatch `HostCtx`, and `HostServices` has no
tool-registration verb (`active_tools`, `all_tool_names`, `set_active_tools`, `all_tools`, `commands`
are all read-or-restrict). The WASM tier reaches the same thing through its `registration` WIT import,
so this is a **two-tier asymmetry in one verb**, not an absent capability.
The *hide* half already works via `HostServices::{active_tools, set_active_tools}` — upstream's own
no-`unregisterTool` branch, a supported upstream configuration.
**Degradation if `HA-1` is not built** (scheduling, not severity): on a cold `mcp-cache.json` the
first session's description names no servers; `mcp({connect:"x"})` cannot refresh it within the
session; every mode still functions. Treat `settings.disableProxyTool` as unsupported until then.
**Not viable**: holding the description behind interior mutability — `Tool::description(&self) -> &str`
returns a borrowed `&str`, so a `RwLock` cannot satisfy the signature without leaking. And
`ToolResult::added_tool_names` is a cache-placement record, not a registration.
**verify** — cyrup-it: connect a server mid-session and assert the next turn's tool catalogue carries
the updated description.

**MCP-194 — Tool-schema property order is alphabetised by `serde_json`** · low · S · **open-decision**
**upstream** — the twelve properties are emitted in source order (`tool, args, connect, describe,
instructions, search, regex, includeSchemas, limit, offset, server, action`).
**behavior** — property order is part of the prompt text the provider sees; it is stable upstream and
becomes alphabetical here.
**cyrup** — `Tool::parameters()` returns `&serde_json::Value` and the workspace builds `serde_json`
without `preserve_order`, so `serde_json::Map` is a `BTreeMap`. **Options:** (a) enable
`serde_json/preserve_order` workspace-wide — affects every `Value` in the tree and needs its own
sign-off; (b) hold the schema as a pre-rendered `&'static str` — **does not work**, parsing still
normalises into a `Map`; (c) accept alphabetical order. **Recommendation: (c)** unless a provider-side
regression appears. Note (b) is the trap.
**verify** — unit: snapshot the serialised schema and assert the chosen order explicitly, so the
decision is visible in the test.

**MCP-195 — Port the ranking conformance suite** · medium · S · **hand-written**
**upstream** — `__tests__/search-ranking.test.ts`: **11 cases** — 8 in `describe("search ranking")`
(including the two `paginate` assertions) and 3 in `describe("resolveSearchKeywords")`.
**behavior** — this is the executable specification for §7; every constant there is asserted by one
of these cases.
**cyrup** — pure `#[test]`s in `cyrup-mcp`'s search module: no fixtures, no async, no host.
**verify** — the suite itself; success criterion is **11/11**.

**MCP-196 — Port the proxy-mode conformance suites** · high · L · **hand-written**
**upstream** — `__tests__/proxy-modes-discovery.test.ts` (20 cases),
`proxy-modes-auto-auth.test.ts` (16), `proxy-modes-instructions.test.ts` (8),
`proxy-modes-manual-auth.test.ts` (3) — **47 cases** — plus `disabled-server.test.ts`'s proxy-mode
cases, which import the surviving `execute*` entry points to assert disabled-server behaviour end to
end. (`proxy-modes-ui-messages.test.ts`'s 3 cases go with Cut 2.)
**behavior** — the executable specification for §4-§11: the regex gate and its rejection codes,
keyword matching by original/prefixed/glob key, fail-closed duplicate-name resolution, the
connecting-server hint, auto-auth single-shot behaviour and abort propagation, instruction
truncation, and the manual-OAuth copy-paste round trip. Without them the proxy modes have no
regression net and every §-claim here is unenforced.
**cyrup** — the instruction suite is pure-state and belongs beside the code as `#[test]`s; discovery,
auto-auth and manual-auth need a fixture MCP server plus an injectable auth flow and belong in
`cyrup-it` behind its `it` feature — which is off by default, so the suite must also be wired into
whatever CI job enables it. The auto-auth suite is the expensive one: it needs a controllable
`needs-auth` connection state and an injectable `authenticate`, which is a test-seam requirement on
the server-manager and auth sections, not just a port of assertions. **One case is re-specified
rather than ported** — "rejects catastrophic-backtracking regex queries", per MCP-159.
**verify** — the suites themselves; success criterion is 46 ported + 1 re-specified, plus the
`disabled-server` proxy cases.

**MCP-197 — Port the render binding, including the `toolResultRendering` fork** · medium · S · **host-verb**
**upstream** — `index.ts` computes `toolRenderOptions = resolveMcpToolRenderOptions(earlyConfig.settings)`
and `toolRenderShell = toolRenderOptions.resultRendering === "compact" ? "self" : "default"`;
`tool-result-renderer.ts`'s `resolveMcpToolRenderOptions` makes `"compact"` the default and `"boxed"`
opt-in.
**behavior** — with default settings the `mcp` tool draws **its own** framing; setting
`settings.toolResultRendering: "boxed"` flips it to the runtime's standard shell. Getting this wrong
double-draws or un-draws the tool row for every MCP call in the session.
**cyrup** — read `settings.toolResultRendering` once at construction and store the `ToolRenderKind`
(`crates/cyrup-core/src/tool.rs`); `McpTool` implements `render_kind`, `render_call`, `render_result`.
The renderers themselves belong to the tool-result-renderer section; this item owns only the binding
and the fork.
**verify** — unit: default settings ⇒ `render_kind() == SelfRendered`; `toolResultRendering: "boxed"`
⇒ `Default`.

**MCP-198 — Port the cross-server candidate-collision set behind the description's counts** · medium · M · **hand-written**
**upstream** — `direct-tools.ts`'s `getOtherCurrentCandidates`, built per tool inside
`buildProxyDescription`.
**behavior** — the per-server tool counts the model reads (`Servers: github (14 tools), …`) are not a
per-server filter: a tool whose formatted name collides with another server's candidate is excluded,
so adding an unrelated server can change a third server's advertised count. `resourceCount` runs the
same computation for `read_<resourceNameToToolName(name)>` when `exposeResources !== false`.
**cyrup** — compute the collision set once per `buildProxyDescription` call rather than per tool
(upstream rebuilds it per tool, which is O(n²) but observationally identical); reuse
`getToolNameCandidates`/`matchesToolPattern` from the types section, and `isUiToolVisibleToModel`,
which survives the MCP Apps cut. **Do not** simplify to a per-server `includeTools`/`excludeTools`
filter — the counts would silently differ from pi's for any workspace with overlapping tool names.
Prefix grammar is MCP-178.
**verify** — unit: two servers each exposing a tool that formats to the same prefixed name — both
counts drop by one relative to the naive filter, and the golden description reflects it.

**MCP-199 — Wire native-tool detection to `all_tool_names`** · low · S · **host-verb**
**upstream** — `getPiTools` is `() => pi.getAllTools()`, passed as an **optional** parameter to
`executeCall` and invoked as `getPiTools?.()`; consumed in phase 5 to detect a same-named native host
tool.
**behavior** — when the model calls `mcp({tool:"read"})`, the adapter answers
`"read" is a native Pi tool. Call read directly instead of using mcp({ tool: "read" }).` with
`error:"native_tool"` instead of a bare `tool_not_found`. This is the only place the adapter reads
the host's tool inventory.
**cyrup** — `HostServices::all_tool_names() -> Option<Vec<String>>`; `None` means "no live session
backend attached", which is **exactly** upstream's `getPiTools === undefined` branch — skip the
native-tool check and fall through to `tool_not_found`. No host addition, no defect, no special
casing. Do not synthesise a built-in name list as a floor: that would answer `native_tool` for a
built-in the session actually disabled, which pi never does.
**verify** — unit: `all_tool_names() == None` ⇒ `mcp({tool:"read"})` yields `tool_not_found`; cyrup-it
with a live session ⇒ `native_tool` with the exact message.

---

### Out of scope

Four decisions, recorded with their reasons so a later pass does not re-file them as gaps.

**`mcpScript` and the JavaScript worker — cut.** `mcp-code.ts`, `mcp-script-worker.mjs`,
`skills/mcp-scripting/SKILL.md`, the `mcpScript` registration in `index.ts`, `McpSettings.scriptMode`,
and `McpToolApprovalOrigin`'s `"script"` variant. *Reason:* the remaining proxy modes cover the same
ground — `mcp({search})` → `mcp({describe})` → `mcp({tool, args})` is the same discover/inspect/call
loop, one call per turn instead of batched. This removes the only JS-engine question in the entire
port: no `rquickjs`, no vendored C, no `boa`, no JS-in-WASM, no `node`. *Seams:* `executeCall`'s
`origin?: "proxy" | "script"` parameter keeps its shape and its `"proxy"` default; only the `"script"`
call site disappears. `buildProxyDescription`'s header sentence "When one request needs several MCP
calls with logic between them, use mcpScript." is removed. The `details.error` vocabulary loses
`timeout`, `script_error` and `invalid_tool_path`, and the `calls[].error = "incomplete"` sentinel in
a separate field. The `mode:"script"` init envelopes go, which removes the asymmetry with the proxy
tool's mode-less init envelopes. `__tests__/mcp-code.test.ts` (20 cases) is not ported. Port units
MCP-179 through MCP-190 are deleted, not stubbed.

**MCP Apps / the UI extension — cut entirely.** *Reason:* decided by the project owner; cyrup supports
exactly the subset of MCP that `rmcp` supports, and the whole app-host surface (a local HTTP server,
an iframe bridge, `ui://` resources) sits outside it. *Seams inside this section:*
`proxy-modes.ts`'s `executeUiMessages` and the `action: "ui-messages"` dispatch arm go; the mode
router drops from ten arms to nine with every other arm keeping its relative order, and an
`action:"ui-messages"` call now falls through to `executeStatus` rather than erroring. `executeCall`
loses its UI-enabled-tool result path — **three** paths remain, and the remaining executor must still
do, in order: resolution → readiness → approval → request options → `withSessionRecovery`-wrapped
call → content transform → output guard → error/abort mapping, with `decrementInFlight` + `touch`
still in the `finally`. `CallToolRequestParams.meta` is no longer populated from a UI session. The
three catch arms lose `sendToolCancelled`. The tool description's
`mcp({ action: "ui-messages" })` usage line goes and the `action` property's description narrows to
two values. **What is kept from the cut file:** `isUiToolVisibleToModel` and `extractUiToolVisibility`
— `buildProxyDescription`'s counts use them to hide tools the server explicitly marked app-only, and
cutting them would expose those tools to the model. `proxy-modes-ui-messages.test.ts` (3 cases) is
not ported; MCP-166 is deleted.

**The `recheck` ReDoS gate — cut with the dependency decision.** *Reason:* `recheck` performs static
ReDoS analysis on a JS `RegExp`; Rust's `regex` compiles to a finite automaton with a linear-time
matching guarantee, so the attack the check exists to stop cannot occur. There is no Rust equivalent
and none is needed. *Consequences, stated precisely in MCP-159:* the `unsafe_pattern` diagnostic has
no producer; compile-time and memory blowup are bounded by explicit `size_limit`/`dfa_size_limit`
rather than the check; JS-only syntax (backreferences, lookaround) becomes `invalid_pattern`;
`MAX_REGEX_SEARCH_QUERY_LENGTH = 256` and the `"i"` flag are unaffected and port directly.

**Legacy HTTP+SSE and raw unix-socket transports — cut.** Not surfaces this section owns, but they
reach it through one line: `executeStatus`'s `needs-auth`/`failed` rungs and `executeConnect`'s
lazy-connect path work over whatever transport the server manager negotiated, which after the cuts is
exactly `stdio` or `streamable HTTP`. No mode in this section branches on transport, so nothing here
changes beyond the set of servers that can reach `connected`.

---

### What does not fit cleanly

**One host addition, and it is shared.** `HA-1` — a native extension has no handle to
`ExtensionHost::register_late_tool`. This section needs it for exactly one thing: refreshing the
`mcp` tool's regenerated description mid-session (MCP-152, MCP-193). The mechanism behind the verb is
complete and reaches a live agent at every turn boundary; the residual is a handle, and the seam map
prices it as small — either `NativeExtension::set_ext_host(Weak<ExtensionHost>)` called beside the
existing `set_host_services`, or a defaulted `HostServices::register_late_tool` backed by a
late-attached sink like `set_overlay_sink` / `attach_dynamic_tools` already are. Every native
extension benefits; the precedents (`execute_shortcut`, `on_bus_event`) were added for the same
reason. Without it this section degrades to a description frozen for one session — graceful, and not
a reason to hold up the port.

**Four decisions needing an owner**, none of them a cyrup-core change:

| id | decision | recommendation |
|---|---|---|
| MCP-178 | `getServerPrefix` grammar: the adapter's four-mode hyphen-preserving rule vs `cyrup-ext-subagents`' three-mode hyphen-replacing one | upgrade `mcp_direct_tools` so one grammar governs the process; record the allowlist behaviour change |
| MCP-191 | `auth-start`/`auth-complete` derive `mcp_list`, a baseline auto-allow target | reproduce upstream + file the bug; harden behind separate sign-off |
| MCP-171 | `localeCompare` tie-break comparator | hand-written ASCII collator with a documented precondition |
| MCP-194 | tool-schema property order alphabetised by `serde_json` | accept alphabetical; do **not** reach for a pre-rendered string, it does not help |

**One naming call**, folded into MCP-163: two model-facing strings say "Pi" (the description header
and the `native_tool` message). Substituting `cyrup` is recommended; it is the only text this port
changes for a reason other than the scope cuts.

---

### Coverage

**Read** — upstream at v2.25.0, in full: `proxy-modes.ts`, `search-ranking.ts`, `error-signal.ts`.
Targeted regions: `index.ts` (constants and `awaitWithTimeout`, `optionalNumber`, the render-options
binding, `getPiTools`, `registerProxyTool` and its `execute`, `syncProxyTool`, `deactivateTools`);
`direct-tools.ts` (`BUILTIN_NAMES`, `INSTRUCTIONS_SNIPPET_LENGTH`, `buildProxyDescription`,
`getOtherCurrentCandidates`); `types.ts` (`searchKeywords`, `ToolPrefix`, `sanitizeServerPrefix`,
`getServerPrefix`); `utils.ts` (`truncateAtWord`); conformance suites by test name for
`search-ranking`, `proxy-modes-discovery`, `proxy-modes-auto-auth`, `proxy-modes-instructions`,
`proxy-modes-manual-auth`.
cyrup by symbol: `cyrup_core::{Tool, ToolResult, ToolRenderKind, ToolError}`;
`cyrup_ext::native::InitApi::{register_tool, register_command, add_autocomplete}`;
`cyrup_ext::facade::ExtensionHost::register_late_tool`;
`cyrup_ext::host::services::HostServices::{confirm, input, select, open_overlay, notify, set_status,
human_interaction_lock, is_run_cancelled, active_tools, all_tool_names, set_active_tools, all_tools}`;
`cyrup_permission_system::manager::{create_mcp_permission_targets, MCP_BASELINE_TARGETS}`;
`cyrup_permission_system::sanitize::tools::{guideline_keep_rule, should_keep_guideline}`;
`cyrup_ext_subagents::exec::mcp_direct_tools::{ToolPrefix, get_server_prefix}`; workspace `Cargo.toml`
and `Cargo.lock` for `serde_json`, `indexmap` 2.14.0, `regex` 1.12.4.
rmcp 3.1.2 by symbol: `service::client::{Peer<RoleClient>::call_tool_once, list_all_tools,
read_resource, send_request_with_option}`, `RunningService::call_tool`, `service::PeerRequestOptions`,
`service::RequestHandle::cancel`, `model::{CallToolRequestParams, CallToolResult, ContentBlock}`.

**Excluded** — one reason each.
- `ui-session.ts`, `ui-server.ts`, `ui-resource-handler.ts`, `glimpse-ui.ts` — MCP Apps, cut.
- `mcp-code.ts`, `mcp-script-worker.mjs`, `skills/mcp-scripting/` — `mcpScript`, cut.
- `tool-approval.ts` — `ensureToolCallApproved`/`isToolCallApprovalRequired` are called from three
  modes; the approval subsystem and its gate wiring are the approval section's.
- `mcp-output-guard.ts` — every mode ends in `guardMcpOutput`; only the `details` keys it contributes
  are pinned here.
- `session-recovery.ts` — `withSessionRecovery` wraps every request in `executeCall`; the recovery
  state machine is transport territory.
- `tool-metadata.ts` / `ts-shape.ts` — `formatSchema` and `renderTsShape` produce text this section
  embeds; schema rendering is its own surface.
- `tool-result-renderer.ts` — this section owns only the binding and the config fork (MCP-197).
- `mcp-auth-flow.ts` and the OAuth files — the auth modes call `authenticate`/`startAuth`/
  `completeAuthFromInput`; the OAuth subsystem is separate.
- `init.ts`, `lifecycle.ts`, `server-manager.ts` — `lazyConnect`, `updateMetadataCache`,
  `notifyToolMetadataUpdated`, `recordFailure`, `getFailureAgeSeconds`, `updateStatusBar`,
  `updateServerMetadata` are called from here, specified elsewhere.
- `direct-tools.ts` beyond `buildProxyDescription` — `createDirectToolExecutor` registers the
  *per-server* tools, a different model-facing surface.
- `metadata-cache.ts`, `resource-tools.ts`, `ui-tool-visibility.ts` — read only far enough to pin
  `isServerCacheValid` / `resourceNameToToolName` / `isUiToolVisibleToModel` as inputs to MCP-198.
- `runtime-owner.ts`, `abort.ts` — the ownership/abort substrate; the *use* of `abortable` (and its
  absence on `readResource`) is pinned here in §10.
- `__tests__/mcp-status.test.ts` — despite the name it tests status *snapshot* publication from
  `init.ts`, not `executeStatus`.
- `__tests__/disabled-server.test.ts` — only its proxy-mode cases are in scope, folded into MCP-196.

**Corrections to the first pass**
- Refuted: "there is no way for a native extension to register a tool after init" / "`register_late_tool`
  has no seam". The mechanism is complete and propagates to a live agent at every turn boundary via
  `AgentSession::{refresh_extension_tools, next_turn_tools, push_active_tools}`; only the *handle* is
  missing. Rerated from `high` open-question to `medium` host-addition (`HA-1`) with graceful
  degradation stated.
- Dissolved: "`HostServices` has no browser-open method, zero hits for `opener`". A native crate is
  not sandboxed; it calls `opener::open` directly, exactly as upstream calls npm `open`. MCP-162 is
  `extension-owned`.
- Dissolved: "`getPiTools` has no non-`None` host analogue" (MCP-199, was an open-question at
  `medium`). Upstream's `getPiTools` is itself an **optional** parameter invoked as `getPiTools?.()`;
  `HostServices::all_tool_names() == None` is that same branch. `host-verb`, `low`, no ruling needed.
- Dissolved: "no workspace regex dependency". `regex` 1.12.4 is already resolved in `Cargo.lock`; a
  native crate adds it directly. With the ReDoS analysis unnecessary by construction, MCP-159 stops
  being an open-question and becomes a `hand-written` port with a named residual.
- Dissolved: "`serde_json` is a `BTreeMap` tree-wide, so map ordering is a problem" as applied to
  config/metadata. `cyrup-mcp` deserialises into its own `IndexMap`-backed types, never through
  `serde_json::Value`; only the fixed tool schema is a `Value`, and that is MCP-194 alone.
- Deleted as dead scaffolding: MCP-150 (a tracker item proposing no work), all `depends` edges,
  `Kind`/`Confidence` fields, and the revision-provenance block naming commits and dirty trees.
- Deleted with Cut 4: MCP-179 through MCP-190 (twelve units), including the JS-engine ruling that
  headlined the draft at `critical`.
- Deleted with Cut 2: MCP-166 (`executeUiMessages`).
- Rerated: MCP-151, MCP-152, MCP-153 from `critical` to `high` — prerequisite-shaped blocking-ness,
  not one of the four severity clauses. MCP-162 from `critical` to `high`. MCP-164 from `critical` to
  `high`. MCP-169 from `high`, held at `high` with the "recorded as a success" consequence stated in
  the body. MCP-163 **kept at `critical`** and re-argued on the correct clause: fail-closed ambiguity
  resolution, whose absence routes a call to the wrong server's same-named tool.
- Corrected: the mode count. The `mcp` tool has ten dispatch arms upstream (not eleven), of which nine
  survive; `mcpScript` was a second registered tool, not a mode.
- Corrected: the `details.error` census under the cuts — 35 upstream codes, 32 surviving, 31
  reachable (`unsafe_pattern` loses its producer).
- Corrected: the conformance census under the cuts — 50 proxy-mode cases become 47 (46 ported, 1
  re-specified); 11 ranking cases all survive; 20 script cases are gone.
