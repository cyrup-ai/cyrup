# 13e · Tool registration, approval, output guard and rendering

> Part of **[13 — cyrup-mcp](13-cyrup-mcp.md)**, which holds the thesis, the seam map, the
> architecture and the one canonical table of every port unit. Method and phasing are in
> **[MCP-PORT-METHODOLOGY.md](MCP-PORT-METHODOLOGY.md)**.

*Upstream is `pi-mcp-adapter` v2.25.0; cyrup is branch `david/cyrup`. Upstream is referenced by file
and symbol, cyrup by symbol and file — never by line number, on either side.*

This subsystem turns an MCP server's `tools/list` into something a language model can actually call.
Everything upstream of it (transport, handshake, OAuth, the metadata cache) produces a list of tools;
everything downstream of it (the proxy modes, the panels, the prompts) consumes the names and schemas
it mints. **It is the largest purely-extension-owned surface in the port: nothing in it touches the
wire, so `rmcp` appears only as a set of model types (`ContentBlock`, `ResourceContents`,
`CallToolResult`, `Tool`), and nothing in it requires a change to cyrup's core except one small
handle.** Seven upstream files do the work: `types.ts`'s name-formatting block decides what a tool is
*called*; `tool-metadata.ts` decides which tools *survive* filtering and what a schema looks like when
printed; `direct-tools.ts` builds the per-tool spec list and the executor behind each registered tool;
`tool-approval.ts` gates a call before it leaves the process; `tool-registrar.ts` converts MCP content
blocks back into host content blocks and spills binary resources to disk; `mcp-output-guard.ts` bounds
what reaches the model; and `tool-result-renderer.ts` draws the call and result rows.

The one thing to understand before touching any of it: **the adapter registers two completely
different tool surfaces at once, and they have different names, different permission paths and
different failure modes.** There is one gateway tool literally named `mcp`, whose `execute` dispatches
on `action > tool > connect > describe > instructions > search > server > nothing`; and there are N
*direct* tools, one per allowed MCP tool and per exposed MCP resource, named
`<serverPrefix>_<toolName>`. Both are registered from the **metadata cache, at extension load time,
before any MCP server process has been started** — that is the design's central trick. The model sees
the full tool surface on turn 1; the server is only spawned when a tool is first called. Everything
about registration, deactivation and re-registration exists to keep that cached surface in sync with
reality without ever blocking a turn.

**Name formatting is a security boundary, not cosmetics.** `formatToolName` is what keeps an MCP
server from claiming the name `read` or `bash`; `resolveDirectTools` drops any formatted name in
`BUILTIN_NAMES = {read, bash, edit, write, grep, find, ls, mcp}`. In cyrup this matters *more* than
upstream: `ExtensionRegistry::active_tools` (`crates/cyrup-ext/src/registry.rs`) walks the base tool
list and replaces each entry with the extension registry's tool of the same name, so an MCP server
shipping a tool called `read` would silently replace cyrup's filesystem read tool for the whole
session if the drop were omitted. `InitApi::register_tool` (`crates/cyrup-ext/src/native.rs`)
documents the same rule from the other side. That is the section's first `critical`.

The second constraint is cross-crate and already in the tree.
`cyrup_ext_subagents::exec::mcp_direct_tools` contains a Rust implementation of these naming rules —
and it **disagrees with `pi-mcp-adapter` v2.25.0 in six ways**, because it is a faithful port of
`pi-subagents`' *own* copy of the rules (`src/runs/shared/mcp-direct-tool-allowlist.ts`), which has
drifted from the adapter's. Both halves were read for this pass and the drift is confirmed on both
sides. `cyrup-mcp` is the *writer* of `<agent_dir>/mcp-cache.json` and the *registrar* of the names;
`mcp_direct_tools.rs` is the *reader* that expands a subagent's `mcp:server/tool` selector into names
it expects the child's tool registry to resolve. If the two disagree, subagent MCP allowlists silently
select nothing. That is MCP-205, and it is a decision, not a bug to fix blindly.

The third: **`mcp-output-guard.ts` is a size guard, not a safety guard.** Read in full, it performs no
prompt-injection detection, no secret redaction and no content classification. It caps bytes and
lines, spills the remainder to a `0600` file in a private directory, and bounds `details.mcpResult`.
The port must ship that posture and say so, because "output guard" reads like a safety control and is
not one — cyrup's own gate (`ExtHooks::before_tool_call` → `cyrup-permission-system`) runs
*before* the call on the arguments and never inspects the result, so MCP tool **output** is
unfiltered text entering the model's context under either system.

### How it lands

| adapter capability | upstream mechanism | cyrup mechanism | verdict |
| --- | --- | --- | --- |
| tool-name formatting, candidates, glob selectors | `types.ts` `sanitizeServerPrefix` / `getServerPrefix` / `formatToolName` / `getToolNameCandidates` / `matchesToolSelector` / `isToolAllowed` | pure `String` + `regex` (per-crate dep; the workspace table declares neither `regex` nor `tracing`) | **hand-written** |
| resource → tool name | `resource-tools.ts` `resourceNameToToolName`, `read_` base name | pure | **hand-written** |
| server recovery from a prefixed name | `types.ts` `resolveServerFromToolName` | pure; consumed by `cyrup-permission-system` if MCP-234 (a) is taken | **hand-written** |
| live tools/resources → `ToolMetadata[]` | `tool-metadata.ts` `buildToolMetadata` | `serde_json::Value` schemas; `Option<T>` skipped on serialize (the cache is a cross-crate contract) | **hand-written** |
| schema pretty-printer | `tool-metadata.ts` `formatSchema` and helpers | recursive `serde_json::Value` walk | **hand-written** |
| direct-tool spec list | `direct-tools.ts` `resolveDirectTools` | pure over the parsed cache; order-preserving `mcpServers` | **hand-written** |
| the `mcp` tool's description | `direct-tools.ts` `buildProxyDescription` | `String` builder; two literal edits from the cuts | **hand-written** |
| initial tool registration | `pi.registerTool` at load | `InitApi::register_tool` (`crates/cyrup-ext/src/native.rs`) | **host-verb** |
| post-init tool registration | `syncDirectTools` / `syncProxyTool` after `onToolMetadataUpdated` | `ExtensionHost::register_late_tool` + `refresh_tools` exist and propagate; a native holds no `ExtensionHost` handle | **host-addition (HA-1)** |
| tool deactivation | optional `pi.unregisterTool`, else `setActiveTools(active \ removed)` | `HostServices::{active_tools, set_active_tools}` — cyrup lands on upstream's own documented fallback branch | **host-verb** |
| render shell selection | `renderShell: "self" \| "default"` | `cyrup_core::ToolRenderKind::{SelfRendered, Default}` (`crates/cyrup-core/src/tool.rs`) | **host-verb** |
| call/result rows | `tool-result-renderer.ts` | `InitApi::register_tool_renderer` + `NativeExtension::{render_call, render_result}` → `ExtensionHost::render_tool_{call,result}` → `cyrup-tui`'s widget flattener | **host-verb** + **hand-written** |
| approval dialog | `tool-approval.ts` `ensureToolCallApproved` → `ui.select` | `HostServices::select` under `HostServices::human_interaction_lock`, with `HostCtx::begin_human_wait` | **host-verb** |
| `approveTools` matching | `tool-approval.ts` `isToolCallApprovalRequired` | pure, reusing the candidate/pattern machinery | **hand-written** |
| cross-extension approval broker | `MCP_TOOL_APPROVAL_REQUEST_EVENT` with a `claim()` closure | `ExtHooks::before_tool_call` + `cyrup-permission-system` **is** the broker, already wired and fail-closed (`EventKind::ToolCall::fails_closed()`) | **host-verb** (broker emit does not port) |
| MCP content → host content | `tool-registrar.ts` `transformMcpContent` | match on `rmcp::model::ContentBlock` shape → `cyrup_core::Content::{Text, Image}` | **hand-written** |
| binary-resource spill | `tool-registrar.ts` `materializeBinaryResource` | `tempfile` (workspace dep 3.27.0) + `create_new` + `0o600` | **hand-written** |
| output guard | `mcp-output-guard.ts` `guardMcpOutput` | own dual byte/line arithmetic; `cyrup_tools::truncate`/`output` are reference only (they never emit a partial line and format `50.0KB`, not `50.0 KiB`) | **hand-written** |
| `{isError:true}` re-flag | `error-signal.ts` `toolErrorOverride` on `tool_result` | `EventKind::ToolResult` + `EventPatch::ToolResult { is_error: Some(true), .. }` (`crates/cyrup-ext/src/contract.rs`) | **host-verb** |
| warnings | five `console.warn` sites | `tracing::warn!` | **extension-owned** |
| terminal-text sanitising | `utils.ts` `sanitizeTerminalText` | in-crate; `cyrup-session-svc`'s `strip_ansi` is private and stops short | **hand-written** |
| MCP-Apps tool visibility, UI sessions, `ui://` | `ui-tool-visibility.ts`, `ui-session.ts`, `ui-app-bridge-helpers.ts` | `isUiToolVisibleToModel` half **kept**; everything else | **cut** (Cut 2) |
| `mcpScript` references in this section | proxy description sentence, `origin:"script"` | — | **cut** (Cut 4) |

### Behavioural specification

#### 1. Name formatting — the four prefix modes

`ToolPrefix = "server" | "none" | "short" | "mcp"`. Effective mode per server is
`resolveToolPrefix(definition, globalPrefix)` = `definition.toolPrefix ?? globalPrefix ?? "server"`;
the global value is `config.settings?.toolPrefix ?? "server"`.

`sanitizeServerPrefix(serverName, preserveProviderValid = true)` maps the server name
character-by-character (`Array.from`, so it iterates by **code point**, not UTF-16 unit):

| char class | `preserveProviderValid = true` (current) | `false` (legacy) |
| --- | --- | --- |
| `[A-Za-z0-9_-]` | kept verbatim | `-` and `_` are **not** valid → escaped |
| `[A-Za-z0-9]` | kept | kept |
| anything else | `_` + `codePointAt(0).toString(16)` + `_` | same |

So `github-mcp` sanitizes to `github-mcp` in current mode and `github_2d_mcp` in legacy mode; `naïve`
becomes `na_ef_ve` in both (`ï` is U+00EF).

`getServerPrefix(serverName, mode)`:

| mode | prefix |
| --- | --- |
| `none` | `""` |
| `short` | `sanitizeServerPrefix(serverName.replace(/-?mcp$/i, ""))`, or the literal `"mcp"` when that is empty |
| `mcp` | `"mcp__" + sanitizeServerPrefix(serverName)` |
| `server` (default) | `sanitizeServerPrefix(serverName)` |

`formatToolName(toolName, serverName, prefix)`: `sanitized = toolName.replace(/\./g, "_")` (dots only
— hyphens in the *tool* name survive), then `prefix ? \`${prefix}_${sanitized}\` : sanitized`.

**`mcp__server__tool` is not a tool name.** In `mcp` prefix mode a tool is
`mcp__<sanitizedServer>_<tool>` — one underscore between server and tool. The double-underscore form
belongs to *prompt slash commands*: `formatPromptCommandName` =
`` `mcp__${serverPart}__${sanitizePromptName(promptName)}` ``, where
`serverPart = getServerPrefix(serverName, prefix) || sanitizeServerPrefix(serverName) || "server"` —
so even `none` mode yields a server segment, because the `||` chain falls through to the raw sanitized
name. `sanitizePromptName`: `[^A-Za-z0-9_-]+` → `_`, strip leading/trailing `[_-]+`, `""` → `"prompt"`,
leading digit → prefix `_`.

`resourceNameToToolName(name)`: `[^a-zA-Z0-9]` → `_`, collapse `_+` → `_`, strip leading `_+`, strip
trailing `_+`, lowercase; then if the result is empty or starts with a digit,
`result = "resource" + (result ? "_" + result : "")`. A resource's *base* tool name is
`` `read_${resourceNameToToolName(resource.name)}` ``, which is then run through `formatToolName` like
any other tool.

`resolveServerFromToolName(toolName, serverNames, prefix)` is the inverse, used by downstream
permission gates: `undefined` for `prefix === "none"`; collect every configured server whose
`getServerPrefix(name, prefix)` is non-empty and where `toolName.startsWith(prefix + "_")`; sort by
descending prefix length; and **fail safe to `undefined` if two different servers produced the same
winning prefix** — `short` mode maps `foo` and `foo-mcp` to the same prefix, and returning the wrong
server to a permission gate would enforce the wrong rule.

#### 2. Candidate sets and pattern matching

`getToolNameCandidates(toolName, serverName, prefix, includeLegacy = true)` builds the set of *every*
name a user might plausibly have written in an `includeTools` / `excludeTools` / `approveTools` entry.
Current candidates — 5 expressions, which dedupe:

1. `toolName` (bare)
2. `formatToolName(toolName, serverName, prefix)` — the effective mode
3. `formatToolName(toolName, serverName, "server")`
4. `formatToolName(toolName, serverName, "short")`
5. `formatToolName(toolName, serverName, "mcp")`

When `includeLegacy` is true, **13** further `add()` calls run (5 + 4 + 4): the bare
`toolName.replace(/-/g, "_")` plus the same four `formatToolName` modes recomputed on it; the four
`formatLegacyToolName(toolName, serverName, m)` for `m ∈ {prefix, server, short, mcp}`
(`getLegacyServerPrefix` = `sanitizeServerPrefix(..., false)`, and the tool name goes through
`replace(/[.-]/g, "_")`, i.e. hyphens in the tool name become underscores too); and the four
current-format names with `-` globally replaced by `_`. The resulting *set* is much smaller than 18
because of heavy overlap: for `("list-sims", "xcodebuild-mcp", "short")` the current set has **4**
members and the full set **12**.

`globToRegExp(pattern)`: escape `[.+^${}()|[\]\\]`, then `*` → `.*` and `?` → `.`, anchored `^…$`.
`matchesToolPattern(candidates, patterns)`: non-array or empty → `false`; a pattern with no `*`/`?` is
an exact `Set.has`; a pattern with either is a regex test over every candidate. Non-string patterns are
skipped.

`matchesToolSelector(toolName, serverName, prefix, patterns, otherCurrentCandidates)` is the
disambiguation rule and the subtlest thing in the file:

1. If any **current** candidate matches → `true`.
2. If `otherCurrentCandidates` was not supplied, fall back to matching the **full legacy** candidate set.
3. Otherwise compute `legacyCandidates = fullSet − currentSet`, and return true only for a pattern that
   matches a legacy candidate **and does not match any other tool's current candidate**.

Rule 3 is what stops a legacy alias from silently excluding the wrong tool once two servers exist whose
sanitized prefixes collide. `isToolIncluded` treats an absent/empty `includeTools` as "everything
allowed"; `isToolExcluded` is a plain selector match; `isToolAllowed` is `included && !excluded`.

The `otherCurrentCandidates` set itself is built at each call site by walking every *other* enabled
server with a valid cache entry, adding all of its tools' and resources' current candidates, and then
**deleting this tool's own current candidates** (`direct-tools.ts` builds it twice — once in
`resolveDirectTools`, once in `buildProxyDescription` — and `tool-metadata.ts` once). In
`tool-metadata.ts` there are two extra arms: if `knownMetadata` has an entry for the other server, its
already-formatted `tool.name`s are added directly plus candidates from its `originalName`; if not, and
either `knownMetadata` is absent or `includeMissingConfiguredCandidates` is set, the *current tool's*
name is speculatively formatted under the other server's prefix — and when
`includeMissingConfiguredCandidates` is set, also under `-`→`_` normalization.

#### 3. `buildToolMetadata` — live tools/resources → `ToolMetadata[]`

Inputs: `tools`, `resources`, the `ServerEntry`, `serverName`, the global `prefix`, optionally
`configuredServers`, `knownMetadata` and `includeMissingConfiguredCandidates`. Outputs
`{ metadata, failedTools }`.

Sequence, per tool:

1. No `tool.name` → push `"(unnamed)"` to `failedTools`, continue.
2. `isToolAllowed(...)` fails → skip silently.
3. `name = formatToolName(...)`; already in `seenNames` → skip.
4. `uiVisibility = extractUiToolVisibility(tool._meta)`; `!isUiToolVisibleToModel(uiVisibility)` → skip.
   Visibility extraction: no `_meta`, or `_meta.ui` absent / non-object / an *array* → `undefined`
   (visible); `_meta.ui.visibility === undefined` → `undefined` (visible); present but not an array →
   `[]` (**hidden**); an array containing anything other than `"model"`/`"app"` → `[]` (**hidden**);
   otherwise the deduped array. `isUiToolVisibleToModel(v)` = `v === undefined || v.includes("model")`.
5. Add to `seenNames` — note this happens *after* the visibility check, so a hidden tool does not
   reserve its name.
6. *(cut — see Out of scope)* `uiResourceUri = getToolUiResourceUri({_meta})` inside a `try`, whose
   throw pushes `tool.name` onto `failedTools` without skipping the tool.
7. *(cut)* `uiStreamMode = extractToolUiStreamMode(tool._meta)`.
8. Push `{name, originalName, description: tool.description ?? "", inputSchema?, uiVisibility?}` —
   each optional key **omitted when undefined**, not set to null.

Then, when `definition.exposeResources !== false`: for each resource, base name
`read_${resourceNameToToolName(resource.name)}`, the same `isToolAllowed` + `seenNames` checks, and
`description = resource.description ?? \`Read resource: ${resource.uri}\`` with `resourceUri` set.
Resources are **not** visibility-filtered, and — unlike the candidate builder, which guards
`resource?.name && resource?.uri` — the resource loop reads `resource.name` unguarded, so a nameless
resource throws in JS. In Rust the type system removes the hazard; do not reproduce a panic there.

`getToolNames(state, serverName)` and `totalToolCount(state)` are trivial projections over
`state.toolMetadata: Map<string, ToolMetadata[]>`. `findToolByName(metadata, toolName)`: exact `name`
match first; otherwise compare with `-` globally replaced by `_` on both sides.

#### 4. `formatSchema` — the schema pretty-printer

Its output is user- and model-visible: it is the body of `mcp({ describe })`, and it is appended as a
`suffix` to the direct-tool `tool_error` and `call_failed` results — **only those two, and only when
`spec.inputSchema` is defined** (`spec.inputSchema ? "\n\nExpected parameters:\n" + formatSchema(...) : ""`).
The other eleven error returns carry no schema.

