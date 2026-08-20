# 13b · Configuration, the type model and errors

> Part of **[13 — cyrup-mcp](13-cyrup-mcp.md)**, which holds the thesis, the seam map, the
> architecture and the one canonical table of every port unit. Method and phasing are in
> **[MCP-PORT-METHODOLOGY.md](MCP-PORT-METHODOLOGY.md)**.

**Provenance.** Upstream is `pi-mcp-adapter` v2.25.0. cyrup is branch `david/cyrup`. Upstream is
referenced by file and symbol; a line range appears only where one algorithm has to be located
inside a large file and no symbol names it. cyrup is referenced by symbol and file, never by line
number and never by commit.

This section owns the seven files every other file in `pi-mcp-adapter` imports: `config.ts`
(discovery, precedence merge, host-config imports, write-back), `types.ts` (the shared type model
plus the naming and tool-filtering algorithms that live on it), `utils.ts` (env interpolation,
command secrets, URL/path resolution, terminal sanitisation), `errors.ts` (the error taxonomy),
`logger.ts` (the singleton logger), `ts-shape.ts` (JSON Schema → TypeScript rendering for the
model-facing `describe`/`search` output) and `json-schema-validator.ts` (the dialect gate handed to
the SDK client). Nothing here talks to an MCP server. Everything here decides *what* the rest of the
adapter will talk to, *under what name*, and *with what credentials*.

**The shape of the answer: every port unit in this section is `extension-owned` or `hand-written`
inside `crates/cyrup-mcp`, and none of it touches cyrup's core.** There is exactly one `host-verb`
(`HostServices::set_status`, for the footer segment), zero host-additions, and one genuine open
decision (whether the two project-scoped config sources honour project trust — a divergence *from*
upstream, not a gap in cyrup). This is what "the port changes nothing in cyrup's core" looks like
when you actually go file by file: a native built-in crate parses its own config files with the
JSONC parser already in the tree, resolves its own env vars, spawns its own credential-helper
processes, validates its own JSON Schemas, and writes its own files — reaching for the host only to
paint one footer string.

Four cyrup assets already exist and are reused rather than rebuilt: `cyrup_permission_system::jsonc`
(the only JSONC parser in the tree, and already what `cyrup-permission-system` uses on `mcp.json`,
so both crates parse that file identically by construction); `cyrup_permission_system::manager`'s
`read_configured_mcp_server_names` and `create_mcp_permission_targets`, which fix `<agent_dir>/mcp.json`
and the `mcp` proxy tool's parameter names as cross-crate contracts;
`cyrup_ext_subagents::exec::mcp_direct_tools`, which already **reads** `<agent_dir>/mcp-cache.json`
and already ports `computeMcpServerHash`; and `cyrup_ext::caps::proc::npx_resolver`, a full port of
`npx-resolver.ts`. Three of the four are contracts this section must match, not choices it gets to
make.

Two properties are load-bearing and are the reason this section is long. **First, the merge is not
`Object.assign`.** `mergeServerMaps` (`config.ts`) carries a documented credential-exfiltration
defence: when a higher-precedence source repoints an existing server at a different `url`, the
lower-precedence entry's `headers` / `bearerToken` / `bearerTokenEnv` / `oauth` are dropped *before*
merging, so a less-trusted project file cannot inherit a global file's credentials and ship them to
an attacker's endpoint. A port that models config as `HashMap<String, ServerEntry>` and calls
`.extend()` loses this while passing every functional test. **Second, defaults live at read sites,
not at parse.** `validateConfig` does not validate `settings` at all — it casts. Every default in
this subsystem is a predicate at the point of use (`?? "server"`, `!== false`, `=== true`,
`typeof v === "number" ? v : 10`, a `1|2|3` whitelist), and several of those predicates do *not*
mean what the documented default says. The tables below give the predicate, and the predicate is the
specification.

Third, and specific to cyrup: `<agent_dir>/mcp-cache.json` is a **shared on-disk contract** with an
existing in-tree reader. `computeServerHash`'s `stableStringify` emits the literal nine-character
string `undefined` for an absent field — the pre-image is deliberately not valid JSON — while the
in-tree Rust `stable_stringify` emits `null`. Left alone, no cyrup-written `configHash` can ever
match a v2.25.0-faithful writer's for any realistic server definition, and the symptom appears three
subsystems away as "direct tools silently didn't appear". Reconciling the two is the single largest
non-obvious obligation in this section.

---

### How it lands

| adapter capability | upstream mechanism | cyrup mechanism | verdict |
|---|---|---|---|
| JSONC config parsing | `strip-json-comments` in `config.ts`'s `parseJsonConfig` | `cyrup_permission_system::jsonc::{parse, parse_config, parse_config_into}` | `extension-owned` (reuse) |
| TOML config import (Codex only) | `smol-toml` in `readImportedConfig` | `toml` (already in tree, direct dep of `cyrup-resources`; promote to `[workspace.dependencies]`) | `extension-owned` (reuse) |
| six-source precedence ladder | `config.ts` `getConfigSources` / `loadMcpConfig` | plain Rust in `cyrup_mcp::config` | `hand-written` |
| per-field merge + URL-bound credential stripping | `config.ts` `mergeServerMaps` | explicit `merge_entry` over a typed `ServerEntry` | `hand-written` |
| 7 host-config import families | `config.ts` `IMPORT_PATHS` / `loadImportedConfig` / `extractServers` | plain Rust; `toml` for the one Codex path | `hand-written` |
| env interpolation, 3 syntaxes | `utils.ts` `interpolateEnvVars` | plain Rust in `cyrup_mcp::util::interp` (the two in-tree copies are 2-form; see MCP-082) | `hand-written` |
| `!` / `!!` command secrets | `utils.ts` `resolveCommandSecret` (`spawnSync`, `shell: true`) | `std::process::Command` with the platform shell, directly — a native crate spawns its own processes | `extension-owned` |
| server URL / path / bearer resolution | `utils.ts` `resolveServerUrl` / `resolveConfigPath` / `resolveBearerToken` | `url` (workspace dep) for the validity check; the rest is plain Rust | `hand-written` |
| tool naming and glob filtering | `types.ts` `getServerPrefix` … `isToolAllowed` | pure functions in `cyrup_mcp::naming`; `regex` for `globToRegExp` | `hand-written` |
| atomic config write-back | `config.ts` `writeRawConfigObject` (`.pid.tmp` + `renameSync`) | `std::fs` write + `rename`, no lock — matching upstream's concurrency contract | `extension-owned` |
| unified-diff write preview | `config.ts` `buildUnifiedDiff` (hand-rolled LCS) | port the DP literally; **not** `similar`, whose hunks differ and are rendered verbatim | `hand-written` |
| dual-dialect JSON Schema validation | `json-schema-validator.ts` on `ajv` + `ajv-formats` | `jsonschema` (already in tree; needs a bump), two `ValidationOptions` builders | `hand-written` |
| JSON Schema → TypeScript for the model | `ts-shape.ts` `renderTsShape` | `serde_json::Value` recursion, insertion-ordered aliases | `hand-written` |
| error taxonomy | `errors.ts` `McpUiError` + subclasses | `thiserror` enum rendering into `cyrup_core::ToolError` at the boundary | `hand-written` |
| levelled logging | `logger.ts` singleton with `[MCP-UI…]` prefixes | `tracing` (workspace dep, already used by `cyrup-ext-subagents`) | `extension-owned` |
| footer status segment text | `utils.ts` `formatMcpStatus` → `ui.setStatus("mcp", …)` | `HostServices::set_status(key, Option<&str>)` | `host-verb` |
| browser / file open | `utils.ts` `openUrl`/`openPath` (exec dispatch, `$BROWSER`, abort) and npm `open` in `mcp-auth-flow.ts` / `elicitation-handler.ts` | `HostServices::exec` for the dispatch form (it carries the cancel token); `opener` for the npm-`open` form | `extension-owned` + `host-verb` |
| project-trust awareness | none — upstream has no trust gate | `HostServices::is_project_trusted` exists if the port chooses to gate | `open-decision` (MCP-096) |
| MCP-UI type surface, `scriptMode`, `socket`, `httpTransport: "sse"` | `types.ts`, `utils.ts`, `config.ts` | — | `cut` |

---

### Behavioural specification

#### 1 · Config file layout and the precedence ladder

`getConfigSources(overridePath?, cwd)` builds an ordered `ConfigSourceSpec[]`; `loadMcpConfig` folds
it **left-to-right, later wins**.

Path constants (`config.ts` head): `GENERIC_GLOBAL_CONFIG_PATH = join(homedir(), ".config", "mcp",
"mcp.json")`; `AGENTS_GLOBAL_CONFIG_PATHS = [~/.agents/mcp.json, ~/.agents/mcp/mcp.json]`;
`PROJECT_CONFIG_NAME = ".mcp.json"`; `PROJECT_PI_CONFIG_NAME = ".pi/mcp.json"`. `userPath` is
`getPiGlobalConfigPath(overridePath)` = `resolve(overridePath)` when `--mcp-config` is set, else
`getAgentPath("mcp.json")`, where `getAgentDir()` (`agent-dir.ts`) is `$PI_CODING_AGENT_DIR`
(trimmed; `~` ⇒ home, `~/…` ⇒ `resolve(home, rest)`, else `resolve`) or `~/.pi/agent`.

| # | `id` | `label` | read path | write path | `shared` | `scope` | `kind` | emitted when |
|---|---|---|---|---|---|---|---|---|
| 0 | *(base)* | host-discovered configs | 7 families, `IMPORT_PATHS` order | — | — | — | — | `settings.hostConfigDiscovery === "on"` |
| 1 | `shared-global` | `user-global standard MCP` | `~/.config/mcp/mcp.json` | `userPath` | `true` | global | `import` (`"global MCP config"`) | `GENERIC ≠ userPath` |
| 2 | `agents-global` | `user-global .agents MCP` | `~/.agents/mcp.json` | `userPath` | `true` | global | `import` (`".agents MCP config"`) | `≠ userPath && ≠ GENERIC` |
| 3 | `agents-nested-global` | `user-global .agents nested MCP` | `~/.agents/mcp/mcp.json` | `userPath` | `true` | global | `import` (`".agents/mcp MCP config"`) | same test |
| 4 | `pi-global` | `Pi global override` | `userPath` | `userPath` | `false` | global | `user` | always |
| 5 | `shared-project` | `project standard MCP` | `<cwd>/.mcp.json` | itself | `true` | project | `project` | `≠ userPath` |
| 6 | `pi-project` | `project Pi override` | `<cwd>/.pi/mcp.json` | itself | `false` | project | `project` | `≠ userPath && ≠ projectPath` |

That `kind`/`importKind` pair is what `getServerProvenance` (§14) reports and what decides where a
write lands.

**After** the ladder, `loadMcpConfig` runs `mergeConfigs(pluginConfig, config)` — Agent-Plugin
servers are the **base**, so every file source outranks them. Note the asymmetry with host-discovered
configs, which are the base *before* the ladder; `config.ts`'s own comment states the intent ("an
opt-in discovery cannot override a shared or Pi-owned definition").

Per source, before merging, `expandImports(loaded, cwd)` folds that file's own `imports` array
**underneath** its own `mcpServers`: imported servers are first-wins across import kinds
(`if (!importedServers[name])`), then `mergeServerMaps(importedServers, config.mcpServers)`.

**cyrup naming.** `cyrup_ext_subagents::exec::mcp_direct_tools`'s `get_config_paths` already reads
four of these six sources and already renames the project override directory `.pi` → `.cyrup`. That
rename is settled by in-tree precedent and applies to all readers. The other two paths
(`~/.agents/mcp.json`, `~/.agents/mcp/mcp.json`) are tool-agnostic by design and stay verbatim; so
does `~/.config/mcp/mcp.json`. `<agent_dir>/mcp.json` is fixed by
`cyrup_permission_system::manager`'s `read_configured_mcp_server_names` and is not negotiable. The
same reader also diverges from upstream on three counts this section must fix: it uses strict JSON
where upstream uses JSONC; its `merge_configs` does a wholesale entry `insert` where upstream does a
per-field merge; and its settings merge is `next.settings.or(base.settings)` where upstream is a
one-level key merge.

#### 2 · Parsing

- `parseJsonConfig(raw)` = `JSON.parse(stripJsonComments(raw, { trailingCommas: true }))`. Used for
  every `mcp.json` and every non-TOML imported config.
- `readImportedConfig(path)` = `path.endsWith(".toml") ? parseToml(raw) : parseJsonConfig(raw)`. The
  only TOML path in the whole package is `~/.codex/config.toml`.
- `readValidatedConfig(path, label)`: non-existent ⇒ `null`; a parse or validate throw ⇒
  `console.warn(\`Failed to load ${label}:\`, error)` and `null`. `label` is always
  `` `MCP config from ${path}` ``.
- `validateConfig(raw)` is **lenient by design**: a non-object root yields `{ mcpServers: {} }`;
  servers come from `raw.mcpServers ?? raw["mcp-servers"]`; `imports` is kept only if
  `Array.isArray`; `settings` is kept verbatim with **no validation at all** (a bare cast).
- `toServerEntries` keeps an entry iff it is a non-array object. A malformed entry is dropped; the
  file survives.

**Port.** `cyrup_permission_system::jsonc` is the only JSONC implementation in the tree and is
exactly this transform — a string-aware preprocessor over `//`, `/* */` and trailing commas, then
`serde_json` — with `parse`, `parse_into`, `parse_ordered`, `parse_config`, `parse_ordered_config`
and `parse_config_into` all `pub`. It is already what `cyrup-permission-system` uses to read
`mcp.json`, so reusing it makes the two crates parse that file identically by construction. There is
no dependency cycle: `cyrup-permission-system` does not depend on `cyrup-mcp`.

#### 3 · `mergeServerMaps` — the merge algorithm, in order

`mergeConfigs(base, next)`:
`imports = mergeImports(base.imports, next.imports)` (concat + `Set` dedup; `undefined` when empty);
`settings = next.settings ? { ...base.settings, ...next.settings } : base.settings` — a **one-level**
merge, so `settings.trace` and `settings.outputGuard` objects are replaced wholesale, never
deep-merged; `mcpServers = mergeServerMaps(base.mcpServers, next.mcpServers)`.

`mergeServerMaps(base, next)`, per `[name, definition]` of `next` with `existing = merged[name]`:

1. *(cut — Cut 3)* If `existing` and `typeof definition.socket === "string"`, clone `existing` and
   delete `command, args, env, cwd, url, headers, auth, bearerToken, bearerTokenEnv, oauth`.
2. *(cut — Cut 3)* Else if `existing.socket` and the override supplies a string `command` or `url`,
   clone and delete `socket`.
3. **URL-bound credential stripping — this survives and is the security core.** If `existing` and
   `typeof definition.url === "string" && definition.url !== existing.url`: delete
   `URL_BOUND_AUTH_FIELDS = ["headers", "bearerToken", "bearerTokenEnv"]` and, unless
   `baseEntry.oauth === false`, delete `oauth`. An explicit `oauth: false` survives a URL change —
   the disable is not credential material.
   > **v2.26.1 — the array is FOUR elements.** `2a2db3c` appends `"requestHeadersCommand"`. Do not
   > implement the three-element form above: a request-signing command is bound to exactly one
   > endpoint, so a higher-precedence source that repoints `url` while inheriting this field would
   > carry the lower-precedence server's signing command to the new host. `config.rs` already has
   > `[&str; 4]`; this is a note for anyone re-reading the spec, not open work.
4. `merged[name] = { ...baseEntry, ...definition }` — **shallow, per-field**. A partial override
   inherits every field it does not mention. Credentials re-supplied by `definition` still apply,
   because `definition` spreads last.

`config.ts`'s comment block names the threat verbatim: without step 3, "a higher-precedence source
that supplies only a new `url` … would otherwise retain the lower-precedence entry's auth material …
and send it to the new url — a credential-exfiltration vector when the higher-precedence source is
less trusted than the one that first defined the server."

Note the clone discipline: `baseEntry` starts as `existing ?? {}` and is only cloned when a rule
fires, so a no-rule merge shares the base object and mutation is never observed. A Rust port with
value semantics gets this for free.

**What the post-cut merge must still do.** With rules 1 and 2 gone, `merge_entry` is: apply rule 3,
then a field-by-field `over.x.clone().or(stripped_base.x.clone())`. The `command` ⇄ `url` case that
rules 1/2 never covered behaves as it does upstream — a base `{command}` overridden by `{url}`
produces a two-transport entry that parses fine and fails at connect with the exactly-one-transport
error (§6). That is upstream behaviour and is preserved.

**Rust shape.** `ServerEntry` must be a struct of `Option<T>` fields (not a `serde_json::Map`) so the
deletion set is typed, and the merge must be written as an explicit
`fn merge_entry(base: Option<&ServerEntry>, over: &ServerEntry) -> ServerEntry`.
`#[serde(flatten)]` catch-alls are forbidden here: an unknown key that survives into `merged` would
bypass the stripping set.

#### 4 · Host-config import families

`IMPORT_PATHS` in `config.ts` and `ImportKind` in `types.ts` — **7 kinds**, iterated in declaration
order:

| kind | candidate paths (first existing wins, via `resolveImportCandidates`) | server key (`extractServers`) |
|---|---|---|
| `cursor` | `~/.cursor/mcp.json` | `mcpServers ?? mcp-servers` |
| `claude-code` | `~/.claude/mcp.json`, `~/.claude.json`, `~/.claude/claude_desktop_config.json` | `mcpServers` |
| `claude-desktop` | `~/Library/Application Support/Claude/claude_desktop_config.json` | `mcpServers` |
| `codex` | `~/.codex/config.toml`, `~/.codex/config.json` | `mcp_servers ?? mcpServers` |
| `opencode` | `~/.config/opencode/opencode.json`, `./opencode.json` *(git-root walk)* | `mcp` |
| `windsurf` | `~/.windsurf/mcp.json` | `mcpServers ?? mcp-servers` |
| `vscode` | `.vscode/mcp.json` *(cwd-relative)* | `mcpServers ?? mcp-servers` |

Candidate resolution: a candidate starting with `.` is `resolve(cwd, candidate)`; otherwise it is
used verbatim.

**`opencode` is special in three ways.** (a) `./opencode.json` is resolved by walking **up** from
`cwd` for a `.git` directory to find `gitRoot`, then walking **down-to-up** from `cwd` returning the
first existing `opencode.json`, stopping at `gitRoot`; with no git root, `join(start,
"opencode.json")`. (b) `loadImportedConfig` merges **all** existing opencode candidates rather than
taking the first, via `mergeOpenCodeConfigs`, and reports `highestPrecedencePath` as the path.
`mergeOpenCodeConfigs` mirrors the same credential-unbinding discipline as `mergeServerMaps` on a
*different* schema: a changed `type` deletes `command, environment, cwd, url, headers, oauth`; a
changed `url` deletes `headers` and `oauth`; a changed `command` array deletes `environment` and
`cwd`; then `environment`/`headers`/`oauth` are one-level object-merged. (c) Its entries are
**translated**, not passed through: `enabled === false` skips; `type: "local"` with a non-empty
all-string `command` array → `{ command: command[0], args: command.slice(1),
env: toStringRecord(environment)?, cwd? }`; `type: "remote"` with a string `url` →
`{ url, headers: toStringRecord(headers)? }`, plus `oauth === false` → `oauth: false`, or an `oauth`
object → `auth: "oauth"` and a projected `{ clientId?, clientSecret?, scope?,
skipIssuerMetadataValidation? }`.

**`codex` is translated too** (`translateCodexServer`): `bearer_token_env_var` → `bearerTokenEnv`
with `auth ??= "bearer"`; an `http_headers` object merged over `headers`; `env_http_headers`
`{ header: ENVVAR }` → `headers[header] ??= \`$env:${ENVVAR}\``; the three snake_case keys are then
deleted. Every other kind passes the raw record through unchanged.

**`hostConfigDiscovery`** reads the merged `settings.hostConfigDiscovery`, accepting only
`"off" | "prompt" | "on"` and defaulting `"off"`. `getMergedSettings` walks the same ladder and
shallow-merges the `settings` objects. `loadDiscoveredHostConfigs` folds all 7 families in
`IMPORT_PATHS` order, later wins.

`cyrup_ext_subagents::exec::mcp_direct_tools`'s `ImportKind` already covers 6 of the 7 (no
`opencode`) and only the `mcpServers`/`mcp-servers` key split; the Codex snake_case remap and the
TOML branch are absent there.

#### 5 · The `settings` block — every key, its type, default and predicate

`McpSettings` in `types.ts` carries **23** keys. **Defaults are enforced at the read site, never at
parse**, so a Rust `Default` impl must reproduce the exact predicate, which is not always "the
documented default". README's Settings table documents 22 of the 23 — `authRequiredMessage` is
undocumented but live.

| key | type | default | read sites (by file/symbol) | predicate |
|---|---|---|---|---|
| `toolPrefix` | `"server"\|"none"\|"short"\|"mcp"` | `"server"` | `init.ts`, `index.ts`, `direct-tools.ts`, `commands.ts`, `mcp-panel.ts`, `prompts.ts`, `proxy-modes.ts`, `search-ranking.ts` | `?? "server"`; per-server override via `resolveToolPrefix` |
| `showStatusIcon` | `bool` | `true` | `utils.ts` `formatMcpStatus` | `=== false ? "MCP: " : "🔌 MCP: "` |
| `mcpFooterStatus` | `"full"\|"compact"\|"off"` | `"full"` | `utils.ts` `formatMcpStatus`, `init.ts` `updateStatusBar` | `=== "off"` clears; `?? "full"` |
| `notifyOnStartupConnect` | `bool` | `true` | `init.ts` `initializeMcp` | `!== false` |
| `hostConfigDiscovery` | `"off"\|"prompt"\|"on"` | `"off"` | `config.ts` `getHostConfigDiscovery` | explicit 3-way test, else `"off"` |
| `agentPluginPaths` | `string[]` | — | `config.ts` `loadMcpConfig` | passed through to `agent-plugin-loader.ts` |
| `idleTimeout` | `number` (minutes) | `10` | `init.ts` (resolve + sweep) | `typeof === "number" ? v : 10` — so `0` is honoured and means disabled |
| `requestTimeoutMs` | `number` (ms) | SDK default | `init.ts` → `server-manager.ts` `normalizeRequestTimeoutMs` | finite **and `> 0`**, else `undefined` |
| `directTools` | `bool` | `false` | `direct-tools.ts`, `mcp-panel.ts`, `metadata-cache.ts` | truthiness; per-server `!== undefined` wins |
| **`scriptMode`** | `bool` | `true` | `index.ts` | **CUT — Cut 4** |
| `toolResultRendering` | `"compact"\|"boxed"` | `"compact"` | `tool-result-renderer.ts` | `=== "boxed" ? "boxed" : "compact"` |
| `collapsedResultLines` | `1\|2\|3` | 1 compact / 3 boxed | `tool-result-renderer.ts` (`DEFAULT_COMPACT_COLLAPSED_LINES` / `DEFAULT_BOXED_COLLAPSED_LINES`) | `v === 1 \|\| v === 2 \|\| v === 3 ? v : (boxed ? 3 : 1)` — a whitelist, not a clamp |
| `approveTools` | `bool \| string[]` | `undefined` | `tool-approval.ts` `isToolCallApprovalRequired` | per-server `!== undefined` wins |
| `disableProxyTool` | `bool` | `false` | `index.ts` `syncProxyTool` | `!== true` keeps the proxy |
| `freezeDirectTools` | `bool` | `false` | `index.ts` | `=== true` |
| `autoAuth` | `bool` | `false` | `proxy-modes.ts`, `direct-tools.ts` | `!== true` ⇒ skip |
| `sampling` | `bool` | `true` when UI or auto-approve | `init.ts` | `!== false && (hasUI \|\| samplingAutoApprove)` |
| `samplingAutoApprove` | `bool` | `false` | `init.ts` | `=== true` |
| `elicitation` | `bool` | `true` when UI | `init.ts` | `!== false && hasUI` |
| `outputGuard` | `bool \| {maxBytes,maxLines,detailsMaxBytes}` | `true` / 51200 / 2000 / 16384 | `mcp-output-guard.ts` `resolveMcpOutputGuardOptions` | see below |
| `trace` | `{enabled,file,maxBytes,maxEvents}` | disabled / `<cwd>/.pi/mcp-traces/mcp-<ts>-<rand>.jsonl` / 262144 / 10000 | `init.ts`; constants and enable test in `mcp-trace.ts` | `definition.trace ?? settings?.enabled === true` |
| `authRequiredMessage` | `string` | built-in text (below) | `utils.ts` `formatAuthRequiredMessage`, `proxy-modes.ts`, `direct-tools.ts` | `template.replaceAll("${server}", serverName)` |
| `oauthDir` | `string` | `undefined` | `config.ts` `resolveConfiguredOAuthDir`, `mcp-auth.ts` `getAuthBaseDir` | see below |

`outputGuard` resolution (`mcp-output-guard.ts`): `enabled = envKillSwitch("MCP_OUTPUT_GUARD") ??
configured !== false`, where `envKillSwitch` trims + lowercases and maps `0/false/no/off → false`,
`1/true/yes/on → true`, anything else (including empty) → `undefined`. Numeric knobs go through
`positiveInt`: must be a finite `number`, `Math.floor`ed, and `> 0`, else the constant default —
`DEFAULT_MCP_OUTPUT_MAX_BYTES = 50 * 1024`, `DEFAULT_MCP_OUTPUT_MAX_LINES = 2000`,
`DEFAULT_MCP_DETAILS_MAX_BYTES = 16 * 1024`. **The env kill switch outranks the config in both
directions** — `MCP_OUTPUT_GUARD=1` re-enables a config that said `false`.

`trace` defaults: `DEFAULT_MCP_TRACE_MAX_BYTES = 256 * 1024`, `DEFAULT_MCP_TRACE_MAX_EVENTS =
10_000`, both applied through `boundedPositiveInteger`; the default destination is
`resolve(sessionCwd ?? process.cwd(), ".pi", "mcp-traces", \`mcp-${timestamp}-${randomSuffix}.jsonl\`)`
(`.cyrup` in the port, per §1).

`oauthDir`: `resolveConfiguredOAuthDir(raw, cwd)` — `undefined`/`null` ⇒ `undefined`; a non-string
**throws** `settings.oauthDir must be a string`; blank after `.trim()` ⇒ `undefined`; else
`resolve(cwd, trimmed)`. `getAuthBaseDir` then applies `process.env.MCP_OAUTH_DIR?.trim()` **first**,
then the configured dir, then `getAgentPath("mcp-oauth")`.

Default `authRequiredMessage` text (`proxy-modes.ts`, and the identical string in `direct-tools.ts`):

```
Server "${serverName}" requires OAuth authentication. Run mcp({ action: "auth-start", server: "${serverName}" }) to get a browser URL, or /mcp-auth ${serverName} in an interactive local session.
```

Auth-failure text, both paths:

```
OAuth authentication failed for "${serverName}": ${message}. Run mcp({ action: "auth-start", server: "${serverName}" }) to get a browser URL, or /mcp-auth ${serverName} in an interactive local session.
```

— but when `settings.authRequiredMessage` is set, the tail is replaced by the formatted custom
message.

Footer text: `formatMcpStatus` returns `undefined` when `mcpFooterStatus === "off"`, else
`` `${showStatusIcon === false ? "MCP: " : "🔌 MCP: "}${message}` ``. The emoji is
`U+1F50C ELECTRIC PLUG` followed by `U+0020`.

#### 6 · `ServerEntry` — every field

`ServerEntry` in `types.ts` carries **28** fields; `ServerDefinition` is an alias; `OAuthConfig`
carries 10. README's Server Options table documents 25 of the 28 — `httpTransport`, `pluginDataDir`
and `literalEnv` are set only by `agent-plugin-loader.ts` and are never hand-written.

| field | type | default / semantics | consumer |
|---|---|---|---|
| `command` | `string` | stdio transport | `server-manager.ts` `createConnection` |
| `args` | `string[]` | each element `interpolateEnvVars`'d | `createConnection` |
| **`socket`** | `string` | rmcp-mux UDS path; `resolveConfigPath`'d | **CUT — Cut 3** |
| `env` | `Record<string,string>` | `resolveCommandSecretsRecord` at connect, layered over the full `process.env` | `createConnection` → `resolveEnv` |
| `cwd` | `string` | `resolveConfigPath` (interp + `~`), else `defaultCwd` | `createConnection` |
| `url` | `string` | `resolveServerUrl` (interp; **throws** on non-string / missing var / invalid URL) | `createConnection` |
| `headers` | `Record<string,string>` | `!`-secret + interp | `connectHttpClient`; hashed by `computeServerHash` |
| `auth` | `"oauth" \| "bearer" \| false` | untagged; absent ⇒ OAuth auto-detect for `url` servers | `connectHttpClient` |
| `bearerToken` / `bearerTokenEnv` | `string` | `resolveBearerToken` | `connectHttpClient` |
| `oauth` | `OAuthConfig \| false` | untagged | `commands.ts`, `mergeServerMaps` |
| `lifecycle` | `"keep-alive"\|"lazy"\|"lazy-keep-alive"\|"eager"` | `"lazy"` | `init.ts`, `index.ts` |
| `idleTimeout` | `number` min | overrides global; **`eager`** and **`lazy-keep-alive`** force `0` (`init.ts`'s `persistsAfterFirstSpawn`); `keep-alive` is handled separately | `init.ts` idle sweep |
| `requestTimeoutMs` | `number` ms | `normalizeRequestTimeoutMs`: `> 0`, else fall through to global | `server-manager.ts` `buildRequestOptions` |
| `exposeResources` | `bool` | `true`; tested as `!== false` everywhere | `direct-tools.ts`, `init.ts` |
| `directTools` | `bool \| string[]` | per-server `!== undefined` beats `settings.directTools` | `direct-tools.ts` |
| `toolPrefix` | `ToolPrefix` | overrides `settings.toolPrefix` | `types.ts` `resolveToolPrefix` |
| `includeTools` / `excludeTools` | `string[]` | glob or exact; exclude applied after include | `types.ts` `isToolAllowed` |
| `searchKeywords` | `Record<string,string[]>` | ranking-only; never in schemas, `describe` output, or the cache | `search-ranking.ts` |
| `approveTools` | `bool \| string[]` | per-server `!== undefined` beats global | `tool-approval.ts` |
| `debug` | `bool` | `false`; `true` ⇒ stderr `"inherit"`, else `"pipe"` | `createConnection` |
| `trace` | `bool` | `definition.trace ?? settings.trace?.enabled === true` | `mcp-trace.ts` `isMcpTraceEnabled` |
| **`httpTransport`** | `"streamable-http"\|"sse"` | absent ⇒ streamable-HTTP with SSE fallback; set ⇒ no fallback | **`"sse"` value CUT — Cut 1**; field survives with one legal value |
| `pluginDataDir` | `string` | `mkdirSync(recursive)` before spawn | `createConnection`; set by `agent-plugin-loader.ts` |
| `literalEnv` | `bool` | `true` ⇒ env values used verbatim: no `!`-secret, no interp | `resolveEnv`; set by `agent-plugin-loader.ts` |
| `protocolVersion` | `"legacy"\|"auto"\|"2026-07-28"` | `"legacy"`; `undefined` and `"legacy"` are byte-identical (both ⇒ no `versionNegotiation` sent) | `server-manager.ts` `resolveVersionNegotiation` |
| `disabled` | `bool` | only literal `true` disables (`isServerDisabled`) | `index.ts`, `mcp-status.ts` |

**Validation rules that fire at connect time, not at parse time:**

1. `[command, url, socket].filter(string && length > 0).length !== 1` ⇒
   `` throw new Error(`Server ${name} must configure exactly one of command, url, or socket`) ``.
   Config load never rejects a two-transport entry; the failure is per-connection. **Post-cut this
   becomes `[command, url]` and the message loses `, or socket`.**
2. `resolveVersionNegotiation` throws `` `Invalid MCP protocolVersion: ${String(definition.protocolVersion)}` ``
   on any value outside the three.
3. `resolveServerUrl` (`utils.ts`) has **three** throws: `` `MCP server URL must be a string` `` for a
   non-string, non-null `url`; `` `Missing environment variable${s} in MCP server URL: ${names.join(", ")}` ``
   (plural `s` when `missing.length !== 1`); and
   `` `Invalid MCP server URL after environment interpolation: ${resolved}` `` with `{ cause }`. A
   `null`/`undefined` `url` returns `undefined` without throwing. This is why `isServerCacheValid`
   wraps `computeServerHash` in `try/catch` and returns `false`.

**Cut-value diagnostics.** `socket` and `httpTransport: "sse"` must be **rejected at config load with
a named diagnostic**, not silently dropped. `agent-plugin-loader.ts` sets `httpTransport` directly
from a manifest's `type: "streamable-http" | "sse"`, so an Agent Plugin declaring `type: sse` is a
live, reachable case — silently ignoring it produces a server that appears configured and never
connects.

#### 7 · Serde attribute contract

Every `ServerEntry` / `McpSettings` field is `camelCase` on the wire and needs
`#[serde(rename_all = "camelCase")]`, with an explicit `#[serde(rename = "…")]` only where the Rust
name would otherwise differ non-mechanically. Beyond that, four decisions:

1. **Optional-vs-null.** Fields are `Option<T>` with `#[serde(default, skip_serializing_if =
   "Option::is_none")]`. Writing `null` for an absent field would change the on-disk file the adapter
   round-trips through `readRawConfigObject`/`writeRawConfigObject` — and, worse, would change the
   `computeServerHash` pre-image, whose scalar branch emits the literal `undefined` for absent and
   `null` for an explicit JSON null. The two are distinguishable and both reachable. See §17.
2. **`bool | T` unions** — `directTools: boolean | string[]`, `approveTools: boolean | string[]`,
   `outputGuard: boolean | McpOutputGuardSettings`, `oauth: OAuthConfig | false`,
   `auth: "oauth" | "bearer" | false` — need `#[serde(untagged)]` enums. `auth` needs a three-arm
   untagged enum `{ Oauth, Bearer, Disabled(bool) }` (the `false` arm is a `bool`, not a unit
   variant) because only the literal `false` is legal, not `true`. `oauth` likewise; and `oauth: true`
   must be treated as upstream treats it — TypeScript's structural cast admits it and the value
   simply never satisfies `oauth !== false`, so the Rust side mirrors the leniency: drop the field,
   do not fail the file.
3. **Unknown keys round-trip.** `writeSharedServerEntry` / `writeDirectToolsConfig` /
   `writeProjectServerDisabledOverride` all operate on the **raw** parsed object
   (`readRawConfigObject`), never on a typed `ServerEntry`, precisely so an unknown key survives a
   write. The Rust port must keep the same split: a typed `ServerEntry` for reading and merging, and
   a `serde_json::Map`-level document for writing. Do **not** unify them with
   `#[serde(flatten)] extra: Map<…>` — that would re-introduce unknown keys into the merge, past the
   credential-stripping set of §3.
4. **`McpSettings` is never validated.** A Rust `#[serde(deny_unknown_fields)]` or a hard
   `Deserialize` failure on a bad type would reject configs upstream accepts. Model it as a struct of
   `Option<T>` with a permissive `deserialize_with` per field that yields `None` on a type mismatch,
   or parse settings from a `Value` field by field.

#### 8 · Naming: `getServerPrefix`, `formatToolName` and friends

`sanitizeServerPrefix(serverName, preserveProviderValid = true)` (`types.ts`, private): per **code
point** (`Array.from`, so astral characters are one unit), `validCharacters = preserveProviderValid ?
/^[A-Za-z0-9_-]$/ : /^[A-Za-z0-9]$/`; a valid char is kept; anything else becomes
`` `_${char.codePointAt(0).toString(16)}_` `` — lowercase hex, no padding. Note `-` is **preserved**
in the default (provider-valid) mode.

`getServerPrefix(serverName, mode)`:

| mode | result |
|---|---|
| `"none"` | `""` |
| `"short"` | `sanitizeServerPrefix(serverName.replace(/-?mcp$/i, ""))`, or `"mcp"` if empty |
| `"mcp"` | `` `mcp__${sanitizeServerPrefix(serverName)}` `` |
| `"server"` (default / any other) | `sanitizeServerPrefix(serverName)` |

`formatToolName(toolName, serverName, prefix)`: `sanitized = toolName.replace(/\./g, "_")` — **only
`.`**, not `-`; then `` p ? `${p}_${sanitized}` : sanitized ``.

`resolveToolPrefix(definition?, globalPrefix?)`: `definition?.toolPrefix ?? globalPrefix ?? "server"`.

`resolveServerFromToolName(toolName, serverNames, prefix)` — the inverse. `"none"` ⇒ `undefined`.
Collect every configured server whose non-empty prefix satisfies `toolName.startsWith(p + "_")`, sort
by **prefix length descending**, take the best; **if any other candidate shares that exact prefix
string, return `undefined`** — the fail-safe for `short` mode collapsing `foo` and `foo-mcp` to the
same prefix, so a downstream policy gate falls back to its wildcard path rather than enforcing a rule
against the wrong server. It has **zero in-package callers** at v2.25.0; its doc comment names its
audience as "downstream policy systems (for example a permission gate)", so it is a published API.
In cyrup that consumer is `cyrup_permission_system::manager`'s `parse_qualified_mcp_tool_name` /
`create_mcp_permission_targets`, which today has no configured-prefix-set inverse.

`sanitizePromptName(name)`: `[^A-Za-z0-9_-]+ → "_"`, then trim leading/trailing `[_-]+`; empty ⇒
`"prompt"`; a leading digit gets a `_` prefix.
`formatPromptCommandName(promptName, serverName, prefix)`:
`` `mcp__${serverPart}__${sanitizePromptName(promptName)}` `` where
`serverPart = getServerPrefix(serverName, prefix) || sanitizeServerPrefix(serverName) || "server"`.

`getToolNameCandidates(toolName, serverName, prefix, includeLegacy = true)` builds a `Set` from **5**
current-form insertions (the raw tool name + the 4 prefix modes) and, when `includeLegacy`, **13
more**: the bare `-→_` tool name, that name under all 4 modes, `formatLegacyToolName` under all 4
modes, and the 4 current names with `-→_` applied. **18 insertions**, and because it is a `Set` the
resulting size is data-dependent: `list_sims`@`xcodebuild` yields **3** distinct names,
`browser.navigate`@`chrome-devtools` yields **7**, `get-code.map`@`figma-mcp` yields **12**. A test
asserting a fixed cardinality will fail. `getLegacyServerPrefix` is `getServerPrefix` with
`preserveProviderValid = false` (so `-` becomes `_2d_`), and `formatLegacyToolName` sanitises
`[.-] → _` in the tool name. This set exists purely so a user's
`includeTools`/`excludeTools`/`approveTools` written against an older naming scheme keeps matching.

`globToRegExp(pattern)`: escape `[.+^${}()|[\]\\]`, then `* → .*`, `? → .`, anchored `^…$`.
`matchesToolPattern(candidates, patterns)`: non-array or empty ⇒ `false`; a pattern with no `*`/`?`
matches by `Set.has`; a glob pattern is tested against every candidate, and `globToRegExp` is
re-compiled per candidate — a hot-path detail a Rust port should hoist, which is not observable.
`matchesToolSelector` first tests the **non-legacy** candidate set, then either the full set (no
`otherCurrentCandidates`) or — when disambiguating against a sibling tool — only legacy-only
candidates that do **not** match the sibling's current candidates. `isToolIncluded` returns `true`
when `includeTools` is absent or empty. `isToolExcluded` is `matchesToolSelector` directly.
`isToolAllowed` = included && !excluded.

**In-tree divergences to reconcile**, all in `cyrup_ext_subagents::exec::mcp_direct_tools`: its
`ToolPrefix` has only `{Server, None, Short}` and `get_tool_prefix` folds `"mcp"` — and every unknown
string — to `Server`; `get_server_prefix` does `server_name.replace('-', "_")` where upstream
*preserves* `-` and hex-escapes everything else (matching neither upstream's current form nor its
legacy `_2d_` form); `format_tool_name` does not sanitise `.` in the tool name; `is_tool_excluded`
compares `-`-normalised names against **4** candidates with no glob support, where upstream compares
5 or up to 18 with globs.

#### 9 · `utils.ts` — interpolation, secrets, sanitisation

**`interpolateEnvVars(value)`** — three passes, in this order: `/\$\{(\w+)\}/g`, then
`/\$env:(\w+)/g`, then `/\{env:(\w+)\}/g`. A missing variable expands to `""`
(`process.env[name] ?? ""`). `\w` is `[A-Za-z0-9_]`. The substitution order matters because an
expanded value can itself contain a later-form placeholder. **Both in-tree Rust implementations omit
the third form**: `cyrup_ext::caps::proc`'s `interpolate_env_vars` (which is `pub(crate)`, so
`cyrup-mcp` cannot call it anyway) and `mcp_direct_tools`'s `interpolate_env_vars`. A config written
with `{env:TOKEN}` interpolates upstream and is passed through literally by both.