`formatSchema(schema, indent = "  ")`:
- non-object / array / falsy → `` `${indent}(no schema)` ``
- `type === "object"` with an object non-array `properties`:
  - `required` = the string members of `schema.required`, or `[]`
  - zero properties → `` `${indent}(no parameters)` ``
  - else one `formatProperty` block per property, joined with `\n`
- else `formatNestedSchema(s, indent)`; if non-empty, join with `\n`
- else `formatType(s)` non-empty → `` `${indent}(${typeStr})` ``
- else `` `${indent}(complex schema)` ``

`formatProperty(name, schema, required, indent)`: a non-object/array schema emits the single line
`` `${indent}${name}${required ? " *required*" : ""}` ``. Otherwise it builds
``parts = [`${indent}${name}`]``, pushes `` `(${typeStr})` `` when `formatType` is non-empty, pushes
`"*required*"` when required, appends annotations, emits `parts.join(" ")`, then recurses via
`formatNestedSchema` at `indent + "  "`.

`formatType(schema)`, first match wins:
1. `Object.hasOwn(schema, "const")` → `` `const ${JSON.stringify(schema.const)}` ``
2. `Array.isArray(schema.enum)` → `` `enum: ${schema.enum.map(JSON.stringify).join(", ")}` ``
3. `Array.isArray(schema.type)` → `schema.type.map(String).join(" | ")`
4. truthy `schema.type` → `String(schema.type)`
5. object non-array `properties` → `"object"`
6. `schema.items !== undefined` → `"array"`
7. else `""`

`appendSchemaAnnotations(parts, schema)`: a string `description` → `` `- ${description}` ``; then, **in
this exact order**, `minLength, maxLength, minimum, maximum, minItems, maxItems, format, pattern` each
as `` `[${key}: ${JSON.stringify(value)}]` `` when not `undefined`; then
`` `[default: ${JSON.stringify(schema.default)}]` ``.

`formatNestedSchema(schema, indent)`, in order: `anyOf` variants, `oneOf` variants, `items` (as a
property literally named `items`, never required), then `properties`. `formatVariants(keyword, variants, indent)`
emits `` `${indent}${keyword}:` ``, then per variant either `` `${indent}  - ${JSON.stringify(variant)}` ``
for a non-object, or `` `${indent}  - ${formatType(s) || "schema"}` `` + annotations, then nested at
`indent + "    "`.

#### 5. `resolveDirectTools` — the spec list

`cache === null` → empty list. **There are no direct tools until a cache file exists.**

Per configured server, in `Object.entries` order (JS insertion order of the merged config):

1. `isServerDisabled(definition)` (only literal `disabled === true`) → skip.
2. No cache entry, or `!isServerCacheValid(serverCache, definition)` → skip.
3. Resolve `toolFilter: true | string[] | false`:
   - if an env selection exists (`MCP_DIRECT_TOOLS`): `envSelection.servers.has(serverName)` → `true`;
     else `envSelection.tools.has(serverName)` → that server's tool array; else `false`.
   - otherwise `definition.directTools` when defined, else `config.settings?.directTools`, else `false`.
4. `false` → skip the server entirely.
5. Per cached tool: visibility filter; `toolFilter !== true && !toolFilter.includes(tool.name)` → skip;
   `isToolAllowed(...)` with the freshly computed `getOtherCurrentCandidates(tool.name)` → skip on
   false; format the name; **builtin collision** →
   `console.warn(\`MCP: skipping direct tool "${prefixedName}" (collides with builtin)\`)` and skip;
   **duplicate** →
   `console.warn(\`MCP: skipping duplicate direct tool "${prefixedName}" from "${serverName}"\`)` and
   skip; else push the spec.
6. Per cached resource when `exposeResources !== false`: base name `read_…`; the same `toolFilter` and
   `isToolAllowed` checks — **no visibility check, resources carry none**; the same two warnings with
   the words `direct resource tool`; spec with `resourceUri` and description
   `resource.description ?? \`Read resource: ${resource.uri}\``.
7. After all servers: `specs.length >= DIRECT_TOOLS_ADVISORY_THRESHOLD` (75) →
   `console.warn(\`MCP: ${specs.length} direct tools resolved. Each direct tool adds prompt context; README guidance recommends targeted sets of 5-20 tools and using the proxy or an explicit string[] when 75+ direct tools would be registered.\`)`.

`parseDirectToolSelectors(selectors)` (`metadata-cache.ts`): strip trailing `/+`; a selector containing
`/` is `split("/", 2)` — **JS `split` with a limit discards a third segment**, so `a/b/c` yields
`["a","b"]`; both non-empty → add to the per-server tool set; only the server non-empty → whole-server
selection; no `/` and non-empty → whole-server selection.
`getMissingConfiguredDirectToolServers(config, cache, envOverride)`: every enabled server that *wants*
direct tools but has no valid cache entry. Feeds `syncProxyTool`.

#### 6. `buildProxyDescription` — the `mcp` tool's description string

The whole string is model-visible context and is rebuilt whenever the tool surface changes; when the
rebuilt string is byte-identical the proxy tool is **not** re-registered, which preserves the prompt
cache prefix. Composition, in order:

1. Fixed header. Upstream:
   `MCP gateway — server status, tool search/describe, auth, and single MCP tool calls. When one request needs several MCP calls with logic between them, use mcpScript. Non-MCP Pi tools should be called directly, not through mcp.\n`
   **The port drops the `mcpScript` sentence** (Cut 4), leaving
   `MCP gateway — server status, tool search/describe, auth, and single MCP tool calls. Non-MCP tools should be called directly, not through mcp.\n`.
   This is a deliberate, named divergence: advertising a tool that does not exist is worse than the
   text delta.
2. If any direct tools exist:
   `` `\nDirect tools available (call as normal tools): ${server} (${count}), …\n` ``.
3. Per enabled server, count the allowed+visible tools and the allowed resources from a *valid* cache
   entry (an invalid entry yields `undefined` and the counts are 0); `totalItems === 0` → skip;
   `proxyCount = totalItems − directCount`; `proxyCount > 0` → `` `${serverName} (${proxyCount} tools)` ``.
   Joined as `` `\nServers: …\n` ``.
4. Disabled servers → `` `\nDisabled servers (enable with /mcp enable <server> and /reload): …\n` ``.
5. Per server with cached `instructions` (whitespace collapsed to single spaces, trimmed, then
   `truncateAtWord(…, 150)` — `INSTRUCTIONS_SNIPPET_LENGTH`), a two-space-indented
   `` `  ${serverName}: ${snippet}` ``, under the header
   `` `\nServer instructions (truncated - full text via mcp({ instructions: "name" })):\n` ``.
   `truncateAtWord(text, target)`: no-op when falsy or `text.length <= target`; slice to `target`, find
   the last space; if that index `> target * 0.6`, cut there and append `"..."`, else append `"..."` to
   the raw slice.
6. The fixed usage block — the literal `\nUsage:\n` and these lines, byte-identical, column alignment
   included, **minus the `ui-messages` line** (Cut 2):

```
  mcp({ })                              → Show server status
  mcp({ server: "name" })               → List tools from server
  mcp({ search: "query" })              → Search MCP tools by name/description
  mcp({ describe: "tool_name" })        → Show tool details and parameters
  mcp({ instructions: "name" })         → Show full server usage instructions
  mcp({ connect: "server-name" })       → Connect to a server and refresh metadata
  mcp({ tool: "name", args: { key: "value" } })         → Call a tool (object args; JSON string also accepted)
  mcp({ action: "auth-start", server: "name" })      → Start manual OAuth and get a browser URL
  mcp({ action: "auth-complete", server: "name", args: { redirectUrl: "..." } }) → Complete manual OAuth
```

   then the trailing
   `\nMode: action > tool (call) > connect > describe > instructions > search > server (list) > nothing (status)`.

#### 7. `createDirectToolExecutor` — the per-tool execute state machine

Every early return is a **successful** tool result carrying a `details.error` code, never a thrown
error — the host merges `{isError:true}` back on for exactly two of those codes via
`error-signal.ts`'s `toolErrorOverride`. Ordered:

| step | condition | returned `details` | text |
| --- | --- | --- | --- |
| 1 | `signal` already aborted | *(throws)* | — |
| 2 | no state, init promise rejects | `{error:"init_failed", message}` | `MCP initialization failed: ${message}` |
| 3 | still no state | `{error:"not_initialized"}` | `MCP not initialized` |
| 4 | server disabled | `{error:"server_disabled", server, message}` | `MCP server "${s}" is disabled. Run /mcp enable ${s} and /reload to enable it.` |
| 5 | `lazyConnect` false **and** connection status `needs-auth` | auto-auth attempt; failure → `{error:"auth_required", server, message}` | `attemptDirectAutoAuth`'s message |
| 6 | still not connected, status `needs-auth` | `{error:"auth_required", server, message, autoAuthAttempted}` | `getDirectAuthRequiredMessage` |
| 7 | still not connected | `{error:"server_unavailable", server}` | `MCP server "${s}" not available` + `` ` (failed ${n}s ago)` `` when a failure age exists |
| 8 | connection missing or status ≠ `connected` | `{error:"not_connected", server}` | `MCP server "${s}" not connected` |
| 9 | approval gate returns `ok:false` | `{error:"approval_denied"\|"approval_required", server, tool}` | see §9 |
| 10a | `spec.resourceUri` set | `{server, resourceUri, …guardedMcpDetails}` | guarded resource contents, `"(empty resource)"` when none |
| 10b | `result.isError` | `{error:"tool_error", server, …guarded}` | guarded content, prefix `"Error: "`, suffix = formatted schema **when `spec.inputSchema` is set**, `emptyTextFallback: "Tool execution failed"` |
| 10c | *(cut — the UI-enabled-tool branch, Cut 2)* | — | — |
| 10d | normal success | `{server, tool, …guarded}` | guarded content, `"(empty result)"` when none |
| 11 | `SessionRecoveryAuthRequiredError` | `{error:"auth_required", server, message, autoAuthAttempted}` | the error's `authMessage` or the default |
| 12 | `UrlElicitationRequiredError` | `{error:"url_elicitation_required", server, action}` | accept → `The original MCP tool did not run. Complete the opened browser interaction, then retry the tool.`; else `The URL interaction was declined.` / `The URL interaction was cancelled.` |
| 13 | any other throw | `{error: aborted ? "aborted" : "call_failed", server, …guarded}` | guarded `[message]`, prefix `"Failed to call tool: "`, suffix = formatted schema **when `spec.inputSchema` is set** |

Ancillary invariants:

- `ownedSignal = combineAbortSignals(state.owner?.signal, signal)` — the tool call is fenced by the
  runtime owner as well as the host's per-call signal.
- `requestOptions = state.manager.getRequestOptions?.(serverName, ownedSignal) ?? (ownedSignal ? { signal: ownedSignal } : undefined)`
  — the per-server request timeout (`settings.requestTimeoutMs`) travels on every
  `callTool`/`readResource` through this object, not through the executor.
- `recoverAuthConnection` is the `onNeedsAuth` callback handed to `withSessionRecovery` on both the
  `readResource` and `callTool` paths — see MCP-214a.
- `state.manager.touch(serverName)` + `incrementInFlight` before the call, `decrementInFlight` +
  `touch` in `finally` — the idle-shutdown timer must not fire during a call.
- **Cut 2 removes**, in this file: `maybeStartUiSession` / `summarizeUiSessionResult`, the
  `uiSession.sendToolResult` / `sendToolCancelled` calls on the success and all three catch arms, the
  `_meta: uiSession?.requestMeta` injection on the request, the `uiSession.reused` close in `finally`,
  and branch 10c with its `uiOpen`/`uiViewer`/`uiUrl` details keys. **The remaining executor must
  still do**, in order: disabled-server check → owned-signal composition → `lazyConnect` →
  auto-auth-on-`needs-auth` → connection assertion → `ensureToolCallApproved` → request options →
  `withSessionRecovery`-wrapped `tools/call` (or `resources/read`) → content transform → output guard →
  error/abort mapping → in-flight decrement.

**`attemptDirectAutoAuth`**: returns `skipped` unless `settings.autoAuth === true`; also `skipped` when
the server is absent/disabled/non-OAuth, or `resolveServerUrl` yields `undefined`. A `resolveServerUrl`
**throw** becomes `failed` with `getDirectAuthFailedMessage`. Without a UI and with grant type ≠
`client_credentials`, it is `failed` with the auth-required message. An abort propagates.

Message templates:
- default auth-required:
  `MCP server "${s}" requires OAuth authentication. Run mcp({ action: "auth-start", server: "${s}" }) to get a browser URL, or /mcp-auth ${s} in an interactive local session.`
- `formatAuthRequiredMessage` (`utils.ts`) replaces it with `settings.authRequiredMessage` when set,
  substituting **every** occurrence of the literal `${server}` (`replaceAll`).
- auth-failed with a custom template:
  `OAuth authentication failed for "${s}": ${message}. ${customTemplateResult}`
- auth-failed without:
  `OAuth authentication failed for "${s}": ${message}. Run mcp({ action: "auth-start", server: "${s}" }) to get a browser URL, or /mcp-auth ${s} in an interactive local session.`

#### 8. Registration, deactivation, freezing and the fingerprint diff

`directToolFingerprint(spec)` is `JSON.stringify` of exactly
`{serverName, originalName, prefixedName, description, inputSchema, resourceUri, uiResourceUri, uiStreamMode}`
— key order is the literal's order, so the fingerprint is order-stable. With Cut 2 the last two are
always absent; **keep them in the fingerprint's key list** only if the on-disk spec still carries them
(it does not), otherwise drop them — the fingerprint is in-process state, not an on-disk contract, so
dropping them is safe. The cache *schema* keeps its UI fields regardless (§ cross-crate contract).

`registerDirectTool(spec)` registers:

| field | value |
| --- | --- |
| `name` | `spec.prefixedName` |
| `label` | `` `MCP: ${spec.originalName}` `` |
| `description` | `spec.description \|\| "(no description)"` |
| `promptSnippet` | `truncateAtWord(spec.description, 100) \|\| \`MCP tool from ${spec.serverName}\`` |
| `parameters` | `toToolParameters(normalizeDirectToolInputSchema(spec.inputSchema))` — a non-object schema becomes `{type:"object", properties:{}}`, and `$schema` **and `additionalProperties`** are stripped; `toToolParameters` is a TypeBox shim with no Rust analogue (`Tool::parameters` is raw JSON Schema) |
| `renderShell` | `"self"` when `resultRendering === "compact"`, else `"default"` |
| `renderCall` / `renderResult` | the renderers from §12 |

`syncDirectTools(config, cache)`: for each resolved spec, compare the fingerprint against
`registeredDirectTools`; on difference, **re-register** (upstream registration is idempotent-by-name)
and record; if the name was in `fallbackDeactivatedTools`, re-add it to the active set via
`setActiveTools([...activeTools, name])`. Then every previously registered name absent from the new set
is dropped from the map and passed to `deactivateTools`.

`deactivateTools(toolNames)` is the load-bearing hack: pi's `ExtensionAPI` has **no** `unregisterTool`
at the pinned version, so the adapter probes for it through a cast and, for every name that was not
truly unregistered, falls back to removing the name from `pi.setActiveTools(...)` and remembering it in
`fallbackDeactivatedTools` so a later re-registration can re-activate it. `getActiveToolsIfReady()`
swallows exactly the pre-bind error whose message contains
`"Action methods cannot be called during extension loading"` and returns `undefined`, because
registration happens before the host runtime is bound. **cyrup lands on the same fallback branch**:
`ExtensionRegistry` has no `unregister_tool` (only `clear`), and `HostServices::{active_tools,
set_active_tools}` are exactly the fallback's two verbs. Accepted delta — a removed tool stops being
callable but its name remains in the registry for the session, which is a supported upstream
configuration, not a gap.

`syncToolSurface(ctx)` is the single entry point: reload the cache, `syncDirectTools`, `syncProxyTool`,
and — when anything changed and `ctx.hasUI` — emit the toast
`` `MCP: direct tools refreshed (+${added}, ~${updated}, -${deactivated})` `` at level `"info"`.

**Freezing.** `settings.freezeDirectTools === true` sets `directToolsFrozen` immediately after the
*initial* post-init sync, logging
`MCP: direct tools frozen after initial sync — reconnects won't rebuild the system prompt; use mcp({ connect: "server" }) to rediscover`.
Once frozen, `onToolMetadataUpdated` still runs `syncPromptCommands()` but skips `syncToolSurface`,
logging `` `MCP: metadata update for ${serverName} (${reason}) skipped — directTools frozen` ``. The two
escape hatches are explicit: `/mcp reconnect` calls `syncToolSurface` *only* when frozen, and
`mcp({connect})` always calls it.

`syncProxyTool(config, cache, directSpecs)`: the `mcp` tool is registered when
`settings.disableProxyTool !== true` **or** there are zero direct specs **or** some configured
direct-tool server has no valid cache entry — i.e. disabling the proxy is only honoured once the direct
surface is actually complete. When it should be registered and the description changed, re-register;
when unchanged, just ensure `"mcp"` is in the active set. When it should not be, attempt deactivation
and clear `proxyToolRegistered` only if a true unregister succeeded.

The `mcp` tool's registration shape: `name: "mcp"`, `label: "MCP"`, `description` = the built string,
`promptSnippet: "MCP gateway — status, search, describe, auth, and single MCP tool calls"`,
`renderShell: toolRenderShell`, `renderCall: createMcpProxyToolCallRenderer(toolRenderOptions)`,
`renderResult: renderMcpToolResult`. Its parameter schema: `tool`, `args` (a union of a JSON string and
an open object), `connect`, `describe`, `instructions`, `search`, `regex`, `includeSchemas`, `limit`
(minimum 1), `offset` (minimum 0), `server`, `action` — twelve, all optional. `action`'s description
upstream reads `Action: 'ui-messages', 'auth-start', or 'auth-complete'`; **the port's reads
`Action: 'auth-start' or 'auth-complete'`** (Cut 2). `args` parsing in `execute`: a string is
`JSON.parse`d and a `SyntaxError` is rethrown as `` `Invalid args JSON: ${error.message}` ``; a
non-object/array/null result throws `` `Invalid args: expected a JSON object, got ${gotType}` ``.

#### 9. The approval gate