**`getMissingEnvVars(value)`** (private) scans a combined alternation
`/\$\{(\w+)\}|\$env:(\w+)|\{env:(\w+)\}/g` and returns the de-duplicated names whose
`process.env[name] === undefined`. **Only `resolveServerUrl` uses it** — env misses are silent
everywhere else.

**Secret markers.** `interpolateSecretExpression(value)` (private): `"!!" + rest` ⇒
`interpolateEnvVars("!" + rest)` (one `!` is consumed, the remainder is interpolated); a single
leading `"!"` ⇒ returned **verbatim, uninterpolated** (it is a deferred command, not a value);
otherwise `interpolateEnvVars(value)`. `interpolateEnvRecord` applies it per value.

`resolveCommandSecret(value, context)` is the *execution* form, reached only at connect/auth time:
same `!!` / no-`!` branches, else
`spawnSync(value.slice(1), { shell: true, encoding: "utf8", timeout: COMMAND_SECRET_TIMEOUT_MS,
maxBuffer: COMMAND_SECRET_MAX_OUTPUT_BYTES, stdio: ["ignore","pipe","ignore"], windowsHide: true })`,
with `COMMAND_SECRET_TIMEOUT_MS = 10_000` and `COMMAND_SECRET_MAX_OUTPUT_BYTES = 1024 * 1024`.
Failure messages, exact:

- `` `Failed to resolve ${context}: command timed out after 10 seconds` `` (`ETIMEDOUT`)
- `` `Failed to resolve ${context}: command output exceeded 1 MiB` `` (`ENOBUFS`)
- `` `Failed to resolve ${context}: command failed to start` `` (any other spawn error)
- `` `Failed to resolve ${context}: command exited with code ${status ?? "unknown"}` ``
- `` `Failed to resolve ${context}: command returned empty output` `` (after `.trim()`)

`context` is user-visible at all four call sites, and all four strings must be reproduced:

| context string | site |
|---|---|
| `` `MCP server "${serverName}" stdio env "${key}"` `` | `server-manager.ts` `resolveEnv` → `resolveCommandSecretsRecord` |
| `` `MCP server "${serverName}" HTTP header "${key}"` `` | `server-manager.ts` `connectHttpClient` → `resolveCommandSecretsRecord` |
| `` `MCP server "${serverName}" HTTP bearer token` `` | `server-manager.ts` `connectHttpClient` |
| `` `MCP server "${this.serverName}" OAuth clientSecret` `` | `mcp-oauth-provider.ts` |

Note the caller-side gating in `connectHttpClient`: a header or bearer value only reaches
`resolveCommandSecret` when it `startsWith("!") && !startsWith("!!")`, so the `!!` escape never
spawns. And `resolveCommandSecret` is reached **only** at connect/auth time — never during discovery,
merge, preview, hashing or rendering. That timing is the security property; the `stdio`
`["ignore","pipe","ignore"]` split is the second one (stderr is discarded so a chatty credential
helper cannot leak into a tool result).

**`resolveConfigPath(value)`**: interpolate, then `"~"` ⇒ `homedir()`, `"~/"`/`"~\\"` prefix ⇒
`join(homedir(), rest)`, else the interpolated string. Applied to `cwd` (and, before Cut 3, `socket`).

**`resolveBearerToken(definition)`**: `bearerToken` present ⇒
`interpolateSecretExpression(bearerToken)`; else `process.env[bearerTokenEnv]` (or `undefined`). The
in-tree `mcp_direct_tools::resolve_bearer_token` uses plain interpolation here, so a
`bearerToken: "!!x"` hashes as `"!!x"` in cyrup and `"!x"` upstream.

**Terminal sanitisation.** `stripOscSequences(text)` is a hand-written scanner that removes `ESC ]`
(0x1b 0x5d) and C1 OSC (0x9d) introducers **and their payload**, terminating on BEL (0x07), ST
(0x9c) or `ESC \`, and consuming to end-of-string when unterminated — the point is that an
*unterminated* OSC payload cannot survive into a TUI. `sanitizeTerminalText` then strips CSI and
other escapes with a regex, replaces every run of C0/DEL/C1 control characters
(`U+0000`–`U+001F` plus `U+007F`–`U+009F`) with a single space, collapses `\s+`, and trims.
`formatTerminalError(error)` walks `AggregateError.errors` then `.cause` (and, when the nested walk
produced nothing, falls back to the aggregate's own `.message`), then `Error.message` then `.cause`,
with a `Set`-based cycle guard on objects and functions, falls back to `String(value)`, de-duplicates
via a `Set` and joins with `": "`, then sanitises.

**Misc.** `parallelLimit(items, limit, fn)` runs `min(limit, items.length)` workers over a shared
`entries()` iterator, writing results **by original index**; the only call site is `init.ts` with
`limit = 10`. `getConfigPathFromArgv()` scans `process.argv` for `--mcp-config` and takes the next
element — needed because pi's flag API is a throwing stub at extension-load time; this is the
*literal* mechanism, and `std::env::args()` is its exact Rust analogue (`InitApi::register_flag`
still declares the flag so it appears in `--help`). `toStringRecord(value)` keeps only string-valued
keys and returns `undefined` for an empty result. `truncateAtWord(text, target)` cuts at the last
space when it is `> target * 0.6`, else hard-cuts, appending `"..."`.
`normalizeDirectToolInputSchema(schema)` defaults a non-object to `{ type: "object", properties: {} }`
and **strips `$schema` and `additionalProperties`** before handing the schema to the host tool
registry — `cyrup_core::Tool::parameters` takes raw JSON Schema, so this is the only shaping needed.
`extractToolUiStreamMode(toolMeta)` is **cut** (Cut 2).

**Browser and path open.** Two distinct mechanisms exist upstream and both port:

*(a)* `execOpen(pi, target, browser?, signal?)` (private in `utils.ts`), a literal platform dispatch
through the host `exec`:

| platform | with `browser` | without |
|---|---|---|
| `darwin` | absolute path whose lowercased `extname !== ".app"` ⇒ `exec(browser, [target])`; else `exec("open", ["-a", browser, target])` | `exec("open", [target])` |
| `win32` | `exec("cmd", ["/c","start","",browser,target])` | `exec("cmd", ["/c","start","",target])` |
| other | `exec(browser, [target])` | `exec("xdg-open", [target])` |

`openUrl` throws `result.stderr || \`Failed to open browser (exit code ${result.code})\`` on
non-zero; `openPath` throws the `Failed to open path (…)` variant and passes **no** `browser` and
**no** signal. The `browser` argument comes from `process.env.BROWSER` (`init.ts`, on the OAuth
authorization URL). `HostServices::exec(cmd, args, opts, cancel)` is the direct analogue and carries
the cancel token, so it is the faithful landing spot.

*(b)* npm `open`, imported directly by `mcp-auth-flow.ts` and `elicitation-handler.ts`. That one is
`opener` per the dependency decision. The two are not alternatives — upstream has both, and so does
the port.

#### 10 · Error taxonomy (`errors.ts`)

`McpUiError extends Error`: `name = "McpUiError"`, readonly `code`, `context: McpUiErrorContext`
(default `{}`), `recoveryHint?`, `cause?`, plus `toJSON()` emitting
`{ name, code, message, context, recoveryHint, stack }`. `McpUiErrorContext` is
`{ server?, tool?, uri?, session?, [key: string]: unknown }`. Each subclass reassigns `this.name`
after `super(...)`.

| class | message template | `code` | `recoveryHint` | context keys | scope |
|---|---|---|---|---|---|
| `ResourceFetchError` | `Failed to fetch UI resource "${uri}": ${reason}` | `RESOURCE_FETCH_ERROR` | `Check that the MCP server is connected and the resource URI is valid.` | `uri`, `server?` | **cut (Cut 2)** |
| `ResourceParseError` | `Invalid UI resource "${uri}": ${reason}` | `RESOURCE_PARSE_ERROR` | `Ensure the resource returns valid HTML with the correct MIME type.` | `uri`, `server?`, `mimeType?` | **cut (Cut 2)** |
| `BridgeConnectionError` | `AppBridge connection failed: ${reason}` | `BRIDGE_CONNECTION_ERROR` | `Check browser console for detailed errors. The iframe may have failed to load.` | `session?` | **cut (Cut 2)** |
| `ConsentError` | denied: `Tool calls for "${server}" were denied for this session`; else `Tool call approval required for "${server}"` | `CONSENT_DENIED` / `CONSENT_REQUIRED` | denied: `The user denied tool access. Start a new session to try again.`; else `Prompt the user for consent before calling tools.` | `server`; extra readonly `denied: boolean` | **ports** |
| `SessionError` | `Session error: ${reason}` | `SESSION_ERROR` | `The session may have expired or been closed. Try opening the UI again.` | `session?` | **cut (Cut 2)** |
| `ServerError` | `UI server error: ${reason}` | `SERVER_ERROR` | `Check if the port is available. Another process may be using it.` | `port?` | **cut (Cut 2)** |
| `McpServerError` | `MCP server "${server}" error: ${reason}` | `MCP_SERVER_ERROR` | `Check that the MCP server is running and responsive.` | `server`, `tool?` | **ports** (no upstream caller) |

`wrapError(error, context?)`: an `McpUiError` in ⇒ a **new** `McpUiError` carrying the same `code`,
`message`, `recoveryHint` and `cause`, with `{ ...error.context, ...context }`; otherwise a fresh
`McpUiError(message, { code: "UNKNOWN_ERROR", context?, cause? })` where `message` is `error.message`
for an `Error` and `String(error)` otherwise. `isErrorCode(error, code)` is an `instanceof` +
`code ===` test.

**Production usage at v2.25.0** (exhaustive grep over every non-test `.ts` at the tag): `ConsentError`
×2 (`consent-manager.ts`), `ResourceParseError` ×3 and `ResourceFetchError` ×1
(`ui-resource-handler.ts`), `ServerError` ×2 and `wrapError` ×1 (`ui-server.ts`).
`BridgeConnectionError`, `SessionError`, `McpServerError`, `isErrorCode` and any direct
`new McpUiError(...)` have **zero** production call sites — they exist only in
`__tests__/errors.test.ts`. **After Cut 2 the only surviving production thrower in the whole file is
`ConsentError`.** The port therefore carries the base shape (code + context + recoveryHint + cause +
`toJSON`), `ConsentError`, and `McpServerError` as the generic MCP-server-failure arm; `wrapError`
survives as taxonomy with no caller until another subsystem needs it. `cyrup_core::ToolError` is
`{ message }` only, so the enum renders into `ToolError::message` at the tool boundary and the
structured `code`/`context`/`recoveryHint` triple stays inside `cyrup-mcp`.

#### 11 · Logger (`logger.ts`)

`LogLevel = "debug" | "info" | "warn" | "error"`;
`LEVEL_PRIORITY = { debug:0, info:1, warn:2, error:3 }`;
`LEVEL_PREFIX = { debug:"[MCP-UI:DEBUG]", info:"[MCP-UI]", warn:"[MCP-UI:WARN]",
error:"[MCP-UI:ERROR]" }`. `Logger` holds `minLevel = "info"`, a handler list and a
`defaultContext`. `emit` filters on `LEVEL_PRIORITY[level] >= LEVEL_PRIORITY[minLevel]`, builds
`{ level, message, context: {...default, ...context}, error?, timestamp: new Date() }`, prints
`` contextStr ? `${prefix} ${message} ${contextStr}` : `${prefix} ${message}` `` to
`console.error(msg, error ?? "")` / `console.warn` / `console.debug` / `console.log`, then calls every
registered handler inside a `try {} catch {}` that **swallows handler errors**. `formatContext`
renders `(k=v, k=v)` skipping `undefined`/`null`, with strings raw and everything else
`JSON.stringify`d; an empty context yields `""`. `ChildLogger` merges its own context **under** each
call's context and supports nesting. Module bootstrap: `MCP_UI_DEBUG === "1" || "true"` ⇒
`logger.setLevel("debug")`.

**Live sites after Cut 2.** Direct calls: 16 `debug` (`index.ts`, `init.ts`, `lifecycle.ts`,
`prompts.ts`, `server-manager.ts`, `session-recovery.ts`), 1 `info` (`index.ts`, the
`freezeDirectTools` notice), and 1 `child` (`consent-manager.ts`, which adds 5 more `debug`). Every
production `warn` (2) and `error` (5) site lived in `ui-resource-handler.ts` / `ui-server.ts` /
`ui-session.ts` and goes with Cut 2, as do three of the four child loggers. The port still maps all
four levels onto `tracing` — the level filter, the `MCP_UI_DEBUG` bootstrap and the prefix are the
user-facing contract, not the current call distribution.

Separately, config-load warnings do **not** go through the logger at all: they are bare
`console.warn` in `config.ts` (`loadImportedConfig`, `resolveImportPath`, `readValidatedConfig`) and
in `agent-plugin-loader.ts`. The port must keep both channels distinct — those sites are unfiltered
diagnostics that predate the logger and are not level-gated. The `[MCP-UI…]` prefix is the package's
historical name and carries no dependency on the cut Apps subsystem; keep it or rename it, but do not
read it as a scope signal.

#### 12 · `ts-shape.ts` — JSON Schema → TypeScript, for the model

Purpose: `mcp({ describe })` and `mcp({ search, includeSchemas })` print a tool's parameters. A raw
JSON Schema is verbose and models read it poorly; a TypeScript type literal is compact and models
read it well. `renderTsShape` returns `string | null`, and **`null` means "fall back to the raw
schema"** — `proxy-modes.ts`'s two call sites each have a fallback beside them. So a Rust port may
safely be conservative: returning `None` more often is a verbosity regression, never a correctness
one. Returning a *wrong string*, however, is not caught anywhere — see MCP-098.

`UNSUPPORTED_KEYWORDS = ["if","then","else","allOf","not","patternProperties",
"additionalProperties"]`; `hasUnsupportedKeyword` treats `additionalProperties: false` as
**supported** (a closed-object constraint, not a shape), and is re-tested at every node, not only at
the root.

Algorithm, whole body wrapped in `try {} catch { return null }`:

1. Collect `$defs` and `definitions` into
   `definitions: Map<"$defs/<name>"|"definitions/<name>", Schema>`, applying `decodePointerToken`
   (`~1→/`, `~0→~`). A non-object group or member ⇒ `null` for the whole render.
2. `aliasFor(key)`: reuse the bare name when it matches `/^[A-Za-z_$][\w$]*$/` and is unused, else
   `Definition${++aliasIndex}`, incrementing until unique.
3. `render(schema)`, in this exact precedence order:
   `$ref` (must match `/^#\/(\$defs|definitions)\/([^/]+)$/` and resolve, else `null`) →
   `enum` (every member through `renderLiteral`, joined ` | `) →
   `const` (via `Object.hasOwn`, so a present-but-`undefined` `const` still takes this branch) →
   `anyOf`/`oneOf` (`anyOf` preferred; empty ⇒ `null`) →
   `type === "object" || properties !== undefined` (no `properties` ⇒ `"{}"`; each property
   `` `${formatPropertyName(name)}${required ? "" : "?"}: ${rendered};` ``, joined by a space, wrapped
   `{ … }`; zero properties ⇒ `"{}"`) →
   `type === "array"` (no `items` ⇒ `"unknown[]"`; else `` `${item}[]` `` with the item parenthesised
   when `needsParentheses` sees `" | "`) →
   `Array.isArray(type)` (each through `renderType`, joined ` | `) →
   `typeof type === "string"` ⇒ `renderType` → fallback `"unknown"`.
4. `renderType`: `string→string`, `number|integer→number`, `boolean→boolean`, `null→null`,
   `object→{}`, `array→unknown[]`, anything else ⇒ `null`.
5. `renderLiteral`: `null`/string/boolean ⇒ `JSON.stringify(value)`; a **finite** number ⇒
   `String(value)`; anything else ⇒ `null`.