Upstream checks **three** gates in this order inside `ensureToolCallApproved`:

1. **Session cache.** Key is `` `${serverName}\u0000${toolMeta.originalName}` `` — a NUL separator,
   deliberately un-forgeable from a name. A hit returns `{ok:true}` immediately.
2. **Broker** (`requestBrokerApproval`). No `state.approvalEvents` → immediate `"abstain"`. Otherwise a
   `McpToolApprovalRequest` with a fresh `randomUUID()` `requestId`, the server name, both tool names,
   the args, an `origin`, the optional signal and a `claim(handler)` closure is emitted
   **synchronously** on the event bus under the topic `pi-mcp-adapter:tool-approval-request`. `claim`
   accepts the first handler and only while the emit is on the stack. No claim → `"abstain"`. A claimed
   handler is awaited under `abortable`; a non-decision return, or any throw with the signal not
   aborted, is coerced to `"deny"`. `allow_once` → ok; `allow_for_session` → cache + ok; `deny` →
   denied; `abstain` → fall through.
   **The port drops this step** — see MCP-233: `ExtHooks::before_tool_call` +
   `cyrup-permission-system` is the same ownership transfer, already wired, already fail-closed, and
   cyrup's `SharedBus` is JSON-payload-only and deferred so the `claim` closure cannot cross it by
   construction. `McpToolApprovalOrigin` also loses its `"script"` (Cut 4) and `"iframe"` (Cut 2)
   variants, leaving `"proxy" | "direct" | "resource"`.
3. **`approveTools` pattern gate** — `isToolCallApprovalRequired`, reached on `abstain` (in the port,
   reached unconditionally after the cache miss). `definition.approveTools` when defined, else
   `settings.approveTools`. `true` → always required. Non-array or empty → not required. Otherwise the
   same legacy-disambiguation dance as `matchesToolSelector`, with one extra wrinkle: the first current
   candidate that differs from the bare `originalName`, `-`→`_` normalized, is explicitly added to the
   legacy set. When the *server-level* `approveTools` was the source, `otherCurrentCandidates` is drawn
   from that server's metadata only; when the *global* setting was the source, from every server's
   metadata under each server's own effective prefix. The two scopes also differ in the no-metadata
   case: the server arm falls back to the *full legacy* set, the global arm returns `false`. That
   asymmetry is real, not a typo.
4. **Headless refusal**: approval required but `state.ui` absent → `{ok:false, reason:"approval_required_headless"}`.
5. **The dialog**: `json = JSON.stringify(args ?? {}, null, 2)`; `sanitized = sanitizeTerminalText(json)`;
   `preview = sanitized.length > 500 ? sanitized.slice(0,500) + "..." : sanitized`; title
   `` `MCP: ${sanitizeTerminalText(serverName)} wants to run ${sanitizeTerminalText(originalName)}` ``;
   the select prompt is `` `${title}\n\nArguments:\n${preview}` `` with options
   `["Allow once", "Allow for session", "Deny"]`, awaited under
   `combineAbortSignals(state.owner?.signal, signal)`. `"Allow once"` → ok; `"Allow for session"` →
   cache + ok; **anything else, including a cancelled dialog returning `undefined`, is a deny.**

`sanitizeTerminalText` is, in order:

1. `stripOscSequences` — a hand-written scanner over both `ESC ]` and C1 `0x9D` introducers, terminated
   by BEL `0x07`, C1 ST `0x9C` or `ESC \`, and tolerating an unterminated payload (it consumes to
   end-of-string).
2. `.replace(/(?:\x1b\[[0-?]*[ -\/]*[@-~]|\x1b[@-Z\\-_])/g, "")` — the ANSI CSI/Fe scrub.
3. `.replace(/[\u0000-\u001f\u007f-\u009f]+/g, " ")` — **every C0 and C1 control run becomes one
   space.** (The source writes this class with two literal control characters; the escaped form here is
   the copyable equivalent.)
4. `.replace(/\s+/g, " ")`
5. `.trim()`

Skipping it would let a server's tool name paint arbitrary text over the approval dialog.

The direct-tool caller passes `origin = spec.resourceUri ? "resource" : "direct"` (the function's own
default is `resourceUri ? "resource" : "proxy"`) and maps the refusal to one of two messages:

- denied: `The user declined approval to run MCP tool "${originalName}" on server "${serverName}".`
- headless: `MCP tool "${originalName}" on server "${serverName}" is approval-gated and requires an interactive session.`

#### 10. `mcp-output-guard.ts` — what it guards and what it does not

**It is a size guard, not a safety guard.** No prompt-injection detection, no secret/credential
redaction, no content classification, no allow/deny on the *text*. What it defends against is a single
MCP tool result blowing out the context window or the session transcript — an availability and cost
property, and (via `details.mcpResult`) an unbounded-session-file property.

Defaults: `DEFAULT_MCP_OUTPUT_MAX_BYTES = 50 * 1024`, `DEFAULT_MCP_OUTPUT_MAX_LINES = 2000`,
`DEFAULT_MCP_DETAILS_MAX_BYTES = 16 * 1024`. Summary caps: `CONTENT_SUMMARY_LIMIT = 20`,
`KEY_PREVIEW_LIMIT = 20`, `KEY_MAX_CHARS = 120`.

`resolveMcpOutputGuardOptions(settings)`: `enabled = envKillSwitch("MCP_OUTPUT_GUARD") ?? (configured !== false)`.
`envKillSwitch` trims+lowercases the env value; `"0"|"false"|"no"|"off"` → `false`;
`"1"|"true"|"yes"|"on"` → `true`; empty or unrecognised → `undefined` (i.e. the env var **cannot**
force a state through a bad value, it falls back to the setting). The three numeric limits come from
`positiveInt` (finite number, floored, `> 0`) over the object form, else the defaults.

`guardMcpOutput(content, options)`:

1. `normalizedContent` = `sanitizeContent(content)` when non-empty, else a single text block with
   `options.emptyTextFallback ?? "(empty result)"`; then `withEmptyTextFallback`.
   - `sanitizeContent` touches image blocks only: a non-blank string `mimeType` is trimmed and
     **sliced to 100 chars**, anything else becomes `"image/png"`.
   - `withEmptyTextFallback`: when a fallback was supplied and the joined text of all text blocks is
     falsy, replace the whole array with `[{text: fallback}, ...imageBlocks]`.
2. `enabled === false` → return `addAffixes(normalizedContent, prefix, suffix)` plus the raw
   `rawMcpResult` untouched. **The kill switch disables the `details.mcpResult` bounding too.**
3. Split off image blocks; join every text block's text with `"\n"`;
   `composedOutput = prefix + text + suffix`; `stats = textStats(composedOutput)` where `textStats` is
   `{bytes: Buffer.byteLength(text,"utf8"), lines: text.length === 0 ? 0 : text.split("\n").length}`.
4. `guardedContent = addAffixes(normalizedContent, prefix, suffix)`: a prefix is prepended to the
   **first** text block (or unshifted as a new block when none exists); a suffix is appended to the
   **last** text block (or pushed as a new block). Non-text blocks keep their positions.
5. Truncation fires when `stats.bytes > maxBytes || stats.lines > maxLines`:
   1. `saveArtifact("output", composedOutput)`: `mkdtemp(join(tmpdir(), "pi-mcp-output-"))`, then
      `join(dir, \`output-${randomBytes(4).toString("hex")}.txt\`)`, written with
      `{encoding:"utf8", mode: 0o600}`. Any throw returns `{error: message}` and no path.
   2. `notice = formatTruncationNotice(stats, path, writeError)`:
      - with a path: `` `[MCP text output truncated: original ${lines.toLocaleString()} lines / ${formatSize(bytes)}. Full text saved to: ${path} — use read with offset/limit or grep to inspect.]` ``
      - without: `` `[MCP text output truncated: original ${lines.toLocaleString()} lines / ${formatSize(bytes)}. Full output could not be saved: ${writeError ?? "unknown error"}]` ``
      - `formatSize`: `< 1024` → `` `${bytes} B` ``; `< 1 MiB` → `` `${(b/1024).toFixed(1)} KiB` ``;
        else `` `${(b/1048576).toFixed(1)} MiB` ``. **Note the space and the `KiB`/`MiB` units.**
      - `toLocaleString()` on the line count is locale-dependent; in the default `en-US` ICU locale it
        is thousands-grouped with commas.
   3. `previewBudget = reserveBudget(maxBytes, maxLines, notice)`: subtract `textStats("\n\n" + notice)`
      from both caps, clamped at 0.
   4. `preview = truncateHead(composedOutput, budget.maxBytes, budget.maxLines)`: split on `"\n"`;
      accumulate whole lines while `output.length < maxLines`; for each line, the cost is
      `byteLength(line)` plus 1 for the joining newline when not the first; if adding it would exceed
      `maxBytes`, take `remaining = maxBytes − bytes − separator` and, when positive, push
      `truncateStringToBytes(line, remaining)` — **so the guard does emit a partial line** — then break.
   5. `truncateStringToBytes(value, maxBytes)`: floor `maxBytes`, then walk `end` backwards while
      `(buffer.readUInt8(end) & 0xc0) === 0x80`, i.e. back off UTF-8 continuation bytes, and slice.
      (Reading at index `end` — the byte *after* the last kept byte — is what makes the boundary check
      correct.)
   6. `finalText = \`${preview.content}\n\n${notice}\``; content becomes
      `[{type:"text", text: finalText}, ...imageBlocks]` — **all other original blocks are discarded**;
      images pass through untouched because they reach the provider as native image content.
   7. `outputGuard = {truncated: true, originalBytes, returnedBytes, originalLines, returnedLines,
      imageBlocksPassedThrough?, fullOutputPath?, writeError?}`, optional keys omitted when absent;
      `returnedBytes`/`returnedLines` come from `textStats(finalText)`.
6. `mcpResult = rawMcpResult === undefined ? undefined : boundMcpResult(rawMcpResult, detailsMaxBytes)`.
   Direct tools **never** pass `rawMcpResult`, so this path is proxy-only.

`boundMcpResult`: `safeStringify` (plain `JSON.stringify`, falling back to `String(value)` on throw —
so a cycle degrades rather than crashing); when `byteLength(raw) <= detailsMaxBytes` the raw value is
kept **by reference**, else `summarizeMcpResult`.

`summarizeMcpResult(result, raw, rawBytes)` spills `raw` via `saveArtifact("mcp-result", raw)` and
returns `McpResultSummary`:
`{omitted: true, reason: "Raw MCP result exceeded the details size limit and was replaced with this summary to keep session context bounded.", isError: record?.isError === true, contentBlocks, contentSummary, rawResultBytes, fullResultPath?, resultWriteError?}`
plus `structuredContent` / `meta` when the corresponding keys are *present* (`in`, not truthy), plus
`extraFields` — every key outside `{content, isError, structuredContent, _meta}`, capped at 20, each as
`{key: truncateKey(key), type: typeof value, estimatedBytes, omitted: true}`.

`summarizeContent(content)`: the first 20 blocks, each as `{type:"text", bytes, lines, textOmitted:true}`
/ `{type:"image", mimeType, dataBytes, dataOmitted:true}` / `{type, estimatedBytes, omitted:true}`; a
non-object block becomes `{type: typeof block, omitted:true}`; overflow appends
`{type:"omitted", count: length − 20}`.

`summarizeValue(value)`: non-object → `{type: value === null ? "null" : typeof value, estimatedBytes, omitted:true}`;
object/array → `{type: "array"|"object", estimatedBytes, keyCount, keysPreview: first 20 keys truncated, omitted:true}`.
`estimateValueBytes(value, depth = 0)` is a **depth-2** recursive sum: null/undefined → 0; string → byte
length; number/boolean/bigint → byte length of its `String()`; object/array at depth < 2 → sum over the
first 20 entries at depth+1; else 0. `truncateKey(key)`: `> 120` chars → first 119 chars + `"…"`.

`guardedMcpDetails(guarded)` is the spread helper: it emits `mcpResult` only when defined and
`outputGuard` only when present.

#### 11. Content transformation and binary-resource materialization

Constants: `MAX_BINARY_RESOURCE_BYTES = 10 * 1024 * 1024`, `MAX_SESSION_RESOURCE_BYTES = 100 * 1024 * 1024`,
`MAX_SESSION_RESOURCE_FILES = 10_000`, `CLEANUP_RETRY_DELAY_MS = 30_000`,
`MAX_CLEANUP_RETRY_ATTEMPTS = 3`.

`transformMcpContent(content, scope)` — every standard MCP content type, in the source's branch order.
**All of these survive the cuts; only `ui://` resource rendering (which lives in
`ui-resource-handler.ts`, not here) is gone:**

| MCP type | host block |
| --- | --- |
| `text` | `{type:"text", text: c.text ?? ""}` |
| `image` | `{type:"image", data: c.data ?? "", mimeType: c.mimeType ?? "image/png"}` — the **only** non-text output |
| `resource` with a string `resource.blob` | `{type:"text", text: materializeBinaryResource(...)}` |
| `resource` otherwise | `{type:"text", text: \`[Resource: ${c.resource?.uri ?? "(no URI)"}]\n${resourceContent}\`}` where `resourceContent = c.resource?.text ?? (c.resource ? JSON.stringify(c.resource) : "(no content)")` — the `(no content)` fallback fires when the `resource` object itself is absent, **not** from a `JSON.stringify` result |
| `resource_link` | `{type:"text", text: \`[Resource Link: ${name ?? uri ?? "unknown"}]\nURI: ${uri ?? "(no URI)"}\`}` |
| `audio` | `{type:"text", text: \`[Audio content: ${mimeType ?? "audio/*"}]\`}` — audio is **never** forwarded as data |
| anything else | `{type:"text", text: JSON.stringify(c)}` |

`transformMcpResourceContents(contents, scope)` is the `resources/read` shape: a string `text` wins;
else a string `blob` is materialized; else the whole record is `JSON.stringify`d.

`resolveMcpResultContent(result, scope)`: transform `result.content` when it is an array; if the
resulting block list is empty and `result.structuredContent` is neither `undefined` nor `null`, return
one text block containing `JSON.stringify(value, null, 2)` (falling back to `String(value)` on throw);
otherwise an empty list.

`materializeBinaryResource(resource, scope)`:

1. `getMaterializedResourceSession(scope)` — an aborted scope (`"aborted" in scope && scope.aborted === true`)
   returns `undefined` → `omitBinaryResource(resource, "runtime stopped")`; no scope → a module-global
   default session; otherwise a `WeakMap`-keyed per-scope session.
2. `decodedBytes = Buffer.byteLength(resource.blob, "base64")` — this measures the *decoded* size of the
   base64 string.
3. `> 10 MiB` → omit with reason `"decoded size exceeds 10 MiB"`.
4. `session.bytes + decodedBytes > 100 MiB || session.files >= 10_000` → omit with
   `"session resource limit reached"`.
5. `session.directory ??= mkdtempSync(join(tmpdir(), "pi-mcp-resource-"))`; a throw → omit with
   `"could not be saved"`.
6. Path `join(dir, \`resource-${++session.sequence}.bin\`)`; the counters are incremented **before** the
   write; `writeFileSync(path, Buffer.from(blob,"base64"), {flag:"wx", mode:0o600})` — `wx` fails if the
   file exists. On failure, `rmSync(path, {force:true})` and roll the counters back; if the removal
   itself throws, the reservation is deliberately **kept**. Return omit `"could not be saved"`.
7. Success → `replaceBlob` (which **deletes** the `blob` key and sets `text` in place — the input object
   is mutated) with the three-line body
   `[Resource: ${uri ?? "(no URI)"}]` / `Binary content saved to ${filePath}` /
   `MIME type: ${mimeType ?? "application/octet-stream"}`. `omitBinaryResource` uses the same shape with
   `Binary content omitted: ${reason}`.

`cleanupMaterializedBinaryResources(scope)`: move the session's directory into
`pendingCleanupDirectories`, zero the counters, drop the scope entry, then
`drainPendingCleanupDirectories()`. The drain does `rmSync(dir, {recursive:true, force:true})` per
directory, removing succeeded ones, clearing the pending timer when the set empties; if any failed it
reschedules (only when at least one directory is still under `MAX_CLEANUP_RETRY_ATTEMPTS`, incrementing
every retryable directory's counter first) and throws an `AggregateError` with the message
`Failed to clean materialized MCP resources`.

#### 12. Result and call rendering

Constants: `DEFAULT_MAX_CALL_INPUT_CHARS = 1500`, `DEFAULT_BOXED_COLLAPSED_LINES = 3`,
`DEFAULT_COMPACT_COLLAPSED_LINES = 1`, `DEFAULT_MAX_COLLAPSED_CHARS = 8000`,
`COLLAPSED_RENDER_CHAR_SLACK = 8`.

`resolveMcpToolRenderOptions(settings)`: `resultRendering = settings.toolResultRendering === "boxed" ? "boxed" : "compact"`;
`collapsedResultLines` accepts only the literals 1, 2, 3, defaulting to 3 in boxed mode and 1 in compact.

**Call rows.** `formatMcpProxyToolCallLines(args, maxInputChars = 1500)`, first match wins:

| condition | lines |
| --- | --- |
| ~~`action === "ui-messages"`~~ | *(cut — Cut 2; the only `ui` reference in the whole file)* |
| `tool` | ``[`mcp call ${server ? `${tool} @ ${server}` : tool}`]``, plus `formatJsonish(args, 1500)` when `args` is truthy |
| `connect` | ``[`mcp connect ${connect}`]`` |
| `describe` | ``[`mcp describe ${describe}`]`` |
| `search` | one line `` `mcp search ${search}` `` + `` ` @ ${server}` `` + `" (regex)"` when `regex === true` + `" (schemas hidden)"` when `includeSchemas === false` |
| `server` | ``[`mcp list ${server}`]`` |
| `action` | ``[`mcp ${action}`]`` |
| otherwise | `["mcp status"]` |

`formatMcpDirectToolCallLines(displayName, args, 1500)`: just `[displayName]` unless
`hasUsefulObjectContent(args)` (a non-array object with ≥1 key), else
`[displayName, formatJsonish(args, 1500)]`.

`formatJsonish(value, maxChars)`: a string is `JSON.parse`d and re-stringified with 2-space indent when
it parses, else used raw; anything else is `JSON.stringify(value, null, 2)`; either way through
`truncateText(value, maxChars)` (`value.slice(0, max(0, maxChars−1)) + "…"`).

`renderToolCallLines(lines, theme)`: the theme defaults to `plainTheme`; the first line is
`theme.fg("toolTitle", theme.bold?(title) ?? title)` with `title` defaulting to `"mcp"`, the rest are
`theme.fg("muted", line)`, joined with `"\n"` into one `Text`.

`renderToolCall` stashes `lines[0] ?? "mcp"` into `context.state.compactTitle`, then returns an
**`EmptyComponent`** when `shouldUseCompactFinalRender` (compact mode, a context exists,
`isPartial === false`, `expanded !== true`, `isError !== true`). This is the mechanism by which compact
mode collapses a call row and a result row into a single line: the *call* row draws nothing and the
*result* row re-prints the stashed title as its own prefix. **cyrup has no equivalent** — see MCP-243.

**Result rows.** `renderMcpToolResult(result, options, theme, context, renderOptions)`:

1. `options.isPartial` → a single `Text` of `theme.fg("warning", "Running MCP tool...")`.
2. `expanded = options.expanded || context?.isError === true || Boolean(result.details.error)` —
   **any `details.error` forces the expanded rendering**.
3. Not expanded and compact → `CompactMcpToolResult(title, display, theme)` where
   `title = context?.state?.compactTitle ?? formatMcpToolResultIdentity(result.details) ?? ""`.
4. Otherwise a `CollapsibleText` over
   `[theme.fg("muted", identity)?, ...lines.map(l => theme.fg("toolOutput", l))].join("\n")`, with
   `maxCollapsedLines = collapsedResultLines + (identity ? 1 : 0)`, ellipsis `theme.fg("muted","…")`,
   expand hint `theme.fg("muted","(Ctrl+O to expand)")`, and `preTruncated = display.truncated`.

`formatMcpToolResultIdentity(details)`: `null` unless `details.mode === "call"`; server is
`details.server` or `details.hintServer`; then `MCP ${server}/${tool}`, or
`MCP ${server} resource ${resourceUri}`, or `MCP ${server}/${requestedTool}`, else `null`.

`formatMcpToolResultLines(result, expanded, maxCollapsedLines, maxCollapsedChars)`: the expanded branch
is `content.flatMap(blockToLines)` or `["(empty result)"]`, never truncated; the collapsed branch
delegates to `collectCollapsedResultLines`.

`collectCollapsedResultLines(content, maxLines, maxChars)`: empty content →
`{lines:["(empty result)"], truncated:false}`. Otherwise walk blocks; a non-text block contributes
`` `[image: ${mimeType}]` ``; a text block is split on `"\n"` **without materializing the array** (an
`indexOf` loop). `appendLine` stops when `lines.length >= maxLines` or the char budget is exhausted
(setting `truncated`); a line longer than the remaining budget is pushed sliced and the budget zeroed;
otherwise the budget drops by `line.length + 1`. At the end an empty result gets one `""` line, and when
truncated **and** at the line cap a lone `"…"` line is appended.

`CompactMcpToolResult.render(width)`: drops a trailing `"…"` line when `display.truncated`; the first
line is prefixed `` `${theme.fg("toolTitle", title)} → ` `` when a title exists; every line is
`theme.fg("toolOutput", …)`. `hiddenText` is true when the display was truncated or any rendered body
exceeds the width. The last line then gets the 21-char `" … (Ctrl+O to expand)"` when
`safeWidth >= suffix.length + 20`, else the 9-char `" (Ctrl+O)"` when `safeWidth >= 14`, else the whole
line becomes `truncateToWidth(theme.fg("muted","(Ctrl+O)"), safeWidth, "…")`. Renders are memoized per
width.

`CollapsibleText.render(width)`: expanded → the full `Text`. Collapsed →
`charBudget = max(1, floor(width)) * (maxCollapsedLines + 1) * 8`; the source string is sliced to that
budget and rendered; if it was **not** pre-truncated, the slice covered the whole string, and the
rendered line count is `<= maxCollapsedLines`, the lines are returned as-is; otherwise the first
`maxCollapsedLines` lines plus a two-line footer (`ellipsis\nexpandHint`).

`blockToLines(block)`: text blocks split on `"\n"`; everything else is `[\`[image: ${mimeType}]\`]`.

The four factory wrappers — `renderMcpProxyToolCall`, `createMcpProxyToolCallRenderer`,
`createMcpDirectToolCallRenderer`, `createMcpToolResultRenderer` — only bind `McpToolRenderOptions`;
they carry no behaviour of their own.

**Where this lands in cyrup.** `InitApi::register_tool_renderer(tool_name)` declares the renderer;
`NativeExtension::{render_call, render_result}` receive `(key, payload)` and return a widget tree as
`serde_json::Value`; `ExtensionHost::render_tool_{call,result}` route by tool name; `cyrup-tui`
flattens the tree to a plain `String` through a fixed vocabulary (bare string, `text`, `markdown`,
`truncated-text`, `spacer`, `box`/`container`, `hstack`, bare array) under two named limits that
bound what a `render_result` may legally emit — `MAX_WIDGET_DEPTH = 16` and
`EXTENSION_RENDER_TIMEOUT = 2 s`, both in `crates/cyrup-tui/src/app.rs`. A tree deeper than
`MAX_WIDGET_DEPTH` is not partially drawn: `flatten_widget` returns `None` and `rendered_text` falls
back to the pretty-printed JSON of the **whole** tree, so one over-deep node costs the entire
rendering. A renderer that has not answered within `EXTENSION_RENDER_TIMEOUT` is **aborted** (not
detached) by `run_renderer` and the row draws with cyrup's built-in framing — so any per-call work a
`render_result` does (formatting, guarding, materialisation) must fit inside two seconds on the
interactive event path, and must never block on a `ui.*` capability whose reply that same loop
delivers. **No width, no styled-span node, and no per-row expansion flag cross that seam.** Two of the
three losses have host-side answers that already exist: `HostServices::tools_expanded()` reports the
global expand toggle and a native extension holds the `Arc<dyn HostServices>` to read it, and
`details.error` forces the expanded form on its own; `HostServices::theme()` / `theme_by_name()` return
the palette, but with no styled node to emit into, the port draws upstream's own `plainTheme` path. The
one true loss is render width, which only changes *which* of two "(Ctrl+O)" affordance strings is
appended. See MCP-241..MCP-245; none of it is a host prerequisite, and an in-tree extension
(`cyrup-ext-subagents`) already ships the same concession in its own `render_result` doc.

#### 13. Cross-crate contracts this section must satisfy

1. **The `mcp` proxy tool's parameter names.** `cyrup_permission_system::manager`'s
   `create_mcp_permission_targets` reads exactly `{tool, server, connect, describe, search}` off the
   call arguments, in that precedence, and falls through to the `mcp_status` baseline otherwise. The
   five baseline targets are `mcp_status`, `mcp_list`, `mcp_search`, `mcp_describe`, `mcp_connect`;
   derived forms are `<server>_<tool>`, `<server>:<tool>`, `<server>`, `<tool>`, plus `mcp_call`,
   `mcp_connect_<name>`, `mcp_server_<name>`. **Renaming a parameter silently changes which permission
   rules apply.** The other seven parameters (`args`, `regex`, `includeSchemas`, `limit`, `offset`,
   `instructions`, `action`) are not read by the target derivation and are safe to shape freely.
2. **`<agent_dir>/mcp-cache.json`, schema version 1.** `cyrup_ext_subagents::exec::mcp_direct_tools`
   already reads it (`CACHE_VERSION = 1`, `CACHE_MAX_AGE_MS = 7 days`, `compute_mcp_server_hash`).
   `cyrup-mcp` is the writer. Do **not** bump `CACHE_VERSION` to drop the now-dead `uiResourceUri` /
   `uiStreamMode` fields — leave them absent and ignored.
3. **The names themselves.** `mcp_direct_tools.rs` expands `mcp:server[/tool]` selectors into names it
   expects the child's registry to resolve, using a *drifted* copy of the rules. MCP-205.
4. **`Content` on the tool-result path.** `cyrup_core::Content` has `Text`, `Thinking`, `ToolCall`,
   `Image { data, mime_type }` — a faithful match for pi's `TextContent | ImageContent`, which is why
   `transformMcpContent`'s collapse of audio/resource/resource_link to text is mandatory, not a
   shortcut.

### Port units

*Id note: MCP-200..MCP-249 are this section's assigned range; MCP-214a, MCP-217a and MCP-217b are
suffixed additions so every existing id stays stable.*

**MCP-200 — The four-mode server-prefix / tool-name formatter** · high · M · **hand-written**
**upstream** — `types.ts`: `ToolPrefix`, `sanitizeServerPrefix` (code-point walk, `[A-Za-z0-9_-]` kept,
everything else `_<hex>_`), `getServerPrefix` (none/short/mcp/server), `formatToolName`
(`toolName.replace(/\./g,"_")`, joined with a single `_`), `resolveToolPrefix`.
**behavior** — every model-visible MCP tool name in the session, and therefore every permission rule,
every `excludeTools` entry and every metadata-cache key a subagent resolves against.
**cyrup** — pure `String` transformation in `cyrup-mcp`; no crate needed. Iterate `str::chars()` (code
points, matching `Array.from`) and format the escape as `format!("_{:x}_", ch as u32)`. The
`-?mcp$` case-insensitive strip is `to_ascii_lowercase().strip_suffix("mcp")` then `strip_suffix('-')`,
slicing the *original* by the resulting prefix length — `mcp_direct_tools.rs`'s `strip_mcp_suffix`
already does this correctly and is the model.
**verify** — table test over `{server, mode} × {plain, hyphenated, trailing-mcp, non-ASCII,
empty-after-strip}` asserting the exact prefixed name; include `("naïve","server") → "na_ef_ve"` and
`("github-mcp","short") → "github"`; plus a property test that no output contains a character outside
`[A-Za-z0-9_-]` except those the tool name itself contributed.

**MCP-201 — `getToolNameCandidates`, including the legacy arm** · high · M · **hand-written**
**upstream** — `types.ts`: `getLegacyServerPrefix` (`sanitizeServerPrefix(..., false)`, so `-`→`_2d_`),
`formatLegacyToolName` (`toolName.replace(/[.-]/g,"_")`), the 5 current expressions and **13** legacy
`add()` calls (5 + 4 + 4). Heavy overlap means the resulting *set* is far smaller than 18.
**behavior** — an `excludeTools`/`includeTools`/`approveTools` entry written against an older adapter
version keeps matching after an upgrade; without the legacy arm a user's existing exclusion silently
stops applying and a tool the user hid reappears.
**cyrup** — `HashSet<String>`; insertion order is irrelevant since only membership is read. Keep the two
sanitizers as separate functions so the `preserve_provider_valid` flag is explicit rather than a bare
bool at a call site.
**verify** — golden set for `("list-sims", "xcodebuild-mcp", Short)` asserting exactly **12** members:
current 4 = `list-sims`, `xcodebuild_list-sims`, `xcodebuild-mcp_list-sims`,
`mcp__xcodebuild-mcp_list-sims`; legacy-only 8 = `list_sims`, `xcodebuild_list_sims`,
`xcodebuild-mcp_list_sims`, `mcp__xcodebuild-mcp_list_sims`, `xcodebuild_2d_mcp_list_sims`,
`mcp__xcodebuild_2d_mcp_list_sims`, `xcodebuild_mcp_list_sims`, `mcp__xcodebuild_mcp_list_sims`.
Re-derive from the source expressions rather than copying this list.

**MCP-202 — `matchesToolPattern` / `matchesToolSelector` / `isToolAllowed`** · high · M · **hand-written**
**upstream** — `types.ts`: `globToRegExp` (escape `[.+^${}()|[\]\\]`, `*`→`.*`, `?`→`.`, anchored),
`matchesToolPattern`, `matchesToolSelector`'s three-step legacy disambiguation,
`isToolIncluded`/`isToolExcluded`/`isToolAllowed`.
**behavior** — the include/exclude contract users configure per server. Step 3 (a legacy candidate
matches **and** no other tool's current candidate matches) prevents a glob from excluding the wrong
server's tool once two sanitized prefixes collide.
**cyrup** — the `regex` crate, declared by `cyrup-mcp` itself: the workspace `[workspace.dependencies]`
table declares neither `regex` nor `tracing` (`grep-regex` is a different crate), and
`cyrup-permission-system` declares `regex = "1"` per-crate — follow that. Escape literal segments with
`regex::escape` and splice `.*`/`.` rather than reproducing the JS character class. Compile once per
pattern per resolution pass, not per candidate: upstream recompiles inside the `.some()` and that is a
hot loop. `regex` rejects lookaround/backreferences, which `globToRegExp` never emits.
**verify** — the disambiguation case: two servers `foo` and `foo-mcp` in `short` mode with
`excludeTools: ["foo_bar"]`, asserting exactly one of the two tools is excluded.

**MCP-203 — `resourceNameToToolName` and the `read_` resource base name** · medium · S · **hand-written**
**upstream** — `resource-tools.ts` `resourceNameToToolName`; the `read_` prefix in `direct-tools.ts` and
`tool-metadata.ts`.
**behavior** — the tool name a model calls to read an MCP resource.
**cyrup** — `cyrup_ext_subagents::exec::mcp_direct_tools`'s `resource_name_to_tool_name` is a correct
port of the slug function (including the astral-character case, because the run-collapse absorbs JS's
surrogate-pair double underscore) — but its caller builds `format!("get_{}", …)`, not `read_`. See
MCP-205.
**verify** — `("", "123abc", "a//b", "___", "Ünïcödé")` → `("resource", "resource_123abc", "a_b",
"resource", "n_c_d")`. The last is **not** `_n_c_d_`: leading and trailing underscores are stripped
before the lowercase step.

**MCP-204 — `resolveServerFromToolName` with its ambiguity fail-safe** · medium · S · **hand-written**
**upstream** — `types.ts` `resolveServerFromToolName`: longest-prefix inverse lookup, `undefined` for
`none` mode, and `undefined` when two different servers share the winning prefix.
**behavior** — a permission gate evaluating a server-scoped rule against a fully-qualified name recovers
the *right* server, or falls back to its wildcard path rather than enforcing the wrong server's rule.
**cyrup** — nothing equivalent exists. `cyrup_permission_system::manager`'s
`parse_qualified_mcp_tool_name` splits on the **first `:` only** and never on `_`; the `_` handling in
`add_derived_mcp_server_targets` reads the configured server names but matches each as a **suffix**
(`ends_with("_<server>")`) and explicitly *skips* the prefix form — the opposite orientation from
upstream. Net effect today: for `github_create_issue` the cyrup gate recovers no server at all. This
function is the missing half, and it is pure; whether the permission system consumes it is MCP-234.
**verify** — servers `{foo, foo-mcp}` in `short` mode with tool `foo_bar` → `None`; servers
`{foo, foobar}` in `server` mode with `foobar_x` → `Some("foobar")`.

**MCP-205 — Reconcile `mcp_direct_tools.rs` with `pi-mcp-adapter` v2.25.0 naming** · high · M · **open-decision**
**upstream** — `types.ts` (`sanitizeServerPrefix`, `getServerPrefix`, `formatToolName`, the candidate and
selector block) and `direct-tools.ts`'s `read_` base name, versus
`pi-subagents/src/runs/shared/mcp-direct-tool-allowlist.ts` (`getServerPrefix`, `formatToolName`,
`get_` base name), which is what the in-tree Rust was ported from. Both sides were read for this pass;
the drift is upstream's and the port inherits it.
**behavior** — a subagent declaring `tools: ["mcp:github"]` must receive the names the adapter actually
registered. Today the two sides disagree in six ways, so on any hyphenated server, any dotted tool name,
any `mcp`-prefix-mode config, and **every** resource, the subagent's allowlist selects names the child's
registry cannot resolve — a silently empty allowlist, which reads to the user as "the subagent ignored
my MCP tools". Nothing errors; the subagent simply answers without the tools it was granted.

| # | `cyrup_ext_subagents::exec::mcp_direct_tools` | `pi-mcp-adapter` v2.25.0 |
| --- | --- | --- |
| 1 | `get_server_prefix(Server)` = `name.replace('-', "_")` | `sanitizeServerPrefix(name)` — `-` is **kept** |
| 2 | `Short` = `strip_mcp_suffix(name).replace('-', "_")` | `sanitizeServerPrefix(strip)` — `-` kept |
| 3 | no `ToolPrefix::Mcp` variant (`get_tool_prefix` folds anything unknown to `Server`) | `"mcp"` mode → `mcp__<sanitized>` |
| 4 | `format_tool_name` passes the tool name through unchanged | `toolName.replace(/\./g, "_")` |
| 5 | resource base name `format!("get_{}", …)` | `` `read_${…}` `` |
| 6 | `is_tool_excluded`: 4 normalized candidates, exact match only | 18 candidate expressions → a deduped set + glob + other-tool disambiguation; plus `includeTools`, absent entirely |

Two further asymmetries worth naming: the Rust reader iterates a `BTreeMap` (name-sorted) where the
adapter iterates config insertion order, so the two can disagree about which of two colliding tools
wins; and the reader's `ServerEntry` has no `disabled` field, so a disabled server's cached tools are
still selectable. Also absent on the Rust side: the `uiVisibility` model-visibility filter. The
`BUILTIN_TOOL_NAMES` 8-element list is already correct there.
**cyrup** — three options, none silently acceptable: **(a)** `cyrup-mcp` emits v2.25.0 names and
`mcp_direct_tools.rs` is upgraded in the same change — self-consistent and upstream-faithful, but edits
a file outside `cyrup-mcp`; **(b)** `cyrup-mcp` emits the *pi-subagents* names — matches the in-tree
consumer, diverges from the adapter this section ports; **(c)** promote the naming rules to a module
both crates depend on (a small `cyrup-mcp-names` crate, or a module in `cyrup-core`) and upgrade the
consumer. **Recommend (c)**: the rules are pure functions with no I/O, the duplication *is* the defect,
it gives `cyrup-permission-system` a legal way to consume MCP-204, and it settles where
MCP-200..MCP-204 live. Note this is a workspace-layout decision between two extension crates, **not** a
host surface change.
**verify** — cyrup-it: register a fake MCP server with a hyphenated name, one dotted tool and one
resource; assert `resolve_mcp_direct_tool_names(["mcp:the-server"], cwd)` returns exactly the set of
names the adapter registered, by comparing against the live registry rather than a hard-coded list.

**MCP-206 — `sanitizePromptName` / `formatPromptCommandName`** · low · S · **hand-written**
**upstream** — `types.ts`: `mcp__${serverPart}__${sanitizedPrompt}`, where `serverPart` falls through
`getServerPrefix(...) || sanitizeServerPrefix(serverName) || "server"` so even `none` mode yields a
server segment.
**behavior** — the slash-command name a discovered MCP prompt is registered under, and the de-duplication
key in `registeredPromptCommands`.
**cyrup** — pure string work in `cyrup-mcp`. The *registration* of prompt commands after init is
`ExtensionHost`'s missing `register_late_command` sibling, folded into MCP-217; this unit is only the
name function, and the prompts section owns the discovery loop.
**verify** — `("summarize", "gh-mcp", ToolPrefix::None)` → `mcp__gh-mcp__summarize`.

**MCP-207 — `buildToolMetadata`** · high · L · **hand-written**
**upstream** — `tool-metadata.ts` `buildToolMetadata`: the full filter/dedupe pipeline including the
`knownMetadata` and `includeMissingConfiguredCandidates` arms, `failedTools` accumulation for unnamed
tools, and the fact that a visibility-hidden tool does **not** claim its name in `seenNames`.
**behavior** — `state.toolMetadata` is the authority for `describe`, `search`, the panel, the proxy tool's
`executeCall` name resolution and the metadata cache written to disk.
**cyrup** — a `Vec<ToolMetadata>` per server in `cyrup-mcp`; `serde_json::Value` for `inputSchema`
(`cyrup_core::Tool::parameters` is raw JSON Schema, so upstream's TypeBox `toToolParameters` shim
disappears). Every optional field must be `Option<T>` and **skipped** on serialize, not serialized as
`null`, because the metadata cache round-trips through the schema `mcp_direct_tools.rs` deserializes.
The `getToolUiResourceUri` / `extractToolUiStreamMode` steps and their `failedTools` arm are cut
(MCP-209); the `uiVisibility` step stays (MCP-208).
**verify** — a server with two tools whose sanitized names collide asserts the second is dropped; a
hidden tool followed by a visible tool of the same formatted name asserts the visible one survives.

**MCP-208 — `extractUiToolVisibility` / `isUiToolVisibleToModel` (the kept half of a cut file)** · medium · S · **hand-written**
**upstream** — `ui-tool-visibility.ts`. Two **fail-closed** paths: a non-array `visibility` and an array
containing an unknown member both return `[]`, which hides the tool from the model. The three *open*
paths are `_meta` absent, `_meta.ui` absent / non-object / an array, and `visibility === undefined`.
**behavior** — an MCP server can declare a tool that only an app surface may call. cyrup has no app
surface (Cut 2), but the server's declaration still means "not for the model", and three call sites in
`direct-tools.ts` plus one in `tool-metadata.ts` honour it. **Cutting this half would expose to the
model tools the server explicitly marked app-only** — a behaviour change in the wrong direction, for
four lines of code. `isUiToolCallableByApp` is cut; it has no caller without apps.
**cyrup** — a `serde_json::Value` walk, not a `#[derive(Deserialize)]`: the fail-closed semantics depend
on distinguishing "absent" from "present but malformed", which a lenient derive flattens.
**verify** — `{"ui":{"visibility":"model"}}` (a string, not an array) → hidden;
`{"ui":{"visibility":["model","weird"]}}` → hidden; `{"ui":["model"]}` (`ui` is an array) → visible;
absent `_meta` → visible.

**MCP-209 — `getToolUiResourceUri` / `extractToolUiStreamMode` and the UI spec fields** · n/a · S · **cut**
**upstream** — `ui-app-bridge-helpers.ts` `getToolUiResourceUri` (nested `_meta.ui.resourceUri` then the
flat `"ui/resourceUri"` key, throwing `Invalid UI resource URI: …` on a non-`ui://` value) and
`utils.ts` `extractToolUiStreamMode` (`_meta.ui["pi-mcp-adapter.streamMode"]`, only `"eager"` /
`"stream-first"`).
**cut** — Cut 2. Both exist solely to decide whether a direct tool opens an MCP-Apps UI session, which
the port does not have. Consequences: `DirectToolSpec.uiResourceUri` / `.uiStreamMode` and the
executor's UI branch disappear; the `failedTools` arm that recorded a malformed `ui://` value
disappears with them (the *other* `failedTools` arm, for unnamed tools, stays). **Keep the fields in the
on-disk cache schema, absent and ignored — do not renumber `CACHE_VERSION`.**
**verify** — unit: a tool whose `_meta.ui.resourceUri` holds a non-`ui://` value (upstream's throw
case) registers normally and does **not** appear in `failedTools`, while an unnamed tool still does;
plus a cache round-trip over a `mcp-cache.json` written by a co-installed pi adapter — entries
carrying `uiResourceUri`/`uiStreamMode` load, those two fields are ignored, `CACHE_VERSION` is
unchanged, and `cyrup_ext_subagents::exec::mcp_direct_tools` still reads the file cyrup wrote back.

**MCP-210 — `findToolByName`, `getToolNames`, `totalToolCount`** · medium · S · **hand-written**
**upstream** — `tool-metadata.ts`: exact match, then a `-`→`_`-normalized comparison on both sides.
**behavior** — a model that emits `github_create-issue` where the registered name is
`github_create_issue` still resolves; without the fallback the proxy returns "unknown tool".
**cyrup** — trivial; keep the two-pass order (exact wins) so a genuine `a-b` / `a_b` pair does not
resolve to the wrong tool.
**verify** — a metadata list containing both `x_a-b` and `x_a_b` asserts `find("x_a-b")` returns the
first.

**MCP-211 — `formatSchema` and its four helpers** · medium · M · **hand-written**
**upstream** — `tool-metadata.ts` `formatSchema`, `formatProperty` (including its non-object early
return), `formatNestedSchema`, `formatVariants`, `formatType`, `appendSchemaAnnotations` with the exact
annotation key order.
**behavior** — the body of `mcp({describe})`, and the `suffix` attached to the direct-tool `tool_error`
and `call_failed` results — **those two only, and only when `spec.inputSchema` is defined**. It is
model-facing text the model uses to correct a bad call, so drift changes retry behaviour.
**cyrup** — recursive `serde_json::Value` walk emitting `Vec<String>`. Two JS behaviours must be
reproduced deliberately: `Object.hasOwn(schema,"const")` distinguishes `const: null` from an absent
`const` (so `Map::contains_key`, not `get(..).is_some_and(..)`); and `JSON.stringify` of a JSON value is
`serde_json::to_string` — compact, no spaces — which is what the `[key: value]` annotations show.
`formatProperty` joins its parts with a single space, so the line is
`<indent><name> (<type>) *required* - <desc> [k: v]`.
**verify** — golden-string test over a nested `anyOf` + `items` + `enum` + `const: null` schema,
asserting the exact multi-line output byte-for-byte.

**MCP-212 — `resolveDirectTools`, including the builtin-collision drop** · **critical** · L · **hand-written**
**upstream** — `direct-tools.ts` `resolveDirectTools`: cache-gated, env-override-first filter resolution,
the four `console.warn` collision paths and the 75-tool advisory; `BUILTIN_NAMES` = `{read, bash, edit,
write, grep, find, ls, mcp}`.
**behavior** — the set of tools the model sees on turn 1. **The `BUILTIN_NAMES` drop is a security
control in cyrup, more so than upstream**: `ExtensionRegistry::active_tools`
(`crates/cyrup-ext/src/registry.rs`) walks the base tool list and substitutes the extension registry's
tool wherever the names match, so an MCP server shipping a tool named `read` would replace cyrup's
filesystem read tool for the whole session — every subsequent `read` call would be routed to the remote
server, silently, with the model's file paths as arguments. `InitApi::register_tool` documents the same
override rule. That is the permission-bypass clause, and it is why this unit is `critical`.
**cyrup** — pure resolution over the parsed cache; `tracing::warn!` for the five warnings, byte-identical
messages. `Vec<DirectToolSpec>` preserving **config iteration order**: `Object.entries` order is what
decides which of two colliding tools wins, and `serde_json` in this workspace is declared without
`preserve_order`, so `mcpServers` must deserialize into an ordered sequence — a `Vec<(String, ServerEntry)>`
visitor, which is exactly the pattern (and the reason) behind
`cyrup_permission_system::ordered::OrderedValue`. `cyrup_tools::registry`'s own `BUILTIN_NAMES` has 7
entries and does not include `mcp`; use the adapter's 8-element list, which
`mcp_direct_tools.rs`'s `BUILTIN_TOOL_NAMES` already carries verbatim.
**verify** — two servers each exposing a tool that formats to the same name → the second is dropped with
the duplicate warning; a server exposing a tool named `read` in `none` prefix mode → dropped with the
builtin warning. cyrup-it: assert the built-in `read` tool is still the one in `active_tools`
afterwards — assert the *built-in survives*, not merely that the MCP tool was skipped.

**MCP-213 — `buildProxyDescription`** · high · M · **hand-written**
**upstream** — `direct-tools.ts` `buildProxyDescription`: six composition steps, the 150-char instruction
snippet via `truncateAtWord`, and the fixed usage block.
**behavior** — the model's entire map of what MCP can do. It is also the **prompt-cache key**: the proxy
tool is re-registered only when the rebuilt string differs, so a non-deterministic rebuild invalidates
the provider's prompt cache on every metadata refresh.
**cyrup** — a `String` builder. Two literal edits from the cuts, both named in §6: drop the `mcpScript`
sentence from the header (Cut 4) and the `mcp({ action: "ui-messages" })` usage line (Cut 2).
Everything else stays byte-identical, column alignment included. Determinism requires the same
order-preserving `mcpServers` reader as MCP-212; a `HashMap` here would silently break prompt caching
without failing any test that only checks content.
**verify** — golden string for a two-server fixture; plus a determinism test that builds the description
100 times from the same config and asserts a single distinct value.

**MCP-214 — The direct-tool execute state machine** · high · L · **hand-written**
**upstream** — `direct-tools.ts` `createDirectToolExecutor`: the ordering in §7, the `finally` block's
`decrementInFlight`/`touch`.
**behavior** — every user-visible MCP failure message and every `details.error` code. Because the codes
drive the `{isError:true}` override, a wrong code silently changes whether the model treats a result as
a failure (see MCP-249, which carries that risk).
**cyrup** — `async fn execute` on the `Tool` impl. The failure shape maps onto
`Ok(ToolResult { content, details, .. })`, **not** `Err(ToolError)` — `cyrup-core`'s own module doc says
tools signal failure by returning `Err(ToolError)`, so this is a deliberate divergence to document at
the impl: an MCP tool error is a *successful* tool execution reporting a remote failure, and returning
`Err` would lose `details`. `combineAbortSignals(owner, signal)` becomes a
`tokio_util::sync::CancellationToken` child (workspace dep) or a `select!` over the owner token and the
`CancelToken` cyrup passes into `execute`. The `finally` block is a guard struct with a `Drop` impl,
because an early `?` must still decrement the in-flight counter. The UI-session interleave is cut
(MCP-209); the remaining order is listed in §7.
**verify** — cyrup-it: a fixture server that (a) is disabled, (b) returns `isError`, (c) times out,
(d) returns empty content — asserting the exact `details.error` value and message text for each.

**MCP-214a — `recoverAuthConnection` and the per-server request options** · high · M · **hand-written**
**upstream** — `direct-tools.ts` `recoverAuthConnection`, wired into both `withSessionRecovery` call
sites, and the `requestOptions` line in `createDirectToolExecutor`.
**behavior** — two things a user feels directly. **(1)** `recoverAuthConnection` is the `onNeedsAuth`
callback for both the `readResource` and `callTool` paths. When the transport drops mid-call into a
`needs-auth` state it runs a *second* auto-auth attempt — but only if `autoAuthAttempted` is still false
— and on failure throws `SessionRecoveryAuthRequiredError(serverName, message)`, which is what makes
step 11 of §7 reachable at all. On success it re-reads the connection, closes a still-`needs-auth` one,
clears the failure record and re-runs `lazyConnect`. Without it, a token that expires *during* a tool
call surfaces as `call_failed` with a transport message instead of `auth_required` with a recovery
instruction, and the model gives up instead of re-authenticating. **(2)** `requestOptions` is the only
path by which `settings.requestTimeoutMs` reaches a `callTool`/`readResource`; drop it and every MCP
call runs on the SDK default timeout.
**cyrup** — the closure captures mutable `autoAuthAttempted`, so in Rust it is a `Cell<bool>` on a
per-call state struct — **not** a fresh `false` per invocation, because the once-only guard is what stops
an auth loop. `getRequestOptions?.()` is an optional method on the manager; model it as an
`Option<PeerRequestOptions>` returned by a manager method. On the rmcp side the timeout lands in
`rmcp::service::PeerRequestOptions { timeout, reset_timeout_on_progress, max_total_timeout }`.
**verify** — cyrup-it: a fixture server that accepts `initialize` then answers `401` on the first
`tools/call`; assert the result is `details.error == "auth_required"` (not `call_failed`) and that the
auto-auth attempt runs exactly once. Unit: `requestTimeoutMs: 50` against a server that sleeps 500 ms →
the call aborts at ~50 ms.

**MCP-215 — `attemptDirectAutoAuth` and the auth message templates** · medium · M · **hand-written**
**upstream** — `direct-tools.ts` `attemptDirectAutoAuth` and `utils.ts` `formatAuthRequiredMessage`
(`replaceAll("${server}", name)`).
**behavior** — whether a `needs-auth` server silently re-authenticates mid-call, and exactly what the
model is told when it cannot. The headless + non-`client_credentials` refusal is what stops a print-mode
run from hanging on a browser flow.
**cyrup** — a small enum `DirectAutoAuth { Skipped, Success, Failed(String) }`; an abort must propagate
rather than becoming `Failed`. The OAuth machinery underneath is `rmcp::transport::auth` plus the
adapter's storage layer, owned by the OAuth section.
**verify** — `settings.authRequiredMessage = "auth ${server} now"` with server `x` → the returned text is
exactly `auth x now`, and the auth-*failed* wrapper is `OAuth authentication failed for "x": <msg>. auth x now`.

**MCP-216 — The direct-tool registration shape** · medium · M · **host-verb**
**upstream** — `index.ts` `registerDirectTool` and `utils.ts` `normalizeDirectToolInputSchema` (strips
`$schema` **and `additionalProperties`**).
**behavior** — `label` `MCP: <original>`, `description` falling back to `(no description)`,
`promptSnippet` = 100-char `truncateAtWord` or `MCP tool from <server>`, `renderShell` following the
render mode.
**cyrup** — every target trait member exists on `cyrup_core::Tool`: `name`, `parameters`, `description`,
`label`, `prompt_snippet`, `render_kind`. `InitApi::register_tool(Arc<dyn Tool>)` installs it at init.
`label`/`description`/`prompt_snippet` return `&str`/`Option<&str>`, so the strings must be **owned by
the tool struct** and computed at construction, not per call. `additionalProperties` stripping matters
for providers that reject an open schema.
**verify** — a spec with an empty description registers `description == "(no description)"` and
`prompt_snippet == "MCP tool from <server>"`.

**MCP-217 — Post-init dynamic tool (and command) registration** · high · L · **host-addition (HA-1)**
**upstream** — `index.ts` `syncDirectTools`' fingerprint diff, `deactivateTools`'
`unregisterTool`-probe-then-`setActiveTools` fallback, `fallbackDeactivatedTools`, and
`getActiveToolsIfReady`'s swallow of the pre-bind error; `syncProxyTool`'s description refresh;
`syncPromptCommands` for prompts.
**behavior** — a `mcp({connect:"x"})` or a server-side `tools/list_changed` adds, updates or removes
model-visible tools **mid-session** without restarting.
**cyrup** — the mechanism is complete and live; only the *handle* is missing.
`ExtensionHost::register_late_tool(owner, tool)` (`crates/cyrup-ext/src/facade.rs`) writes into
`ExtensionRegistry` and marks it dirty; `ExtensionHost::refresh_tools` / `active_tools` re-materialise;
`AgentSession::refresh_extension_tools` and `next_turn_tools`
(`crates/cyrup-session-svc/src/session.rs`) merge into `DynamicToolState`, auto-activate new names,
rebuild the system prompt and push to the live agent **at every turn boundary within a live run**. What
a native extension has are the `Arc<dyn HostServices>` late-bound by
`NativeExtension::set_host_services` and the per-dispatch `HostCtx`; `HostServices` has five tool-shaped
verbs (`active_tools`, `all_tool_names`, `set_active_tools`, `all_tools`, `commands`) and none that
*adds*. The WASM tier reaches the same thing through its `registration` WIT import, so this is a
two-tier asymmetry in one verb, not an absent capability. Two acceptable shapes: **(a)**
`NativeExtension::set_ext_host(&self, Weak<ExtensionHost>)`, a defaulted no-op called from
`ExtensionHost::load_native_with_services` beside the existing `set_host_services`; **(b)** defaulted
`register_late_tool` / `register_late_command` methods on `HostServices` backed by `LiveHostServices`
through a late-attach sink, the same shape `set_overlay_sink` / `attach_dynamic_tools` already use.
Take `register_late_command` with it — MCP prompt slash-commands need the identical seam
(`InitApi::register_command` is `&mut` and init-only). Deactivation needs nothing new:
`ExtensionRegistry` has no `unregister_tool`, so cyrup lands on upstream's own documented
`setActiveTools` fallback branch.
**Scheduling, not severity:** without it, on a cold `<agent_dir>/mcp-cache.json` the first session
exposes only the `mcp` proxy tool and direct tools appear from the next session; `mcp({connect})` cannot
surface a server's tools within the session; the proxy description cannot refresh mid-session; and
`McpSettings.disableProxyTool` must be treated as unsupported. Nothing is lost, nothing is wrong,
nothing crashes — upstream itself registers direct tools from the cache synchronously at load, so the
warm path is identical either way.
**verify** — cyrup-it: register the extension with an empty cache, connect a fixture server mid-session,
assert the new tool name appears in the agent's tool array on the **next turn** without a session
restart; then disable the server and assert the name leaves the active set.

**MCP-217a — `freezeDirectTools` and the frozen-surface escape hatches** · medium · S · **hand-written**
**upstream** — `types.ts`'s setting and `index.ts`'s `directToolsFrozen` handling.
**behavior** — a prompt-cache control users set deliberately. With `settings.freezeDirectTools: true`,
`directToolsFrozen` is set right after the *initial* post-init sync and logged as
`MCP: direct tools frozen after initial sync — reconnects won't rebuild the system prompt; use mcp({ connect: "server" }) to rediscover`.
Thereafter every automatic metadata update still refreshes prompt commands but **skips**
`syncToolSurface`, logging
`` `MCP: metadata update for ${serverName} (${reason}) skipped — directTools frozen` ``. Two paths still
rebuild: `/mcp reconnect` calls `syncToolSurface` *only when frozen* (the un-frozen case already gets it
via the metadata callback) and `mcp({connect})` always calls it. Omitting this makes every reconnect
invalidate the provider's prompt-cache prefix — exactly the cost the setting exists to avoid.
**cyrup** — a `bool` on the extension state, set once after the first successful sync, checked in the
metadata-changed callback. The `/mcp reconnect`-only call is **not** redundant and must not be
"simplified" away: without the freeze it would double-sync. Note cyrup's `push_active_tools` rebuilds the
system prompt unconditionally, so the setting's value is if anything higher here.
**verify** — with `freezeDirectTools: true`, a simulated `tools/list_changed` after the initial sync
leaves the registered tool set byte-identical and emits the skip log; `mcp({connect})` on the same state
does rebuild it.

**MCP-217b — The tool-surface refresh notification** · low · S · **host-verb**
**upstream** — `index.ts` `syncToolSurface` emits
`` ctx.ui.notify(`MCP: direct tools refreshed (+${added}, ~${updated}, -${deactivated})`, "info") `` when
the added+updated+deactivated count is non-zero **and** `ctx.hasUI`.
**behavior** — the only signal a user gets that the model's tool list changed mid-session; deliberately
gated on `hasUI` so a print-mode run stays silent.
**cyrup** — `HostServices::notify(message, NotifyKind::Info)`. Keep the `+n, ~n, -n` shape
byte-identical. Guard on the presence of an interactive surface exactly as upstream guards on
`ctx.hasUI`, or a headless run gains a toast it never had.
**verify** — a sync that adds 2 and removes 1 produces exactly `MCP: direct tools refreshed (+2, ~0, -1)`;
a sync with no changes produces none.

**MCP-218 — `syncProxyTool`'s registration/deactivation predicate** · medium · S · **hand-written**
**upstream** — `index.ts` `syncProxyTool`: the three-way OR (`disableProxyTool !== true` **or** no direct
specs **or** some configured direct-tool server has no valid cache entry) and the description-identity
short-circuit.
**behavior** — `disableProxyTool: true` only takes effect once the direct-tool surface is genuinely
complete; otherwise the user would lose all MCP access whenever the cache is cold.
**cyrup** — needs `getMissingConfiguredDirectToolServers` (MCP-219) and, for the mid-session half, the
MCP-217 seam. While MCP-217 is unbuilt this predicate still runs at init, which is where it matters most.
**verify** — `disableProxyTool: true` with a cold cache → the `mcp` tool is registered; warm cache with
direct specs → it is not.

**MCP-219 — `MCP_DIRECT_TOOLS`, the `__none__` sentinel and `parseDirectToolSelectors`** · medium · S · **hand-written**
**upstream** — `index.ts`'s env read (comma-split + trim + drop-empty; the literal `__none__` suppresses
all direct tools **and** is passed as `undefined` to `getMissingConfiguredDirectToolServers`) and
`metadata-cache.ts` `parseDirectToolSelectors` (`split("/", 2)` discards a third segment).
**behavior** — the env override subagents and CI use to pin a minimal MCP tool surface.
**cyrup** — `cyrup_ext_subagents::exec::mcp_direct_tools`'s `parse_selections` already ports the selector
parser correctly, including the two-segment truncation, but is private to that crate — reuse it through
MCP-205's shared module rather than duplicating. `std::env::var` read once at construction; cyrup forbids
`unsafe` env mutation, so tests inject the value (the `McpDirs` pattern in that same file is the
precedent).
**verify** — the env string `"a/b/c, ,d/"` → selectors `["a/b/c", "d/"]` → servers `{d}`, tools `{a: {b}}`.

**MCP-220 — `transformMcpContent` for every standard MCP content type** · high · M · **hand-written**
**upstream** — `tool-registrar.ts` `transformMcpContent`: the branch table in §11, including `audio`
becoming the text placeholder `[Audio content: <mime>]` and an unknown type becoming `JSON.stringify(c)`.
**behavior** — everything an MCP server returns except images reaches the model as text, in exactly these
shapes. `resource_link` is rendered as two lines and never fetched. The non-blob `resource` arm's
`(no content)` fallback fires only when `c.resource` itself is absent, never as a `JSON.stringify`
fallback.
**cyrup** — match on the content-block discriminant over `rmcp::model::ContentBlock`, but keep the
unknown-type arm re-serializing the *original* JSON: a lossy typed enum would destroy it. Target type is
`cyrup_core::Content::{Text, Image { data, mime_type }}` — pi's `TextContent | ImageContent`, which is
why the text collapse is mandatory. **This is the full standard content set; only `ui://` handling is
cut, and it never lived in this file.**
**verify** — one fixture per content type asserting the exact emitted text, plus an unknown type
`{"type":"video","x":1}` → `{"type":"video","x":1}` verbatim, plus a `resource` with no `resource` key →
`[Resource: (no URI)]\n(no content)`.

**MCP-221 — `transformMcpResourceContents`** · medium · S · **hand-written**
**upstream** — `tool-registrar.ts` `transformMcpResourceContents`: `text` wins, then `blob` materializes,
then the whole record is stringified.
**behavior** — what `mcp({tool: "read_<resource>"})` and the resource direct tools return.
**cyrup** — same walk over `rmcp::model::ResourceContents`; the fallback stringifies the **whole**
resource record including its `uri` and `mimeType`, not just an unknown field.
**verify** — a resource with neither `text` nor `blob` → the serialized record.

**MCP-222 — `resolveMcpResultContent` and the structured-content fallback** · high · S · **hand-written**
**upstream** — `tool-registrar.ts` `resolveMcpResultContent`: falls back to `structuredContent` only when
the transformed block list is **empty**, and only when `structuredContent` is neither `undefined` nor
`null`; formats it with `JSON.stringify(value, null, 2)`, with a `String(value)` catch.
**behavior** — modern MCP servers that answer with `structuredContent` and an empty `content` array still
return something the model can read. rmcp carries the field (`CallToolResult::structured_content`) but
does no client-side validation of it — that is MCP-247's sibling in the proxy section; here only the
rendering fallback matters.
**cyrup** — `serde_json::to_string_pretty` matches `JSON.stringify(v, null, 2)` for objects and arrays;
key order differs unless the parse preserved order, so the result reader must be order-preserving here
too.
**verify** — `{"content":[], "structuredContent":{"a":1}}` → one text block containing `{\n  "a": 1\n}`.

**MCP-223 — Binary-resource materialization with its four limits** · high · M · **hand-written**
**upstream** — `tool-registrar.ts` `materializeBinaryResource`: 10 MiB per resource, 100 MiB and 10 000
files per session, `mkdtempSync(tmpdir()/"pi-mcp-resource-")`,
`writeFileSync(..., {flag:"wx", mode:0o600})`, counters incremented before the write and rolled back on
failure **except** when the cleanup `rmSync` also fails.
**behavior** — an MCP server returning a 500 MB blob cannot exhaust RSS or the disk, and the file it does
write is not world-readable. The `wx` flag makes the write fail rather than clobber, which matters
because the temp directory is shared. **This is one of the two security-relevant units in the section:**
MCP resource blobs routinely contain API payloads and customer data, and a 0644 file in the shared temp
dir is readable by every local user.
**cyrup** — `tempfile` (workspace dep 3.27.0) gives
`Builder::new().prefix("pi-mcp-resource-").tempdir()` (0700); the per-file exclusive create is
`OpenOptions::new().write(true).create_new(true).mode(0o600)` behind `#[cfg(unix)]` with a non-mode
fallback elsewhere. The `WeakMap`-keyed per-scope session becomes a
`HashMap<ScopeId, MaterializedSession>` keyed by the owner cancellation token's id, since Rust has no
weak-keyed identity map for arbitrary values; the "aborted scope yields no session" rule becomes an
`is_cancelled()` check. Note `cyrup_tools::output` writes its spill via
`std::env::temp_dir().join(name)` + `File::create` with a `pid-nanos-counter` name and **no mode** —
predictable path, default umask, shared directory. Do not copy that posture here.
**verify** — an 11 MiB blob → the `decoded size exceeds 10 MiB` omission text; a successful write →
mode `0o600` asserted via `metadata().permissions().mode() & 0o777`, and the containing directory 0700.

**MCP-224 — The materialized-resource cleanup drain and retry** · medium · M · **hand-written**
**upstream** — `tool-registrar.ts`'s module-global pending set, per-directory attempt counters capped at
3, a single 30 s timer guarded by "already pending or nothing retryable", the timer-clear when the set
empties, and an `AggregateError("Failed to clean materialized MCP resources")` on any failure.
**behavior** — a directory that cannot be removed (a Windows lock, an NFS stale handle) is retried rather
than leaked silently, but at most three times, and never spins.
**cyrup** — the module-global state becomes an instance field on the extension so two sessions do not
share a retry budget; the timer becomes `tokio::spawn` + `tokio::time::sleep` guarded by an
`Option<JoinHandle>` so a second schedule is a no-op. The `AggregateError` maps to a custom error
carrying `Vec<std::io::Error>`.
**verify** — make a directory undeletable, assert three retry attempts then quiescence.

**MCP-225 — `resolveMcpOutputGuardOptions` and the `MCP_OUTPUT_GUARD` kill switch** · medium · S · **hand-written**
**upstream** — `mcp-output-guard.ts`: the three defaults, `positiveInt` (finite, floored, `> 0`), and
`envKillSwitch`'s tri-state where an unrecognised value falls back to the setting rather than forcing a
state.
**behavior** — `MCP_OUTPUT_GUARD=0` restores raw MCP output for debugging, including the raw
`details.mcpResult`.
**cyrup** — plain struct + `std::env::var`. `cyrup_tools::truncate` happens to define the same two
numbers (2000 lines / 50 KiB) for the built-in tools, but they are a different setting surface and must
not be aliased — the MCP guard's limits are user-tunable per `settings.outputGuard`.
**verify** — `MCP_OUTPUT_GUARD="maybe"` with `outputGuard: false` → disabled; `maxBytes: 0.5` → the
default, not 0.

**MCP-226 — `guardMcpOutput`'s normalize / affix / passthrough path** · high · M · **hand-written**
**upstream** — `mcp-output-guard.ts` `sanitizeContent` (image mime trimmed and **sliced to 100 chars**,
blank → `image/png`), `withEmptyTextFallback`, `addAffixes` (prefix on the *first* text block, suffix on
the *last*), and the `enabled === false` early return that also bypasses `details.mcpResult` bounding.
**behavior** — the prefix/suffix mechanism is how `Error: ` and the expected-parameters schema attach to
a failed call without a second content block, and image blocks reach the provider as native image
content rather than text.
**cyrup** — the 100-char mime slice is a UTF-16 code-unit slice in JS and is safe only because the string
is ASCII in practice; port it as a char-boundary-safe truncation to 100 chars so a hostile mime type
cannot panic.
**verify** — content `[image, text]` with a prefix → the prefix lands on the text block, not a new leading
block; content `[image]` only with a prefix → a new text block is unshifted.

**MCP-227 — The truncation arithmetic and notice format** · high · M · **hand-written**
**upstream** — `mcp-output-guard.ts` `textStats` (0 lines for empty text), `reserveBudget`,
`truncateHead` (emits a **partial line**), `truncateStringToBytes` (UTF-8 continuation-byte backoff),
`formatTruncationNotice`, `formatSize` (`" B"` / `" KiB"` / `" MiB"` with `toFixed(1)`).
**behavior** — the model sees the head of the output plus one bracketed notice naming the spill path and
telling it to use `read`/`grep`. Both the wording and the size format are asserted on upstream.
**cyrup** — `cyrup_tools::truncate` is a **reference, not a dependency**: its `truncate_head` documents
"Never returns a partial line", its report struct has different fields, and its `format_size` emits
`50.0KB` — no space, `KB` not `KiB`. Share the dual byte/line *model* and its `TruncOpts` shape if
convenient, but implement this arithmetic in `cyrup-mcp` to keep byte parity. Operate on `&[u8]` for the
byte accounting and `str::is_char_boundary` for the backoff (equivalent to and cheaper than the
`& 0xc0 == 0x80` walk). `toLocaleString()` on the line count must be reproduced: emit thousands
separators with `,` rather than pulling an i18n crate.
**verify** — a 60 KiB single-line input → output is one partial line plus the notice, and
`outputGuard.returnedBytes <= maxBytes`; golden test on the notice for 2 500 lines / 60 000 bytes →
`original 2,500 lines / 58.6 KiB.` (60000/1024 = 58.59375 → `58.6`).

**MCP-228 — `saveArtifact`'s private-directory spill** · high · S · **hand-written**
**upstream** — `mcp-output-guard.ts` `saveArtifact`: `mkdtemp(tmpdir()/"pi-mcp-output-")` then
`join(dir, \`${kind}-${randomBytes(4).toString("hex")}.txt\`)` written with `mode: 0o600`; every failure
is captured as `{error: message}` and surfaced in the notice rather than thrown. The directory prefix is
`pi-mcp-output-` for **both** kinds (`"output"` and `"mcp-result"`); only the filename differs.
**behavior** — the full MCP output — which routinely contains API responses, tokens echoed by a server
and customer data — lands in a per-invocation private directory, not a predictable path in the shared
temp dir. The second of the section's two security-relevant units.
**cyrup** — `tempfile::Builder::prefix("pi-mcp-output-").tempdir()` (0700) plus
`OpenOptions::create_new(true).mode(0o600)`; 4 random bytes hex for the filename, matching upstream,
though the `mkdtemp` already provides uniqueness. Do **not** let the `TempDir` guard drop: upstream's
directory outlives the call so the model can `read` the path.
**verify** — assert the containing directory's mode is `0o700` and the file's is `0o600`; assert a write
into a read-only tmpdir yields the `Full output could not be saved: …` notice rather than an error
result; assert the file still exists after the guard call returns.

**MCP-229 — `boundMcpResult` and the result-summary schema** · medium · M · **hand-written**
**upstream** — `mcp-output-guard.ts` `boundMcpResult` / `summarizeMcpResult` / `summarizeContent` /
`summarizeValue` / `estimateValueBytes` / `truncateKey`: the 16 KiB threshold, the exact `reason`
sentence, the three block shapes and the `{type:"omitted", count}` tail, the depth-2 / 20-entry bound,
and the 120-char key cap with a trailing `…`.
**behavior** — bounds what the **session file** accumulates. `details` is not sent to the model (cyrup's
`ToolResult` says the same), so this is a disk/replay-cost control, not a context control — but an
unbounded `mcpResult` makes a session file unloadable.
**cyrup** — `ToolResult::details` is `Option<serde_json::Value>`, so the summary is just a `Value`.
`boundMcpResult` returns the raw value **by reference** under the threshold, so no clone on the common
path. `estimateValueBytes` counts a number's `String()` length, not 8 bytes — reproduce with
`Number::to_string().len()`. `structuredContent`/`meta` are emitted on key *presence* (`in`), so use
`Map::contains_key`, not `get(..).is_some()`.
**verify** — a 20 KiB result → the summary; assert `contentSummary` has exactly 21 entries for 25 content
blocks and that the 21st is `{"type":"omitted","count":5}`.

**MCP-230 — Record the output guard's actual security contract** · medium · S · **hand-written**
**upstream** — `mcp-output-guard.ts`, read in full.
**behavior** — the guard performs **no** prompt-injection detection, **no** secret or credential
redaction and **no** content classification. It caps bytes and lines, spills the remainder to a 0600
file and bounds `details.mcpResult`. The only content-shaping it does is the image-mime clamp.
**cyrup** — port the posture as-is and document it in the crate's module docs. The relevant cyrup fact:
the permission gate runs at `EventKind::ToolCall` — **before** the call, on the arguments — and never
inspects the result, so MCP tool **output** is unfiltered text entering the model's context under either
system. A result-side hook would be a new `EventKind` plus a new `EventPatch` arm and has no upstream
counterpart; that is a cyrup product decision, not a port unit, and this document does not file it as
one.
**verify** — n/a (a documentation unit); the assertion is that the crate docs state the contract in these
terms.

**MCP-231 — `isToolCallApprovalRequired`** · high · M · **hand-written**
**upstream** — `tool-approval.ts` `isToolCallApprovalRequired`: server-level `approveTools` overrides the
global setting; `true` always requires; the legacy-alias arms for server scope and global scope,
including the explicit injection of the first non-bare current candidate with `-`→`_`.
**behavior** — which MCP tools prompt before running. A false negative here is a silent bypass of a
user's approval rule.
**cyrup** — reuse MCP-201/MCP-202's candidate and pattern machinery. The two scopes differ **only** in how
`otherCurrentCandidates` is built (this server's metadata vs every server's under its own prefix), so
factor that as a parameter rather than duplicating the block as upstream does. The server arm falls back
to the *full legacy* set when no `toolMetadata` was supplied while the global arm returns `false` — that
asymmetry is real and must be preserved.
**verify** — global `approveTools: ["foo_bar"]` with servers `foo` and `foo-mcp` in `short` mode asserting
the ambiguous legacy alias does **not** gate the wrong tool; plus the `toolMetadata == None` asymmetry
between the two scopes.

**MCP-232 — `ensureToolCallApproved` and the approval dialog** · **critical** · M · **host-verb**
**upstream** — `tool-approval.ts` `ensureToolCallApproved`: NUL-separated session cache key, the exact
dialog title/body/options, the 500-char argument preview, and the fail-closed default where any
non-`"Allow …"` answer denies.
**behavior** — the user's last line of defence before an MCP tool runs with model-chosen arguments. Two
distinct failure modes make this `critical`: a dialog whose cancellation is read as approval is a
permission bypass, and a headless run that silently proceeds instead of returning
`approval_required_headless` is the same bypass without a human in the room.
**cyrup** — `HostServices::select(prompt, options, opts) -> Option<String>` is an exact fit, and it
returns `None` when there is no interactive surface — which maps onto **both** the cancelled dialog
(deny) and the headless refusal, so the port must check for a UI *before* calling, exactly as upstream
does, or the two collapse into one. Run the dialog under `HostServices::human_interaction_lock` (the one
session-scoped `HumanInteractionLock`) so a permission prompt and an MCP approval can never both be on
screen, and under `HostCtx::begin_human_wait` so the dispatcher's invocation-budget watchdog is suspended
and the gate cannot fail **open** on a timeout. A `HashSet<(String, String)>` keyed on
`(server, original_tool)` is cleaner than the NUL-joined string and is not observable; keep it
per-session, cleared on `EventKind::SessionShutdown`. Argument sanitisation is MCP-235.
**verify** — cyrup-it (live pty): a gated tool, answer `Deny` → `details.error == "approval_denied"`;
answer `Allow for session`, call again → no second prompt. Unit: no UI → `approval_required_headless`,
and a cancelled dialog with a UI → `denied` (the two must not be confused).

**MCP-233 — Drop the cross-extension approval broker; `before_tool_call` is the broker** · medium · S · **host-verb**
**upstream** — `tool-approval.ts` `requestBrokerApproval` and `types.ts`'s approval event types: a
**synchronous** `EventEmitter.emit` of `pi-mcp-adapter:tool-approval-request` carrying a `claim(handler)`
closure accepted only while the emit is on the stack, followed by awaiting the claimed handler's
four-valued decision.
**behavior** — upstream lets a permission extension take ownership of MCP approval and answer
`allow_once` / `allow_for_session` / `deny` / `abstain`, with the adapter's own `approveTools` gate
running only on `abstain`.
**cyrup** — the mechanism is not expressible and is not needed. Not expressible: a native can
`subscribe_bus` and receives `on_bus_event` returning `Result<(), ExtError>` — no return channel — and
cyrup's bus is *deferred* (`drain_bus` / `deliver_bus_events`) where pi's is synchronous, so neither the
claim handshake nor the decision can travel over it. Not needed: `ExtHooks::before_tool_call` **is** the
broker, structurally — it dispatches `HostEvent::ToolCall` block/mutate, `EventKind::ToolCall` is the
one kind whose `fails_closed()` is `true` (a handler that traps, panics or blows its budget **denies**),
and `cyrup-permission-system` already subscribes it and already derives MCP targets from the `mcp`
tool's arguments. Every surviving origin (`proxy` = the `mcp` tool, `direct` and `resource` = registered
direct tools) flows through it identically. **Keep** `ensureToolCallApproved`'s local gate — the
`approveTools` config, the session cache, the three-way select — and drop the broker emit. Record the
delta: cyrup's gate has no `abstain` (a permission extension that declines to decide simply does not
block, which lands in the same place) and no `allow_for_session` at the host level (the adapter's own
session cache covers it for MCP).
**verify** — cyrup-it: a permission rule denying an MCP target blocks the call before `execute` runs; with
no rule, the adapter's own `approveTools` gate still prompts.

**MCP-234 — Direct MCP tools do not reach the `mcp` permission category** · high · M · **open-decision**
**upstream** — each direct tool is registered under its own prefixed name, so pi's own permission
extension sees a plain tool name, not `mcp`. The split is upstream's, not cyrup's.
**behavior** — a user who writes an `mcp` policy expects it to cover MCP tool calls. With direct tools
enabled it does not — on either side.
**cyrup** — confirmed by reading `cyrup_permission_system::manager`'s `check_permission`: the `mcp` arm
fires only when the normalized tool name is exactly `"mcp"`, producing `CheckSource::Mcp`, the five-key
target decomposition, the `MCP_BASELINE_TARGETS` auto-allow and `DefaultCategory::Mcp`. A direct tool
named `github_create_issue` skips that arm, skips the built-in arm, and lands in the arbitrary-extension-tools
arm: with a matching `tools` rule → `CheckSource::Tool` with `target: None`; with none →
`CheckSource::Default` with `DefaultCategory::Tools`. So the call **is** gated, but under the `tools`
category, and the MCP-specific denial/ask copy in `gate.rs` never appears for it. Options: **(a)** teach
`check_permission` to recognise an MCP-owned tool name — needs MCP-204 plus the configured prefixes,
which the permission system already reads `mcp.json` for (`read_configured_mcp_server_names`), so it is
tractable and is the only option that makes `mcp` policies mean what users think; **(b)** stop
registering direct tools — a parity loss; **(c)** document the split and match upstream exactly.
**Recommend (c) for the port, with (a) filed as a cyrup product follow-up** — and state plainly in the
permission system's docs that `defaultPolicy.mcp = "deny"` does **not** deny direct MCP tool calls; they
fall under `tools`.
**verify** — cyrup-it: with `defaultPolicy.mcp = "deny"` and a direct tool registered, assert the call's
`CheckSource` and whether it is blocked. This test **documents** the behaviour whichever option is taken.

**MCP-235 — `sanitizeTerminalText` / `stripOscSequences`** · high · S · **hand-written**
**upstream** — `utils.ts`: an OSC scanner handling both `ESC ]` and C1 `0x9D` introducers, terminated by
`BEL` / `0x9C` / `ESC \` and tolerating an unterminated payload; then the ANSI regex
`/(?:\x1b\[[0-?]*[ -\/]*[@-~]|\x1b[@-Z\\-_])/g` → `""`; then `/[\u0000-\u001f\u007f-\u009f]+/g` → `" "`;
then `/\s+/g` → `" "`; then `trim()`.
**behavior** — a malicious MCP server cannot use its tool name, server name or argument values to paint
over the approval dialog or the status line.
**cyrup** — port the whole function into `cyrup-mcp`'s own util module. `cyrup-session-svc`'s
`strip_ansi` covers OSC + CSI stripping with adversarial-input tests but is private to that crate and
stops before the control-char-to-space and whitespace-collapse tail; **do not depend on
`cyrup-session-svc` from an extension crate**, and do not file a shared-text-crate prerequisite for
40 lines of pure string work. The unterminated-OSC tolerance is load-bearing: a bare `ESC ]` with no
terminator must consume to end-of-string. Write the C0/C1 class as an explicit range, not copied literal
bytes.
**verify** — adversarial fixtures — an unterminated OSC, a C1 `0x9D` introducer, an embedded `\r`
cursor-return and a DEL — asserting a single-space-collapsed printable result and that the function never
panics on arbitrary bytes.

**MCP-236 — Give the `mcp` tool its prompt guideline** · medium · S · **hand-written**
**upstream** — the guideline text is a cyrup-side contract, not an adapter one; the adapter's own
equivalent is the proxy tool's `promptSnippet`
(`MCP gateway — status, search, describe, auth, and single MCP tool calls`).
**behavior** — cyrup's system-prompt sanitizer keeps the guideline
`use mcp for mcp discovery first: search by capability, describe one exact tool name, then call it.`
**only** when a tool named `mcp` is in the allowed set (`cyrup_permission_system::sanitize::tools`). The
match runs on normalized text (`split_whitespace().join(" ").to_lowercase()`), so the produced text must
be whitespace-normalizable to that exact lowercase literal or the sanitizer silently drops it and the
model loses the discovery instruction. A tree-wide grep finds exactly one occurrence of the literal —
the matcher. There is no producer today.
**cyrup** — a `const &'static str` on the tool struct returned from `Tool::prompt_guidelines()`; keep it
lowercase verbatim.
**verify** — assert `prompt_guidelines()` contains the literal, and a cyrup-it test that the rendered
system prompt contains it when `mcp` is allowed and not when it is denied.

**MCP-237 — The call-row formatters** · medium · S · **hand-written**
**upstream** — `tool-result-renderer.ts` `truncateText`, `formatJsonish` (a string is re-pretty-printed
when it parses as JSON), `hasUsefulObjectContent`, `formatMcpProxyToolCallLines`,
`formatMcpDirectToolCallLines`.
**behavior** — the one-line summary of an MCP call in the transcript.
**cyrup** — `NativeExtension::render_call(key, call)` is the extension-wide seam (routed by
`ExtensionHost::render_tool_call`); `cyrup_core::Tool::render_call(&Value) -> Option<String>` is the
per-tool one. Emit a bare string or `{"widget":"text"}`. `truncateText` slices by UTF-16 code units in
JS; port as a char-boundary-safe slice to `max−1` chars plus `…`. **The `action === "ui-messages"` branch
is cut** (Cut 2) — it is the file's only `ui` reference; the other seven branches are unchanged.
**verify** — golden lines for each of the seven surviving branches.

**MCP-238 — `resolveMcpToolRenderOptions` and the `renderShell` selection** · low · S · **host-verb**
**upstream** — `tool-result-renderer.ts` `resolveMcpToolRenderOptions` and `index.ts`'s
`toolRenderShell`: `toolResultRendering` defaults to `compact`, `collapsedResultLines` accepts only 1/2/3
with a mode-dependent default, and `renderShell` is `"self"` in compact mode.
**behavior** — the two visual modes of every MCP tool row.
**cyrup** — `Tool::render_kind()` returning `ToolRenderKind::SelfRendered` is `renderShell: "self"`;
`ToolRenderKind::Default` is `"default"`. A plain settings struct otherwise.
**verify** — `collapsedResultLines: 7` → the mode default, not 7.

**MCP-239 — `collectCollapsedResultLines` / `formatMcpToolResultLines` / `blockToLines`** · medium · M · **hand-written**
**upstream** — `tool-result-renderer.ts`: the 8000-char budget, the per-line accounting
(`line.length + 1`), the mid-line slice when a single line exceeds the remaining budget, the `""` line
for an empty result, and the trailing `"…"` appended only when truncated **and** at the line cap.
**behavior** — the collapsed transcript row for an MCP result.
**cyrup** — the char budget is counted in JS `String.length`, i.e. UTF-16 units; port as
`encode_utf16().count()` for exactness, matching the in-tree precedent in `cyrup_tools::truncate`.
**verify** — 5 blocks of 3 lines with `maxLines = 3` → exactly 4 output lines, the last being `"…"`.

**MCP-240 — `formatMcpToolResultIdentity`** · low · S · **hand-written**
**upstream** — `tool-result-renderer.ts` `formatMcpToolResultIdentity`: `null` unless
`details.mode === "call"`; `server` then `hintServer`; then `tool`, `resourceUri`, `requestedTool` in
that order.
**behavior** — the `MCP <server>/<tool>` muted line above a boxed result.
**cyrup** — trivial `Value` reads; note `details.mode` is set only by the proxy's `executeCall`, so direct
tools never render an identity line (their `details` carry `server`/`tool` but no `mode`).
**verify** — the three positive branches and the `mode !== "call"` null.

**MCP-241 — The compact result row without a render width** · low · M · **hand-written**
**upstream** — `tool-result-renderer.ts` `CompactMcpToolResult.render(width)`: computes
`visibleWidth(body) > safeWidth`, then chooses between a 21-char `" … (Ctrl+O to expand)"`, a 9-char
`" (Ctrl+O)"` and a bare truncated hint depending on `safeWidth`, memoizing per width.
**behavior** — the compact single-row MCP result and the affordance telling the user the row is
expandable.
**cyrup** — the renderer contract passes `(key, payload)` and flattens the returned widget tree to a
plain `String`; **no width crosses it.** Emit `{"widget":"truncated-text","text":…}` and let the host
truncate. Accepted delta, stated once: the port loses the width-dependent *choice* of affordance string,
not the row. This is not a host prerequisite — upstream itself degrades to `plainTheme` and a fixed
suffix when its theme/width inputs are absent, and an in-tree extension (`cyrup-ext-subagents`) already
ships the same concession. If cyrup later widens the renderer contract for its own reasons, this unit
gains fidelity for free; nothing in the port waits on it.
**verify** — the compact row renders as a single line whose text matches the collapsed budget, and a
result longer than the budget carries the `truncated-text` widget rather than a pre-sliced string.

**MCP-242 — Expanded rendering without a per-row expansion flag** · low · S · **host-verb**
**upstream** — `tool-result-renderer.ts`: `expanded = options.expanded || context.isError || Boolean(details.error)`,
and `CollapsibleText`'s collapsed footer.
**behavior** — Ctrl+O expands an MCP result in place; an errored result is always expanded.
**cyrup** — two of the three inputs are already reachable: `details.error` is in the payload the renderer
receives, and `HostServices::tools_expanded()` reports the global expand toggle — a native extension
holds the `Arc<dyn HostServices>` stashed by `set_host_services`, so `render_result` can read it. Only
the *per-row* flag is absent, and cyrup's expansion model is global rather than per row, so there is no
per-row flag to pass. Implement `expanded = tools_expanded() || details.error.is_some()`; the resulting
behaviour is upstream's minus per-row granularity.
**verify** — a result carrying `details.error` renders the expanded form regardless of the toggle; with
the toggle on, a clean result renders expanded too.

**MCP-243 — The compact call-row suppression has no cyrup equivalent** · low · S · **hand-written**
**upstream** — `tool-result-renderer.ts`: in compact mode, on a settled non-error non-expanded row,
`renderCall` returns an `EmptyComponent` after stashing `lines[0]` into `context.state.compactTitle`,
which the result row re-emits as its own `title → ` prefix.
**behavior** — compact mode shows **one** line per MCP call instead of two.
**cyrup** — `render_call` and `render_result` are separate, stateless calls with no shared per-row context
and no call id, and `Tool::render_call` returning `None` means "use the default framing", not "draw
nothing". **Recommendation: drop the stash entirely.** `render_call` always returns the call line and
`render_result` returns the collapsed body — i.e. cyrup draws the two-row shape for both `compact` and
`boxed` settings, differing only in the collapsed line budget. That is a smaller, honest divergence than
either faking a per-row cache keyed on an id the host does not pass, or setting `details.mode = "call"`
on direct-tool results to make `formatMcpToolResultIdentity` produce a title (which would mutate a
details schema MCP-249 freezes).
**verify** — a direct-tool call in compact mode renders a call row with the display name and a result row
with the collapsed body; neither is empty.

**MCP-244 — The renderer contract carries no theme** · low · S · **hand-written**
**upstream** — `tool-result-renderer.ts` wraps every emitted line in `theme.fg(name, text)` with the
palette entries `toolTitle`, `toolOutput`, `muted`, `warning`, and optionally `theme.bold`. `plainTheme`
is the no-colour degradation and is upstream's own fallback.
**behavior** — MCP rows are coloured consistently with the rest of the transcript.
**cyrup** — `HostServices::theme()` returns the active theme name and `theme_by_name()` its full
definition, so the palette *is* reachable — but the widget vocabulary the host flattens has no
styled-span node, so there is nothing to emit colour into. Ship uncoloured rows, which is upstream's own
`plainTheme` path, not a fabrication. A `{"widget":"styled","fg":…}` node would be a nice additive
`cyrup-tui` change for every extension; it is not a port prerequisite and this document does not file it
as one.
**verify** — rows render legibly with no escape sequences in the flattened string.

**MCP-245 — Width-aware truncation is not needed** · low · S · **extension-owned**
**upstream** — `tool-result-renderer.ts` imports `truncateToWidth` / `visibleWidth` from pi-tui and calls
`truncateToWidth(body, width, "…")`.
**behavior** — grapheme- and escape-aware truncation so a row never splits an emoji or leaves a dangling
escape.
**cyrup** — with no width crossing the renderer seam (MCP-241), there is nothing to truncate *to width*;
the host does its own truncation on `truncated-text`. The char-budget truncation the port does need
(MCP-239) is UTF-16-count-based and in-crate. `cyrup-intercom` has faithful `visible_width` /
`truncate_to_width` ports, but they live in an unrelated extension crate and depending on them would be
a layering error — and promoting them to a shared crate buys nothing this section needs. **Dissolved:
the first pass filed this as a `cyrup-core::text` prerequisite; there is no prerequisite.**
**verify** — n/a (nothing to build); MCP-239's budget test covers the residual behaviour.

**MCP-246 — Route the five collision/advisory warnings** · low · S · **extension-owned**
**upstream** — `direct-tools.ts`'s five `console.warn` sites: two tool, two resource, one advisory at
≥75 specs.
**behavior** — the only signal a user gets that a configured MCP tool was silently dropped.
**cyrup** — `tracing::warn!` for all five, byte-identical messages (upstream's `console.warn` goes to the
log, not the transcript). Do **not** promote them to `notify` — 75 direct tools would produce a toast
storm. `tracing` is not a workspace-table dependency: four crates pin it individually at the same
version and `cyrup-mcp` declares its own to match.
**verify** — capture the tracing subscriber and assert the exact five message strings.

**MCP-247 — The `mcp` proxy tool's parameter schema** · high · S · **hand-written**
**upstream** — `index.ts` `registerProxyTool`: the registration shape and twelve optional properties with
their exact descriptions, `args` as a union of a JSON string and an open object, `limit` minimum 1,
`offset` minimum 0.
**behavior** — the shape `cyrup_permission_system::manager`'s `create_mcp_permission_targets` reads:
`tool`, `server`, `connect`, `describe`, `search`, in that precedence, with `mcp_status` as the
fallthrough. **Renaming or omitting any of those five silently disables the corresponding permission
targets.** The `action` description loses its `'ui-messages'` mention (Cut 2); the property itself stays,
since `auth-start` / `auth-complete` still use it.
**cyrup** — `Tool::parameters` is raw JSON Schema, so upstream's TypeBox shim (`optionalNumber` and
friends) disappears — a `serde_json::json!` literal in a `OnceLock`, owned by the tool struct because the
trait returns `&Value`.
**verify** — assert the five permission-relevant property names exist; cyrup-it:
`mcp({tool:"x", server:"s"})` under a policy denying `s:x` is blocked with `CheckSource::Mcp`.