6. `formatPropertyName`: bare identifier if `/^[A-Za-z_$][\w$]*$/`, else `JSON.stringify(name)`.
7. Output: aliases emitted as `` `type ${alias} = ${rendered};` `` joined by `\n`, then `\n\n`, then
   the root — or just the root when no alias was used. **Aliases are emitted in `Map` insertion
   order**, i.e. first-referenced-first, not alphabetical. Critically, the emission loop
   `for (const [key, alias] of aliases)` iterates a `Map` that `render(definition)` inside the loop
   may **grow** — a `$ref` inside a `$defs` member registers a new alias mid-iteration, and JS `Map`
   iterators are live, so the new alias is visited and emitted. A Rust port that snapshots the map
   into a `Vec` before looping silently omits those `type X = …;` lines and emits a shape referencing
   an undefined name. See MCP-098.

#### 13 · `json-schema-validator.ts` — the dialect gate

`createJsonSchemaValidator()` returns a lazily-memoising provider handed to the SDK `Client` in
`server-manager.ts`'s `createClient`. `schemaDialect(schema)`: no string `$schema` ⇒
`{status:"unstamped"}`; else strip **one** trailing `#` and report `{status:"stamped", uri}`.

- `unstamped` **or** `https://json-schema.org/draft/2020-12/schema` (`DRAFT_2020_12_SCHEMA_URIS`) ⇒
  the 2020-12 validator, `new Ajv2020({ strict: false, allErrors: true })` + `addFormats`.
- `http://json-schema.org/draft-07/schema` or the `https://` variant (`DRAFT_07_SCHEMA_URIS`) ⇒
  `new Ajv({ strict: false, validateFormats: true, validateSchema: false, allErrors: true })` +
  `addFormats`.
- Anything else ⇒ `` throw new Error(\`Unsupported JSON Schema dialect: ${dialect.uri}\`) ``.

Note the asymmetry: `validateFormats: true` appears **only** on the draft-07 arm. Format assertion is
still active for both, because ajv's `validateFormats` option defaults to `true`. The Rust
equivalent — `jsonschema`'s `ValidationOptions::should_validate_formats(true)` — must therefore be
set on **both** compiled validators, because `jsonschema` treats `format` as annotation-only by
default. That asymmetry is the trap: reading the source literally and setting the flag on one
validator reproduces the *code* and breaks the *behaviour*.

The workspace already carries `jsonschema` with `default-features = false`; keep that setting (the
workspace comment explains why remote and file `$ref` resolution stays off, and the reasoning applies
here identically) and bump the version. `ajv-formats` supplies `url, int32, int64, float, double,
byte, binary, password, iso-time, iso-date-time, json-pointer-uri-fragment` beyond `jsonschema`'s
built-ins; each missing one is registered with `ValidationOptions::with_format`.

`rmcp` does no client-side JSON-Schema validation of tool arguments — there is no validator hook on
`Peer<RoleClient>`, unlike the TS SDK's `jsonSchemaValidator` client option. This subsystem is
therefore hand-written, not SDK-supplied.

#### 14 · Config write-back

All writers funnel through two functions and never touch a typed struct:

- `readRawConfigObject(filePath)`: missing ⇒ `{}`; parse failure ⇒ `{}` (silently — this is a
  *writer* helper, and clobbering an unparseable file is the accepted cost); a non-object root ⇒
  `{}`.
- `writeRawConfigObject(filePath, raw)`: `mkdirSync(dirname, {recursive:true})`, write
  `` `${JSON.stringify(raw, null, 2)}\n` `` (2-space indent, **trailing newline**) to
  `` `${filePath}.${process.pid}.tmp` ``, then `renameSync` — an atomic same-directory replace. There
  is **no file lock**; concurrency safety comes only from the rename. `cyrup-config`'s
  `FileSettingsStore` takes a cross-process advisory `FileLock` on its own files; do **not** adopt it
  here without sign-off, because it changes the concurrency contract and serialises against any other
  holder of that lock convention. `cyrup_permission_system::ext_config`'s `ExtensionConfig::save` is
  the closer in-tree precedent for a merge-preserving extension-config writer.
- `serializeRawConfig(raw)` is the same 2-space + `\n` serialisation, used for previews.
- `getServersObject`/`setServersObject`: read `mcpServers ?? mcp-servers`; on write, **delete
  `raw["mcp-servers"]` and always write `mcpServers`** — a hyphenated file is silently normalised on
  any write.

`buildConfigWritePreview` produces `{ path, existed, changed, beforeText, afterText, diffText }`.
**`beforeText` is not the file's bytes**: it is `serializeRawConfig(readRawConfigObject(path))`, i.e.
the *reserialised parse*. So comments, formatting and indent style are stripped from the "before"
side, an unparseable file previews as `{}`, and `changed` is computed against that normalised text. A
port that diffs the real file contents produces a different — and, for a commented `mcp.json`,
dramatically larger — preview, and under-reports what the write is about to do. `diffText` comes from
`buildUnifiedDiff` — a hand-rolled LCS (`(rows+1)×(cols+1)` DP table filled bottom-up) emitting
`--- before` / `+++ after` headers and `  `/`+ `/`- ` prefixed lines, or the literal `"(no changes)"`
when the texts are equal. Its tie-break (`lcs[i][j+1] >= lcs[i+1][j]` ⇒ prefer the addition) fixes
the diff shape, and the panel renders the text verbatim, so `similar` — which *is* a workspace
dependency, already consumed by `cyrup-tools` and `cyrup-test-support` — would produce different
(equally valid) hunks and a user-visible divergence. Port the DP.

Four typed writers sit on top:

1. **`writeProjectServerDisabledOverride(overridePath, cwd, serverName, disabled)`** — writes **only**
   the `disabled` field into `<cwd>/.pi/mcp.json` (`.cyrup` in the port), never a server definition
   or credentials. Reads the existing file with `parseJsonConfig` and **throws** on any structural
   problem, with these exact messages:
   `` `Failed to read project MCP override at ${filePath}: ${msg}` `` (with `{ cause }`; `msg` is
   `"root value must be an object"` for a non-object root),
   `` `Failed to update project MCP override at ${filePath}: ${serverKey} must be an object` ``,
   `` `Failed to update project MCP override at ${filePath}: server "${serverName}" must be an object` ``,
   `` `Failed to update project MCP override at ${filePath}: imports contains an unsupported config kind` ``.
   `serverKey` preserves whichever spelling the file already uses. Disabling writes
   `{ ...existing, disabled: true }`. **Enabling** deletes the `disabled` key and then re-merges
   *every other* source (skipping this file) plus this file's own `imports`, and writes an explicit
   `disabled: false` **only if the lower-precedence merge is itself disabled**. If the resulting
   object is empty the server key is deleted entirely. A no-op (`JSON.stringify` equality) returns
   `{ changed: false }` without writing.
2. **`writeDirectToolsConfig(changes, provenance, fullConfig)`** — groups changes by
   `provenance.path`; for `kind === "import"` it writes the **fully merged** definition plus
   `directTools` into the Pi-owned file, because the import source is not a Pi write target;
   otherwise it patches the existing entry in place.
3. **`ensureCompatibilityImports` / `previewCompatibilityImports`** — `Set`-dedup the `imports` array
   into the Pi global file; a no-op returns `{ added: [] }` without writing.
4. **`writeSharedServerEntry` / `previewSharedServerEntry`** and **`writeStarterProjectConfig` /
   `previewStarterProjectConfig`**, the latter writing `{ mcpServers: {} }` from
   `buildStarterProjectConfig` to `<cwd>/.mcp.json`.

`getServerProvenance(overridePath, cwd)` returns
`Map<serverName, { path, kind: "user"|"project"|"import", importKind? }>` where `path` is the
source's **`writePath`**, not its read path — so a shared global server's writes land in the Pi
global file. Host-discovered servers (when enabled) and per-file `imports` both map to
`{ path: userPath, kind: "import", importKind }`, and later sources overwrite earlier ones for direct
`mcpServers` keys while imports are first-wins.

#### 15 · Discovery, conflicts, fingerprint, RepoPrompt

`getMcpDiscoverySummary(overridePath, cwd, { includeHostConfigs = true })` returns the whole
setup-panel model: `sources` (one `ConfigDiscoverySource` per ladder entry with `exists` and
`serverCount`, built by `getConfigSourceSummaries`), `imports`, `hostConfigs` (imports tagged
`active: hostConfigDiscovery === "on"`), `hostConfigDiscovery`, `agentPlugins`, `conflicts`, four
booleans (`hasAnyConfig`, `hasAnyDetectedPaths`, `hasSharedServers`, `hasPiOwnedServers`),
`totalServerCount`, a `fingerprint`, and `repoPrompt`.

Two smaller public accessors feed the same panel and are separate entry points:
`getConfigDiscoveryPaths(overridePath, cwd)` maps the ladder to `{ label, path, exists }[]`
**without parsing any file** — cheap enough to call on every render, unlike `getConfigSourceSummaries`
which reads and validates each source — and `findAvailableImportConfigs(cwd)` returns
`{ kind, path }[]` for whichever of the 7 families resolves (via `resolveImportPath`, which *does*
parse and warns with `` `Failed to discover imported MCP config from ${kind}:` ``).

`fingerprint` is `JSON.stringify` over
`{ sources: [[id, exists, serverCount]…], imports: [[kind, path, serverCount]…],
agentPlugins: [[path, name, serverCount]…], hostConfigDiscovery, conflicts }` — a change-detection
key the panel polls. A Rust port must reproduce the *field order* of this object literal, since
`JSON.stringify` on an object literal is insertion-ordered and the value is compared as a string; use
`serde_json::Map` in the same key order, not a `BTreeMap`. (`getMcpStandardConfigSummary` has its own
narrower fingerprint with only `sources`.)

`getConfigConflicts` records host candidates **first** (lowest precedence), then per ladder source
records that source's `imports`-derived names as `kind:"host"` and its own `mcpServers` keys as
`kind: shared ? "shared" : "pi"`; a `(kind, path)` pair is recorded once. Any name with `> 1` source
becomes a conflict whose `winner` is `sources[sources.length - 1]` — last-recorded wins, matching the
merge. Output is sorted by `serverName.localeCompare`, for which the in-tree precedent is `feruca`
(already a workspace dependency, adopted for `ls` ordering parity).

`detectRepoPrompt` scans only **shared** sources with `serverCount > 0`; a server is RepoPrompt if
its name lowercased contains `repoprompt` or equals `rp`, or its command lowercased contains
`repoprompt`/`rp-mcp` or ends with `repoprompt_cli`, or any arg lowercased contains `repoprompt`.
Otherwise it probes `REPOPROMPT_BINARY_CANDIDATES = [~/RepoPrompt/repoprompt_cli,
/Applications/Repo Prompt.app/Contents/MacOS/repoprompt-mcp]` and proposes
`{ serverName: "repoprompt", entry: { command: executablePath, args: [], lifecycle: "lazy" } }`
written to `findProjectRoot(cwd)/.mcp.json` — where `findProjectRoot` walks up for `.git`,
`package.json`, `.mcp.json` or `.pi` — falling back to `~/.config/mcp/mcp.json`.

`KNOWN_SERVER_PRESETS` — 5 curated entries the setup panel offers. Each is a
`KnownServerPreset { id, name, summary, entry }` and **all four fields are user-visible**:

| `id` | `name` | `summary` | `entry` |
|---|---|---|---|
| `deepwiki` | `DeepWiki` | `Ask questions about public GitHub repositories.` | `{url:"https://mcp.deepwiki.com/mcp", protocolVersion:"auto"}` |
| `context7` | `Context7` | `Look up current library documentation and examples.` | `{url:"https://mcp.context7.com/mcp", protocolVersion:"auto"}` |
| `notion` | `Notion` | `Search and work with your Notion workspace.` | `{url:"https://mcp.notion.com/mcp", auth:"oauth", protocolVersion:"auto"}` |
| `github` | `GitHub` | `Work with GitHub through your Copilot account.` | `{url:"https://api.githubcopilot.com/mcp", auth:"oauth", protocolVersion:"auto"}` |
| `chrome-devtools` | `Chrome DevTools` | `Inspect and automate a local Chrome browser.` | `{command:"npx", args:["-y","chrome-devtools-mcp@1.6.0"]}` |

The `chrome-devtools` preset spawns a third-party Node MCP server over stdio via `npx`. That is
inherent to MCP, is what `cyrup_ext::caps::proc::npx_resolver` already exists to pre-resolve, and is
an external process — not a JS runtime inside cyrup.

#### 16 · Environment variables read by this subsystem

| variable | read at | semantics |
|---|---|---|
| `PI_CODING_AGENT_DIR` | `agent-dir.ts` `getAgentDir` | trimmed; `~` ⇒ home, `~/…` ⇒ `resolve(home, rest)`, else `resolve`; default `~/.pi/agent` |
| `PI_PACKAGE_DIR` | `agent-dir.ts` `readPiConfig` | manifest dir for `piConfig.name` / `piConfig.clientUri` |
| `MCP_DIRECT_TOOLS` | `index.ts`, `init.ts` | comma-split, trimmed, empties dropped; the sentinel `"__none__"` means *register no direct tools* and is checked as a raw string **before** splitting |
| `MCP_OUTPUT_GUARD` | `mcp-output-guard.ts` `envKillSwitch` | tri-state kill switch; outranks `settings.outputGuard` in **both** directions |
| `MCP_OAUTH_DIR` | `mcp-auth.ts` `getAuthBaseDir` | trimmed; outranks `settings.oauthDir`; legacy plaintext import dir only |
| `MCP_UI_DEBUG` | `logger.ts` module bootstrap | `"1"` or `"true"` ⇒ logger level `debug` |
| `BROWSER` | `init.ts` | passed as `browser` to `openUrl` |
| `MCP_OAUTH_CALLBACK_PORT` | `mcp-oauth-provider.ts` | fixed callback port |
| `NPM_CONFIG_CACHE` | `npx-resolver.ts` | npm cache root (already ported) |
| `MCP_UI_VIEWER`, `GLIMPSE_BINARY`, `SSH_CONNECTION`/`SSH_TTY` | `ui-session.ts`, `glimpse-ui.ts` | **cut (Cut 2)** |
| *(dynamic)* | `utils.ts` `interpolateEnvVars` | every `${VAR}` / `$env:VAR` / `{env:VAR}` in `args`, `env`, `cwd`, `url`, `headers`, `bearerToken` |

`MCP_OUTPUT_GUARD` and `MCP_OAUTH_DIR` are README-documented; `MCP_DIRECT_TOOLS` and `MCP_UI_DEBUG`
are undocumented but live. All four `MCP_*` names are **not** pi-branded and are preserved verbatim.
Renaming `PI_*` → `CYRUP_*` is the tree's convention: `mcp_direct_tools`'s `resolve_agent_dir`
already accepts `CYRUP_AGENT_DIR` then `PI_CODING_AGENT_DIR`, and
`cyrup_permission_system::ext_config` renames pi's config-path env key to
`CYRUP_PERMISSION_SYSTEM_CONFIG_PATH`. Two divergences in that existing `resolve_agent_dir` are fixed
by MCP-068: it filters on `!v.is_empty()` where upstream `.trim()`s first (so a whitespace-only value
becomes a literal path in cyrup and falls back to the default upstream), and its non-`~` arm is
`PathBuf::from(v)` where upstream is `resolve(configured)` (so a relative value is not made
absolute).

#### 17 · The `mcp-cache.json` identity contract

`computeServerHash` (`metadata-cache.ts`) builds an identity object out of 14 possibly-`undefined`
fields — `command, args, socket, env, cwd, url, headers, auth, protocolVersion, bearerToken,
bearerTokenEnv, exposeResources, includeTools, excludeTools`, with `url` taken from
`resolveServerUrl(definition)`, not raw — and hashes `stableStringify(identity)`. That function's
scalar branch emits the **literal nine-character string `undefined`** for an absent field (because
`JSON.stringify(undefined) === undefined` falls through), and `"null"` only for an explicit JSON
`null`. The resulting pre-image is not valid JSON; it is a deterministic byte sequence.

`cyrup_ext_subagents::exec::mcp_direct_tools` already **reads** this file — `CACHE_VERSION = 1`,
`CACHE_MAX_AGE_MS = 7 days`, `load_metadata_cache`, `is_server_cache_valid` — and already ports the
hash as `compute_mcp_server_hash`. Its version diverges on three counts: it inserts `Value::Null` for
absent fields and its `stable_stringify` renders `Value::Null` as `"null"`; it hashes only **11** of
the 14 keys (missing `socket`, `protocolVersion`, `includeTools`); and it uses the **raw**
`definition.url`. The `null`-vs-`undefined` difference alone means **no cyrup-written `configHash`
can match a v2.25.0-faithful writer's for any definition that leaves any of the 14 fields unset** —
which is every realistic definition. `cyrup-mcp` is the **writer** of a file that already has a
reader; the two must agree byte for byte or every `mcp:` subagent tool selector silently resolves to
nothing.

With Cut 3, `socket` leaves the identity object, taking the field count to 13. That is a *deliberate*
divergence from the upstream pre-image and must be recorded as one: a config that could carry a
`socket` no longer exists in the port, so no cyrup hash needs to encode it. `CACHE_VERSION` stays
`1` — do not bump it to drop the now-dead `uiResourceUri` / `uiStreamMode` fields from `CachedTool`;
leave them absent and ignored, because the schema is a live contract with an existing reader.

---

### Port units

**MCP-050 — Create `cyrup-mcp` and its config module skeleton** · n/a · M · `extension-owned`
**upstream** — `config.ts` is one module with 34 exported symbols plus ~20 private functions over a
shared `ConfigSourceSpec` ladder.
**behavior** — `loadMcpConfig(overridePath?, cwd)` returns a single merged `McpConfig` and never
throws; a broken file warns and is skipped.
**cyrup** — a new workspace member `crates/cyrup-mcp`, a native built-in in the shape of
`crates/cyrup-ext-subagents`: its own `Cargo.toml`, `#![forbid(unsafe_code)]` and the crate-level
`#![deny(clippy::{unwrap_used, expect_used, panic, indexing_slicing})]` that crate already carries.
Module layout `config::{sources, merge, imports, write, discovery}`, plus `types`, `naming`, `util`,
`error`, `log`, `schema`, `tools::ts_shape`.
**verify** — unit: `load_mcp_config` over a tempdir fixture reproducing all six ladder files returns
the documented winner per server.

**MCP-051 — Read `mcp.json` as JSONC, not JSON** · high · S · `extension-owned`
**upstream** — `config.ts` `parseJsonConfig` = `JSON.parse(stripJsonComments(raw, { trailingCommas:
true }))`, used by `readValidatedConfig`, `readRawConfigObject`,
`writeProjectServerDisabledOverride` and `readImportedConfig`.
**behavior** — a config with `//` comments or a trailing comma loads normally.
**cyrup** — `cyrup_permission_system::jsonc::{parse, parse_config, parse_config_into}` — a
string-aware comment/trailing-comma preprocessor then `serde_json`, all `pub`, and already the parser
`cyrup_permission_system::manager` uses on `mcp.json`. Reuse it; `cyrup-permission-system` does not
depend on `cyrup-mcp`, so there is no cycle. `cyrup_ext_subagents::exec::mcp_direct_tools`'s
`read_config` uses bare `serde_json::from_str` and must be repointed at the same parser (MCP-094), or
a commented config stays invisible to the subagent resolver.
**verify** — unit: the same `mcp.json` containing a `//` comment and a trailing comma yields
identical server sets from `cyrup-mcp`, from
`cyrup_permission_system::manager::read_configured_mcp_server_names`, and from
`mcp_direct_tools`'s resolver.

**MCP-052 — Port the six-source precedence ladder** · high · M · `hand-written`
**upstream** — `config.ts` `getConfigSources` emits 4–6 specs with per-entry dedup guards;
`loadMcpConfig` folds them left-to-right after an optional host-config base and before an
Agent-Plugin base.
**behavior** — a project `.pi/mcp.json` beats `.mcp.json` beats `<agent_dir>/mcp.json` beats
`~/.agents/mcp/mcp.json` beats `~/.agents/mcp.json` beats `~/.config/mcp/mcp.json` beats
host-discovered configs; Agent-Plugin servers lose to every file. Each spec also carries the
`kind`/`importKind`/`shared`/`scope`/`writePath` quintuple the panel and `getServerProvenance`
consume.
**cyrup** — a `ConfigSourceSpec { id, label, read_path, write_path, kind, import_kind, shared, scope }`
vector built by the identical guard sequence. `cyrup_ext_subagents::exec::mcp_direct_tools`'s
`get_config_paths` reads four of the six and already renames `.pi` → `.cyrup`; that rename is settled
and applies here. `<agent_dir>/mcp.json` is fixed by
`cyrup_permission_system::manager::read_configured_mcp_server_names`.
**verify** — conformance: a fixture defining the same server name in all six files asserts the winner
and the exact `ConfigDiscoverySource[]` for the panel.

**MCP-053 — Port `mergeServerMaps`, including URL-bound credential stripping** · critical · M · `hand-written`
**upstream** — `config.ts` `URL_BOUND_AUTH_FIELDS` + `mergeServerMaps`; the comment block above the
function names the credential-exfiltration threat.
**behavior** — a lower-trust source that repoints an existing server's `url` cannot inherit the
higher-trust source's `headers` / `bearerToken` / `bearerTokenEnv` / `oauth`. An explicit
`oauth: false` survives a URL change. Everything else is a shallow per-field spread with
`definition` last.
**cyrup** — an explicit `fn merge_entry(base: Option<&ServerEntry>, over: &ServerEntry) ->
ServerEntry`; `ServerEntry` is a typed struct with no `#[serde(flatten)]` catch-all so the deletion
set is exhaustive at compile time. `cyrup_ext_subagents::exec::mcp_direct_tools`'s `merge_configs`
replaces the whole entry with an `insert` — no per-field merge, therefore also no stripping — and is
part of MCP-094.
**verify** — unit, one case per rule: (a) a `url` change drops the three auth fields and `oauth`;
(b) `oauth: false` survives a `url` change; (c) an identical `url` inherits; (d) an override omitting
`url` inherits. Plus a compile-time exhaustiveness test asserting `URL_BOUND_AUTH_FIELDS` against the
struct's field list.

**MCP-054 — socket ⇄ command/url transport-swap stripping** · n/a · S · `cut`
**upstream** — `config.ts` `mergeServerMaps` rules 1 and 2: a `socket` override deletes ten fields; a
`command`/`url` override over an existing `socket` deletes `socket`.
**cut because** — Cut 3 removes the raw unix-socket transport, so `socket` is not a `ServerEntry`
field and neither rule has an input. Recorded so a later pass does not re-file the deletion sets as a
gap. **What the remaining merge must still do** is in MCP-053: rule 3 plus the shallow spread. The
`command` ⇄ `url` case was never covered by rules 1/2 and still produces a two-transport entry that
fails at connect, exactly as upstream.
**verify** — unit: `ServerEntry` has no `socket` field, asserted by the same field-list exhaustiveness
test MCP-053 uses for `URL_BOUND_AUTH_FIELDS`, so neither deleted rule can have an input; a
two-source fixture whose higher-precedence entry supplies `socket` is rejected at load with the named
Cut-3 diagnostic of §6 and leaves the base entry's `command`/`env`/`cwd` untouched rather than
silently dropping the key; and a base `{command}` overridden by `{url}` still merges to a
two-transport entry that fails at connect with the exactly-one-transport error.

**MCP-055 — Port `expandImports` / `mergeImports`** · medium · S · `hand-written`
**upstream** — `config.ts` `mergeImports` (concat + `Set` dedup, `undefined` when empty) and
`expandImports` (per-source, first-wins across kinds, folded beneath that source's own `mcpServers`).
**behavior** — `imports: ["cursor"]` in the global file makes Cursor's servers visible but
overridable by any later ladder source.
**cyrup** — `cyrup_ext_subagents::exec::mcp_direct_tools`'s `expand_imports` already implements
exactly this shape (first-wins via `entry().or_insert`, then project servers over imports) for 6 of
the 7 kinds. Port the same shape into `cyrup-mcp` and add `opencode`.
**verify** — unit: a source with `imports:["cursor","claude-code"]` where both define `foo` resolves
to Cursor's `foo`.

**MCP-056 — Port the 7 host-config import families** · medium · M · `hand-written`
**upstream** — `config.ts` `IMPORT_PATHS`, `resolveImportCandidates`, `loadImportedConfig`,
`extractServers`, `translateCodexServer`.
**behavior** — the family table in §4: candidate order, per-family server key, and the Codex field
mapping (`bearer_token_env_var` → `bearerTokenEnv` with `auth ??= "bearer"`; `http_headers` merged
over `headers`; `env_http_headers` → `headers[h] ??= "$env:VAR"`; the three snake_case keys deleted).
**cyrup** — plain Rust. The Codex `config.toml` path needs `toml`, which is in the tree as a direct
dependency of `cyrup-resources` but **not** in `[workspace.dependencies]`; promote it. The in-tree
`ImportKind` in `mcp_direct_tools` has 6 kinds and only the `mcpServers`/`mcp-servers` key split.
**verify** — conformance: one fixture per family asserting the extracted `ServerEntry` map, including
both Codex remaps.

**MCP-057 — Port the `opencode` multi-file merge and entry translation** · medium · M · `hand-written`
**upstream** — `config.ts`'s git-root walk inside `resolveImportCandidates`, the merge-all branch in
`loadImportedConfig`, `mergeOpenCodeConfigs`, and the local/remote translation in `extractServers`.
**behavior** — OpenCode is the only family whose candidates are merged rather than first-wins, and
the only one whose entries are re-shaped (`command: string[]` → `command` + `args`; `type:"remote"`
with an `oauth` object → `auth:"oauth"` + a projected `OAuthConfig`). `enabled: false` skips the
entry.
**cyrup** — `mergeOpenCodeConfigs` repeats MCP-053's credential-unbinding discipline on a *different*
schema (a `type` change deletes 6 fields; a `url` change deletes `headers` + `oauth`; a `command`
array change deletes `environment` + `cwd`), so it needs its own typed merge, not a generic JSON
deep-merge. Absent from the in-tree `ImportKind` entirely.
**verify** — unit: global + project `opencode.json` where the project changes `url` asserts the
global `headers` are dropped; the git-root walk is covered by a tempdir with a nested `.git`.

**MCP-058 — Port `hostConfigDiscovery` and `loadDiscoveredHostConfigs`** · medium · S · `hand-written`
**upstream** — `config.ts` `getMergedSettings`, `getHostConfigDiscovery`,
`loadDiscoveredHostConfigs`, and the base-layer fold inside `loadMcpConfig`.
**behavior** — with `"off"` (the default) no host config is read for merging at all; `"prompt"`
detects but does not load; `"on"` loads all 7 families at lowest precedence.
**cyrup** — plain Rust over MCP-056.
**verify** — unit: `"off"` yields zero host servers even when all 7 files exist; `"on"` yields them
but a same-named ladder server wins.

**MCP-059 — Port `getMcpDiscoverySummary`, conflicts and the fingerprint** · medium · M · `hand-written`
**upstream** — `config.ts` `getConfigSourceSummaries`, `getMcpStandardConfigSummary`,
`getMcpDiscoverySummary`, `getConfigConflicts`.
**behavior** — the setup panel's whole model, plus a `fingerprint` string the panel polls for change
detection; conflicts are sorted by `localeCompare` with `winner = sources[last]`.
**cyrup** — the fingerprint is a `JSON.stringify` of an object literal and is therefore
insertion-ordered; reproduce it with `serde_json::Map` in the same key order
(`sources, imports, agentPlugins, hostConfigDiscovery, conflicts`) and `serde_json::to_string`, not a
`BTreeMap`. `localeCompare` has an in-tree precedent: `feruca`, already a workspace dependency,
adopted for `ls` ordering parity.
**verify** — unit: a fixture asserts the exact fingerprint string, then asserts it changes when one
server is added and not when an unrelated file is touched.

**MCP-060 — Port RepoPrompt detection and `KNOWN_SERVER_PRESETS`** · low · S · `hand-written`
**upstream** — `config.ts` `REPOPROMPT_BINARY_CANDIDATES`, `KnownServerPreset`,
`KNOWN_SERVER_PRESETS`, `isRepoPromptServer`, `findProjectRoot`, `buildRepoPromptEntry`,
`detectRepoPrompt`.
**behavior** — `/mcp setup` offers 5 curated servers and a one-key RepoPrompt add whose target is the
nearest project root's `.mcp.json`, else `~/.config/mcp/mcp.json`.
**cyrup** — `KnownServerPreset` carries `{ id, name, summary, entry }` and the panel renders all
four, so the port carries the display `name` and `summary` strings verbatim (§15 table), not just the
`entry`.
**verify** — unit: preset table equality including `name`/`summary`; `findProjectRoot` over a tempdir
with each of the four markers.

**MCP-061 — Port the atomic raw-config writer** · high · S · `extension-owned`
**upstream** — `config.ts` `serializeRawConfig`, `readRawConfigObject`, `writeRawConfigObject`,
`getServersObject`/`setServersObject` — 2-space `JSON.stringify` + `"\n"`, write to
`${path}.${pid}.tmp`, `renameSync`; `setServersObject` always normalises `mcp-servers` → `mcpServers`.
**behavior** — a crash mid-write never leaves a truncated `mcp.json`; unknown top-level keys survive
a write because the writer operates on the raw parsed object.
**cyrup** — `std::fs` directly: a native crate writes its own files. Literal `<path>.<pid>.tmp` +
`rename`. Do **not** adopt `cyrup-config`'s `FileSettingsStore` advisory `FileLock` without sign-off;
upstream takes no lock and adding one changes the concurrency contract.
`cyrup_permission_system::ext_config`'s `ExtensionConfig::save` is the in-tree precedent for a
merge-preserving extension-config writer.
**verify** — unit: a write preserves an unknown `"$schema"` key; a hyphenated `mcp-servers` becomes
`mcpServers`; no `.tmp` file remains.

**MCP-062 — Port `buildUnifiedDiff` (LCS) and `ConfigWritePreview`** · low · S · `hand-written`
**upstream** — `config.ts` `buildUnifiedDiff` and `buildConfigWritePreview` — a bottom-up
`(rows+1)×(cols+1)` LCS DP table with an addition-preferring tie-break, `--- before` / `+++ after`
headers, and the literal `"(no changes)"`.
**behavior** — `/mcp setup` shows the exact before/after diff before any write.
**cyrup** — port the DP literally rather than reusing `similar` (which *is* a workspace dependency,
already consumed by `cyrup-tools` and `cyrup-test-support`): the tie-break fixes which side a change
is attributed to, and the panel renders the text verbatim, so a different — equally correct — hunk
shape is a user-visible divergence. Using `similar` here would be a mechanism substitution, not a
dependency saving.
**verify** — unit: golden-text assertions on four fixtures (insert-only, delete-only, replace, and a
commented source file whose preview is a full rewrite).

**MCP-063 — Port `writeProjectServerDisabledOverride`** · high · M · `hand-written`
**upstream** — `config.ts` `writeProjectServerDisabledOverride` and its
`ServerDisabledOverrideResult`.
**behavior** — `/mcp` toggling a server's enabled state writes **only** `disabled` into the project
override file and never copies a definition or its credentials there. Enabling re-merges every other
source to decide whether an explicit `disabled: false` is needed. Four exact error strings (§14).
**cyrup** — plain Rust over MCP-061 and MCP-052. The "never copy credentials into the project file"
property is the whole point: an implementation that writes the merged entry would leak a global
`bearerToken` into a repo-visible file.
**verify** — unit: disable-then-enable round-trips to no `disabled` key when nothing lower is
disabled, and to `disabled: false` when a lower source is; assert the file never gains a
`bearerToken`; assert each of the four error strings byte for byte.

**MCP-064 — Port `getServerProvenance` and `writeDirectToolsConfig`** · medium · M · `hand-written`
**upstream** — `config.ts` `getServerProvenance` and `writeDirectToolsConfig` — provenance maps a
server name to its source's **`writePath`**; an `import`-provenance change writes the fully merged
definition plus `directTools` into the Pi-owned file.
**behavior** — toggling direct-tools in `/mcp` persists to the file the panel says it will, and an
imported server's toggle materialises a Pi-owned copy rather than editing Cursor's config.
**cyrup** — plain Rust over MCP-061 and MCP-052.
**verify** — unit: toggling a `shared-global` server writes to `<agent_dir>/mcp.json`, not
`~/.config/mcp/mcp.json`; toggling an imported server materialises the merged definition.

**MCP-065 — Port `ensureCompatibilityImports`, starter config and shared-entry writers** · low · S · `hand-written`
**upstream** — `config.ts` `previewCompatibilityImports`/`ensureCompatibilityImports`,
`buildStarterProjectConfig`, `previewStarterProjectConfig`/`writeStarterProjectConfig`,
`previewSharedServerEntry`/`writeSharedServerEntry`.
**behavior** — the init path and `/mcp setup` add missing `imports` idempotently (no write when
nothing is added) and scaffold `{ "mcpServers": {} }`.
**cyrup** — plain Rust over MCP-061.
**verify** — unit: a second `ensure` call returns `added: []` and does not touch the file's mtime.

**MCP-066 — Port `McpSettings` as a permissive struct with per-site defaults** · high · M · `hand-written`
**upstream** — `types.ts` `McpSettings` — 23 keys; defaults enforced at the read sites tabulated in
§5.
**behavior** — every key's default is the *predicate at its read site*, not a parse-time default:
`notifyOnStartupConnect` is `!== false`, `disableProxyTool` is `!== true`, `idleTimeout` is
`typeof === "number" ? v : 10` (so `0` disables), `requestTimeoutMs` needs `> 0` to take effect,
`collapsedResultLines` is a `1|2|3` whitelist falling back to 3 (boxed) / 1 (compact).
**cyrup** — `Option<T>` fields plus accessor methods named for the read site
(`fn notify_on_startup_connect(&self) -> bool { self.notify_on_startup_connect != Some(false) }`), so
the predicate lives in one place. `validateConfig` does not validate settings, so a type mismatch
must degrade to `None`, not fail the file — a permissive `deserialize_with` per field, or parse from
a `Value`. `scriptMode` is not modelled (Cut 4). `cyrup_ext_subagents::exec::mcp_direct_tools`'s
`McpSettings` models one key (`toolPrefix`).
**verify** — unit: a table test over all 22 live keys asserting each key's resolved value for
`absent`, `null`, wrong-type, and both boolean settings.

**MCP-067 — Port the settings merge as a one-level key merge** · medium · S · `hand-written`
**upstream** — `config.ts` `mergeConfigs` and `getMergedSettings` — `{ ...base.settings,
...next.settings }`, one level only.
**behavior** — a project file setting `settings.trace.enabled` **replaces** the whole global `trace`
object; it does not merge into it. Same for `outputGuard`'s object form.
**cyrup** — per-field `over.x.clone().or(base.x.clone())` on `McpSettings`; explicitly **not**
`cyrup_config::settings`'s `deep_merge`, which recurses into objects.
`cyrup_ext_subagents::exec::mcp_direct_tools`'s `merge_configs` does
`next.settings.or(base.settings)` — a wholesale replacement, so a project file setting only
`toolPrefix` would discard every global setting (MCP-094).
**verify** — unit: global `{trace:{enabled:true,maxBytes:1}}` + project `{trace:{enabled:false}}`
yields `{enabled:false}` with **no** `maxBytes`.

**MCP-068 — Port env-var overrides, including the `__none__` sentinel** · high · S · `hand-written`
**upstream** — `index.ts`, `init.ts`, `mcp-output-guard.ts` `envKillSwitch`, `mcp-auth.ts`
`getAuthBaseDir`, `logger.ts` bootstrap, `agent-dir.ts` `getAgentDir`/`readPiConfig`.
**behavior** — the §16 table. `MCP_DIRECT_TOOLS="__none__"` registers no direct tools and is tested
as a raw string **before** the comma split; `MCP_OUTPUT_GUARD` is tri-state and outranks config in
both directions.
**cyrup** — the four `MCP_*` names stay verbatim; `PI_CODING_AGENT_DIR` gains a `CYRUP_AGENT_DIR`
alias checked first, matching `cyrup_ext_subagents::exec::mcp_direct_tools`'s `resolve_agent_dir` and
`cyrup_permission_system::ext_config`. Fix that function's two divergences in the same change: it
filters on `!v.is_empty()` where upstream `.trim()`s first, and its non-`~` arm is `PathBuf::from(v)`
where upstream is `resolve(configured)`.
**verify** — unit with an injected env lookup (the crate cannot mutate process env under edition
2024): each variable's tri-state or sentinel behaviour, plus a whitespace-only and a relative
`CYRUP_AGENT_DIR`.

**MCP-069 — Port `ServerEntry` as a typed struct** · high · M · `hand-written`
**upstream** — `types.ts` `ServerEntry` (28 fields; **v2.26.1: 29**, adding `requestHeadersCommand` —
already ported, see the retarget section of `13-cyrup-mcp.md`), `OAuthConfig` (10), `isServerDisabled`, the
`ServerDefinition` alias.
**behavior** — the §6 table, including the "only literal `true` disables" rule and the connect-time
validation errors (three of them in `resolveServerUrl` alone).
**cyrup** — `#[serde(rename_all = "camelCase")]`, every field `Option<T>` with
`#[serde(default, skip_serializing_if = "Option::is_none")]`, no `#[serde(flatten)]` catch-all
(MCP-053). `auth` and `oauth` need `#[serde(untagged)]` enums whose `false` arm is a `bool`, not a
unit variant. **26 live fields**: `socket` is not modelled (Cut 3) and `httpTransport` is a
one-variant enum (Cut 1). Both cut shapes must be **rejected at load with a named diagnostic**, not
silently dropped — `agent-plugin-loader.ts` sets `httpTransport` straight from a manifest's
`type: "sse"`, so a plugin declaring it is a live case that would otherwise appear configured and
never connect. The exactly-one-transport message loses `, or socket`.
`cyrup_ext_subagents::exec::mcp_direct_tools`'s `ServerEntry` models 11 of the 28.
**verify** — unit: round-trip a fixture with all 26 live fields; a `socket` entry and an
`httpTransport: "sse"` entry each produce the named diagnostic; a two-transport entry parses but
fails the connect-time check with the exact message.

**MCP-069a — Fail CLOSED on a malformed `requestHeadersCommand`** · critical · S · `hand-written` + `open-decision`
*Filed 2026-08-20 by the v2.25.0 → v2.26.1 retarget. NOT implemented.*
**upstream** — `request-headers-command.ts:153-190` `resolvedCommand` throws **five** sentences. It is
called twice: **eagerly** by `createRequestHeadersCommandFetch` at connect (`:309`, comment
"Validate static configuration before the first request") and again per request (`:196`). The eager
call is the one that matters here — a malformed block fails the **connection**, so the server never
exists to send an unsigned request. The five sentences: `"HTTP request headers command must be an object"` (non-object, `null`, or an
array, `:160`), `"… requires a non-empty command"` (`:163`), `"… args must be strings"` (`:166`),
`"… env values must be strings"` (`:173`), `"… timeoutMs must be an integer between 1 and 60000"`
(`:177`).
**behavior** — this is the one module in the adapter whose entire contract is fail-closed. A user who
writes `"requestHeadersCommand": "sign.sh"` (a string, not an object) has asked for every request to
be signed and got the shape wrong. Upstream refuses. **The port connects the server unsigned.**
`crate::config::lenient` degrades any wrong-typed field to `None`, so the whole block vanishes at
parse time, `resolve_request_headers_command` is never reached, and only **two** of upstream's five
sentences have any input at all — the crate's own test is named
`the_two_reachable_configuration_throws_carry_upstreams_sentences`. Same for a non-array `args`, a
non-object `env`, and a non-numeric `timeoutMs`.
**cyrup** — the fix is not "stop using `lenient` here": this file's rule 4 (a malformed value must
not take the whole config down) is deliberate and correct for every *other* field, and an entry that
refuses to *parse* would take its 28 siblings with it. The shape that satisfies both is a
hand-written `Deserialize` for `HttpRequestHeadersCommand` that **never errors** and instead records
the defect — `defect: Option<&'static str>`, `#[serde(skip)]`, carrying the exact upstream sentence —
which `resolve_request_headers_command` raises as its first check, at connect, where upstream throws.
`ServerEntry.request_headers_command` must then deserialize a present-but-non-object value into
`Some(defect)` rather than `None`, which is the whole point and the part `lenient` cannot express.
**Two rulings the port owner must make before this is built**, which is why it is `open-decision` and
not a patch: (1) does rule 4 get a named exception for fail-closed fields, or does this field get a
bespoke reader; (2) `computeServerHash`'s pre-image renders the *malformed* value upstream
(`stableStringify` emits `"sign.sh"`) and `undefined` here — unobservable while the entry refuses to
connect, but it is a hash divergence and should be recorded either way (MCP-070).
**verify** — unit: each of `"requestHeadersCommand": "sign.sh"`, `[]`, `{"args": 3}`,
`{"env": {"K": 1}}` parses without taking the file down, and each raises upstream's byte-exact
sentence from `resolve_request_headers_command`; the sibling entries in the same file still load.
cyrup-it: a server so configured reports the sentence and **does not connect**, rather than
connecting and sending unsigned requests.
*Blocked-by:* nothing. Observable end to end only once MCP-115a lands, but the parse-and-raise halves
are testable today.

**MCP-070 — Enforce the absent-vs-null hash pre-image contract** · high · M · `hand-written`
**upstream** — `metadata-cache.ts` `computeServerHash` + `stableStringify`: 14 identity fields, `url`
via `resolveServerUrl`, and the literal string `undefined` for absent (§17).
**behavior** — the `configHash` written into `mcp-cache.json` is a SHA-256 over a pre-image that is
not valid JSON. Any writer disagreeing on this byte for byte produces caches the reader rejects,
which manifests three subsystems away as "direct tools silently disappeared" and as `mcp:` subagent
selectors resolving to nothing.
**cyrup** — `cyrup_ext_subagents::exec::mcp_direct_tools`'s `compute_mcp_server_hash` inserts
`Value::Null` for absent fields, its `stable_stringify` renders that as `"null"`, it hashes 11 keys
not 14, and it uses the raw `definition.url`. **Blocking**: no cache writer can ship until this is
settled, because both crates read the same file. Three options — (a) write the 11-field/`null` hash
and upgrade the consumer later; (b) upgrade `mcp_direct_tools` in the same change to emit `undefined`
for absent fields, hash the (post-Cut-3) 13 keys, and resolve `url`; (c) write both hashes.
**Recommend (b)**: it is the only option that leaves the tree self-consistent *and* upstream-faithful
modulo the recorded `socket` divergence, and it touches one file outside `cyrup-mcp` — an extension
crate, not cyrup's core.
**verify** — conformance: a golden vector — a fixed `ServerEntry` and its exact 64-hex `configHash`,
generated by running the TypeScript at v2.25.0 with `socket` unset — asserted by both `cyrup-mcp`'s
writer and `mcp_direct_tools::compute_mcp_server_hash`.

**MCP-071 — Port `ToolPrefix` with all four modes and `sanitizeServerPrefix`** · high · S · `hand-written`
**upstream** — `types.ts` `ToolPrefix`, `sanitizeServerPrefix`, `getServerPrefix` — four modes;
`sanitizeServerPrefix` **preserves** `-` and hex-escapes anything else as `_<lowercase-hex>_` per
code point; `"mcp"` mode prefixes `mcp__`.
**behavior** — a server named `my-server` yields tool `my-server_do_thing`, not `my_server_do_thing`;
a server named `a.b` yields `a_2e_b_do_thing`.
**cyrup** — `enum ToolPrefix { Server, None, Short, Mcp }` with a `FromStr` mapping unknown strings to
`Server` (matching `?? "server"`), plus a code-point-wise
`sanitize_server_prefix(name, preserve_provider_valid: bool)` over `chars()` (Rust `chars()` is code
points, matching `Array.from`). `cyrup_ext_subagents::exec::mcp_direct_tools` has three modes, folds
`"mcp"` into `Server`, and does `replace('-', "_")` — matching neither upstream's current form nor
its legacy `_2d_` form.
**verify** — conformance: a table of (server name, mode) → prefix covering `-`, `.`, a non-ASCII and
an astral code point, an empty `short` result, and all four modes; asserted identically by
`cyrup-mcp` and `mcp_direct_tools`.

**MCP-072 — Port `formatToolName` / `resolveToolPrefix`** · high · S · `hand-written`
**upstream** — `types.ts` `formatToolName` and `resolveToolPrefix` — the tool name is sanitised
`.` → `_` **only**; `-` is preserved. Per-server `toolPrefix` beats `settings.toolPrefix` beats
`"server"`.
**behavior** — an MCP tool literally named `browser.navigate` on server `chrome-devtools` becomes
`chrome-devtools_browser_navigate`.
**cyrup** — pure functions over MCP-071. `cyrup_ext_subagents::exec::mcp_direct_tools`'s
`format_tool_name` applies no tool-name sanitisation and has no per-server override.
**verify** — unit: `format_tool_name("browser.navigate", "chrome-devtools", Server) ==
"chrome-devtools_browser_navigate"`.

**MCP-073 — Port `resolveServerFromToolName` with its ambiguity fail-safe** · high · S · `hand-written`
**upstream** — `types.ts` `resolveServerFromToolName` — longest matching prefix wins; **if any other
configured server shares that exact prefix string, return `undefined`**.
**behavior** — a policy gate resolving `foo_search` when both `foo` and `foo-mcp` exist under `short`
mode gets `undefined` and falls back to its wildcard path, rather than enforcing a rule against the
wrong server. This is a fail-safe, not an optimisation. It has **zero in-package callers** at
v2.25.0 — it is a `pub` API whose doc comment names downstream policy systems as its audience.
**cyrup** — a `pub` function on `cyrup-mcp`. `cyrup_permission_system::manager` has
`parse_qualified_mcp_tool_name` for the `<server>_<tool>` / `<server>:<tool>` forms and
`create_mcp_permission_targets` for the target vector, but no configured-prefix-set inverse; wiring
the two is a separate, deliberate decision, not part of this port.
**verify** — unit: servers `{foo, foo-mcp}` under `Short` yield `None` for `foo_search`; under
`Server` they yield `foo` and `foo-mcp` distinctly.

**MCP-074 — Port `sanitizePromptName` / `formatPromptCommandName`** · medium · S · `hand-written`
**upstream** — `types.ts` `sanitizePromptName` and `formatPromptCommandName` —
`mcp__<serverPart>__<sanitizedPrompt>`, where `serverPart` falls through `getServerPrefix` →
`sanitizeServerPrefix` → `"server"`.
**behavior** — an MCP prompt becomes the slash command `/mcp__agent_board__create_plan`.
**cyrup** — pure functions over MCP-071; nothing about them depends on a host surface. The *later*
question — registering a prompt command after `init` — is the seam map's P-4 and belongs to the
prompts section, not here; it is not a blocker on this unit.
**verify** — unit: a table over prompt names with leading digits, all-punctuation, and empty.

**MCP-075 — Port `getToolNameCandidates` (the legacy candidate set)** · high · M · `hand-written`
**upstream** — `types.ts` `getLegacyServerPrefix`, `formatLegacyToolName`, `getToolNameCandidates` —
**5 current-form + 13 legacy = 18 insertions** into a `Set`. Deduped size is data-dependent: 3 for
`list_sims`@`xcodebuild`, 7 for `browser.navigate`@`chrome-devtools`, 12 for
`get-code.map`@`figma-mcp`.
**behavior** — a user's `excludeTools: ["figma_get_code_connect_map"]` written before the naming
change keeps matching after it.
**cyrup** — pure functions over MCP-071/072. `cyrup_ext_subagents::exec::mcp_direct_tools` builds 4
`-`-normalised candidates, so a legacy `excludeTools` entry silently stops excluding.
**verify** — conformance: assert the exact candidate **set contents** (not a cardinality) for at
least the three (tool, server) pairs above, plus the `-`-free case where legacy and current collapse
to 3 names.

**MCP-076 — Port glob matching and `isToolIncluded`/`isToolExcluded`/`isToolAllowed`** · high · M · `hand-written`
**upstream** — `types.ts` `globToRegExp`, `matchesToolPattern`, `matchesToolSelector`,
`isToolIncluded`, `isToolExcluded`, `isToolAllowed` — escape `[.+^${}()|[\]\\]`, `* → .*`, `? → .`,
anchored; a glob-free pattern is a `Set.has` lookup; `matchesToolSelector` prefers current-form
candidates and only consults legacy-only candidates that do not also match a sibling tool.
**behavior** — `includeTools: ["get_*", "dokploy_list_apps"]` filters direct tools, proxy
search/list/describe **and** the `/mcp` panel identically.
**cyrup** — port `globToRegExp` literally onto `regex` rather than `globset` (which *is* a workspace
dependency but treats `/` as a path separator and gives `*` different semantics across it — wrong for
tool names). `regex` is **not** in `[workspace.dependencies]`; it reaches the tree only as a direct
dependency of `cyrup-permission-system`. Promote it — a one-line Cargo chore, not a host concern.
Rust's `regex` has a linear-time matching guarantee, so the compiled pattern needs an explicit
`RegexBuilder::size_limit`/`dfa_size_limit` rather than the default, and a compile failure surfaces as
the same "invalid pattern" outcome upstream produces. `cyrup_ext_subagents::exec::mcp_direct_tools`
supports only exact `-`-normalised matches with no glob support at all.
**verify** — conformance: a table of (pattern, candidate) pairs including `.`, `+`, `[`, `?`, and a
pattern containing both `*` and a literal `.`.

**MCP-077 — Port the metadata/cache type model** · high · S · `hand-written`
**upstream** — `types.ts` `ToolMetadata`, `PromptMetadata`, `DirectToolSpec`, `ServerProvenance`,
`CachedTool`, `CachedResource`, `CachedPrompt`, `ServerCacheEntry`, `MetadataCache`.
**behavior** — `ServerCacheEntry` carries `configHash`, `tools`, `resources`, `prompts?`,
`instructions?`, `cachedAt`; `CachedTool` carries `description`, `inputSchema` and the UI fields
beyond the bare name.
**cyrup** — `cyrup-mcp` is the **writer**; `cyrup_ext_subagents::exec::mcp_direct_tools` is an
existing **reader** modelling a strict subset with no `deny_unknown_fields`, so extra members
round-trip harmlessly as long as the writer emits them. `version` must stay `1` or the whole file is
treated as absent, and entries older than 7 days are skipped by the reader.
`DirectToolSpec.uiResourceUri`/`.uiStreamMode` and `CachedTool`'s UI fields become dead under Cut 2
but **stay in the on-disk schema, absent and ignored** — do not renumber `CACHE_VERSION`.
**verify** — conformance: a cache file written by `cyrup-mcp` deserialises in
`mcp_direct_tools::load_metadata_cache` and yields the expected tool names.

**MCP-078 — Port the status-snapshot types** · medium · S · `extension-owned`
**upstream** — `types.ts` `MCP_STATUS_EVENT = "pi-mcp-adapter/status/v1"`,
`MCP_STATUS_SNAPSHOT_VERSION = 1`, `McpServerRuntimeStatus` (6 variants), `McpServerStatusSnapshot`,
`McpStatusSnapshot`.
**behavior** — a versioned read-only snapshot; `McpServerRuntimeStatus` is a closed 6-variant enum
(`connected | cached | failed | needs-auth | not-connected | disabled`); per-server `resourceCount`
and `failedAgoSeconds` are optional and **omitted rather than nulled**.
**cyrup** — the types are plain `serde` structs in `cyrup-mcp`. The snapshot is published in-crate on
a `tokio::sync::watch`, not on a bus: `cyrup-mcp` owns both ends and **cyrup has no consumer for an
MCP status event**, so building a bus route would be a dead primitive. There is consequently no
channel string to rename.
**verify** — unit: snapshot serialisation asserts absent optional fields are omitted, not `null`.

**MCP-079 — Port the tool-approval decision and origin types** · medium · S · `hand-written`
**upstream** — `types.ts` `MCP_TOOL_APPROVAL_REQUEST_EVENT`, `McpToolApprovalOrigin`,
`McpToolApprovalDecision`, `McpToolApprovalHandler`, `McpToolApprovalRequest` — the request carries a
`claim(handler): boolean` callback so the **first synchronous claim wins**.
**behavior** — decisions are `allow_once | allow_for_session | deny | abstain`; origins tag where a
call entered.
**cyrup** — `McpToolApprovalDecision` and `McpToolApprovalOrigin` port as plain enums for the *local*
gate (`tool-approval.ts`'s `approveTools` matching + session cache, section 06). `McpToolApprovalOrigin`
reduces to `proxy | direct | resource`: `script` goes with Cut 4 and `iframe` with Cut 2. The
**broker event does not port and does not need to**: `ExtHooks::before_tool_call` is structurally the
same gate, is already wired for every surviving origin, fails closed
(`EventKind::ToolCall::fails_closed()`), and `cyrup_permission_system::manager`'s
`create_mcp_permission_targets` already derives MCP targets from the `mcp` tool's arguments. cyrup's
`SharedBus` is JSON-payload-only and deferred, so a closure could not cross it in any case.
**verify** — unit: the origin and decision enums round-trip; a `script`/`iframe` origin string is
rejected by the deserializer rather than silently accepted.

**MCP-080 — MCP-UI type surface in `types.ts`** · n/a · S · `cut`
**upstream** — `types.ts`'s `UiResourceMeta` … `UiDisplayModeResult` block, including the re-exports
from `ui-stream-types.ts` / `ui-tool-visibility.ts` and the three functions `extractUiPromptText`,
`parseUiPromptHandoff` (intent must match `/^[A-Za-z][A-Za-z0-9_-]*$/`, payload must be a non-array
object) and `createUiModelContextUpdate` (`maxChars = 12_000`, truncation appends `…`) — roughly a
quarter of the file.
**cut because** — Cut 2 removes MCP Apps entirely. These types describe the iframe bridge protocol
and the `ui://` resource envelope, both of which have no remaining consumer. The browser-side
`app-bridge.bundle.js` they describe was page content served into an iframe, never a cyrup runtime,
so nothing about this cut is a JS-engine question.
**seam** — `ui-tool-visibility.ts` is **split**, not cut: `extractUiToolVisibility` and
`isUiToolVisibleToModel` are kept, because `direct-tools.ts` and `metadata-cache.ts` use them to hide
tools a server marked app-only from the model, and cutting them would expose those tools. Their type
(`_meta.ui.visibility: ("model" | "app")[]`) therefore survives into `cyrup-mcp`; that is section
05's unit, not this one's. `isUiToolCallableByApp` is cut with no caller.
**verify** — conformance: a server advertising a `ui://` resource and a tool carrying
`_meta.ui.resourceUri` connects, lists and calls normally with **no** session, no registration and no
error — the cut surface is inert, not a failure path — while the surviving seam still holds
(`_meta.ui.visibility: ["app"]` hides the tool from the model, `["model","app"]` exposes it). Plus a
unit test over `cyrup-mcp`'s public items asserting no bridge-protocol or `ui://`-envelope type is
exported, so the quarter-file cut cannot be half-restored.

**MCP-081 — Port `McpAdapterOptions` / programmatic config mode** · medium · S · `hand-written`
**upstream** — `types.ts` `McpAdapterOptions`; `config.ts` `cloneMcpConfig`; `index.ts`'s
constructor branch.
**behavior** — a supplied `config` is a **complete isolated snapshot**: not merged with files,
imports, global/project config or `--mcp-config` (`index.ts` forces the early config path to
`undefined`), never mutated, and each factory/session gets its own `structuredClone`. With
`configPath` and no `config`, normal file merge applies and that path outranks argv.
**cyrup** — `#[derive(Clone)] McpConfig` plus a constructor variant that takes an owned config and
skips discovery entirely.
**verify** — unit: a programmatic config plus an on-disk `~/.config/mcp/mcp.json` yields only the
programmatic servers.

**MCP-082 — Port `interpolateEnvVars` including the `{env:VAR}` form** · high · S · `hand-written`
**upstream** — `utils.ts` `interpolateEnvVars` and the private `getMissingEnvVars` — **three**
syntaxes in a fixed order (`${VAR}`, `$env:VAR`, `{env:VAR}`); missing ⇒ `""`; `getMissingEnvVars`
uses one combined alternation and is used only by `resolveServerUrl`.
**behavior** — a header written `{env:TOKEN}` resolves. The pass order matters: an expanded value can
itself contain a later-form placeholder.
**cyrup** — a three-pass implementation in `cyrup_mcp::util::interp`, taking an injected lookup so it
is testable without mutating process env (edition 2024 makes `std::env::set_var` `unsafe`). Both
existing in-tree copies — `cyrup_ext::caps::proc`'s `interpolate_env_vars` and `mcp_direct_tools`'s —
implement only the first two forms, and the former is `pub(crate)` so `cyrup-mcp` cannot call it
regardless. Owning the three-form implementation in `cyrup-mcp` and fixing the drift in
`mcp_direct_tools` (MCP-094) is the right split; `cyrup-ext`'s copy serves its own `proc` capability
and can be aligned separately.
**verify** — unit with an injected lookup: all three forms, a missing var, an unterminated `${`, and
a `${}` with a non-word name (emitted verbatim).

**MCP-083 — Port `!` / `!!` command-secret resolution** · critical · M · `extension-owned`
**upstream** — `utils.ts` `interpolateSecretExpression`, `interpolateEnvRecord`,
`COMMAND_SECRET_TIMEOUT_MS`/`COMMAND_SECRET_MAX_OUTPUT_BYTES`, `resolveCommandSecret`,
`resolveCommandSecretsRecord`.
**behavior** — a value beginning with `!` runs a **shell** command at connect/auth time only — never
during discovery, merge, preview, hashing or rendering. `!!` escapes a literal `!` and *is*
interpolated. `spawnSync(cmd, { shell: true, timeout: 10_000, maxBuffer: 1_048_576,
stdio: ["ignore","pipe","ignore"], windowsHide: true })`; stdout trimmed and non-empty or it throws.
Five exact failure messages and four exact `context` strings (§9).
**cyrup** — `std::process::Command` with the platform shell (`sh -c` / `cmd /c`) directly: a native
crate spawns its own processes and needs no host verb. Wait with a deadline; cap the read at 1 MiB.
The `stdio: ["ignore","pipe","ignore"]` split is load-bearing — stderr is discarded so a chatty
credential helper cannot leak into a tool result. Reproduce the caller-side
`startsWith("!") && !startsWith("!!")` gates in `connectHttpClient` so `!!` never spawns.
`cyrup_ext::provider`'s `resolve_api_key` implements pi's `!command` form for provider keys and is
the shape precedent, but is provider-scoped.
**Why critical**: getting the *timing* wrong — resolving `!` during merge, hashing or preview rather
than at connect — means merely listing config in a repo with a hostile `.mcp.json` executes arbitrary
shell the user never approved.
**verify** — unit: each of the five failure messages; all four context strings; a `!!`-escaped
literal; and an assertion that hashing a definition with a `!command` env value spawns nothing.

**MCP-084 — Port `resolveServerUrl` / `resolveConfigPath` / `resolveBearerToken`** · high · S · `hand-written`
**upstream** — `utils.ts` `resolveServerUrl` (three throws: non-string `url`; missing env var(s) with
singular/plural wording; a post-interpolation URL-parse failure with `{ cause }`; a `null`/`undefined`
`url` returns `undefined`), `resolveConfigPath` (interpolate then `~`, `~/`, `~\` expansion),
`resolveBearerToken` (uses `interpolateSecretExpression`, not plain interpolation).
**behavior** — a URL with a missing variable fails **before any request is sent** — the whole reason
`getMissingEnvVars` exists.
**cyrup** — `url` (a workspace dependency) for the validity check; the rest is plain Rust.
`cyrup_ext_subagents::exec::mcp_direct_tools` ports `resolve_config_path` faithfully modulo the
missing `{env:}` form, has no `resolve_server_url` (its hash uses the raw `url`), and its
`resolve_bearer_token` uses plain interpolation — so `bearerToken: "!!x"` resolves to `"!!x"` instead
of `"!x"`. `cyrup_ext::caps::proc`'s `resolve_config_path` is a second, `pub(crate)` copy for `cwd`.
**verify** — unit: exact error strings for a non-string url and for one and two missing variables;
`~`/`~/`/`~\` expansion; the `!!` bearer token.

**MCP-085 — Port terminal sanitisation and error flattening** · medium · M · `hand-written`
**upstream** — `utils.ts` `stripOscSequences`, `sanitizeTerminalText`, `formatTerminalError`,
`truncateAtWord`.
**behavior** — an MCP server's stderr or error message cannot inject an OSC 8 hyperlink, a title-set
sequence or a colour run into the TUI; an unterminated OSC payload is consumed to end-of-string.
`formatTerminalError` walks `AggregateError.errors` then `.cause` chains with a cycle guard, falls
back to the aggregate's own message when the nested walk yielded nothing, dedups, and joins with
`": "`. `truncateAtWord` cuts at the last space if it is beyond `target * 0.6`, else hard-cuts,
appending `"..."`.
**cyrup** — port `stripOscSequences` as a hand-written byte scanner (a regex cannot express "consume
to EOF when unterminated"); the CSI strip and the C0/C1 → space collapse map onto `regex` (same
workspace-promotion chore as MCP-076). The Rust error walk is over `std::error::Error::source()`
chains; cyrup's nearest `AggregateError` analogue is `cyrup_provider::session_resources`' aggregate
cleanup-failure type, which is documented against pi's `AggregateError`, so a multi-error arm is
explicit rather than free.
**verify** — unit: an unterminated `ESC ]` payload; a C1 `0x9d` introducer; a cyclic `cause` chain;
an aggregate whose children are all message-less.

**MCP-086 — Port the browser/path open dispatch** · medium · S · `extension-owned`
**upstream** — `utils.ts`'s private `execOpen` plus `openUrl` and `openPath` — a literal
three-platform dispatch through the host `exec` with an `AbortSignal`, honouring a `browser` override
from `$BROWSER`, with the macOS `.app`-vs-executable distinction. `openPath` passes neither `browser`
nor a signal. Callers: `init.ts` (OAuth authorization URL), `commands.ts` and `mcp-setup-panel.ts`
(open a config file).
**behavior** — `$BROWSER=/usr/bin/firefox` launches firefox directly on macOS; `$BROWSER="Google
Chrome"` goes through `open -a`; cancellation propagates.
**cyrup** — `HostServices::exec(cmd, args, opts, cancel)` is the exact analogue and carries the
cancel token, so it is the faithful landing spot for this dispatch. **Correction to the first pass**:
this is *not* an alternative to `opener`. Upstream also imports npm `open` directly in
`mcp-auth-flow.ts` and `elicitation-handler.ts`, and *that* is what `opener` replaces (seam map OA-7,
E-4). Both mechanisms exist upstream and both exist in the port; collapsing the dispatch into
`opener` would silently drop the `$BROWSER` override and the abort path.
**verify** — unit with a stub exec recording argv, one case per platform × `browser` present/absent.

**MCP-087 — Port `parallelLimit`, argv scan, `toStringRecord`, `normalizeDirectToolInputSchema`** · medium · S · `hand-written`
**upstream** — `utils.ts` `parallelLimit`, `getConfigPathFromArgv`, `toStringRecord`,
`normalizeDirectToolInputSchema`.
**behavior** — `parallelLimit` preserves **result order by original index** while running
`min(limit, len)` workers (the only call site uses `limit = 10`); `getConfigPathFromArgv` exists
because the host flag API is a throwing stub at extension-load time;
`normalizeDirectToolInputSchema` strips `$schema` and `additionalProperties` before the schema reaches
the host tool registry and defaults a non-object to `{type:"object",properties:{}}`.
**cyrup** — `parallelLimit` maps onto `futures::stream::iter(..).buffered(10)` (order-preserving;
`futures` is a workspace dependency) or a `JoinSet` with index tagging. The argv scan is
`std::env::args()` — the *literal* upstream mechanism, with `InitApi::register_flag` declaring
`--mcp-config` so it appears in `--help`. There is no flag-read-back gap here.
`cyrup_core::Tool::parameters` takes raw JSON Schema, so `normalizeDirectToolInputSchema` is the only
shaping needed — pi's TypeBox shim has no analogue and needs none.
**verify** — unit: `parallel_limit` returns results in input order with a shuffled-latency stub;
schema normalisation drops exactly two keys.

**MCP-088 — Port `formatMcpStatus` and `formatAuthRequiredMessage`** · medium · S · `host-verb` + `hand-written`
**upstream** — `utils.ts` `formatAuthRequiredMessage`, `formatMcpStatus`, `extractToolUiStreamMode`.
`formatMcpStatus` returns `undefined` for `mcpFooterStatus === "off"` and otherwise prefixes
`"🔌 MCP: "` (U+1F50C + space) or `"MCP: "`; `formatAuthRequiredMessage` does
`replaceAll("${server}", serverName)`.
**behavior** — the footer segment text and the exact model-facing auth-required text (§5).
**cyrup** — the formatting is pure. The sink is `HostServices::set_status(key, Option<&str>)` —
keyed segments, clearable with `None` — paired with `HostServices::theme()` for the accent colour
upstream applies via `ui.theme.fg("accent", …)`. `extractToolUiStreamMode` is **cut** (Cut 2).
**verify** — unit: byte-exact assertions on both prefixes and on a template with two `${server}`
occurrences.

**MCP-089 — Port the error taxonomy** · medium · S · `hand-written`
**upstream** — `errors.ts` — `McpUiError` base + 7 subclasses + `wrapError` + `isErrorCode`; the full
message/code/recoveryHint table is in §10.
**behavior** — every thrown error carries a machine-readable `code`, a structured `context`, and a
user-facing `recoveryHint`; `toJSON` is the serialisation used when an error crosses into a tool
result.
**cyrup** — `#[derive(thiserror::Error)] enum McpError` with `#[error("…")]` reproducing each template
byte for byte, plus `fn code(&self) -> &'static str` and `fn recovery_hint(&self) -> &'static str`.
**Post-cut the taxonomy is the base shape + `ConsentError` + `McpServerError`**; the five Apps
classes (`ResourceFetchError`, `ResourceParseError`, `BridgeConnectionError`, `SessionError`,
`ServerError`) go with Cut 2, and `wrapError`'s only production caller went with them.
`cyrup_core::ToolError` is `{ message }` only, so the enum renders into `ToolError::message` at the
tool boundary and the structured triple stays inside `cyrup-mcp`. This resolves the first pass's open
question (port 7 or port 4) by scope rather than by preference.
**verify** — unit: one construction per live class asserting `message`, `code`, `context` keys and
`recovery_hint`.

**MCP-090 — Port the logger as a `tracing` adapter** · low · S · `extension-owned`
**upstream** — `logger.ts` — level priorities, `[MCP-UI…]` prefixes, `(k=v, k=v)` context rendering,
child loggers, pluggable handlers whose errors are swallowed, the `MCP_UI_DEBUG` bootstrap.
**behavior** — after Cut 2 the live sites are 16 direct `debug`, 1 direct `info`, and one child
logger (`consent-manager.ts`) adding 5 more `debug`; every production `warn`/`error` site was in an
Apps file. The port still maps all four levels, because the level filter and `MCP_UI_DEBUG` are the
user-facing contract. Separately, config-load warnings bypass the logger entirely as bare
`console.warn` in `config.ts` and `agent-plugin-loader.ts` and must stay a distinct, unfiltered
channel.
**cyrup** — a thin module over `tracing` (a workspace dependency `cyrup-ext-subagents` already uses
directly) with a stable target and structured fields. The pluggable handler list has no analogue and
no production consumer; drop it and say so.
**verify** — unit: `MCP_UI_DEBUG=1` raises the level; context rendering skips `None`; a child
logger's context sits under the call's.

**MCP-091 — Port `renderTsShape`** · medium · M · `hand-written`
**upstream** — `ts-shape.ts`, called from `proxy-modes.ts`'s `executeDescribe` and `executeSearch`,
each with a raw-schema fallback beside the call.
**behavior** — renders the useful JSON Schema subset as a TypeScript type literal; **returns `null`
on anything unsupported and the caller falls back to the raw schema**, so over-returning `None` is a
verbosity regression, never a correctness bug. The precedence order, alias naming, pointer-token
decoding, `additionalProperties: false` exemption and parenthesisation rule are in §12.
**cyrup** — `serde_json::Value` recursion with an `Option<String>`-shaped inner function; alias
ordering must be insertion-ordered (a `Vec<(key, alias)>` or an `IndexMap`), not a `BTreeMap`.
**verify** — conformance: golden outputs for `$ref` + `$defs`, `enum`, `const`, `anyOf`, a nested
object with optional properties, an array-of-union (parenthesised), a `type: [..]` array, and each
unsupported keyword returning `None`.

**MCP-092 — Port the dual-dialect JSON Schema validator** · high · S · `hand-written`
**upstream** — `json-schema-validator.ts` `DRAFT_07_SCHEMA_URIS`, `DRAFT_2020_12_SCHEMA_URIS`,
`schemaDialect` (one trailing `#` stripped), `createJsonSchemaValidator` — unstamped or 2020-12 ⇒ the
2020 validator; the two draft-07 URIs ⇒ the draft-07 validator; anything else ⇒
`` throw `Unsupported JSON Schema dialect: ${uri}` ``. Both memoised, `allErrors: true`,
`strict: false`; draft-07 additionally `validateFormats: true, validateSchema: false`.
**behavior** — a server whose tool schema declares an unsupported dialect fails loudly with that
exact message rather than silently validating nothing.
**cyrup** — `jsonschema`, already a workspace dependency at `default-features = false` (keep that;
the workspace comment on remote/file `$ref` resolution applies here identically) — bump the version.
Two `ValidationOptions` builders, one per draft, **both** with `.should_validate_formats(true)`,
because `jsonschema` treats `format` as annotation-only by default and ajv's asymmetric-looking
source is a default, not a behaviour difference. Cache compiled validators keyed on the schema.
`rmcp` does no client-side argument validation, so nothing here comes from the SDK.
**verify** — unit: each of the four accepted `$schema` spellings routes correctly, a trailing `#` is
stripped, and an unknown dialect yields the exact error string.

**MCP-093 — Register the `ajv-formats` formats `jsonschema` does not ship** · medium · S · `hand-written`
**upstream** — `json-schema-validator.ts`'s `addFormats(ajv)` on both validators.
**behavior** — a tool schema using `"format": "int64"` (routine in MCP servers generated from
OpenAPI) asserts upstream and would silently pass anything in a bare `jsonschema` port.
**cyrup** — `ValidationOptions::with_format(name, fn)` per missing format, on both builders.
`ajv-formats` supplies `url, int32, int64, float, double, byte, binary, password, iso-time,
iso-date-time, json-pointer-uri-fragment` beyond `jsonschema`'s built-in set; the exact delta must be
re-derived against the pinned `jsonschema` version rather than carried forward.
**verify** — unit: one accept/reject pair per registered format, plus a test that enumerates
`jsonschema`'s built-ins so the "missing" list cannot silently rot.

**MCP-094 — Reconcile `mcp_direct_tools` with this section's contract** · high · L · `hand-written`
**upstream** — `metadata-cache.ts` `computeServerHash`/`stableStringify`; `types.ts`'s naming
functions; `config.ts`'s source list and `parseJsonConfig`.
**behavior** — `<agent_dir>/mcp-cache.json` and `mcp.json` are **shared contracts** between
`cyrup-mcp` (writer) and `cyrup_ext_subagents::exec::mcp_direct_tools` (reader). Any disagreement
makes `mcp:` subagent selectors resolve to nothing, silently.
**cyrup** — the reader diverges on seven counts, each indexed to its own unit: absent-field encoding
(`null` vs `undefined`, MCP-070); hash field set (11 vs 13 post-cut — **v2.26.1: 11 vs 14**, the
15th upstream key `requestHeadersCommand` less Cut-3's `socket`; every server hash changed once as a
result, exactly as it did upstream — and raw vs resolved `url`,
MCP-070); config sources (4 vs 6, MCP-052); strict JSON vs JSONC (MCP-051); entry merge (wholesale vs
per-field, MCP-053); settings merge (wholesale vs one-level, MCP-067); `ToolPrefix` and filtering
(3 vs 4 modes, `-→_` vs preserve-and-hex-escape, no glob support — MCP-071, MCP-072, MCP-075,
MCP-076). Plus the two `resolve_agent_dir` divergences in MCP-068 and the missing `{env:}` form in
MCP-082. This unit proposes no work of its own; it exists so the reconciliation is scheduled as one
change rather than discovered seven times. It is a change to an extension crate, not to cyrup's core.
**verify** — a conformance suite shared by both crates: one golden `mcp.json` + `mcp-cache.json`
pair, asserted by `cyrup-mcp` and by `mcp_direct_tools`'s resolver.

**MCP-095 — JSONC parser home** · n/a · S · `extension-owned`
**upstream** — n/a; a cyrup layering question raised by MCP-051.
**behavior** — three crates want JSONC: `cyrup-mcp` (new), `cyrup-permission-system` (owner today),
and `cyrup-ext-subagents` (should).
**cyrup** — **settled, not open.** `cyrup_permission_system::jsonc` is `pub`, is already the parser
that crate uses on `mcp.json`, and `cyrup-permission-system` does not depend on `cyrup-mcp` — so the
`cyrup-mcp → cyrup-permission-system` edge is a plain dependency, not a cycle, and it has the
positive property that both readers of `mcp.json` parse it identically by construction. Depend on it.
If the edge is later disliked, relocating the module (it also uses that crate's `OrderedValue` for
`parse_ordered`) is a mechanical move with no behaviour change — a chore, not a prerequisite.
**verify** — build: `cargo tree` shows the edge and no cycle; the MCP-051 three-way parse test passes.

**MCP-096 — Project trust and the two project-scoped config sources** · high · S · `open-decision`
**upstream** — `config.ts` `loadMcpConfig` — an untrusted project's `.mcp.json` and `.pi/mcp.json`
are loaded and merged with no trust gate of any kind.
**behavior** — a project-local config can define a stdio server with an arbitrary `command`, an
`env` value beginning with `!` (a shell command, MCP-083), and a `cwd`. Upstream connects `eager`
servers at load.
**cyrup** — the mechanism to gate exists and is one call: `HostServices::is_project_trusted`.
`cyrup_config::settings`'s `SettingsManager` already skips the whole project layer when the project
is untrusted, so gating would match cyrup's own posture for every other project-scoped config; not
gating would match upstream exactly and match the `cyrup_permission_system::ext_config` precedent of
an extension owning its own file end to end.
**Options** — (a) mirror upstream: no gate, `cyrup-mcp` owns `mcp.json` completely; (b) drop sources
5 and 6 when `is_project_trusted()` is false, and say so in `/mcp` status.
**Recommendation** — **(b)**, with the divergence recorded in the port's parity notes. The security
delta is real and one-sided (a project file that can spawn arbitrary commands), the mechanism costs
one call, and cyrup's own trust model already draws this line for every comparable file. This is the
only genuine open decision in the section, and it is a policy choice, not a missing capability.
**verify** — unit: with `is_project_trusted() == false`, `.mcp.json` and `.cyrup/mcp.json` contribute
zero servers and the discovery summary reports them as present-but-untrusted.

**MCP-097 — Port `getConfigDiscoveryPaths` and `findAvailableImportConfigs`** · low · S · `hand-written`
**upstream** — `config.ts` `getConfigDiscoveryPaths`, `findAvailableImportConfigs`,
`resolveImportPath`.
**behavior** — `/mcp setup` lists every candidate config path with a present/absent marker before the
user has any config at all, and lists the importable host configs it detected.
`getConfigDiscoveryPaths` maps the ladder to `{ label, path, exists }[]` **without parsing any
file** — cheap enough to call on every render, unlike `getConfigSourceSummaries`.
`findAvailableImportConfigs` *does* parse, and warns with
`` `Failed to discover imported MCP config from ${kind}:` ``.
**cyrup** — trivial once MCP-052 and MCP-056 exist; the one thing to preserve is that
`get_config_discovery_paths` must **not** parse — a port that reuses the summary path changes both
the cost and the warning output of the setup panel.
**verify** — unit: with no config files present, `get_config_discovery_paths` still returns the full
ladder with `exists: false` and emits no warnings.

**MCP-098 — Preserve `renderTsShape`'s re-entrant alias emission** · medium · S · `hand-written`
**upstream** — `ts-shape.ts`'s output loop: `for (const [key, alias] of aliases)` iterates a `Map`
that the `render(definition)` call inside the loop may grow — a `$ref` inside a `$defs` member calls
`aliasFor` and inserts a new entry, and JS `Map` iterators are **live**, so the newly inserted alias
is visited in the same loop and its `type X = …;` line is emitted.
**behavior** — a schema whose `$defs` members reference each other renders every needed alias. A port
that snapshots the alias map into a `Vec` before looping emits a type literal referencing a name it
never defined — a **wrong string**, not a `None`, so the caller's raw-schema fallback (which only
triggers on `null`) never fires and the model is shown broken TypeScript. Nothing observable fails;
the schema just reads wrong.
**cyrup** — an index-based loop over a growing `Vec<(String, String)>`
(`while i < aliases.len() { … i += 1 }`), never `for (k, v) in &aliases` and never a pre-collected
snapshot. Insertion order must also be preserved (MCP-091). This unit exists so the trap is recorded
before the code is written.
**verify** — conformance: a golden fixture with `$defs.A = { $ref: "#/$defs/B" }` and
`$defs.B = {type:"string"}` where only `A` is referenced from the root — the output must contain both
`type A = B;` and `type B = string;`.

**MCP-099 — Reproduce `buildConfigWritePreview`'s reserialised "before" text** · low · S · `hand-written`
**upstream** — `config.ts` `buildConfigWritePreview` — `beforeText = existed ?
serializeRawConfig(readRawConfigObject(filePath)) : ""`, i.e. the file is parsed and reserialised
with 2-space indent + trailing newline before being diffed; `changed` compares against that
normalised text.
**behavior** — for an `mcp.json` containing comments, 4-space indent, or a hyphenated `mcp-servers`
key, `/mcp setup`'s preview shows a whole-file rewrite, and `changed` is `true` even when the
semantic content is unchanged. For an unparseable file it shows a diff from `{}` — it announces the
clobber. This is upstream behaviour and is arguably the point (the writer really does normalise), so
the port reproduces it rather than "fixes" it.
**cyrup** — build the "before" side from the same read-and-reserialise path the writer uses, not from
`fs::read_to_string`. A port that shows a byte-accurate diff under-reports what the write is about to
do.
**verify** — unit: a source file with `//` comments and 4-space indent previews as a full replacement
with `changed: true` even when the parsed object is identical.

---

### Out of scope

Four surfaces this section touches are **cut by decision**, not deferred. They are recorded with
their reasons so a later pass does not re-file them as gaps.

**The raw unix-socket transport (Cut 3).** `ServerEntry.socket` and everything keyed off it:
`mergeServerMaps`' two transport-swap stripping rules (MCP-054), `socket`'s place in
`computeServerHash`'s identity object (§17), `resolveConfigPath`'s second caller, and the `, or
socket` clause in the exactly-one-transport error. **Reason:** rmcp ships `UnixSocketHttpClient`, but
that is streamable-HTTP-over-UDS — a different wire shape from the adapter's raw framed socket, which
targets `rmcp-mux`. Supporting it would mean hand-writing a protocol transport, which is precisely
what the dependency decision exists to avoid. **Consequence to propagate:** a config carrying
`socket` must produce a *named diagnostic* at load, not a silent skip (MCP-069).

**The legacy HTTP+SSE transport (Cut 1).** In this section that is exactly one thing: the `"sse"`
value of `ServerEntry.httpTransport`. **Reason:** rmcp 3.1.2 ships no SSE client transport at all.
**Consequence to propagate:** the field survives with one legal value, and `"sse"` is a named
load-time diagnostic rather than an ignored value — `agent-plugin-loader.ts` sets it directly from a
manifest's `type: "sse"`, so a plugin declaring it is a reachable case that would otherwise appear
configured and never connect. `ServerEntry.protocolVersion` is **not** part of this cut: it is era
negotiation, not transport, and maps onto rmcp's `ClientLifecycleMode`.

**MCP Apps (Cut 2).** In this section: the whole `Ui*` type block in `types.ts` and its three
functions (MCP-080); `extractToolUiStreamMode` in `utils.ts`; five of the seven `errors.ts` classes
and `wrapError`'s only caller (MCP-089); every production `warn`/`error` logger site (MCP-090); the
`iframe` variant of `McpToolApprovalOrigin` (MCP-079); and the `MCP_UI_VIEWER` / `GLIMPSE_BINARY` /
SSH-detection env vars. **Reason:** the whole Apps subsystem — local HTTP host server, iframe bridge,
`ui://` resources, native-webview launcher — is out of scope by decision, taking `axum` and the
`@modelcontextprotocol/ext-apps` dependency with it. **Seams to hold:** `ui-tool-visibility.ts` is
split, not cut — `extractUiToolVisibility` and `isUiToolVisibleToModel` stay so the model does not
start seeing tools a server marked app-only; and the `CachedTool` / `DirectToolSpec` UI fields stay
in the on-disk schema, absent and ignored, because `CACHE_VERSION` is a live contract. The
browser-side `app-bridge.bundle.js` these types described was page content served into an iframe,
never a cyrup runtime, so nothing here is a JS-engine question.

**`mcpScript` / the JavaScript worker (Cut 4).** In this section: `McpSettings.scriptMode` and the
`script` variant of `McpToolApprovalOrigin`. **Reason:** the remaining proxy modes cover the same
ground, and removing this removes the only JS-engine question in the entire port. `renderTsShape`'s
third call site (`mcp-code.ts`) goes with it; the two `proxy-modes.ts` call sites and their
raw-schema fallbacks are what MCP-091 must satisfy. The `chrome-devtools` preset's `npx -y …` command
is unaffected — that spawns a third-party MCP server as an external OS process, which is what MCP is.

---

### What does not fit cleanly

**One open decision: MCP-096, project trust.** Upstream loads an untrusted project's `.mcp.json` and
`.pi/mcp.json` with no gate; cyrup's `SettingsManager` skips the entire project layer for an
untrusted project, and `HostServices::is_project_trusted` makes gating a one-call change. A
project-local config can name an arbitrary `command` and an `!`-prefixed `env` value that runs a
shell command at connect. Recommendation: gate (option b), record the divergence. This is a policy
choice with the mechanism already present — not a missing capability, and not a host addition.

**No host additions survive from this section.** Every candidate the first pass filed dissolves
against the extension surface: the JSONC parser is `cyrup_permission_system::jsonc`; the status bus
has no consumer and the snapshot stays in-crate; the approval broker is
`ExtHooks::before_tool_call` plus `cyrup_permission_system::create_mcp_permission_targets`; the
`--mcp-config` value comes from `std::env::args()` exactly as upstream reads `process.argv`; command
secrets, file writes and browser dispatch are things a native crate does with its own dependencies.
The section's only host verb is `HostServices::set_status`.

**Two one-line Cargo chores, named so they are not mistaken for prerequisites.** `regex` is not in
`[workspace.dependencies]` (it reaches the tree only as a direct dependency of
`cyrup-permission-system`) and is needed by MCP-076 and MCP-085; `toml` is likewise a direct
dependency of `cyrup-resources` only and is needed by MCP-056. Promoting both changes no behaviour
and adds no surface to any host trait.

**One scheduling constraint, not a severity.** MCP-070 blocks every cache writer in the port, because
`cyrup-mcp` and `cyrup_ext_subagents::exec::mcp_direct_tools` read and write the same file. It should
land with MCP-094 as a single change.

---

### Coverage

**Read**

*Upstream, in full at `v2.25.0`:* `config.ts`, `types.ts`, `utils.ts`, `errors.ts`, `logger.ts`,
`ts-shape.ts`, `json-schema-validator.ts`, `agent-dir.ts`.

*Upstream, targeted regions re-read to resolve defaults, validation rules and consumers:*
`metadata-cache.ts` (`computeServerHash`, `stableStringify`, `isServerCacheValid`, `CACHE_MAX_AGE_MS`,
`CACHE_VERSION`); `mcp-output-guard.ts` (`resolveMcpOutputGuardOptions`, `positiveInt`,
`envKillSwitch`, the three constants); `mcp-trace.ts` (defaults, path construction,
`isMcpTraceEnabled`); `server-manager.ts` (`resolveVersionNegotiation`, `normalizeRequestTimeoutMs`,
`createConnection`'s transport mutual exclusion, `connectHttpClient`'s header/bearer secret gating,
`resolveEnv`, `createClient`'s validator wiring); `mcp-oauth-provider.ts` (callback port, the
`clientSecret` context string); `tool-result-renderer.ts` (`collapsedResultLines` constants and
resolution); `direct-tools.ts`, `proxy-modes.ts` (auth message strings, `autoAuth`,
`renderTsShape` call sites and fallbacks); `index.ts`, `init.ts` (settings read sites, env sentinels,
`$BROWSER`, lifecycle `idleTimeout` forcing); `mcp-auth.ts` (`getAuthBaseDir`); `commands.ts`,
`mcp-panel.ts`, `prompts.ts`, `search-ranking.ts`, `tool-approval.ts`, `mcp-status.ts`;
`agent-plugin-loader.ts` (`httpTransport` / `pluginDataDir` / `literalEnv` origin);
`consent-manager.ts`, `ui-resource-handler.ts`, `ui-server.ts`, `ui-session.ts` (error and logger call
sites, to establish what Cut 2 removes); `README.md` (the published config reference, cross-checked
against both tables).

*Upstream, exhaustive greps at the tag:* every `errors.ts` class construction and `wrapError` /
`isErrorCode` call across all non-test `.ts` (8 production sites); every `logger.*` and child-`log.*`
call site; every `resolveServerFromToolName` reference (definition only — zero callers); every
`open` npm import (2); the `ServerEntry` and `McpSettings` field lists read directly from `types.ts`;
the `getToolNameCandidates` insertion set read directly from its body (18 insertions).

*cyrup, by symbol:* `cyrup_ext_subagents::exec::mcp_direct_tools` (`ServerEntry`, `McpSettings`,
`ToolPrefix`, `ImportKind`, `get_config_paths`, `read_config`, `validate_config`, `merge_configs`,
`expand_imports`, `compute_mcp_server_hash`, `stable_stringify`, `opt_str_value`, `get_tool_prefix`,
`get_server_prefix`, `format_tool_name`, `is_tool_excluded`, `interpolate_env_vars`,
`resolve_config_path`, `resolve_bearer_token`, `resolve_agent_dir`, `load_metadata_cache`,
`is_server_cache_valid`, `CACHE_VERSION`, `CACHE_MAX_AGE_MS`); `cyrup_permission_system::jsonc` (all
six `pub` entry points); `cyrup_permission_system::manager`
(`read_configured_mcp_server_names`, `parse_qualified_mcp_tool_name`, `create_mcp_permission_targets`,
`MCP_BASELINE_TARGETS`); `cyrup_permission_system::ext_config` (`ExtensionConfig::save`, the config
env-key rename); `cyrup_ext::caps::proc` (`interpolate_env_vars`, `resolve_config_path`, both
`pub(crate)`; the private `npx_resolver` module); `cyrup_ext::host::services`
(`HostServices::{set_status, exec, notify, confirm, input, select, theme, is_project_trusted,
open_overlay, human_interaction_lock, is_run_cancelled}`); `cyrup_ext::native`
(`InitApi::{register_tool, register_command, register_flag, add_autocomplete, subscribe,
subscribe_bus}`); `cyrup_ext::provider` (`resolve_api_key`); `cyrup_core::tool`
(`ToolError`, `Tool::parameters`); `cyrup_config::settings` (`SettingsManager`'s project-trust gate,
`deep_merge`, `FileSettingsStore`'s `FileLock`); `cyrup_config::env` (`ConfigDirs` and its
accessors — no `mcp_config_path`); `cyrup_provider::session_resources` (the `AggregateError`
analogue); the workspace `Cargo.toml` dependency table.

**Excluded**

- `agent-plugin-loader.ts` — read only where it sets `httpTransport`, `pluginDataDir` and
  `literalEnv`. It is a self-contained Agent-Plugins translator with its own manifest schema,
  sandboxing rules and namespacing; its port unit belongs with whichever section owns plugin loading.
- `metadata-cache.ts` beyond `computeServerHash`, `stableStringify`, `isServerCacheValid` and the
  constants — the cache **writer** is a separate subsystem. This section specifies the type model
  (MCP-077) and the hash pre-image contract (MCP-070, §17) and hands the writer on.
- `mcp-output-guard.ts` and `mcp-trace.ts` beyond their settings resolution — read only to fix the
  `outputGuard` and `trace` defaults; their own logic is another section's.
- `index.ts`, `init.ts`, `commands.ts`, `direct-tools.ts`, `proxy-modes.ts`, `prompts.ts`,
  `tool-approval.ts`, `search-ranking.ts`, `tool-result-renderer.ts`, `mcp-panel.ts`,
  `mcp-setup-panel.ts`, `server-manager.ts`, `lifecycle.ts`, `session-recovery.ts`,
  `mcp-oauth-provider.ts` — read only at the sites that *consume* a setting, a naming function, a
  secret or an error, to establish defaults and call semantics.
- `abort.ts`, `error-signal.ts`, `mcp-probe.ts`, `npx-resolver.ts`, `onboarding-state.ts`,
  `panel-keys.ts`, `resource-tools.ts`, `runtime-owner.ts`, `sampling-handler.ts`,
  `elicitation-handler.ts`, `state.ts`, `tool-metadata.ts`, `tool-registrar.ts`, `oauth*.ts`,
  `mcp-auth-flow.ts`, `mcp-callback-server.ts` — not imported by any of the seven assigned files.
- `__tests__/errors.test.ts`, `__tests__/logger.test.ts` — read only far enough to establish that
  they are the **sole** consumers of four error symbols; the tests themselves port as Rust unit
  tests.
- `cli.js` — npm packaging. cyrup is a Cargo workspace with a single binary.
- `unix-socket-transport.ts`, `mcp-code.ts`, `mcp-script-worker.mjs`, `ui-*.ts`,
  `host-html-template.ts`, `glimpse-ui.ts`, `app-bridge.bundle.js` — **cut**; see Out of scope.
- `cyrup_config::settings`'s `SettingsManager` / `EffectiveSettings` — read, and deliberately **not**
  used as the landing spot for `McpSettings`. `mcp.json` is a separate file with its own six-source
  ladder and its own one-level merge, and the in-tree precedent for an extension owning its config
  file is `cyrup_permission_system::ext_config`. Its project-trust gate is cited in MCP-096 as a
  posture reference, not as a mechanism to adopt wholesale.
- `cyrup_provider::auth::store` and `cyrup_config::auth` — `settings.oauthDir` touches them only as a
  *legacy plaintext import* directory; the credential-store decision belongs to the OAuth section.

**Corrections to the first pass**

- Dissolved: "no way for a native extension to reach a JSONC parser / the parser must move to
  `cyrup-core` first" (MCP-095). `cyrup_permission_system::jsonc` is `pub`, already parses
  `mcp.json` for `cyrup-permission-system`, and creates no cycle. Settled reuse, not an open
  decision.
- Dissolved: "`HostServices` has no bus-emit, so the MCP status snapshot needs a new host surface"
  (MCP-078). No consumer for the event exists in cyrup; the snapshot stays in-crate on a
  `tokio::sync::watch`, and there is no channel string to rename.
- Dissolved: "the approval broker's synchronous `claim()` is not expressible over cyrup's deferred
  bus, so the channel is a design problem" (MCP-079). `ExtHooks::before_tool_call` is the same gate,
  fails closed, and `cyrup_permission_system::create_mcp_permission_targets` already derives MCP
  targets. The broker does not port because it is not needed.
- Dissolved: "prompt slash-command naming is blocked because natives can only register commands
  inside `init`" (MCP-074). The naming functions are pure and unblocked; late command registration is
  the seam map's P-4 and belongs to the prompts section.
- Dissolved: "`cyrup-mcp` cannot reuse `interpolate_env_vars`/`resolve_config_path` because they are
  `pub(crate)`" (MCP-082, MCP-084). Both in-tree copies implement only two of the three syntaxes, so
  the correct answer is that `cyrup-mcp` owns the three-form implementation regardless of visibility.
- Dissolved: "the extension has no way to know whether the project is trusted" (implicit in MCP-096's
  framing). `HostServices::is_project_trusted` exists; MCP-096 is a policy choice, not a capability
  gap.
- Refuted: "port the open dispatch as exec, **not** the `opener` crate" (MCP-086). Upstream has
  *both* mechanisms — `utils.ts`'s exec dispatch with `$BROWSER` and an abort signal, and a direct
  npm `open` import in `mcp-auth-flow.ts` and `elicitation-handler.ts`. The port has both:
  `HostServices::exec` for the first, `opener` for the second.
- Refuted: "6 of the 7 error classes are thrown only from UI files, so the taxonomy's fate depends on
  a scope question" (MCP-089). With Cut 2 decided, the answer is determined: base +
  `ConsentError` + `McpServerError` port; five classes are cut. No open question remains.
- Refuted: "`renderTsShape` has three call sites". Two survive (`proxy-modes.ts`); the third
  (`mcp-code.ts`) goes with Cut 4.
- Demoted: MCP-052, MCP-069, MCP-070, MCP-071 and MCP-094 were rated `critical`. None meets the
  four-clause bar. MCP-070's blocking-ness (no cache writer can ship before it) is scheduling
  information and now lives in its body. Two criticals remain — MCP-053 (credential exfiltration on
  merge) and MCP-083 (unapproved shell execution if the `!` resolution timing is wrong).
- Re-verdicted: 44 units that the first pass framed as `not-ported` against a missing cyrup surface
  are `hand-written` or `extension-owned` inside `cyrup-mcp`; three are `cut`; one is `host-verb`;
  one is `open-decision`. None is `host-addition`.
- Removed: every cyrup line number and commit anchor, the revision-provenance section, the
  `depends` graph pointing at RECON documents, the "Blind spots" and "Negative results" sections
  whose content is folded into the relevant units, and the unverifiable latest-stable version claims
  (versions are named as "already in tree, bump required" rather than pinned).
- Upheld: the `undefined`-vs-`null` hash pre-image finding; the ajv `validateFormats` asymmetry and
  its consequence for `jsonschema`; the 5 + 13 = 18 `getToolNameCandidates` insertions with
  data-dependent deduped size; 28 `ServerEntry` fields and 23 `McpSettings` keys; the four
  `resolveCommandSecret` context strings and three `resolveServerUrl` throws; `similar` being a
  workspace dependency while `regex` and `toml` are not.
- Added from the README cross-check: `authRequiredMessage` is the one `McpSettings` key the published
  config reference does not document; `httpTransport`, `pluginDataDir` and `literalEnv` are the three
  `ServerEntry` fields it does not document, and all three are set only by `agent-plugin-loader.ts`.