**MCP-248 — Tracker: tool registration, approval, guard and rendering** · n/a · S · **hand-written**
**upstream** — `{direct-tools, tool-result-renderer, mcp-output-guard, tool-metadata, tool-registrar,
tool-approval, resource-tools}.ts`.
**behavior** — indexes MCP-200..MCP-249 including MCP-214a, MCP-217a, MCP-217b. Critical path to a first
callable MCP tool, none of which waits on a host change:
MCP-200 → MCP-201 → MCP-202 → MCP-203 → MCP-207 → MCP-212 → MCP-216 → MCP-220 → MCP-222 → MCP-226 →
MCP-214. **MCP-217 (HA-1) is not on that path** — with the cache warm, the init-time
`InitApi::register_tool` surface is the same surface upstream uses; HA-1 buys mid-session refresh.
Two decisions to settle early because they change other people's code: **MCP-205** (naming
reconciliation, touches `cyrup-ext-subagents`) and **MCP-234** (permission category, touches
`cyrup-permission-system` docs at minimum).
**verify** — n/a.

**MCP-249 — Freeze the `details` schema this subsystem emits** · high · S · **hand-written**
**upstream** — `direct-tools.ts`'s complete set of `details.error` codes: `init_failed`,
`not_initialized`, `server_disabled`, `auth_required`, `server_unavailable`, `not_connected`,
`approval_denied`, `approval_required`, `tool_error`, `url_elicitation_required`, `aborted`,
`call_failed` — twelve; plus the non-error keys `server`, `tool`, `resourceUri`, `message`,
`autoAuthAttempted`, `action`, and the guard's `mcpResult` / `outputGuard`. The `uiOpen` / `uiViewer` /
`uiUrl` keys are cut with branch 10c (Cut 2).
**behavior** — three consumers depend on these exact strings: `error-signal.ts`'s `toolErrorOverride`
re-flags **only** `tool_error` and `call_failed` as `{isError:true}` (its own doc says the others "are
not failed tool calls, so they get no override"); the renderer forces expanded rendering when any
`details.error` is truthy; and the proxy's `details.mode === "call"` drives the identity line. A wrong
code silently changes whether the model treats a result as a failure and therefore whether it retries —
the section's clearest silent-wrong-output risk after MCP-212.
**cyrup** — define the codes as a Rust enum with `#[serde(rename_all = "snake_case")]` so they cannot
drift, rather than as string literals at fifteen call sites. The `{isError:true}` override maps to
`EventPatch::ToolResult { is_error: Some(true), content: None, details: None, usage: None }` from the
`tool_result` hook, which cyrup merges field-by-field.
**verify** — an exhaustive match over the enum asserting each serialized form; cyrup-it: assert
`is_error` is set for exactly `tool_error` and `call_failed`.

### Out of scope

Four scope decisions from the project owner touch this section. They are **decisions**, recorded with
their reasons so a later pass does not re-file them as gaps.

**MCP Apps / the UI extension (Cut 2)** — cut entirely; there is no app surface, no local HTTP host
server, no iframe bridge, no `ui://` resource path. In this section that removes:

* `getToolUiResourceUri` and `extractToolUiStreamMode`, the `DirectToolSpec.uiResourceUri` /
  `.uiStreamMode` fields, and the `failedTools` arm that recorded a malformed `ui://` value (MCP-209).
  *Reason:* they exist only to decide whether a tool opens a UI session.
* The UI-session interleave inside `createDirectToolExecutor` — `maybeStartUiSession`,
  `summarizeUiSessionResult`, `sendToolResult` / `sendToolCancelled`, the `_meta: requestMeta`
  injection, the `reused` close, and result branch 10c with its `uiOpen` / `uiViewer` / `uiUrl` details
  keys. **The rest of the executor is unchanged and its ordering is specified in full in §7.**
* `isUiToolCallableByApp`. *Reason:* no caller without apps. **Its sibling
  `extractUiToolVisibility` / `isUiToolVisibleToModel` is KEPT** (MCP-208): cutting it would expose to
  the model tools the server explicitly marked app-only, which is a behaviour change in the wrong
  direction, and it costs four lines.
* The `mcp({ action: "ui-messages" })` usage line in the proxy description, the `'ui-messages'` mention
  in the `action` property's description, and the `action === "ui-messages"` branch of
  `formatMcpProxyToolCallLines` — the only `ui` reference in `tool-result-renderer.ts`, which otherwise
  survives whole. `tool-result-renderer.ts` contains **no** `ui://` code; that lived in
  `ui-resource-handler.ts`.
* `McpToolApprovalOrigin`'s `"iframe"` variant.
* **Kept, explicitly:** every standard MCP content type in `transformMcpContent` — text, image, audio,
  resource, resource_link — plus the `structuredContent` fallback. The cut removes `ui://` handling,
  nothing else.
* **On-disk consequence:** the cache's UI fields stay in the schema, absent and ignored.
  `CACHE_VERSION` is **not** renumbered — `cyrup-ext-subagents` reads that file and the schema is a
  contract.

**`mcpScript` / the JavaScript worker (Cut 4)** — cut. In this section that removes:

* The `mcpScript` sentence from the proxy tool's description header (MCP-213), which otherwise would
  advertise a tool that does not exist. *Reason:* the remaining proxy modes cover the same ground —
  `mcp({search})` → `mcp({describe})` → `mcp({tool, args})` is the same discover/inspect/call loop, one
  call per turn instead of batched.
* `McpToolApprovalOrigin`'s `"script"` variant and its call site; the `origin` parameter and its
  `"proxy"` default stay.
* **Consequence to propagate:** there is no JS engine anywhere in this section, and `node` is not a
  dependency of anything it describes.

**The legacy HTTP+SSE transport (Cut 1)** and **the raw unix-socket transport (Cut 3)** do not appear in
this section — every unit here is transport-agnostic — but two second-order effects land nearby: the
`traceTransportKind` variants for those transports disappear (the transport section owns that), and a
config carrying `httpTransport: "sse"` or `socket` produces a named load-time diagnostic rather than a
silent skip, so a server that *would* have exposed direct tools is reported rather than quietly
missing from the cache.

**Also out of scope by mandate, not by cut:** roots, MCP logging, MCP completions and resource
subscriptions. rmcp ships all four; **the adapter implements none of them** (a `grep` over the package
finds zero occurrences), so wiring them would be new functionality outside the 1:1 mandate.

### What does not fit cleanly

**One host addition survives here, and it is a handle, not a capability.** `HA-1` / MCP-217: a native
extension cannot reach `ExtensionHost::register_late_tool`. The registration machinery is complete and
propagates to a live agent every turn; what is missing is that a native's only host handles are
`Arc<dyn HostServices>` and `HostCtx`, neither of which exposes the ext host, while the WASM tier
reaches the same thing through its `registration` WIT import. Two shapes are acceptable and both are
small (§ MCP-217); take `register_late_command` with it for MCP prompt slash-commands. **It is not on
the critical path** — with a warm cache the init-time registration surface is exactly what upstream
uses — and its absence degrades gracefully to "new tools appear next session". Rated `high` for
functional weight, not `critical`: no data loss, no wrong output, no permission bypass, no crash.

**Two decisions are genuinely open and both touch someone else's crate.**

* **MCP-205 — which naming rules win.** `cyrup-mcp` writes names; `cyrup-ext-subagents` reads
  selectors and expects them. The two upstreams have drifted from each other in six ways, verified on
  both sides. Recommend option (c): a shared naming module both crates depend on, plus upgrading the
  consumer in the same change. Options (a) and (b) are both defensible; doing nothing is not, because
  the failure is silent.
* **MCP-234 — whether the `mcp` permission category covers direct MCP tools.** It does not, on either
  side, and cyrup's gate routes them through `DefaultCategory::Tools` instead. Recommend porting the
  upstream split as-is and documenting it, with "teach `check_permission` to recognise MCP-owned names
  (needs MCP-204)" filed as a cyrup product follow-up rather than a port unit.

**Four accepted deltas, recorded so nobody re-files them as gaps.** (i) The renderer seam carries no
width, no styled-span node and no per-row expansion flag; the port emits `truncated-text`, draws
upstream's own `plainTheme` path, and reads `HostServices::tools_expanded()` plus `details.error` for
expansion (MCP-241, MCP-242, MCP-244). An in-tree extension already ships the same concession.
(ii) The compact one-row collapse is not expressible without a shared per-row context; the port draws
two rows in both modes (MCP-243). (iii) Tool *removal* lands on upstream's own `setActiveTools`
fallback branch because `ExtensionRegistry` has no `unregister_tool` — a supported upstream
configuration, with the consequence that a removed tool's name remains registered for the session.
(iv) The cross-extension approval broker does not port; `before_tool_call` + `cyrup-permission-system`
is the same gate, already wired and fail-closed, minus `abstain` and a host-level
`allow_for_session` (MCP-233).

### Coverage

**Read** —
*Upstream at v2.25.0, in full:* `direct-tools.ts`, `tool-result-renderer.ts`, `mcp-output-guard.ts`,
`tool-metadata.ts`, `tool-registrar.ts`, `tool-approval.ts`, `error-signal.ts`, `resource-tools.ts`,
`ui-tool-visibility.ts`, `ui-app-bridge-helpers.ts`.
*Upstream at v2.25.0, targeted regions:* `types.ts` (settings, `ServerEntry`, approval event types,
`ToolMetadata` / `DirectToolSpec` / `CachedTool`, and the whole name-formatting and pattern-matching
block from `sanitizeServerPrefix` through `isToolAllowed`), `index.ts` (registration, `syncDirectTools`,
`deactivateTools`, freezing, `syncProxyTool`, `registerProxyTool` and its `execute` preamble),
`utils.ts` (`stripOscSequences`, `sanitizeTerminalText`, `truncateAtWord`,
`normalizeDirectToolInputSchema`, `formatAuthRequiredMessage`, `extractToolUiStreamMode`),
`metadata-cache.ts` (`parseDirectToolSelectors`, `getMissingConfiguredDirectToolServers`).
*Cross-repo:* `pi-subagents/src/runs/shared/mcp-direct-tool-allowlist.ts` — read this pass (not carried
forward on trust) to establish that the in-tree Rust naming divergence is a faithful port of a drifted
upstream.
*cyrup on branch `david/cyrup`, by symbol:* `cyrup_core::{Tool, ToolResult, ToolRenderKind, Content}`;
`cyrup_ext::{InitApi::{register_tool, register_command, register_tool_renderer},
NativeExtension::{set_host_services, render_call, render_result}, HostCtx::begin_human_wait,
ExtensionHost::{register_late_tool, refresh_tools, active_tools, render_tool_call, render_tool_result},
ExtensionRegistry::active_tools, ExtHooks::before_tool_call, EventKind::fails_closed, EventPatch::ToolResult}`;
`cyrup_ext::host::services::HostServices` (every `fn` enumerated; `select`, `notify`,
`human_interaction_lock`, `active_tools`, `set_active_tools`, `theme`, `theme_by_name`,
`tools_expanded` read in full); `cyrup_session_svc::{AgentSession::refresh_extension_tools,
SessionFactory::with_native_extension}`; `cyrup_tui`'s `run_renderer` / `rendered_text` /
`flatten_widget` and the documented widget vocabulary; `cyrup_permission_system::{manager::{check_permission,
create_mcp_permission_targets, MCP_BASELINE_TARGETS, parse_qualified_mcp_tool_name,
add_derived_mcp_server_targets}, sanitize::tools, ordered::OrderedValue, jsonc}`;
`cyrup_ext_subagents::exec::mcp_direct_tools` (`ServerEntry`, `CachedTool`, `resolve_direct_tool_names`,
`parse_selections`, `get_server_prefix`, `strip_mcp_suffix`, `format_tool_name`, `is_tool_excluded`,
`resource_name_to_tool_name`, `BUILTIN_TOOL_NAMES`) and its `extension.rs` `render_result` doc;
`cyrup_tools::{truncate::{truncate_head, format_size, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES},
output, registry::BUILTIN_NAMES}`; `cyrup_intercom::ui::{visible_width, truncate_to_width}`; the
workspace `Cargo.toml` dependency table.

**Excluded** —
- `proxy-modes.ts` — the mode bodies behind the `mcp` tool. This section covers how the tool is
  *registered*, what its description and schema are, and how its result is guarded and rendered.
- `mcp-code.ts`, `mcp-script-worker.mjs`, `skills/mcp-scripting` — **Cut 4**; nothing to read.
- `ui-session.ts`, `ui-server.ts`, `ui-resource-handler.ts`, `host-html-template.ts`,
  `ui-stream-types.ts`, `app-bridge.bundle.js`, `glimpse-ui.ts` — **Cut 2**; only the four call sites
  inside `direct-tools.ts` were read, to locate the seam precisely.
- `mcp-auth-flow.ts`, `mcp-auth.ts`, `mcp-oauth-provider.ts`, `session-recovery.ts` — read only their
  entry points as called from `direct-tools.ts` (`authenticate`, `supportsOAuth`, `withSessionRecovery`,
  `SessionRecoveryAuthRequiredError`); the OAuth subsystem is its own section.
- `init.ts`, `state.ts`, `lifecycle.ts`, `runtime-owner.ts`, `abort.ts` — lifecycle and connection
  management, consumed by the executor but owned elsewhere; only the call contracts appear here
  (MCP-214, MCP-214a).
- `metadata-cache.ts`'s cache read/write, `computeServerHash` and `isServerCacheValid` — the cache
  section owns the format and the hash; only the selector parser and the call contract are here.
- `search-ranking.ts`, `ts-shape.ts`, `json-schema-validator.ts` — consumed by the proxy's
  search/describe, not by any file in this section.
- `mcp-panel.ts`, `mcp-setup-panel.ts`, `panel-keys.ts` — the panels consume `ToolMetadata` but do not
  produce it.
- `registerPromptCommands` / `syncPromptCommands` — only the *name* function is here (MCP-206).
- `applyDirectToolConfigChanges` — the panel's write path into `definition.directTools`; this section
  owns `resolveDirectTools`'s *read* of the field. Recorded rather than dropped because it is the only
  mutation of the server map outside config load.
- The upstream `__tests__` — the natural source of golden fixtures for the `verify` lines above, and
  they should be ported as Rust tests, but a test file is not a port unit.
- `crates/cyrup-ext-sdk` — the WASM guest SDK. A native built-in never crosses the component boundary.

**Corrections to the first pass** —
- *"There is no way for a native extension to register a tool after init" (rated `critical`).* Half
  wrong. `ExtensionHost::register_late_tool` + `refresh_tools` exist and propagate to a live agent
  through `AgentSession::{refresh_extension_tools, next_turn_tools}`. Only the **handle** is missing.
  Reframed as MCP-217/HA-1 at `high`, off the critical path.
- *"The renderer contract must be widened (width + expansion + partial + error) and given a styled-span
  node" — filed as a `cyrup-ext` + `cyrup-tui` prerequisite blocking four items.* Dissolved.
  `HostServices::tools_expanded()` serves expansion, `details.error` serves the forced-expand case, and
  `theme()` / `theme_by_name()` serve the palette; the remaining width and colour losses are upstream's
  own `plainTheme` degradation and are already shipped by an in-tree extension. MCP-241/242/244 are
  accepted deltas, not blockers.
- *"`cyrup-core::text` must gain public `visible_width` / `truncate_to_width` / `strip_ansi_sequences`
  / `grapheme_clusters`" — filed as a prerequisite.* Dissolved. No width crosses the renderer seam, so
  width-aware truncation is not needed (MCP-245); `sanitizeTerminalText` is 40 lines of pure string work
  the extension owns (MCP-235).
- *"A value-returning host request channel is needed for the approval broker."* Dissolved.
  `ExtHooks::before_tool_call` is the broker, already wired, and `EventKind::ToolCall` is the one kind
  that fails **closed** (MCP-233).
- *"`cyrup-mcp` does not exist — a prerequisite of the whole section."* Dissolved as a prerequisite:
  creating the native built-in crate *is* the port, with `cyrup-ext-subagents` as the working precedent
  and `SessionFactory::with_native_extension` as the attachment point.
- *"The workspace must declare `regex` and `tracing`."* Dissolved: both are per-crate declarations
  today (four crates pin `tracing`; `cyrup-permission-system` declares `regex = "1"`), so `cyrup-mcp`
  declaring its own is the existing convention, not a workspace change.
- *Severity inflation.* The first pass rated nine items `critical` (~18%). Two survive on the house
  scale: MCP-212 (an MCP server's tool silently replacing a cyrup built-in — verified against
  `ExtensionRegistry::active_tools`) and MCP-232 (an approval gate that cannot distinguish a cancelled
  dialog from a headless session fails open). MCP-205, MCP-214, MCP-217, MCP-223, MCP-228 and MCP-234
  are demoted to `high` with their blocking-ness or security consequence stated in the body.
- *`resolveServerFromToolName` "is partially ported in `cyrup-permission-system`."* Sharpened: that
  crate splits on the first `:` only, and its `_` handling matches configured server names as a
  **suffix** while explicitly skipping the prefix form — the opposite orientation. cyrup recovers no
  server for `github_create_issue` at all; there is no longest-prefix lookup and no ambiguity fail-safe
  anywhere in the tree.
- *`mcp_direct_tools.rs` divergence carried at "likely" strength.* Now confirmed on both sides:
  `pi-subagents`' own allowlist file uses `serverName.replace(/-/g, "_")` and a `get_` resource prefix,
  so the Rust is a faithful port of a drifted upstream. Two further asymmetries added: `BTreeMap`
  (name-sorted) iteration versus config insertion order, and no `disabled` check on the reader side.
- *Order-preservation was named as a requirement without a mechanism.* The workspace declares
  `serde_json = "1"` with no `preserve_order`, so `mcpServers` needs an explicit ordered
  deserialization — the pattern (and rationale) `cyrup_permission_system::ordered::OrderedValue`
  already establishes for exactly this class of bug.
- *`cyrup_tools::truncate` was described only as "reference only".* Sharpened with the two concrete
  divergences that force it: `truncate_head` never emits a partial line (the MCP guard does) and
  `format_size` emits `50.0KB`, not `50.0 KiB`.
- *The cut seams inside otherwise-surviving files were not identified.* Now named exactly: the
  `mcpScript` sentence in the proxy description header, the `ui-messages` usage line, the `'ui-messages'`
  mention in the `action` property description, the single `ui-messages` branch in
  `formatMcpProxyToolCallLines` (the renderer's only `ui` reference), the five UI call sites in
  `createDirectToolExecutor`, and `isUiToolCallableByApp` as the only cut half of
  `ui-tool-visibility.ts`.
- All cyrup line numbers, the HEAD/sha provenance note, the "read from disk" framing, the `depends`
  edges and the citation-count tally are removed: cyrup moves under this document, and a line-anchored
  plan is stale on arrival.
