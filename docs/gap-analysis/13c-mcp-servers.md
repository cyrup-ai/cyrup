# 13c · Server manager, transports and the metadata cache

> Part of **[13 — cyrup-mcp](13-cyrup-mcp.md)**, which holds the thesis, the seam map, the
> architecture and the one canonical table of every port unit. Method and phasing are in
> **[MCP-PORT-METHODOLOGY.md](MCP-PORT-METHODOLOGY.md)**.

Upstream is `pi-mcp-adapter` v2.25.0. cyrup is branch `david/cyrup`. rmcp is the checkout at
`/Users/davidmaple/cyrup.ai/rmcp` (`rmcp-v3.1.2-7-gf713ebd`). cyrup is referenced by symbol and
file only; upstream by file and symbol.

`server-manager.ts`'s `McpServerManager` is the adapter's engine room. It owns every live MCP
connection: it builds the transport, runs the handshake, negotiates the protocol revision and the
client capabilities, enumerates tools/resources/prompts with pagination, subscribes to
`list_changed` refreshes, tracks in-flight work and idle time, and tears everything down without
leaking a child process or clobbering a connection a concurrent caller already replaced. Four small
satellites sit around it: `mcp-probe.ts` (an unauthenticated three-strategy HTTP probe used *only*
to make a failed HTTP connect message actionable), `session-recovery.ts` (the spec's Streamable-HTTP
"404 means your session is gone" retry), `mcp-status.ts` (a connection-free status snapshot), and
`metadata-cache.ts` (the on-disk `mcp-cache.json` that lets the adapter register the model-visible
tool surface *before any MCP server process starts*).

**The wire is not this section's work.** With the transport scope fixed at stdio plus streamable
HTTP, every byte that leaves the process is `rmcp`'s: `TokioChildProcess` /
`TokioChildProcessBuilder` for stdio (including `env`, `cwd` and a piped `ChildStderr`),
`StreamableHttpClientTransport` + `StreamableHttpClientTransportConfig` for HTTP,
`ClientLifecycleMode` + `ClientServiceExt::serve_with_lifecycle` for revision negotiation,
`Peer::list_all_tools` / `list_all_prompts` / `list_all_resources` for paginated discovery,
`PeerRequestOptions` for per-request timeouts, `serve_client_with_ct` for connection-scoped
cancellation. `cyrup-mcp` writes **no protocol code and no framing code at all** — the NDJSON
question the first pass could not settle simply evaporates, because `AsyncRwTransport` (which
`TokioChildProcess` wraps) owns the line framing inside the SDK.

**What remains is policy, and policy is the majority of the file.** Upstream's non-obvious lines are
almost all defending against one race shape — a session restarted, or a caller reconnected, while an
`await` was in flight, with a stale callback that must not resume into fresh state. Five distinct
mechanisms implement that defence: `closeGenerations` (a per-server monotonic counter bumped by every
`close` and re-checked after `connect` resolves); `connectAttempts` (a per-attempt `AbortController`
that `close` fires so an in-flight connect tears down its own half-built transport); object-identity
guards in `client.onclose`, the three `list_changed` handlers and `doReconnect`; the
`abortCleanupPromises` `WeakMap<object, Promise<void>>` so the exact `transport.close()` an abort
started is the one awaited; and the `connectPromises`/`reconnectPromises`/`closePromises`
single-flight maps that each delete themselves only on identity match. All five port literally. So do
the connection state machine, the implicit-vs-explicit OAuth ladder, the per-list failure policy, the
error taxonomy, the idle accounting, the probe, the session-recovery predicate, the status snapshot
and the cache.

**The hard external constraint is the metadata cache, and it is already half-built in cyrup.**
`cyrup_ext_subagents::exec::mcp_direct_tools` already *reads* `<agent_dir>/mcp-cache.json` in Rust at
schema version 1 with the 7-day TTL, and already ports `computeMcpServerHash`. `cyrup-mcp` is the
**writer** of a file that already has a reader — but the two do not agree today on four independent
axes (the hashed field set, the `undefined`-vs-`null` token in the hash pre-image, the third
env-interpolation pattern, and the `!`/`!!` secret-expression rule) and disagree on a fifth surface
(`read_` vs `get_` resource-tool naming). A writer built to upstream spec against today's reader
emits digests the reader rejects for *every* server, and the symptom is not an error — it is a
subagent silently receiving no MCP tools. That is where this section's only `critical` items live,
and they must land as one coordinated change across the two crates.

`npx-resolver.ts` needs no re-specification: `cyrup_ext::caps::proc::npx_resolver` is already a
direct port of it, with the same `CACHE_VERSION`/TTL/force-cache-timeout shape and the same
read-merge-write-rename save. It is a port of an *earlier* upstream revision, and this section
reports the six places it has fallen behind rather than restating the algorithm.

---

### How it lands

| adapter capability | upstream mechanism | cyrup mechanism | verdict |
|---|---|---|---|
| stdio child process, `env`, `cwd`, stderr capture | `createConnection`'s `StdioClientTransport` branch | `rmcp::transport::TokioChildProcess`, `TokioChildProcessBuilder::{stderr, spawn}`, `ConfigureCommandExt` over `tokio::process::Command` | **rmcp** |
| `npx`/`npm` → real binary pre-resolution | `npx-resolver.ts` `resolveNpxBinary`, called before the transport is built | `cyrup_ext::caps::proc::npx_resolver::resolve_npx_binary` — already ported; needs a `pub` promotion and six fixes | **extension-owned** (reuse) |
| streamable HTTP | `connectHttpClient`'s `StreamableHTTPClientTransport` | `rmcp::transport::StreamableHttpClientTransport` + `StreamableHttpClientTransportConfig` | **rmcp** |
| legacy HTTP+SSE + `shouldFallbackToSse` | `server-manager.ts` | — | **cut** |
| raw unix socket | `unix-socket-transport.ts` | — | **cut** |
| NDJSON framing | `@modelcontextprotocol/client`'s `ReadBuffer`/`serializeMessage` | `rmcp::transport::async_rw::AsyncRwTransport`, inside `TokioChildProcess` | **rmcp** |
| transport selection + mutual exclusion | `createConnection`'s three-way count | reduced to `command` xor `url`; `socket` and `httpTransport: "sse"` become named load-time diagnostics | **hand-written** |
| protocol-revision negotiation | `resolveVersionNegotiation` → SDK `versionNegotiation` | `ClientLifecycleMode::{Initialize, Auto, Discover}` + `ClientServiceExt::serve_with_lifecycle`; `ProtocolVersion::V_*` | **rmcp** (one named delta) |
| per-request timeout | `buildRequestOptions` / `getResolvedRequestTimeoutMs` | `rmcp::service::PeerRequestOptions { timeout, reset_timeout_on_progress, max_total_timeout }` | **rmcp** + **hand-written** (the normalisation rule) |
| connection-scoped cancellation | `combineAbortSignals`, `abortable`, `throwIfAborted` | `rmcp::serve_client_with_ct(CancellationToken)`; `cyrup_core::CancelToken` **is** `tokio_util::sync::CancellationToken`, so it passes straight in | **rmcp** + **host-verb** `is_run_cancelled` |
| 401 detection for the auth ladder | `isUnauthorizedHttpError` (instanceof + `status === 401`) | `ClientInitializeError::auth_challenge()` / `StreamableHttpError::{AuthRequired, InsufficientScope}` — typed, walks the `source()` chain | **rmcp** |
| custom headers, bearer token | `connectHttpClient`, `resolveBearerToken` | `StreamableHttpClientTransportConfig::{auth_header, custom_headers}` | **rmcp** + **hand-written** (env / `!command` resolution) |
| paginated `tools`/`resources`/`prompts` list | `fetchAllTools` / `fetchAllResources` / `fetchAllPrompts` cursor loops | `Peer::{list_all_tools, list_all_prompts, list_all_resources}` | **rmcp** + **hand-written** (capability gate, per-list failure policy) |
| server `instructions`, server capabilities | `client.getInstructions?.()`, `getServerCapabilities?.()` | `RunningService::peer_info()` → `InitializeResult { capabilities, instructions }` | **rmcp** |
| `list_changed` refresh | SDK `listChanged.{tools,resources,prompts}.onChanged` | `ClientHandler::on_{tool,prompt,resource}_list_changed` (bare notification; `Peer` self-invalidates) then re-`list_all_*` | **rmcp** + **hand-written** glue |
| child teardown without orphans | `client.close()` → SDK stdio escalation | `TokioChildProcess::graceful_shutdown` (close transport → 3 s → `kill`), plus `ChildWithCleanup`'s `Drop` | **rmcp** (one named delta) |
| terminated-session detection | `session-recovery.ts` `isTerminatedSession` | `StreamableHttpError::SessionExpired` covers the 404-with-session arm exactly; the 400/`-32000` arm and the reconnect-and-retry wrapper stay adapter policy | **rmcp** + **hand-written** |
| connection registry, single-flight, generations | `McpServerManager` maps | `Mutex<HashMap<..>>` + `Arc::ptr_eq` + `Shared` futures | **hand-written** |
| lifecycle guards, backoff, idle accounting | `McpServerManager` + `lifecycle.ts` | `tokio` timers on a task owned by the extension | **hand-written** |
| endpoint probe | `mcp-probe.ts` `probeMcpEndpoint` | `reqwest` directly | **extension-owned** |
| status snapshot | `mcp-status.ts` `createMcpStatusSnapshot` | serde structs; published in-crate on a `tokio::sync::watch` | **extension-owned** |
| status snapshot on the shared event bus | `publishMcpStatusSnapshot` on `pi.events` | no consumer exists in cyrup; keep the snapshot in-crate | **extension-owned** |
| `mcp-cache.json` write | `metadata-cache.ts` `saveMetadataCache` | `cyrup-mcp::cache` — writer for the reader in `cyrup_ext_subagents::exec::mcp_direct_tools` | **hand-written** (contract-locked) |
| `computeServerHash` | `metadata-cache.ts` `computeServerHash` + `stableStringify` | `sha2` over the 14-key pre-image; must match `compute_mcp_server_hash` byte-for-byte | **hand-written** (parity-constrained) |
| tool/resource/prompt (de)serialisation and reconstruction | `metadata-cache.ts` serialisers + `reconstructToolMetadata` | serde with `skip_serializing_if` | **hand-written** |
| adapter-private UI stream-patch notifications | `attachAdapterNotificationHandlers`, `registerUiStreamListener` | — | **cut** |

---

### Behavioural specification

#### 3.1 The connection record, the manager's state, and its public API

`ServerConnection` fields: `client`, `transport`, `definition` (the config snapshot this connection
was built from), `tools`/`resources`/`prompts` (live inventory, replaced wholesale by
`list_changed`), `promptDiscoveryFailed` (true when the `prompts` capability was advertised but
`prompts/list` threw), `instructions` (present only when defined), `lastUsedAt` (epoch ms),
`inFlight`, `status` ∈ `{"connected", "closed", "needs-auth"}`, and `credentialsInvalidated` (true
once *this* needs-auth episode discarded the cached credential).

Manager-owned maps, each of which must exist in the port with the same key type and the same
identity-delete discipline: `connections`, `connectPromises`, `reconnectPromises`, `closePromises`,
`closeGenerations`, `connectAttempts` (→ `AbortController`), `acceptedUrlElicitations`
(`server → Set<elicitationId>`). Scalars: `samplingConfig`, `metadataListChangedListener`,
`elicitationConfig`, `authStorageOptions`, `oauthRuntime`, `defaultRequestTimeoutMs`,
`runtimeSignal`, `traceSettings`, `traceWriter` (lazily memoised with `??=`), `stopped`, and the
constructor's `defaultCwd`. (`uiStreamListeners`, keyed by *stream token* rather than server name,
is cut with MCP Apps.)

**Public API surface** — the port must expose all of it, because `lifecycle.ts`, `mcp-status.ts`,
`session-recovery.ts` and the `/mcp` panel are all written against it:

* the eight setters `setSamplingConfig`, `setMetadataListChangedListener`, `setElicitationConfig`,
  `setRuntimeSignal`, `setDefaultRequestTimeoutMs` (normalises on the way in), `setTraceConfig`,
  `setAuthStorageOptions`, `setOAuthRuntime`;
* `getRequestOptions(name, signal?)` — looks the connection's `definition` up by name and defers to
  `buildRequestOptions`; this is what `getPrompt`/`readResource` call per request;
* `connect`, `reconnect`, `close`, `closeAll`, `getPrompt`, `readResource`,
  `handleUrlElicitationRequired`;
* `isConnecting(name)` = `connectPromises.has(name)`;
* `getConnection(name)`, and `getAllConnections()` which returns a **copy** (`new Map(this.connections)`),
  not the live map;
* `touch` / `incrementInFlight` / `decrementInFlight` / `isIdle` — all public, driven by `lifecycle.ts`.

Module constants: `MAX_CAPTURED_STDERR_BYTES = 8 * 1024`, `MAX_CAPTURED_STDERR_LINES = 3`.

#### 3.2 Transport selection, reduced to two arms

Upstream's `createConnection` counts the configured transports and enforces mutual exclusion:

```
configuredTransports = [definition.command, definition.url, definition.socket]
    .filter(v => typeof v === "string" && v.length > 0)
if configuredTransports.length !== 1:
    throw Error(`Server ${name} must configure exactly one of command, url, or socket`)
```

An **empty string counts as unconfigured**, and `${name}` is unquoted in this message (unlike most
other messages in the file).

In cyrup this becomes **exactly one of `command` or `url`**, with the message adjusted accordingly.
Two config values that upstream accepts must now produce a **named load-time diagnostic** rather
than a silent skip or a wrong-shape connect:

* `socket` present → *"MCP server \"<name>\" configures `socket`; cyrup supports only stdio
  (`command`) and streamable HTTP (`url`)."*
* `httpTransport: "sse"` → *"MCP server \"<name>\" requests the legacy HTTP+SSE transport, which
  cyrup does not support; use streamable HTTP."*

An Agent Plugin declaring `type: sse` would otherwise appear to connect and never work, so silence
is the one unacceptable behaviour here.

`requestOptions = this.buildRequestOptions(definition, requestSignal)` is computed **once**, before
any transport is built, and reused for the connect **and** all three discovery list calls.
`definition.httpTransport` retains only the `"streamable-http"` value; `definition.protocolVersion`
is *not* about the SSE transport and stays in full (§3.6).

#### 3.3 stdio transport

The ordered sequence, and the order matters:

1. Create the client **before** any argument work, so an npx resolution failure still has a client
   to close.
2. `args = (definition.args ?? []).map(interpolateEnvVars)`. Arguments get `${VAR}`/`$env:VAR`/
   `{env:VAR}` interpolation **only** — never `!command` secret execution. `command` itself is *not*
   interpolated.
3. If `command === "npx" || command === "npm"`: `resolved = await resolveNpxBinary(command, args, signal)`.
   On a hit: `command = resolved.isJs ? "node" : resolved.binPath`;
   `args = resolved.isJs ? [resolved.binPath, ...resolved.extraArgs] : resolved.extraArgs`; debug log
   `` `${name} resolved to ${resolved.binPath} (skipping npm parent)` ``. On `null`, both pass
   through untouched. This is what makes the tracked pid the MCP server rather than an npm launcher,
   and it is why killing the single child pid is sufficient.
4. `throwIfAborted(signal)`.
5. `if (definition.pluginDataDir) mkdirSync(definition.pluginDataDir, { recursive: true })`.
6. `cwd = resolveConfigPath(definition.cwd) ?? this.defaultCwd`, and the key is **omitted entirely**
   when both are undefined.
7. Build the transport with `{ command, args, env: resolveEnv(...), cwd?, stderr: definition.debug ? "inherit" : "pipe" }`.
8. When the transport exposes `stderr`, attach a `"data"` listener accumulating into `stderrTail`.

`resolveEnv(env, serverName, literalEnv)`: the full process environment is copied first (dropping
`undefined` values), then overrides are layered on top.

* `literalEnv === true` → `env ? {...resolved, ...env} : resolved` verbatim: **no interpolation, no
  `!command` execution**. This is the Agent-Plugin path.
* otherwise `resolveCommandSecretsRecord(env, key => \`MCP server "${serverName}" stdio env "${key}"\`)`,
  i.e. per value via `resolveCommandSecret`:
  * `!!X` → `interpolateEnvVars(value.slice(1))` — **one** `!` is consumed, so the result still
    begins with `!`. This is the escape for a literal leading `!`, not a way to strip it.
  * `!cmd` → `spawnSync(value.slice(1), { shell: true, encoding: "utf8", timeout: 10_000,
    maxBuffer: 1 MiB, stdio: ["ignore","pipe","ignore"], windowsHide: true })`, `stdout.trim()`,
    throwing `` `Failed to resolve ${context}: ${reason}` `` where reason ∈ {`command timed out
    after 10 seconds` (ETIMEDOUT), `command output exceeded 1 MiB` (ENOBUFS), `command failed to
    start` (any other spawn error), `` `command exited with code ${result.status ?? "unknown"}` ``
    — **the `"unknown"` fallback is part of the string** —, `command returned empty output`}.
  * else `interpolateEnvVars(value)`.

**stderr tail algorithm** — a bounded ring, not a growing buffer, with the bound applied *before* the
string→bytes conversion so a multi-megabyte burst never allocates in full:

```
boundedStderrChunk(chunk):
  if Buffer:  return last MAX_CAPTURED_STDERR_BYTES bytes
  else:       suffix = chunk.length > MAX ? chunk.slice(-MAX) : chunk        // chars, not bytes
              bytes  = utf8(suffix)
              return bytes.byteLength > MAX ? last MAX bytes of bytes : bytes
appendStderrTail(tail, chunk):
  bytes = boundedStderrChunk(chunk)
  if bytes empty -> tail;  if tail empty -> bytes
  combined = tail ++ bytes
  return combined.length > MAX ? last MAX bytes : combined
```

On a connection failure the tail becomes the error-message suffix: `tail.toString("utf8").trim()`,
split on `/\r?\n/`, each line trimmed, empties dropped; if any survive, throw
`` new Error(`${baseMessage} (${detail})`, { cause: reportedError }) `` where
`detail = lines.slice(-MAX_CAPTURED_STDERR_LINES).join(" — ")` — **em-dash-space separated, last 3
lines**.

**The rmcp fit is exact.** `TokioChildProcessBuilder`'s default is `stderr: Stdio::inherit()` —
upstream's `debug: true`. Calling `.stderr(Stdio::piped())` makes `spawn()` return
`(TokioChildProcess, Option<ChildStderr>)` — upstream's `debug: false` plus the handle the tail
accumulator reads. `env` and `cwd` are set through `ConfigureCommandExt::configure` on the
`tokio::process::Command` before it becomes a `CommandWrap`. Nothing here needs `HostServices`:
a native crate spawns its own children, the same way `cyrup-ext-subagents` does.

#### 3.4 HTTP transport, secrets and the auth ladder

Pre-flight, all before any socket is opened:

1. `serverUrl = resolveServerUrl(definition)!` — reject a non-string url (`"MCP server URL must be a
   string"`); **throw** `` `Missing environment variable${s} in MCP server URL: ${missing.join(", ")}` ``
   listing every missing var; interpolate; then **throw**
   `` `Invalid MCP server URL after environment interpolation: ${resolved}` `` if `new URL()` rejects.
2. `hasCommandHeader` = any header value starts with `!` and not `!!`.
3. `headers = resolveCommandSecretsRecord(definition.headers, key => \`MCP server "${serverName}" HTTP header "${key}"\`) ?? {}`.
4. `commandBearer` = `definition.bearerToken` starting `!` but not `!!`.
5. If `definition.auth === "bearer"`: `token = commandBearer ? resolveCommandSecret(commandBearer,
   \`MCP server "${serverName}" HTTP bearer token\`) : resolveBearerToken(definition)`; if truthy,
   `headers["Authorization"] = \`Bearer ${token}\``.
6. If `hasCommandHeader || commandBearer`, validate by constructing `new Headers(headers)`; on throw:
   `` `Failed to resolve MCP server "${serverName}" HTTP command secret: command returned an invalid header value` ``.
   This is the injection guard — a command that emits a newline must not be able to add a header.
7. `requestInit` exists only when at least one header does.

Auth-provider state, `HttpAuthProviderState`:

| `supportsOAuth(definition)` | `definition.auth` | initial state |
|---|---|---|
| false | — | `{status:"disabled"}` |
| true | `undefined` | `{status:"implicit-deferred"}` — **no** provider constructed, so secure storage is not touched until the server proves auth is needed |
| true | anything else | `{status:"explicit", provider}` |

Each attempt builds transport options `{requestInit?, authProvider?, skipIssuerMetadataValidation?}`
where the last is set only when a provider exists **and** `definition.oauth !== false` **and**
`definition.oauth?.skipIssuerMetadataValidation === true`; constructs a **fresh** client per attempt;
and on failure closes exactly once (`abortCleanupPromises.get(transport) ?? client.close()`), skipped
entirely when the error *is* the abort-cleanup aggregate, with any cleanup throw becoming
`AggregateError([error, cleanupError], "MCP HTTP connection cleanup failed")`.

The loop, in exact evaluation order — **this ordering is the specification**:

```
kind        = definition.httpTransport ?? "streamable-http"
invalidated = credentialsInvalidated
loop {
  result = attempt(kind)
  if connected                                            -> return {..., credentialsInvalidated: invalidated}
  if error is AggregateError "MCP connection abort cleanup failed" -> throw it        // FIRST
  if signal?.aborted                                      -> throwIfAborted(signal)   // SECOND
  if authState == implicit-deferred && isUnauthorizedHttpError(err)
        -> authState = implicit-challenged(createAuthProvider()); continue            // THIRD, SAME kind
  if isUnauthorizedHttpError(err) {                                                   // FOURTH
      if supportsOAuth(definition) {
          if !invalidated { invalidateAuthEntryCache(serverName); invalidated = true }
          return { client, transport, status: "needs-auth", credentialsInvalidated: invalidated }
      }
      throw err
  }
  throw err
}
```

The fifth arm — `shouldFallbackToSse` — is cut (Out of scope, Cut 1), so the loop terminates on the
`throw err` above. Everything before it survives verbatim.

`isUnauthorizedHttpError(e)` = `e instanceof UnauthorizedError || (e instanceof SdkHttpError && e.status === 401)`.
In cyrup this is **not** hand-written: `ClientInitializeError::auth_challenge()` walks the error's
`source()` chain looking for `AuthRequiredError` (401) or `InsufficientScopeError` (403), returning
the raw `WWW-Authenticate` header, and it explicitly recurses through `LegacyFallbackFailed`. It is
strictly more informative than upstream's predicate — it hands the challenge string straight to
`AuthorizationRequest::with_challenge`, and it additionally distinguishes a 403 `insufficient_scope`
that upstream has no arm for. The port treats `AuthRequired` as upstream's 401 and routes
`InsufficientScope` to the same needs-auth exit (section 05 owns the scope-upgrade decision).

#### 3.5 Handshake, capabilities, client identity

* Client identity is `{ name: \`pi-mcp-${serverName}\`, version: "1.0.0" }` — **the server sees this
  string**. The port emits `cyrup-mcp-<server>` and records the rename: any server that
  allow-lists the pi client name will not recognise cyrup.
* `jsonSchemaValidator: createJsonSchemaValidator()` is passed into every client. rmcp does **no**
  client-side JSON-Schema validation, so this is hand-written on `jsonschema` and owned by the
  tool-execution section, not here.
* `capabilities` from `buildClientCapabilities()`, **omitted entirely when empty**: `sampling: {}`
  iff a sampling config is set; `elicitation: { form: {}, url?: {} }` iff an elicitation config is
  set, with `url` present only when `elicitationConfig.allowUrl`. rmcp's `ClientCapabilities` mirrors
  this exactly through `ElicitationCapability::{with_form, with_url}`, and the client-side
  elicitation types are unconditional under the `client` feature — the `elicitation` cargo feature is
  server-side and is **not** needed.
* `listChanged: { tools: {onChanged}, resources: {onChanged}, prompts: {onChanged} }` — the SDK
  performs the refresh itself and hands back `(error, list)`. rmcp's `on_*_list_changed` are bare
  notifications; `Peer` invalidates its own response cache on them, so the handler re-calls
  `list_all_*` itself. A ~20-line difference, not a capability difference.
* Then `registerSamplingHandler` and `registerElicitationHandler` (section 05), and — when
  `allowUrl` — a handler for `notifications/elicitation/complete` (§3.10).

#### 3.6 Protocol-revision negotiation

`resolveVersionNegotiation(definition)`:

| `definition.protocolVersion` | upstream | cyrup |
|---|---|---|
| `undefined` or `"legacy"` | **omit the `versionNegotiation` option entirely** (byte-equivalent to pre-2026 behaviour) | `ClientLifecycleMode::Initialize` |
| `"auto"` | `{ mode: "auto" }` | `ClientLifecycleMode::Auto { preferred_versions, legacy_version }` |
| `"2026-07-28"` | `{ mode: { pin: "2026-07-28" } }` | `ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] }` |
| anything else | `` throw new Error(`Invalid MCP protocolVersion: ${String(...)}`) `` | same string, at config load |

Entered through `ClientServiceExt::serve_with_lifecycle`. `ProtocolVersion` carries `V_2024_11_05`,
`V_2025_03_26`, `V_2025_06_18`, `V_2025_11_25`, `V_2026_07_28`, with `LATEST = V_2025_11_25`;
`select_protocol_version` does the intersection.

**One named delta, and it is the sharpest mechanism divergence in this section.** SDK v2 detects its
base stdio transport before connect so it can run `server/discover` on a **disposable sibling
process** — `mcp-trace.ts`'s `wrapTransportWithMcpTrace` composes callbacks in place rather than
returning a wrapper object precisely because "returning a wrapper hides that identity and makes
tracing change negotiation behavior". rmcp does not do this: `serve_client_with_lifecycle` runs
`discover_startup` and, on `Auto`, `legacy_startup` on the **same** `&mut transport`, and rmcp's own
`DiscoverOutcome` doc says `Legacy` is returned *only* when the probe produced a complete, correlated
JSON-RPC error — "i.e. the transport is in a known-good state". A legacy stdio server that **exits**
on `server/discover` therefore produces a transport error, not `Legacy`, and there is no fallback;
upstream burned a disposable sibling and still connected. Upstream ships a dedicated fixture for
exactly this case (`__tests__/fixtures/legacy-exits-on-discover-server.mjs`). rmcp also bounds the
probe with `DEFAULT_AUTO_DISCOVER_TIMEOUT = 10 s`, after which it falls back to legacy on the same
transport. See *What does not fit cleanly*.

#### 3.7 `connect` — exact sequence

```
1  if isServerDisabled(definition)            throw Error(`MCP server "${name}" is disabled`)
2  if this.stopped                            throw Error("MCP server manager is closed")
3  ownedSignal = combineAbortSignals(runtimeSignal, signal); throwIfAborted(ownedSignal)
4  closing = closePromises.get(name); if closing await abortable(closing, ownedSignal); throwIfAborted
5  if connectPromises.has(name)               return abortable(connectPromises.get(name), ownedSignal)
6  existing = connections.get(name)
   if existing?.status === "connected"        existing.lastUsedAt = now(); return existing
7  credentialsInvalidated = existing?.status === "needs-auth" && existing.credentialsInvalidated === true
8  generation        = closeGenerations.get(name) ?? 0
   attemptController = new AbortController()
   attemptSignal     = combineAbortSignals(ownedSignal, attemptController.signal)
9  attempt = createConnection(name, definition, attemptSignal, /*requestSignal*/ ownedSignal, credentialsInvalidated)
   promise = definition.url ? attempt.catch(async e => { throw await enrichHttpConnectionError(definition, e) })
                            : attempt
   connectPromises[name] = promise; connectAttempts[name] = attemptController
10 try {
     connection = await promise
     if attemptController.signal.aborted || (closeGenerations.get(name) ?? 0) !== generation {
        await disposeConnection(connection)
        throwIfAborted(attemptSignal)
        throw Error(`MCP connection for ${name} was closed while connecting`)
     }
     connections.set(name, connection); return connection
   } finally { delete connectPromises[name] / connectAttempts[name] IF still identical }
```

Two subtleties. A `needs-auth` connection is **not** returned from step 6 — a fresh attempt runs
every time, carrying `credentialsInvalidated` forward so the credential cache is invalidated at most
once per episode. And `createConnection` receives *two* signals: `attemptSignal` gates the connect
itself, while `ownedSignal` is what goes into `RequestOptions` — so a per-attempt abort tears down
the transport without poisoning the request-options object later per-call requests reuse.

In cyrup, `combineAbortSignals` has no direct analogue: `RunCancel::child()` covers parent→child but
not two independent parents. A small `combine(a, b) -> CancelToken` helper (a forwarder task, or a
`DropGuard`) lives in `cyrup-mcp`; the cost is one task per combined pair, and it is named rather
than papered over. `cyrup_core::CancelToken` is a re-export of `tokio_util::sync::CancellationToken`,
which is exactly the type `rmcp::serve_client_with_ct` takes, so the combined token binds the
connection with no adapter layer.

#### 3.8 Abort during connect, and once-only cleanup

```
throwIfAborted(signal)
closeTransport = () => { abortCleanup = Promise.resolve().then(() => transport.close());
                         abortCleanupPromises.set(transport, abortCleanup) }
signal?.addEventListener("abort", closeTransport, { once: true })
try   { await abortable(client.connect(transport, requestOptions), signal); await abortCleanup }
catch (error) {
        if abortCleanup { try { await abortCleanup }
                          catch (ce) { throw new AggregateError([error, ce], "MCP connection abort cleanup failed") } }
        throw error }
finally { signal?.removeEventListener("abort", closeTransport) }
```

The `WeakMap` (keyed on `object`, not `Transport`) exists so the outer handler awaits **that exact**
close rather than issuing a second `client.close()`, which the SDK does not tolerate. The success
path also awaits `abortCleanup`, so a close that started during a racing abort is not orphaned.

Much of this collapses in cyrup. `serve_client_with_ct(handler, transport, ct)` owns the whole
initialise-or-fail path: on `Err`, the local transport is dropped, and `TokioChildProcess`'s
`ChildWithCleanup::drop` spawns a `kill()` task, so no child survives an aborted connect. What
survives as adapter policy is (a) the *distinction* between a connect failure and a cleanup failure,
and (b) the once-only guarantee for the HTTP retry ladder, where the adapter (not rmcp) owns the
per-attempt teardown. In Rust the `WeakMap` becomes a field on the connect-attempt handle —
`Option<futures::future::Shared<BoxFuture<'static, Result<(), Arc<Error>>>>>` — because `Shared` gives
the "await the same future from several places" property, and Rust has no weak object-identity map.

#### 3.9 Post-connect: instructions, `onclose`, discovery

Order: `throwIfAborted` → connect (skipped when already connected) →
`attachAdapterNotificationHandlers` → `instructions = client.getInstructions?.()` → build the
`ServerConnection` with `instructions` present only when `!== undefined` → install the
identity-guarded `client.onclose` → `Promise.all([fetchAllTools, fetchAllResources, fetchAllPrompts])`
→ assign `tools`/`resources`/`prompts`/`promptDiscoveryFailed`.

* `fetchAllTools` — **unconditional**, cursor loop, `result.tools ?? []`, `do…while(cursor)`. Errors
  propagate.
* `fetchAllResources` — **capability-gated**: when `getServerCapabilities?.()?.resources` is absent,
  return `[]` **without a request**. On error: re-throw if `requestOptions?.signal?.aborted`;
  re-throw if `isUnauthorizedHttpError`; otherwise **swallow and return `[]`**, with no log.
* `fetchAllPrompts` — capability-gated on `.prompts` → `{prompts: [], failed: false}`. On error:
  re-throw on abort; re-throw on 401; else `` logger.debug(`MCP: prompts/list failed: ${message}`) ``
  and return `{prompts: [], failed: true}`. The `failed` flag is what distinguishes "server has no
  prompts" from "we could not ask", which the prompt-command layer needs.

In cyrup, `Peer::list_all_tools` / `list_all_prompts` / `list_all_resources` own the cursor loops.
The capability gate reads `RunningService::peer_info()` → `InitializeResult.capabilities`
(`ServerCapabilities { prompts: Option<PromptsCapability>, resources: Option<ResourcesCapability>, .. }`).
The three run under `tokio::join!` and are reduced — **not** `try_join!`, which short-circuits and
would cancel the siblings on the first error, destroying the per-list failure policy.

#### 3.10 `list_changed` handlers, and the URL-elicitation registry

All three `list_changed` handlers are identical in shape:

```
if (error) { logger.debug(`MCP: <kind>/list_changed refresh failed for ${serverName}: ${error.message}`); return }
if (!list) return
connection = connections.get(serverName)
if (!connection || connection.client !== client || connection.status !== "connected") return
connection.<field> = list
[prompts only] connection.promptDiscoveryFailed = false
metadataListChangedListener?.(serverName, "<kind>-list-changed")
```

Reason strings, byte-exact: `"tools-list-changed"`, `"prompts-list-changed"`,
`"resources-list-changed"`. In cyrup the notification carries no list, so the handler re-calls
`list_all_*` after the identity check; the identity check is `Arc::ptr_eq` against the live map.

`rememberUrlElicitation(serverName, elicitationId)` records an accepted URL elicitation per server
and is a **no-op when `runtimeSignal?.aborted`**. When the server sends
`notifications/elicitation/complete`, the id is removed from the set and — **only if `Set.delete`
returned true** — the user is told `MCP browser interaction for <server> completed. You can retry the
tool now.` at info level. The handler is registered only when `allowUrl` and is gated on
`!runtimeSignal?.aborted`. The set is cleared for one server on `close(name)` and wholesale on
`closeAll`. `handleUrlElicitationRequired` (public) returns `"cancel"` outright when aborted or when
`allowUrl` is off, otherwise walks every elicitation in the error and returns the first non-`accept`
action, else `"accept"`. rmcp does not model `notifications/elicitation/complete` as a first-class
notification, so it arrives at `ClientHandler::on_custom_notification`.

#### 3.11 Reconnect

```
if isServerDisabled(definition)  throw Error(`MCP server "${name}" is disabled`)
if this.stopped                  throw Error("MCP server manager is closed")
ownedSignal = combineAbortSignals(runtimeSignal, signal); throwIfAborted
inFlight = reconnectPromises.get(name); if inFlight return abortable(inFlight, ownedSignal)
promise = doReconnect(...).finally(() => { if reconnectPromises.get(name) === promise delete })
reconnectPromises.set(name, promise); return abortable(promise, ownedSignal)
```

Both guards are load-bearing and are *not* inherited from `connect` — a `reconnect` on a
just-disabled server must fail **before any teardown happens**.

```
doReconnect:
  throwIfAborted(signal)
  current = connections.get(name)
  if current !== staleConnection            return current ?? this.connect(name, definition, signal)
  staleInFlight = staleConnection.inFlight
  await this.close(name)
  fresh = await this.connect(name, definition, signal)
  fresh.inFlight = max(fresh.inFlight, staleInFlight)
  return fresh
```

The `inFlight` carry-over is what stops the health-check idle sweep from closing a server that has
calls waiting on the retry.

#### 3.12 Close, closeAll and error aggregation

```
close(name):
  closeGenerations[name] = (closeGenerations[name] ?? 0) + 1
  connectAttempts[name]?.abort(new Error(`MCP connection ${name} was closed`))
  connection = connections.get(name)
  if (!connection) {
      pending = closePromises[name];   if pending { await pending; return }
      pending = connectPromises[name]; if pending { try { await pending } catch (e) { if containsCleanupFailure(e) throw e } }
      return
  }
  connection.status = "closed"
  connections.delete(name); acceptedUrlElicitations.delete(name)      // BEFORE awaiting SDK cleanup
  closing = disposeConnection(connection).finally(() => { if (closePromises[name] === closing) delete closePromises[name] })
  closePromises[name] = closing
  return closing
```

`disposeConnection` = `Promise.allSettled([client.close(), traceWriter?.flush() ?? resolve()])`;
rejections → `AggregateError(failures, "MCP connection cleanup failed")`. Only `client.close()` is
called — the client owns its transport.

`closeAll`: set `stopped = true`; for the union of `connections` and `connectPromises` keys, bump the
generation and abort the attempt; snapshot `pendingConnects` and `currentNames`; `allSettled` the
pending connects; `allSettled` `close(name)` over `currentNames`; **then re-read `connections.keys()`
and close again** — the late sweep that catches a connect which resolved and inserted itself during
the first pass; filter all rejections through `containsCleanupFailure`; clear
`acceptedUrlElicitations`, null `samplingConfig` and `elicitationConfig` so a late callback cannot
re-enter a dead runtime, `await traceWriter?.flush()` last; rejections →
`AggregateError(failures, "MCP manager cleanup failed")`.

`containsCleanupFailure(error)` is an **iterative** walk (explicit `pending` stack plus a `seen` set,
so a cyclic chain terminates) over `Error`s; for an `AggregateError` it tests
`/cleanup failed|setup failed/` against `.message` and pushes `.errors`; it always pushes `.cause`
when defined. It returns true on the first match. This is the predicate that decides whether a
failure is a *teardown* failure worth surfacing versus an ordinary connect failure that is expected
during shutdown.

The five aggregate messages that must be reproduced byte-exactly, because `containsCleanupFailure`
and `createConnection`'s catch both pattern-match on them: `"MCP connection abort cleanup failed"`,
`"MCP connection setup failed"`, `"MCP HTTP connection cleanup failed"`,
`"MCP connection cleanup failed"`, `"MCP manager cleanup failed"`.

`createConnection`'s catch, in order: look up `abortCleanupPromises.get(transport)`; if the error is
*itself* the abort-cleanup aggregate, run **no** cleanup; else `allSettled([abortCleanup ?? client.close()])`;
if any cleanup failed, `reportedError = AggregateError([error, ...cleanupFailures], "MCP connection setup failed")`;
**only if there were zero cleanup failures** may a 401 be downgraded to `needs-auth` — "a cleanup
failure remains a setup failure rather than being hidden behind needs-auth"; finally the stderr-tail
enrichment (§3.3).

**Child teardown in cyrup.** `TokioChildProcess::close()` is `graceful_shutdown()`: close the
transport (which drops the child's stdin), then `select!` between the child's `wait()` and a
`MAX_WAIT_ON_DROP_SECS = 3` sleep, killing on timeout. `ChildWithCleanup::drop` additionally spawns a
`kill()` task so a dropped transport never leaves a zombie. The delta against the SDK's stdio
escalation (close stdin → 2 s → SIGTERM → 2 s → SIGKILL) is: **one 3-second window instead of two
2-second windows, and no SIGTERM leg** — a server that ignores stdin closure but would have honoured
SIGTERM is hard-killed. Both signal a single pid rather than a process group, which is correct
*because* npx pre-resolution (§3.3 step 3) removes the npm launcher that would otherwise be the
grandparent. Record the delta; do not reach for a process group without evidence of an orphan.

#### 3.13 Request options, timeouts and idle accounting

`normalizeRequestTimeoutMs(v)` = `typeof v === "number" && Number.isFinite(v) && v > 0 ? v : undefined`.

`getResolvedRequestTimeoutMs(def)`: if `def?.requestTimeoutMs !== undefined` use
`normalize(def.requestTimeoutMs)` — **an invalid per-server value yields `undefined`; it does not
fall back to the global default**; else `this.defaultRequestTimeoutMs`. `setDefaultRequestTimeoutMs`
normalises on the way in. `init.ts` feeds it `config.settings?.requestTimeoutMs`.

`buildRequestOptions(def, signal)`: `ownedSignal = combineAbortSignals(runtimeSignal, signal)`; if
neither a signal nor a timeout exists, return `undefined` — no options object at all.

In cyrup this is `PeerRequestOptions { timeout, meta, reset_timeout_on_progress, max_total_timeout }`.
`reset_timeout_on_progress` and `max_total_timeout` have no upstream analogue and must be left at
their defaults for parity; naming them here so nobody switches them on as an "improvement".

Idle/liveness: `touch(name)` sets `lastUsedAt = Date.now()`; `incrementInFlight`/`decrementInFlight`
(the latter guarded on truthiness so it never goes negative); `isIdle(name, timeoutMs)` requires the
connection to exist **and** `status === "connected"` **and** `inFlight === 0` **and**
`(Date.now() - lastUsedAt) > timeoutMs` (strict `>`).

Both `getPrompt` and `readResource` wrap the call in
`touch → incrementInFlight → … → finally { decrementInFlight; touch }` — `touch` runs **twice**, so a
long call cannot be reaped mid-flight and the clock restarts on completion. Both throw
`` `Server "${name}" is not connected` `` when the connection is missing or not `connected`.
`readResource` additionally re-checks `isServerDisabled(this.connections.get(name)?.definition)`
**first**, throwing `` `MCP server "${name}" is disabled` `` — this catches a server disabled in
config *after* it connected. `getPrompt` omits `arguments` entirely when `args` is falsy. Both pass
`this.getRequestOptions(name, signal)`, i.e. options rebuilt per call.

**Startup concurrency limit**: `parallelLimit(startupServers, 10, …)` — `Math.min(limit, items.length)`
workers pulling from one shared `items.entries()` iterator, results written by original index. Owned
by section 04 (`init.ts` has two call sites), cited here because it is the only concurrency bound on
connection creation.

#### 3.14 Probe

Constants: `PROBE_TIMEOUT_MS = 5_000`, `MODERN_PROTOCOL_VERSION = "2026-07-28"`,
`LEGACY_PROTOCOL_VERSION = "2025-06-18"`, `JSON_ACCEPT = "application/json, text/event-stream"`,
`SSE_ACCEPT = "text/event-stream"`, `MODERN_FALLBACK_STATUSES = {400,401,404,405,406,415}`,
`POST_ENDPOINT_MISMATCH_STATUSES = {404,405,406,415}`.

| strategy | method | headers | body | `allowJson` |
|---|---|---|---|---|
| `modern` | POST | `Accept: application/json, text/event-stream`, `Content-Type: application/json`, `MCP-Protocol-Version: 2026-07-28`, `Mcp-Method: server/discover` | `{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{}}` | true |
| `legacy-post` | POST | `Accept: application/json, text/event-stream`, `Content-Type: application/json` | `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"pi-mcp-probe","version":"2.1.2"}}}` | true |
| `legacy-sse` | GET (no `method` key, so `fetch` defaults to GET) | `Accept: text/event-stream` | — | false |

All three hit the **same** URL — `legacy-sse` is a GET against the configured endpoint, not a
separate `/sse` path. Every request carries a 5 s timeout and **no** credentials, cookies or config
headers: the probe is deliberately unauthenticated.

**Do not confuse this probe's pinned versions with the OAuth discovery pre-flight's.** The endpoint
probe hard-codes `MODERN_PROTOCOL_VERSION`/`LEGACY_PROTOCOL_VERSION` above because it is *testing*
for a specific revision. The OAuth pre-flight — `mcp-auth-flow.ts`'s `probeAuthDiscovery`, the other
unauthenticated `POST` this port sends, whose only purpose is to read `WWW-Authenticate` — instead
sends `protocolVersion: LATEST_PROTOCOL_VERSION`, and
**`LATEST_PROTOCOL_VERSION = "2025-11-25"`**, imported from `@modelcontextprotocol/client` and
defined in `@modelcontextprotocol/core`, both pinned to an exact `2.0.0` (no caret) in v2.25.0's
`package.json`. That value is **already the Rust default**: rmcp's `ProtocolVersion::LATEST` is
`V_2025_11_25` = `"2025-11-25"`, so a port that sends `ProtocolVersion::LATEST` reproduces the
upstream byte exactly and needs no literal of its own. It is *not* `MODERN_PROTOCOL_VERSION`
(`"2026-07-28"`), which only ever appears on the endpoint probe and on the pinned
`protocolVersion: "2026-07-28"` negotiation arm of §3.6. Section 07's MCP-309 owns whether the
pre-flight is sent at all (proactive probe vs. reactive challenge); this note fixes the constant so
whichever arm wins carries the right revision.

`classifyResponse`:
1. `isSse` = lowercased `content-type` starts with `text/event-stream`. `response.ok && isSse` →
   `{isMcp:true, classification:"endpoint responded with an MCP event stream"}`.
2. The JSON envelope is read **only** when `strategy.allowJson || status === 401`, via
   `jsonRpcEnvelopeInfo(JSON.parse(await response.text()))` inside a swallow-all `try`.
3. `response.ok && allowJson && envelope`: for `modern`, if `envelope.kind === "error"` **or**
   `envelope.protocolVersion !== "2026-07-28"` → `unsupported-modern`; otherwise `isMcp:true` with
   `` `endpoint supports stateless MCP 2026-07-28 server/discover` `` (modern) or
   `"endpoint responded with a JSON-RPC 2.0 envelope"` (legacy).
4. `status === 401 && isBearerChallenge && envelope` → `isMcp:true`, classification
   `` `endpoint requires Bearer authentication during MCP 2026-07-28 server/discover probing` `` (modern)
   or `"endpoint requires Bearer authentication and responded with a JSON-RPC 2.0 error"`.
5. else `unrecognized`.

`jsonRpcEnvelopeInfo`: object, non-null, `jsonrpc === "2.0"`; `"result" in value` →
`{kind:"result", protocolVersion: (typeof result === "object" && result !== null) ? result.protocolVersion : undefined}`;
`"error" in value` → `{kind:"error"}`; else `null`.
`isBearerChallenge`: `/(?:^|,)\s*Bearer\b/i` over the `www-authenticate` header (`?? ""`).
`responseKind`: content-type `.split(";", 1)[0]?.trim().toLowerCase()`; `text/html` → `"HTML"`; else
the content-type; else `"an untyped response"`.
`notMcp`: `` `endpoint returned ${responseKind(r)} (${r.status}) — this URL does not appear to speak MCP` ``.

Ladder: modern → if `mcp`, return. If the outcome is not `unsupported-modern` **and** the status is
not in `MODERN_FALLBACK_STATUSES` → `notMcp(modernResponse)`. legacy-post → if `mcp`, return; if the
status is not in `POST_ENDPOINT_MISMATCH_STATUSES` → `notMcp(postResponse)`. legacy-sse → `mcp` ?
result : `notMcp(getResponse)`.

Sole consumer: `enrichHttpConnectionError`, which wraps the *original* error as
`` new Error(`${originalMessage} — probe: ${probe.classification}`, { cause: error }) `` and —
critically — **swallows any probe failure**, returning the original error unchanged. A probe must
never be able to turn a connect failure into a different failure. The probe target is recomputed with
`resolveServerUrl(definition)!`, so a URL that only *now* fails to interpolate makes the whole
enrichment a no-op through the same catch.

**Cut-1 seam.** All three strategies survive; they are diagnostics, not transport selection. But the
final `legacy-sse` arm now describes a shape cyrup cannot connect to, so its success classification
would read "endpoint responded with an MCP event stream" attached to a connect failure. The port adds
one arm to the ladder, and only this one: when both POST strategies fail with an
endpoint-mismatch status and only the GET stream answers, classify as *"endpoint speaks the legacy
HTTP+SSE transport, which cyrup does not support"*. That is the whole point of the probe — making
the failure actionable — and it is a new string, recorded as a divergence rather than smuggled in.

#### 3.15 Session recovery

Constants: `CONNECTION_CLOSED_PROTOCOL_CODE = -32000`;
`SERVER_NOT_INITIALIZED_MCP_MESSAGES = {"Server not initialized", "Bad Request: Server not initialized"}`.

```
isTerminatedSession(err, hadSessionId):
  if !hadSessionId                       -> false      // hard gate
  if err is SdkHttpError                 -> err.status === 404
                                         || (err.status === 400
                                             && /"code"\s*:\s*-32000/.test(err.message)
                                             && /"message"\s*:\s*"Bad Request: Server not initialized"/.test(err.message))
  else                                   -> err is ProtocolError && err.code === -32000
                                            && SERVER_NOT_INITIALIZED_MCP_MESSAGES.has(err.message)
```

`hasSessionId(connection)` reads `connection.transport?.sessionId != null` — only the streamable-HTTP
transport exposes it; stdio (and test doubles with no `transport` at all) read as absent. It must be
captured **before** the call that produced the error, not read at catch time.

The module's negative space is a specification in its own right: it must **not** match broad error
messages without a prior session id, **not** match generic HTTP 400, **not** treat generic
`-32000`/ConnectionClosed as expiry, and **not** treat abort/cancellation as session failure.

```
withSessionRecovery(deps, serverName, fn):
 1 if isServerDisabled(config.mcpServers[serverName])  throw Error(`MCP server "${serverName}" is disabled`)
 2 connection = manager.getConnection(serverName);  if !connection throw Error(`Server "${serverName}" is not connected`)
 3 hadSessionId = hasSessionId(connection)                 // captured BEFORE the call
 4 try return await fn(connection)
   catch err:
 5   definition = config.mcpServers[serverName]            // re-read LIVE config, not the stale snapshot
 6   if definition && supportsOAuth(definition) && (err is UnauthorizedError || (SdkHttpError && 401))
          invalidateAuthEntryCache(serverName)             // BEFORE the isTerminatedSession gate
 7   if !isTerminatedSession(err, hadSessionId)  throw err
 8   if !definition                              throw err // server removed from config meanwhile
 9   throwIfAborted(deps.signal)
10   logger.debug(`MCP session for "${serverName}" expired; reconnecting`, { server: serverName })
11   fresh = await manager.reconnect(serverName, definition, connection[, deps.signal])
12   throwIfAborted(deps.signal)
13   if fresh.status === "needs-auth"  fresh = await deps.onNeedsAuth?.(serverName) ?? fresh; throwIfAborted
14   if fresh.status === "needs-auth"  throw new SessionRecoveryAuthRequiredError(serverName)
15   if fresh.status !== "connected"   throw err           // the ORIGINAL error, not a new one
16   return fn(fresh)                                      // retried EXACTLY once
```

`SessionRecoveryAuthRequiredError` carries `serverName` and an optional `authMessage`, defaulting to
`` `MCP server "${serverName}" requires OAuth authentication after reconnect.` ``.

The safety argument for retrying is load-bearing: the spec requires the server to reject a
stale-session request *before processing it*, so the retry cannot double-execute. That reasoning does
not extend to any other error class, which is why the predicate is so narrow.

**rmcp overlap, stated precisely.** `StreamableHttpError::SessionExpired` is raised on exactly
`status == NOT_FOUND && session_was_attached` — upstream's first arm, gate included. rmcp also offers
`StreamableHttpClientTransportConfig::reinit_on_expired_session`, a bounded single-attempt in-transport
recovery that replays `initialize`, re-establishes streaming state and retries the in-flight request.
It does **not** cover the 400/`-32000` arm or the `ProtocolError` arm, and it does not run the
manager-level reconnect that upstream's `onNeedsAuth` hook hangs off. See *What does not fit cleanly*.

**What survives a restart.** Nothing in the manager: `ServerConnection`, every transport, every
client, the `Mcp-Session-Id`, the in-flight counters and `acceptedUrlElicitations` are all
process-local and are discarded on session shutdown. Exactly three artefacts cross a process
boundary: `<agent_dir>/mcp-cache.json` (§3.17), `<agent_dir>/mcp-npx-cache.json` (§3.18) and the
OS-keychain OAuth entries (section 05). `withSessionRecovery` recovers from the **remote** server
losing its session table, never from the adapter restarting.

#### 3.16 Status snapshot

Channel `MCP_STATUS_EVENT = "pi-mcp-adapter/status/v1"`, payload version
`MCP_STATUS_SNAPSHOT_VERSION = 1`, `FAILURE_BACKOFF_MS = 60 * 1000`. The bus contract upstream is one
method: `interface McpStatusEventBus { emit(channel: string, data: unknown): void }`.

`getActiveFailureAgeSeconds`: `failedAt = failureTracker.get(name)`; **falsy** (so an epoch-`0`
timestamp counts as absent) → `undefined`; `ageMs > 60_000` → `undefined` (the failure has aged out);
else `Math.round(ageMs / 1000)`.

`createMcpStatusSnapshot(state)` iterates **`Object.keys(state.config.mcpServers)`** — JS insertion
order, i.e. **config-file order**. Per server:

```
disabled      = definition?.disabled === true
connection    = disabled ? undefined : manager.getConnection(name)
metadata      = disabled ? undefined : toolMetadata.get(name)
toolCount     = metadata?.length ?? (connection?.status === "connected" ? connection.tools.length : 0)
resourceCount = disabled ? undefined
                         : resourceCounts?.get(name) ?? (connection?.status === "connected" ? connection.resources.length : undefined)
failedAgo     = disabled ? undefined : getActiveFailureAgeSeconds(state, name)
```

Status precedence, first match wins: `disabled` → `"disabled"` (++disabledCount) ·
`connection?.status === "connected"` → `"connected"` (++connectedCount) ·
`connection?.status === "needs-auth"` → `"needs-auth"` · `failedAgo !== undefined` → `"failed"` ·
`metadata !== undefined` → `"cached"` · else `"not-connected"`.

Totals: `totalTools += disabled ? 0 : toolCount`; `totalResources += resourceCount` only when not
disabled and defined.

The emitted per-server object has **six** keys: `{ name, status, toolCount, resourceCount?,
failedAgoSeconds?, disabled }`. `resourceCount` and `failedAgoSeconds` are **omitted when absent**
(never `null`), and `failedAgoSeconds` is present only when `status === "failed"` **and** it is
defined. `disabled: boolean` is **always** emitted, even for enabled servers — it duplicates
`status === "disabled"` and consumers read both. The envelope is
`{ version, servers, totalTools, totalResources, connectedCount, disabledCount }`.

`publishMcpStatusSnapshot(state, snapshot?)` takes an **optional pre-built snapshot** so a caller that
already built one does not rebuild it, returns immediately when the bus is undefined, and wraps the
emit in `try {} catch {}` — "Event consumers must not be able to interrupt MCP operations".
`publishMcpStatusShutdown(events)` takes the **bus itself**, not the state, and emits the literal
all-zero snapshot with `servers: []` under the same swallow.

In cyrup the snapshot stays **in-crate**, published on a `tokio::sync::watch` that the `/mcp` panel,
the footer status segment and the proxy tool's `status` mode read. There is no consumer for a bus
topic in the workspace, and building the emit path for a consumer that does not exist would be a dead
primitive. The config server map **must** be insertion-ordered — a `Vec<(String, ServerEntry)>` or an
ordered map, never a `BTreeMap` — or the panel and the footer list servers alphabetically instead of
as configured.

#### 3.17 The metadata cache — a frozen cross-crate contract

This is the hardest external constraint in the port. `cyrup-mcp` is the **writer** of a file whose
**reader** already exists in `cyrup_ext_subagents::exec::mcp_direct_tools`. Both sides are specified
here, and every mismatch is flagged.

**Path.** Upstream: `getMetadataCachePath()` = `getAgentPath("mcp-cache.json")` =
`join(getAgentDir(), "mcp-cache.json")`, where `getAgentDir()` reads **`PI_CODING_AGENT_DIR?.trim()`
only**, defaults to `join(homedir(), ".pi", "agent")`, maps `~` → `homedir()`, `~/x` →
`resolve(homedir(), x)`, and otherwise `resolve(configured)` — i.e. **absolutised against cwd**.

cyrup diverges on three axes, and only the first is sanctioned:

1. **Rebrand (sanctioned).** cyrup reads `CYRUP_AGENT_DIR` first, falls back to `PI_CODING_AGENT_DIR`,
   and defaults to `<home>/.cyrup/agent`. This is the workspace convention (`cyrup_config`'s env
   layer). Keep it — but record that a pi-era `~/.pi/agent/mcp-cache.json` is invisible to cyrup and
   vice versa.
2. **`resolve()` is dropped.** Both in-tree Rust copies build a `PathBuf` from the configured value
   directly, so a relative `CYRUP_AGENT_DIR=foo` stays relative where upstream would absolutise it.
3. **The two in-tree resolvers disagree with each other on `home`.**
   `npx_resolver`'s `agent_dir` anchors on `caps::proc::host_home_dir` = `HOME` → `USERPROFILE` →
   `PathBuf::from(".")` and `.trim()`s the env value; `mcp_direct_tools`'s `resolve_agent_dir` anchors
   on its own `home_dir` = `CYRUP_HOME` → `HOME` → `std::env::temp_dir()` and does **not** trim. With
   `CYRUP_HOME` set to anything other than `HOME`, `mcp-cache.json` and `mcp-npx-cache.json` land in
   **different directories**; with neither var set they land in `./.cyrup/agent` and
   `<tmpdir>/.cyrup/agent` respectively. A third resolver exists in `cyrup-config`
   (`ConfigDirs::agent_dir`). This is the configuration CI and subagent isolation actually use —
   `cyrup_permission_system::forwarding` writes an isolated `CYRUP_AGENT_DIR` into child env.

**Version.** Upstream `CACHE_VERSION = 1`; the Rust reader's `CACHE_VERSION: i64 = 1`. **Agreed.**
Do **not** bump it to drop the now-dead UI fields (Cut 2) — the schema is a contract and the fields
are simply absent and ignored.

**TTL.** Upstream `CACHE_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000`; the Rust reader's `CACHE_MAX_AGE_MS`
is identical. **Agreed.** The comparisons differ only at the boundary: upstream invalidates on
`Date.now() - cachedAt > maxAgeMs`, the Rust treats `now_ms() - cached_at <= CACHE_MAX_AGE_MS` as
valid — the same predicate.

**JSON schema.** The writer must produce this in full even though the in-tree reader models a subset.
`tools` and `resources` are **required** members of a server entry; `prompts` and `instructions` are
optional.

```jsonc
{
  "version": 1,
  "servers": {
    "<serverName>": {
      "configHash": "<64 lowercase hex>",     // required
      "tools": [ { "name": "…",               // required member; each entry requires name
                   "description": "…",         // optional
                   "inputSchema": {…},         // optional, raw JSON Schema
                   "uiVisibility": ["…"] } ],  // optional; kept — see Cut 2 seam below
      "resources": [ { "uri": "…", "name": "…", "description": "…"? } ],   // required member
      "prompts":   [ { "name": "…", "title": "…"?, "description": "…"?,
                       "arguments": [ { "name": "…", "description": "…"?, "required": bool? } ]? } ],
      "instructions": "…",                     // optional
      "cachedAt": 1760000000000                // required, epoch ms
    }
  }
}
```

The Rust reader models `configHash`, `tools[].name`, `resources[].{uri,name}` and `cachedAt` only,
with no `deny_unknown_fields`, so the extra members round-trip harmlessly — but they **must** be
written, because the adapter's own cold-start tool registration reads them.

**Cut-2 seam inside the schema.** `uiResourceUri` and `uiStreamMode` become dead and are simply not
written (absent, not null). `uiVisibility` **stays**: `reconstructToolMetadata` filters on
`isUiToolVisibleToModel(tool.uiVisibility)`, and dropping it would expose to the model tools the
server explicitly marked app-only — a behaviour change in the wrong direction. Keep all three field
*names* reserved in the on-disk schema and do not renumber `CACHE_VERSION`.

**Load.** Missing file → `null`; parse error → `null`; non-object → `null`; `raw.version !== 1` →
`null`; `raw.servers` missing or non-object → `null`. **Never throws.**

**Save** — read-merge-write-rename, **per-server overlay, never a replacement**:

```
mkdirSync(dirname(cachePath), { recursive: true })
merged = { version: 1, servers: {} }
try { if exists: existing = JSON.parse(read); if existing.version === 1 && existing.servers: merged.servers = {...existing.servers} } catch {}
merged.version  = 1
merged.servers  = { ...merged.servers, ...cache.servers }
tmp = `${cachePath}.${process.pid}.tmp`
writeFileSync(tmp, JSON.stringify(merged, null, 2), "utf-8")
renameSync(tmp, cachePath)
```

Three properties to preserve: **2-space pretty JSON** (the file is user-inspectable), **entries are
never pruned** (a server deleted from config keeps its cache entry until the file is deleted), and the
merge does **no per-entry validation** — a parseable `version:1` file's `servers` map is spread in
verbatim, malformed entries included. There is **no file lock**; concurrency rests on the pid-suffixed
temp name plus `rename`'s atomicity, so two concurrent processes are last-writer-wins on the whole
map. Do not add a lock: it changes cross-process behaviour from last-writer-win to blocking.

**`computeServerHash(definition)`** — SHA-256 over `stableStringify(identity)`, hex, where `identity`
has **exactly these 14 keys**:

| key | value expression | in the Rust reader today |
|---|---|---|
| `command` | `definition.command` (raw) | present |
| `args` | `definition.args` (raw array) | present |
| `socket` | `resolveConfigPath(definition.socket)` | **ABSENT** |
| `env` | `interpolateEnvRecord(definition.env)` | present, but see the `!`/`!!` mismatch |
| `cwd` | `resolveConfigPath(definition.cwd)` | present |
| `url` | `resolveServerUrl(definition)` — interpolated, **throws** on missing var / invalid URL | present but **raw**, never interpolated, cannot throw |
| `headers` | `interpolateEnvRecord(definition.headers)` | present, same `!`/`!!` mismatch |
| `auth` | `definition.auth` (raw: `"oauth"`/`"bearer"`/`false`/absent) | present |
| `protocolVersion` | `definition.protocolVersion` (raw) | **ABSENT** |
| `bearerToken` | `resolveBearerToken(definition)` | present, same `!`/`!!` mismatch |
| `bearerTokenEnv` | `definition.bearerTokenEnv` (raw) | present |
| `exposeResources` | `definition.exposeResources` (raw bool) | present |
| `includeTools` | `definition.includeTools` (raw array) | **ABSENT** |
| `excludeTools` | `definition.excludeTools` (raw array) | present |

Deliberately excluded upstream, with the reason given in the source: `lifecycle`, `idleTimeout`,
`requestTimeoutMs`, `debug` — runtime behaviour that does not change which tools a server exposes.
Also absent in practice: `disabled`, `directTools`, `toolPrefix`, `searchKeywords`, `approveTools`,
`trace`, `httpTransport`, `pluginDataDir`, `literalEnv`, `oauth`.

**Cut-3 seam, and it is subtle.** The `socket` transport is cut, but the `socket` **key stays in the
hash pre-image**. Because a cyrup config can never carry a `socket` value (it is rejected at load,
§3.2), the value is always absent and hashes as the `undefined` token — which is exactly what it
hashes as for the overwhelming majority of upstream servers too. Keeping the key costs one line and
preserves digest compatibility with every pi-written cache; dropping it changes the digest for
**every** server. Same reasoning for `protocolVersion`, whose transport half is cut but whose config
field stays live anyway (§3.6).

**`stableStringify`**:

```
non-object (incl. null/undefined):  s = JSON.stringify(value);  return s === undefined ? "undefined" : s
array:                              `[${parts.join(",")}]`
object:                             keys sorted lexicographically; `{${`${JSON.stringify(k)}:${stringify(v)}`.join(",")}}`
```

The `undefined` branch emits the **bare 9-character token `undefined`**, which is not valid JSON.
Since the identity object always carries every key and a typical server sets two or three, the
pre-image for a plain stdio server is exactly (no line breaks in the real pre-image):

> **Count note.** "14" was written before v2.26.0 added `requestHeadersCommand`; at `v2.26.1`
> (`fafae21`) the identity object has **15** keys and the listing below is missing that one.
> `socket` — which the listing *does* carry, correctly — is the fifteenth in the Rust port, and both
> `cyrup_mcp::dirs::server_identity_pre_image` and `mcp_direct_tools`'s twin now emit it. Verified by
> running upstream's own `computeServerHash` on node 22, not by counting the prose.

```
{"args":["-y","@modelcontextprotocol/server-filesystem"],"auth":undefined,"bearerToken":undefined,
 "bearerTokenEnv":undefined,"command":"npx","cwd":undefined,"env":undefined,"excludeTools":undefined,
 "exposeResources":undefined,"headers":undefined,"includeTools":undefined,"protocolVersion":undefined,
 "socket":undefined,"url":undefined}
```

The Rust reader's `stable_stringify` maps `Value::Null → "null"`, and every absent field is
materialised as `Value::Null` by `opt_str_value`, by `interpolate_env_record`'s absent arm, and by the
`.unwrap_or(Value::Null)` arms for `args`, `auth`, `exposeResources` and `excludeTools`. That is a
different digest for **essentially every server**.

**`interpolateEnvRecord(values)`** returns `undefined` for an absent map and otherwise maps each value
through `interpolateSecretExpression`: `!!X` → `interpolateEnvVars(value.slice(1))` (one `!` consumed,
so a leading `!` survives); `!X` → **returned verbatim, unexecuted** (hashing must never run a
command); otherwise `interpolateEnvVars(value)`. `resolveBearerToken` is
`interpolateSecretExpression(bearerToken)` when `bearerToken !== undefined`, else
`process.env[bearerTokenEnv]` when `bearerTokenEnv` is truthy, else `undefined`. The Rust reader's
`interpolate_env_record` and `resolve_bearer_token` call plain `interpolate_env_vars` instead — so
`!!X` is not un-escaped and `!cmd` **is** interpolated rather than passed through. Its
`interpolate_env_record` additionally **drops non-string values** where upstream would attempt
`.startsWith` on them.

**`interpolateEnvVars`** applies **three** sequential `String.replace` passes over the whole string,
each with `\w+` names and a `?? ""` fallback: `${NAME}`, then `$env:NAME`, then `{env:NAME}`. Order
matters, because each pass runs over the previous pass's output. `getMissingEnvVars` matches the same
three alternatives in one alternation, so a `{env:MISSING}` in a URL must also raise the
missing-variable throw. **Both** in-tree Rust implementations have only the first two patterns:
`caps::proc`'s `interpolate_env_vars_with` (= `interpolate_braces` + `interpolate_dollar_env`) and
`mcp_direct_tools`'s `interpolate_env_vars` (two passes through `expand_pattern`). `resolve_config_path`
inherits the gap in both.

**`resolveConfigPath`** interpolates, then expands a leading `~` (exactly `"~"` → `homedir()`; a `~/`
or `~\` prefix → `join(homedir(), rest)`), else returns the interpolated string.

**`isServerCacheValid(entry, definition, maxAgeMs = 7d)`**: compute the hash inside a `try`,
**returning `false` on throw** — this is the sole mechanism by which a URL server referencing a
missing environment variable is kept out of the cold-start tool surface; then
`!entry || entry.configHash !== hash` → false; `!entry.cachedAt || typeof !== "number"` → false;
`maxAgeMs > 0 && Date.now() - cachedAt > maxAgeMs` → false; else true. A `maxAgeMs` of `0` disables
the age check entirely. The Rust reader has no `max_age_ms` parameter and its hash function cannot
throw, so the missing-variable rule is absent there.

**Serialisers.** `serializeTools` filters on `t?.name`, then emits `name` plus each optional field
**only when defined** (never an explicit `null`) — `uiVisibility` via `extractUiToolVisibility(t._meta)`;
`uiResourceUri` and `uiStreamMode` are cut. `serializeResources` filters on `r?.name && r?.uri`.
`serializePrompts` filters on `prompt?.name` and each argument on `argument?.name`, emitting
`arguments` only when `Array.isArray(prompt.arguments)`.

**Reconstructors.** `reconstructToolMetadata` turns a cache entry back into the registerable surface.
In order, per tool: skip when `!tool?.name`; **skip when `!isUiToolVisibleToModel(tool.uiVisibility)`**;
skip when `!isToolAllowed(tool.name, serverName, effectivePrefix, includeTools, excludeTools,
getOtherCurrentCandidates(tool.name))`; `name = formatToolName(...)`, skip when already in `seenNames`,
insert — **first wins**; push `{ name, originalName: tool.name, description: tool.description ?? "",
inputSchema?, uiVisibility? }`, where `description` defaults to the **empty string**, not to
`undefined`.

Resource tools are emitted only when **`definition.exposeResources !== false`**, filtered on
`resource?.name && resource?.uri`, named `` `read_${resourceNameToToolName(resource.name)}` ``, run
through the same `isToolAllowed`/`seenNames` gates, and pushed as `{ name, originalName: baseName,
description: resource.description ?? \`Read resource: ${resource.uri}\`, resourceUri: resource.uri }`.

`getOtherCurrentCandidates` performs **cross-server collision detection** against every other server
that is cache-valid and not disabled, applying the same `isUiToolVisibleToModel` filter and the same
`exposeResources !== false` gate to the *other* server's entries, then deleting this server's own
candidates from the set.

`reconstructPromptMetadata` accepts `McpPrompt | CachedPrompt`, filters on `prompt?.name`, and emits
`{ serverName, originalName, commandName: formatPromptCommandName(...), title?,
description: prompt.description ?? "", arguments }` where `arguments` is **always an array** (empty
when absent).

`resourceNameToToolName` (`resource-tools.ts`): non-alphanumerics → `_`, collapse runs, strip leading
then trailing `_`, lowercase; if the result is empty or starts with a digit,
`result = "resource" + (result ? "_" + result : "")`.

> **`read_` vs `get_` — confirmed mismatch.** Upstream forms resource tool names as `read_<name>` in
> `metadata-cache.ts` (both the collision-candidate scan and the emission loop) and identically in
> `direct-tools.ts`; grepping `direct-tools.ts` at v2.25.0 for `get_` returns zero hits. The in-tree
> Rust reader's `resolve_direct_tool_names` builds `format!("get_{}", resource_name_to_tool_name(name))`.
> They disagree on **every** resource-backed tool name.

**Direct-tool selectors.** `parseDirectToolSelectors`: strip trailing `/`+; a selector containing `/`
splits at the **first** `/` via `split("/", 2)` — which in JS **discards** everything after the second
element — adding to `tools[server]` when both halves are non-empty; if only the server half is
non-empty it goes to `servers`; a non-empty selector without `/` goes to `servers`.
`getMissingConfiguredDirectToolServers`: skip disabled; a server "has direct tools" when the env
selection names it (either as a bare server or via a `server/tool` selector), else
`definition.directTools` if defined, else `settings.directTools`; a server with direct tools whose
cache entry is missing or invalid under the **default 7-day** `isServerCacheValid` is reported
missing — this is what forces `index.ts` to block on initialization at startup.

#### 3.18 `npx-resolver.ts` vs the shipped Rust port

`cyrup_ext::caps::proc::npx_resolver` is already a direct port. Upstream at v2.25.0 has moved on and
the Rust has six specific gaps.

**The named constants, both sides.** Upstream (`npx-resolver.ts`): `CACHE_VERSION = 2`;
**`CACHE_TTL_MS = 24 * 60 * 60 * 1000`** (24 hours — the age past which a cached resolution is
ignored, tested as `Date.now() - cached.resolvedAt < CACHE_TTL_MS`);
`EXACT_PACKAGE_VERSION_RE = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z][0-9A-Za-z.-]*)?(?:\+[0-9A-Za-z][0-9A-Za-z.-]*)?$/`;
**`FORCE_CACHE_TIMEOUT_MS = 30_000`** (30 s — the whole budget for the cold
`npm exec --yes --package <spec> -- node -e 1` warm-up, after which the resolution gives up); cache
path `getAgentPath("mcp-npx-cache.json")`. The Rust already carries three of the four under the same
names — `CACHE_TTL_MS` (24 h) and `FORCE_CACHE_TIMEOUT` (a `Duration::from_secs(30)`, so the `_MS`
suffix is dropped) **agree with upstream and must not move**; only `CACHE_VERSION` (still `1`)
diverges, which is gap 1. `EXACT_PACKAGE_VERSION_RE` has no Rust counterpart at all, which is gap 2.
Both surviving values bound the two units below: MCP-107's cancellation path exists because
`FORCE_CACHE_TIMEOUT` is 30 s of otherwise-uninterruptible wall clock, and MCP-105's version filter
must not weaken the `CACHE_TTL_MS` arm of the hit predicate it is being added beside.

Everything not listed matches: arg parsing for both `npx` and
`npm exec` shapes, `default_bin_name`, bin-candidate derivation, the `_npx` directory scan ordered by
descending mtime, `.bin` symlink → realpath preference with a `packageDir/binRel` fallback,
`detect_js_binary` (extension `.js`/`.mjs`/`.cjs`, else first 256 bytes shebang containing `node`),
the `NPM_CONFIG_CACHE` short-circuit with memoisation, `save_cache_entry`'s
read-merge-write-`${path}.${pid}.tmp`-rename, and the `npm exec --yes --package <spec> -- node -e 1`
force-cache argv. The Rust additionally holds a process-wide `SAVE_CACHE_LOCK` to restore the
single-writer property Node gets free from its event loop — a justified addition, documented in
place, not a divergence.

| # | upstream v2.25.0 | the Rust today | consequence |
|---|---|---|---|
| 1 | `CACHE_VERSION = 2`, with `clearLegacyCache()` unlinking (or truncating) a v1 file at module load **and** again on every `loadCache()`, which returns `null` when it evicted | `CACHE_VERSION: u32 = 1`; `load_cache_at` rejects a version mismatch but never deletes; no module-load eviction | version skew: a v1 file written by cyrup is deleted on sight by any co-installed pi adapter, and vice versa |
| 2 | `parsePackageSpec` extracts an **exact** version; the cache-hit predicate requires `!packageSpec?.exactVersion \|\| cached.packageVersion === packageSpec.exactVersion`; `findCachedPackageDir` skips `_npx` dirs whose `package.json` `version !== exactVersion` | `extract_package_name` discards the version; `NpxCacheEntry.package_version` **exists** and **is written** from `pkg.version`, but is never read — no version arm in the hit predicate, and `find_cached_package_dir` takes no `exact_version` | `npx -y srv@1.2.3` spawns whatever version has the newest `_npx` mtime. **Silently wrong binary** |
| 3 | `cacheKey = JSON.stringify([command, parsed.packageSpec, parsed.binName ?? ""])`, computed **after** parsing | `cache_key(command, args)` = `JSON.stringify([command, ...args])`, computed before the parse result is used | different key space: invocations differing only in trailing args do not share an entry, and `npx pkg bin` vs `npx --package pkg bin` never share. Cache thrash plus repeated cold `_npx` scans |
| 4 | `resolveNpxBinary(command, args, signal)` — `throwIfAborted` on entry, on `forceNpxCache` entry and on its exit; the force-cache child is killed on abort, rejecting with `signal.reason` or `Error("MCP request aborted")` | `resolve_npx_binary(command, args)` has no signal; `force_npx_cache` is a 50 ms-poll loop bounded only by the 30 s wall clock | a session shutdown cannot interrupt a cold resolution; up to 30 s of a blocking worker pinned past teardown |
| 5 | `toNpxCache`/`toNpxCacheEntry` validate **per entry** and drop only the bad ones; the entries map is `Object.create(null)` | `serde_json::from_str::<NpxCache>` fails the whole file on any malformed entry | one corrupt entry discards every cached resolution. Degrades to re-resolution, not incorrectness |
| 6 | `crossSpawn.sync` for `npm config get cache` and `crossSpawn` for the force-cache spawn — resolves `npm.cmd` on Windows | `std::process::Command::new("npm")` in both places — no PATHEXT/`.cmd` resolution | npx resolution is a silent no-op on Windows |

Two visibility notes. `resolve_npx_binary` and `NpxResolution` are `pub(super)` inside
`caps::proc`, so `cyrup-mcp` cannot call them without a `pub` promotion and a re-export — a one-line
change with no behavioural surface, not a host addition. And `caps` (with `host`) is behind
`cyrup-ext`'s `wasm-host` feature, which is default-on and which the binary links anyway; depending on
it costs `cyrup-mcp` a transitive Wasmtime edge and nothing else.

Also note the rewiring: upstream resolves npx **in the connection builder**, and cyrup's existing
`apply_npx_resolution` sits inside `ProcCaps::spawn` — the *WASM guest* spawn path. `cyrup-mcp` does
not go through `ProcCaps`; it calls `resolve_npx_binary` itself and applies the
`isJs ? "node" : binPath` rewrite before handing the command to `TokioChildProcess`. That is exactly
where upstream calls it.

---

### Port units

Verdicts: **rmcp** · **host-verb** (named) · **extension-owned** (the native crate does it itself) ·
**hand-written** (new code in `cyrup-mcp`) · **host-addition** · **open-decision** · **cut**.

**MCP-100 — `McpServerManager`: the five race guards and the full public API** · high · L · hand-written
**upstream** — `server-manager.ts`'s `McpServerManager`: the connection registry plus the
`connectPromises`/`reconnectPromises`/`closePromises` single-flight maps, `closeGenerations`,
`connectAttempts`, and the module-level `abortCleanupPromises` `WeakMap<object, Promise<void>>`. The
public surface is the eight setters, `getRequestOptions`, `isConnecting`, `getConnection` and
`getAllConnections` (which returns a **copy**).
**behavior** — A session restart, a `/mcp reconnect`, an idle sweep and a model tool call can all touch
the same server concurrently; none may tear down a connection another owns, none may resurrect a
connection whose generation has advanced, and a transport is closed exactly once. `lifecycle.ts`,
`mcp-status.ts` and `session-recovery.ts` are written against the accessor surface, so it is part of
the contract, not incidental.
**cyrup** — `cyrup-mcp::server_manager`. Maps become `Mutex<HashMap<String, Arc<ServerConnection>>>`;
`closeGenerations` is `HashMap<String, u64>`; `connectAttempts` is `HashMap<String, AbortHandle>`
where `AbortHandle { token: cyrup_core::CancelToken, reason: Mutex<Option<String>> }` — the token
carries no reason and `` `MCP connection ${name} was closed` `` reaches the user through the connect
error path. `combineAbortSignals` has no analogue (`RunCancel::child` is parent→child only): a small
`combine(a, b) -> CancelToken` helper, one forwarder task per pair, named as a cost. The abort-cleanup
`WeakMap` becomes an `Option<futures::future::Shared<BoxFuture<'static, Result<(), Arc<Error>>>>>` on
the connect-attempt handle. Identity guards are `Arc::ptr_eq`. Blocking-ness: nothing else in the
section runs until this exists.
**verify** — connect twice concurrently, assert one transport is created; fire `close` mid-connect and
assert the resolved connection is disposed and the **abort reason** `` `MCP connection <name> was
closed` `` is raised — *not* `` `MCP connection for <name> was closed while connecting` ``. That
second string is reachable only when the generation advanced **without** the attempt being aborted
(what `reconnect`/`closeAll` can produce), because step 10 runs `throwIfAborted(attemptSignal)`
*before* it, and `close` aborts the attempt controller with that reason
(`server-manager.ts:1099`, `:1147`). MEASURED, not read: replaying step 10
(`server-manager.ts:288-296`) on node 22 against `tmp/pi-mcp-adapter` @ `v2.26.1` (`fafae21`), with
upstream's own `combineAbortSignals` and `throwIfAborted`, yields `MCP connection filesystem was
closed` for a close racing a connect and `MCP connection for filesystem was closed while connecting`
for the generation-only case. Assert both. Then call `reconnect` with a stale handle after another
`connect` won and assert the fresh connection is returned untouched; assert `get_all_connections()`
returns a snapshot a subsequent `close` does not mutate.

**MCP-101 — stdio transport: spawn, env resolution, cwd, plugin data dir** · high · M · rmcp + extension-owned
**upstream** — `server-manager.ts`'s stdio branch of `createConnection`, plus `resolveEnv` and
`utils.ts`'s `resolveCommandSecret` / `resolveCommandSecretsRecord`.
**behavior** — §3.3. The child inherits the full host environment overlaid with per-server overrides
that are either interpolated + `!command`-resolved or taken verbatim under `literalEnv`; `args` are
interpolated but never command-resolved; `cwd` is `resolveConfigPath(definition.cwd) ?? defaultCwd`
and the key is **omitted entirely** when both are undefined; `pluginDataDir` is `mkdir -p`'d before
spawn. The five `!command` failure strings are user-visible verbatim, including the
`` `command exited with code ${result.status ?? "unknown"}` `` fallback.
**cyrup** — `rmcp::transport::TokioChildProcess::builder(cmd)` over a `tokio::process::Command`
configured through `ConfigureCommandExt` (`.envs`, `.current_dir`). Env resolution, `literalEnv`,
`!command` secret execution and `pluginDataDir` all live in `cyrup-mcp::secrets` / the connection
builder, using `std::process::Command` with `sh -c` / `cmd /c`, a 10 s timeout and a 1 MiB cap. A
native crate spawns its own children — nothing here touches `HostServices`, and the WASM-guest
`ProcCaps` grant is not on this path.
**verify** — spawn a fixture stdio server with `literalEnv: true` and an env value containing
`${HOME}`, assert the child sees the literal; spawn with `!echo hunter2` and assert the child sees
`hunter2`; assert `!false` surfaces
`Failed to resolve MCP server "x" stdio env "K": command exited with code 1`; assert `!!${HOME}`
yields `!` + the expanded home (one `!` consumed, not both).

**MCP-102 — stderr tail capture and failure-message enrichment** · medium · S · rmcp + hand-written
**upstream** — `server-manager.ts`'s `MAX_CAPTURED_STDERR_BYTES` / `MAX_CAPTURED_STDERR_LINES`,
`boundedStderrChunk`, `appendStderrTail`, the `stderr: definition.debug ? "inherit" : "pipe"` choice,
and the failure-suffix construction in `createConnection`'s catch.
**behavior** — A stdio server that dies during handshake produces
`<original message> (<last up to 3 non-empty trimmed stderr lines joined by " — ">)`. Memory is bounded
at 8 KiB regardless of how much the child writes. In `debug: true` mode stderr is inherited by the host
terminal instead of piped — so in debug mode there is **no** tail and therefore no `(...)` suffix.
**cyrup** — `TokioChildProcessBuilder`'s default is `Stdio::inherit()` (debug mode, exactly) and
`.stderr(Stdio::piped())` makes `spawn()` hand back `Option<ChildStderr>` (non-debug, exactly). The
tail is an 8 KiB `VecDeque<u8>` in the connection builder fed from that handle. Port
`bounded_stderr_chunk` / `append_stderr_tail` literally — the bound is applied before the
string→bytes conversion so a multi-megabyte burst never allocates in full.
**verify** — unit for `bounded_stderr_chunk`/`append_stderr_tail` against a 1 MiB burst and a chunk
that splits a multi-byte UTF-8 sequence at the 8 KiB boundary; integration: a fixture server that
writes 3 stderr lines then exits, asserting the exact `(a — b — c)` suffix, and a `debug: true` run
asserting no suffix.

**MCP-103 — Wire npx/npm resolution into the connection builder** · medium · S · extension-owned (reuse)
**upstream** — `server-manager.ts`'s `resolveNpxBinary` call and the `isJs ? "node" : binPath` rewrite.
**behavior** — When the configured command is `npx` or `npm`, the tracked child is the real MCP server
binary, not an npm launcher — otherwise `kill` leaves an orphaned grandchild, and the single-pid kill
in §3.12 stops being sufficient. A debug line
`` `${name} resolved to ${resolved.binPath} (skipping npm parent)` `` is logged on a hit.
**cyrup** — Call `cyrup_ext::caps::proc::npx_resolver::resolve_npx_binary` directly from
`cyrup-mcp`'s connection builder, before constructing the `tokio::process::Command`. `resolve_npx_binary`
and `NpxResolution` are `pub(super)` today and need a `pub` promotion plus a re-export from
`cyrup_ext::caps::proc` — a visibility change with no behavioural surface. Do **not** re-port the 892
lines, and do not route through `ProcCaps::spawn`, which is the WASM-guest path and applies the
resolution internally where `cyrup-mcp` cannot see whether it fired.
**verify** — assert the debug line is emitted on a hit and not on a miss; assert the resolved argv for
both the `isJs` and non-`isJs` arms matches upstream's rewrite.

**MCP-104 — npx cache: bump to `CACHE_VERSION = 2` and port `clearLegacyCache`** · medium · S · hand-written
**upstream** — `npx-resolver.ts`'s `CACHE_VERSION` and `clearLegacyCache`, invoked at module load
**and** on every `loadCache()`, which returns `null` when it evicted.
**behavior** — A stale v1 resolution cache is never trusted and never lingers on disk.
**cyrup** — In `cyrup_ext::caps::proc::npx_resolver`: set `CACHE_VERSION = 2`; add
`clear_legacy_cache() -> bool` called from `load_cache`, with the same `remove_file` → `write("")`
fallback and the same "return `None` when it evicted" short-circuit. The module-load invocation
becomes a `std::sync::Once` inside `load_cache`.
**verify** — write a `{"version":1,...}` fixture, call `load_cache_at`, assert `None` and that the file
is gone (or empty when unlink fails).

**MCP-105 — npx resolver: exact package-version pinning is missing** · high · M · hand-written
**upstream** — `npx-resolver.ts`'s `EXACT_PACKAGE_VERSION_RE`, `parsePackageSpec`, the cache-hit
predicate's `cached.packageVersion === packageSpec.exactVersion` arm, `findCachedPackageDir`'s
version filter, and `resolveFromNpmCache` recording `packageVersion: pkg.version` only when defined.
**behavior** — `npx -y pkg@1.2.3` must resolve to the `_npx` copy whose `package.json` `version` is
exactly `1.2.3`, and a cache entry recorded for a different `packageVersion` must be rejected.
`EXACT_PACKAGE_VERSION_RE` restricts this to full semver (optionally with pre-release/build metadata);
a range like `^1.2.0` carries no `exactVersion` and pins nothing. The requested version is normalised
by stripping a leading `=` then a leading `v`/`V`.
**cyrup** — `npx_resolver`'s `extract_package_name` discards the version. `NpxCacheEntry.package_version`
exists and is populated, but no code reads it. Add
`struct ParsedPackageSpec { package_name, exact_version: Option<String> }` and
`fn parse_package_spec(&str) -> Option<ParsedPackageSpec>` reproducing the `@scope/name`
`rfind('@') > find('/')` rule and the `^=` then case-insensitive `^v` strip; validate against the exact
upstream pattern with `regex`; thread `exact_version` into both the hit predicate and
`find_cached_package_dir`. `regex` is not a workspace dependency; the in-tree precedent is a per-crate
`regex = "1"` in `cyrup-permission-system`.
**verify** — a hermetic `_npx` fixture holding two versions of the same package with the wrong one
having the newer mtime; assert `pkg@1.0.0` resolves to the 1.0.0 copy, `pkg@^1.0.0` resolves to the
newest mtime, and a cache entry with a stale `packageVersion` is rejected.

**MCP-106 — npx resolver: cache key must be `[command, packageSpec, binName]`** · low · S · hand-written
**upstream** — `npx-resolver.ts`'s `cacheKey`, computed **after** parsing.
**behavior** — Two invocations of the same package/bin with different trailing arguments share one
cache entry; the cache is keyed on identity, not on the whole argv.
**cyrup** — `npx_resolver`'s `cache_key(command, args)` serialises `[command, ...args]` and runs
before the parse result is used. Move the computation after the parse and serialise
`[command, &parsed.package_spec, parsed.bin_name.as_deref().unwrap_or("")]`.
**verify** — two `resolve_npx_binary` calls differing only in trailing args produce one cache entry;
`npx pkg bin` and `npx --package pkg bin` produce the same key.

**MCP-107 — npx resolver: no cancellation path** · medium · S · hand-written
**upstream** — `npx-resolver.ts`'s `resolveNpxBinary(command, args, signal)` with `throwIfAborted`
before parsing, `forceNpxCache`'s abort check on entry, its kill-the-child-on-abort handler rejecting
with `signal.reason` or `Error("MCP request aborted")`, and the re-check on exit.
**behavior** — Aborting a connect during a cold npx resolution stops it promptly instead of blocking
for up to 30 s.
**cyrup** — Add a `cancel: &cyrup_core::CancelToken` parameter to `resolve_npx_binary`; in
`force_npx_cache`'s poll loop, check `cancel.is_cancelled()` each tick and kill+reap on cancel. The
mechanism divergence already conceded in the module's own doc (blocking `std::process::Command` rather
than Node's event-loop-yielding await) is unchanged — this only adds the interrupt.
**verify** — cancel the token 100 ms into a `force_npx_cache` against a `sleep 60` stand-in; assert
return within ~200 ms and that no child survives.

**MCP-108 — npx resolver: entry-level cache validation and Windows `npm` resolution** · low · S · hand-written
**upstream** — `npx-resolver.ts`'s `toNpxCache`/`toNpxCacheEntry` per-entry validation (including the
`Number.isFinite` check on `resolvedAt` and the `packageVersion` type check), and `cross-spawn` for
both `npm` invocations.
**behavior** — One corrupt entry does not discard the whole cache; `npm` resolves on Windows.
**cyrup** — Deserialise `entries` as `HashMap<String, serde_json::Value>` and convert per entry,
dropping failures. For Windows, either resolve `npm.cmd`/`npm.exe` via a PATH + PATHEXT walk before
`Command::new` or invoke through `cmd /c npm`. (rmcp's own `which_command` behind the `which-command`
feature does the same for its transport, but this call site is in `cyrup-ext`, not on rmcp's path.)
**verify** — a cache file with one good and one malformed entry yields exactly one usable entry.

**MCP-109 — Streamable HTTP client transport** · high · S · rmcp
**upstream** — `connectHttpClient`'s `StreamableHTTPClientTransport(url, {requestInit?, authProvider?,
skipIssuerMetadataValidation?})`; the transport owns `Mcp-Session-Id`, which `session-recovery.ts`
reads back.
**behavior** — POST JSON-RPC to the endpoint, accept either a JSON body or an SSE stream in reply,
carry `Mcp-Session-Id` across requests, surface a 401 in a form the auth ladder can see, and expose the
session id for recovery.
**cyrup** — `rmcp::transport::StreamableHttpClientTransport` with
`StreamableHttpClientTransportConfig { uri, auth_header, custom_headers, retry_config,
channel_buffer_capacity, allow_stateless, max_sse_event_size, reinit_on_expired_session }`, under
features `transport-streamable-http-client-reqwest` **and** `reqwest` (the latter is what selects
rustls; `__reqwest` alone ships a TLS-less client and every `https://` server would fail). 401/403 are
typed as `StreamableHttpError::{AuthRequired, InsufficientScope}` carrying the `WWW-Authenticate`
header. `reinit_on_expired_session` defaults off and stays off — see MCP-135.
**verify** — run the `@modelcontextprotocol/conformance` client baseline the adapter already uses
(`conformance/baseline-client.yml`) against `cyrup-mcp`; plus a fixture HTTP server that issues a
session id and then 404s it.

**MCP-110 — Legacy HTTP+SSE transport and the `shouldFallbackToSse` ladder** · n/a · n/a · cut
See *Out of scope*, Cut 1.
**verify** — conformance: `httpTransport: "sse"` is rejected at config load with its named Cut-1
diagnostic and opens no socket (an Agent-Plugin manifest with `type: sse` takes the same path, since
that is the reachable producer); and a server whose endpoint answers only the legacy GET stream fails
to connect carrying the added probe classification of §3.14, not a bare transport error.

**MCP-111 — Unix-domain-socket transport** · n/a · n/a · cut
See *Out of scope*, Cut 3.
**verify** — unit: `ServerEntry` has no `socket` field and a config carrying one is rejected at load
with its named Cut-3 diagnostic; MCP-113's transport-selection table then asserts the surviving
two-arm exactly-one-transport error, whose text must no longer name `socket` (§3.2).

**MCP-112 — MCP NDJSON framing** · n/a · S · rmcp
**upstream** — `ReadBuffer` / `serializeMessage`, imported from `@modelcontextprotocol/client` and used
by both `StdioClientTransport` and `unix-socket-transport.ts`.
**behavior** — One JSON-RPC message per line; a malformed line is a hard error, not a skipped line.
**cyrup** — Nothing to write. `TokioChildProcess` wraps `rmcp::transport::async_rw::AsyncRwTransport`,
which owns the framing for the only transport in scope that needs it. The first pass filed this as an
open question about `\r` handling and partial-line retention because the pinned SDK source was not
readable; with the unix-socket transport cut and the stdio transport delegated to rmcp, there is no
framing decision left to make.
**verify** — none of our own; covered by the conformance baseline in MCP-109.

**MCP-113 — Transport selection and mutual exclusion** · medium · S · hand-written
**upstream** — `createConnection`'s three-way `configuredTransports` count and its error message.
**behavior** — §3.2. Exactly one transport must be a non-empty string; an empty string counts as
unset; `${name}` is unquoted in this one message.
**cyrup** — A `TransportKind` enum resolved once in the connection builder, reduced to `command` xor
`url`, with the empty-string rule preserved. `socket` and `httpTransport: "sse"` become **named
load-time diagnostics** with the two strings in §3.2 — never a silent skip.
**verify** — `{command:"", url:"http://x"}` → Http; `{command:"a", url:"b"}` → the exact error;
`{}` → the exact error; `{socket:"/tmp/s"}` and `{url:"…", httpTransport:"sse"}` each produce their
named diagnostic and no connection attempt.

**MCP-114 — HTTP header, bearer and command-secret resolution** · high · M · extension-owned + hand-written
**upstream** — `connectHttpClient`'s pre-flight block plus `utils.ts`'s `resolveCommandSecret`,
`resolveCommandSecretsRecord`, `resolveBearerToken` and `resolveServerUrl`.
**behavior** — §3.4 steps 1–7. Secrets are resolved per connection attempt and never written back to
config. `!cmd` runs a shell command; `!!X` is an escape consuming exactly one `!`. If any
command-sourced value is present, the whole header set is validated by constructing `Headers` so a
command emitting a newline cannot inject a header; the failure message is
`Failed to resolve MCP server "<name>" HTTP command secret: command returned an invalid header value`.
`auth: "bearer"` adds `Authorization: Bearer <token>` only when a token resolves truthy.
`resolveServerUrl`'s two throw strings are user-visible verbatim.
**cyrup** — `cyrup-mcp::secrets`, shared with MCP-101. Header validation is
`http::HeaderName::from_bytes` + `http::HeaderValue::from_str`, which rejects CR/LF and control bytes
— exactly what `new Headers()` is doing. `http` is not a workspace dependency; it is in the lock file
transitively via `reqwest` and must be declared per-crate. Resolved headers go into
`StreamableHttpClientTransportConfig::custom_headers`; the bearer token into `auth_header` (which
takes the token **without** the `Bearer ` prefix).
**verify** — `!printf 'a\nb'` as a header value produces the exact invalid-header-value error;
`!!${HOME}` yields `!` + the expanded home; `bearerTokenEnv` falls back correctly; a `!cmd` bearer
token uses the `HTTP bearer token` context string, not the header one; a URL with a missing var
produces the exact `Missing environment variable…` string with correct pluralisation.

**MCP-115 — Implicit-vs-explicit OAuth provider state machine and the attempt loop** · high · M · hand-written
**upstream** — `HttpAuthProviderState`, `createAuthProvider`, and the ordered attempt loop in
`connectHttpClient`.
**behavior** — §3.4. With `auth` unset on a URL server, OAuth is *implicit*: no provider is
constructed and the OS credential store is not touched until the server answers 401 once. That 401
constructs the provider and retries the **same** transport kind. A second 401 becomes `needs-auth`.
With `auth` set explicitly the provider exists from the first attempt. The loop's arm order is the
specification: abort-cleanup-aggregate rethrow **first**, abort check **second**, implicit-challenge
**third**, generic 401 **fourth**.
**cyrup** — `enum HttpAuthProviderState { Disabled, ImplicitDeferred, Explicit(Arc<..>),
ImplicitChallenged(Arc<..>) }` driving the loop verbatim. The 401 predicate is **not** hand-written:
`ClientInitializeError::auth_challenge()` walks the `source()` chain for `AuthRequiredError` /
`InsufficientScopeError` and returns the challenge header, which feeds
`AuthorizationRequest::with_challenge`. The provider itself is section 05's `rmcp::transport::auth`
work; this item owns the state machine and the ordering. The SSE-fallback arm is gone (Cut 1) and the
loop terminates on the preceding `throw err`.
**verify** — a fixture that 401s once then succeeds: assert exactly two attempts, one provider
construction, and that no keychain read happened before the first 401. A permanent-401 fixture yields
`needs-auth` rather than an error.

**MCP-115a — Wire the per-request header command into `connectHttpClient`** · high · S · hand-written
*Filed 2026-08-20 by the v2.25.0 → v2.26.1 retarget. **IMPLEMENTED 2026-08-22** (wave 5), together
with MCP-101/109/113/114/115 — see `13-cyrup-mcp-STATUS.md`'s wave-5 section. One correction to the
`cyrup` note below: the decorator is built **once per connect**, above the retry closure, because
that is where `server-manager.ts:868-870` builds `requestFetch`; building it inside would re-run
`:309`'s eager validation per attempt. It is still shared by every attempt, which is what the note
was asking for. The `Authorization` fold-in is done and divergence 3 is closed.*
**upstream** — `server-manager.ts:868-870` and `:895-896` @v2.26.1 (import at `:61`; commit
`2a2db3c`). Three lines, inside `connectHttpClient`:
`const requestFetch = definition.requestHeadersCommand ? createRequestHeadersCommandFetch(definition.requestHeadersCommand) : undefined;`
built **once per connect**, then spread into the `attempt` closure's `transportOptions` as
`...(requestFetch !== undefined ? { fetch: requestFetch } : {})` — so it applies to **both** transport
kinds and survives the implicit-OAuth retry, because `attempt` is what MCP-115's ladder re-enters.
The factory itself (`request-headers-command.ts:304-336`) does two things worth porting exactly:
it calls `resolvedCommand(config)` **eagerly, at factory time** ("Validate static configuration
before the first request", `:309`) so a malformed block fails the **connect**, not the first request;
and its returned `fetch` runs the command on **every** outbound request with a
`HttpRequestCommandEnvelope` (`{version:1, method, url: request.url, bodyBase64}`) on stdin, merging
the JSON header object from stdout with `headers.set(name, value)` over the request's own headers.
**behavior** — `headers` is resolved once at connect; this is resolved per request, because a
caller-bound signature (HMAC over the body, DPoP, SigV4) is a function of the exact bytes about to be
sent. Without the wiring, a server configured with `requestHeadersCommand` connects and sends
**unsigned** requests, and every such server 401s against a gateway that requires the signature.
**cyrup** — the engine is **already built and tested**: `crate::request_headers_command`
(1,284 lines, 13 tests) ports the whole of `request-headers-command.ts`. Upstream's seam is a
`fetch` override; rmcp's equivalent seam is the `StreamableHttpClient` trait, so the port is a
**decorator** — `RequestHeadersCommandClient` implements all five trait methods — rather than a
function returning a `FetchLike`. That is a mechanism substitution, and it is the right one: rmcp has
no `fetch` to replace. The eager-validation half already matches upstream —
`RequestHeadersCommandClient::new` runs `resolve_request_headers_command(&config)?` before it
returns, which is `:309`'s eager `resolvedCommand`.
It has **no caller**, because `connectHttpClient` has no Rust counterpart yet — this unit, MCP-100
and MCP-123…MCP-126 are all unbuilt, and `runtime.rs:789 http_transport_with_client` is a seam with
no production caller either. Landing this is one arm at the transport-construction site:
`http_transport_with_client(RequestHeadersCommandClient::new(client, cfg, ct)?, config)`. Build it
inside the retry closure, not above it, so it composes with MCP-115's ladder the way `requestFetch`
does. Cut 1 removes upstream's SSE arm, so only the streamable-http kind needs it here.
**Fold in the `Authorization` divergence while you are here** (recorded as divergence 3 in the
module header): upstream's `headers.set` **replaces**, so a derived `Authorization` overwrites the
bearer one; the decorator currently *appends* to rmcp's separate `auth_header` argument, so a
bearer-configured server with a signing command would send **two** `Authorization` headers.
`apply_derived` can match upstream exactly by clearing `auth_header` when the derived set contains
`authorization` (case-insensitively). It is untestable until this unit exists, which is why it is
here and not fixed in place.
**verify** — cyrup-it: a fixture server that 401s unless a per-request HMAC header is present
connects and calls a tool successfully; the helper is invoked **once per request**, not once per
connection; a bearer + signing-command server sends exactly one `Authorization`, the derived one.
Unit: a failing helper aborts the request with upstream's sentence rather than sending unsigned.
*Blocked-by:* MCP-115 / MCP-123…MCP-126 (`connectHttpClient` itself). This is blocked by an unported
dependency, not deferred.

**MCP-116 — `needs-auth` connection state and one-shot credential invalidation** · high · S · hand-written
**upstream** — `ServerConnection.credentialsInvalidated`, `connect`'s step-7 carry-forward, the HTTP
ladder's needs-auth exit, and `createConnection`'s catch-path downgrade.
**behavior** — A 401 against an OAuth-capable server does not fail the connect; it yields a connection
record with `status: "needs-auth"` the UI can act on. `invalidateAuthEntryCache(name)` runs **at most
once per episode** — the flag rides on the connection record and is fed back into the next `connect`,
so a retry loop cannot repeatedly discard a good cached credential. A **cleanup failure suppresses the
downgrade**: a transport that failed to close is a setup failure, not a needs-auth. There are **two**
needs-auth exits: the HTTP ladder's own and `createConnection`'s catch-path downgrade.
**cyrup** — `ServerConnection.status: ConnStatus` plus `credentials_invalidated: bool`, threaded
through `connect`'s step 7 exactly as upstream.
**verify** — two consecutive `connect` calls against a permanent-401 fixture invalidate the cache once;
a fixture whose close errors on a 401 path surfaces `MCP connection setup failed`, not `needs-auth`.

**MCP-117 — Protocol-revision negotiation** · medium · S · rmcp (+ one open decision)
**upstream** — `resolveVersionNegotiation` and its use in `createClient`; `ServerEntry.protocolVersion`;
the disposable-sibling note in `mcp-trace.ts`'s `wrapTransportWithMcpTrace` doc comment.
**behavior** — §3.6, including the exact `` `Invalid MCP protocolVersion: ${String(...)}` `` string and
the "omit the option entirely" arm for `undefined`/`"legacy"`.
**cyrup** — `ClientLifecycleMode::{Initialize, Auto, Discover}` through
`ClientServiceExt::serve_with_lifecycle`, with `ProtocolVersion::V_2026_07_28` for the pinned arm. The
mapping is 1:1. One behavioural delta stands: rmcp runs `discover_startup` and the legacy fallback on
the **same** transport and returns `DiscoverOutcome::Legacy` only when the transport is known-good, so
a legacy stdio server that *exits* on `server/discover` fails under `Auto` where pi succeeded. See
*What does not fit cleanly*.
**verify** — port the four upstream fixtures (`modern-discover-server`, `legacy-no-discover-server`,
`legacy-exits-on-discover-server`, `tools-only-server`); assert a `legacy` server never sees a
`server/discover`, and assert whatever the chosen option promises for `legacy-exits-on-discover-server`.

**MCP-118 — Client capability advertisement** · medium · S · rmcp
**upstream** — `buildClientCapabilities` and its conditional spread into the client options;
`init.ts` sets `allowUrl` from `mode === "tui"`.
**behavior** — `sampling: {}` is advertised **only** when a sampling handler is wired; `elicitation`
carries `form: {}` always and `url: {}` only when `allowUrl`. When both are absent, the `capabilities`
key is omitted from the client options entirely — servers gate features on this.
**cyrup** — `ClientHandler::get_info` returns `ClientInfo` with
`ClientCapabilities { sampling, elicitation: ElicitationCapability::{with_form, with_url} }` built from
`Option<SamplingConfig>`/`Option<ElicitationConfig>`. The client-side elicitation types are
unconditional under the `client` feature; the `elicitation` cargo feature is server-side and must
**not** be enabled. The client name becomes `cyrup-mcp-<server>` (§3.5) — a recorded rename.
**verify** — assert the `initialize` frame's `capabilities` shape in all four on/off combinations,
including the key being absent entirely.

**MCP-119 — Paginated discovery with capability gating and per-list failure policy** · high · M · rmcp + hand-written
**upstream** — the `Promise.all` over `fetchAllTools`/`fetchAllResources`/`fetchAllPrompts`.
**behavior** — §3.9. Tools are always listed and errors propagate. Resources and prompts are listed
**only** when the server advertised the capability. On error: abort and 401 always re-throw; a prompts
failure is logged `MCP: prompts/list failed: <message>` and recorded as `promptDiscoveryFailed: true`;
a resources failure is silently `[]`. All three run concurrently sharing the single `requestOptions`
built before the transport.
**cyrup** — `Peer::{list_all_tools, list_all_prompts, list_all_resources}` own the cursor loops.
Capability gate reads `RunningService::peer_info()` → `ServerCapabilities { prompts, resources }`.
Use `tokio::join!` and reduce — **not** `try_join!`, which short-circuits and cancels the siblings on
the first error, destroying the per-list failure policy.
**verify** — a tools-only fixture yields `resources == []` with **no `resources/list` on the wire**; a
server advertising prompts that errors yields `prompts == []` **and** `prompt_discovery_failed == true`;
a 401 from `resources/list` propagates rather than degrading to `[]`.

**MCP-120 — `list_changed` refresh with identity guards** · medium · S · rmcp + hand-written
**upstream** — the `listChanged.{tools,resources,prompts}.onChanged` wiring and
`handleToolsListChanged` / `handlePromptsListChanged` / `handleResourcesListChanged`.
**behavior** — §3.10. A refresh is applied only when the manager's current connection for that name is
still *this* client and still `connected`. A `null` list is ignored without touching anything. Prompts
additionally clear `promptDiscoveryFailed`. The listener is notified with one of three exact reason
strings.
**cyrup** — `ClientHandler::on_{tool,prompt,resource}_list_changed`. rmcp's notifications carry no
list; `Peer` invalidates its own response cache, so the handler re-calls `list_all_*` after the
identity check. Identity is `Arc::ptr_eq` against the live connection map. The handler type is shared
across the connection through an `Arc`, which is what makes the self-reference (upstream's
`let client: Client;` assigned immediately after the callbacks close over it) expressible.
**verify** — fire a stale client's notification after a reconnect and assert the fresh connection's
tool list is untouched and no listener call is made; assert the three reason strings byte-for-byte.

**MCP-121 — Adapter-private UI stream-patch notification handler** · n/a · n/a · cut
See *Out of scope*, Cut 2.
**verify** — unit: a stub server that emits the adapter-private stream-patch notification mid-session
lands in `ClientHandler::on_custom_notification` and is dropped — no handler, nothing logged above
debug, and the connection stays live through a subsequent `tools/list`, so an unhandled notification
can never close a connection.

**MCP-122 — URL-elicitation acceptance tracking and completion notice** · medium · S · hand-written + host-verb
**upstream** — `rememberUrlElicitation`, the `notifications/elicitation/complete` handler,
`handleUrlElicitationRequired`, and the set-clearing in `close`/`closeAll`.
**behavior** — §3.10. `rememberUrlElicitation` is a no-op when the runtime signal is aborted. The
completion notice fires **only if `Set.delete` returned true**, so a duplicate completion is silent,
with the exact string `MCP browser interaction for <server> completed. You can retry the tool now.` at
info level. `handleUrlElicitationRequired` returns `"cancel"` outright when aborted or when `allowUrl`
is off, without consulting the elicitations.
**cyrup** — `Mutex<HashMap<String, HashSet<String>>>` on the manager; the notice goes through
`HostServices::notify(.., NotifyKind::Info)`. rmcp has no first-class variant for
`notifications/elicitation/complete`, so it arrives at `ClientHandler::on_custom_notification`. The
elicitation handler itself is section 05; this item owns the registry, its lifecycle and the batch
walker.
**verify** — two identical completion notifications produce exactly one notify call; a completion for
an id that was never accepted produces none; `handle_url_elicitation_required` returns `Cancel` when
`allow_url` is off without consulting the elicitations; `close(name)` clears one server's set and
`close_all` clears all.

**MCP-123 — Connect-time abort and once-only transport cleanup** · medium · S · rmcp + hand-written
**upstream** — `connectClientWithAbort`, the `abortCleanupPromises` `WeakMap`, and the three call
sites that consult it (`createConnection`'s catch, the HTTP attempt's failure path, the ladder's
aggregate rethrow).
**behavior** — §3.8. An abort during connect closes the transport exactly once and that exact close is
what every downstream handler awaits. A failure *of that cleanup* is distinguished from a failure of
the connect, and once it occurs no further cleanup is attempted anywhere up the stack. The success path
also awaits the cleanup so a close started during a racing abort is not orphaned.
**cyrup** — Most of this collapses: `serve_client_with_ct(handler, transport, ct)` owns the
initialise-or-fail path and drops the transport on `Err`, and `ChildWithCleanup::drop` spawns a `kill()`
so no child survives an aborted connect. What remains adapter policy is the cleanup-vs-connect
*distinction* and the once-only guarantee across the HTTP retry ladder, held as
`Option<futures::future::Shared<BoxFuture<'static, Result<(), Arc<Error>>>>>` on the attempt handle.
**verify** — abort mid-connect with a transport whose close errors → exactly one close call and the
abort-cleanup variant; abort with a clean close → the original abort error, no aggregate; a connect
that succeeds while an abort fired concurrently still awaits the cleanup before returning; a
process-table assertion that no child survives an aborted connect.

**MCP-124 — Error taxonomy and `containsCleanupFailure`** · high · S · hand-written
*IMPLEMENTED 2026-08-22 (wave 5). One measured correction to the `cyrup` note below: `Display` must
NOT render the head. `formatTerminalError` (`utils.ts:229-236`) recurses into `.errors` and `.cause`
first and pushes the aggregate's own message only `if (messages.length === countBefore)`, so a
non-empty aggregate renders as its children alone — measured on node 22 over five shapes. The head
is `error.message`, which is a different projection and is what `server-manager.ts:591/918/939`
compare; `McpError::aggregate_head` exposes it. The five heads are still byte-exact.*
**upstream** — the five aggregate messages and `containsCleanupFailure`.
**behavior** — §3.12. During shutdown, an ordinary connect failure is expected and swallowed; a
*teardown* failure must surface. The discriminator is an iterative walk of the error graph (an
aggregate's `.errors` plus `.cause` when defined, with a `seen` set to survive cycles) testing
`/cleanup failed|setup failed/` against aggregate messages only.
**cyrup** — `enum McpError { AbortCleanupFailed(Vec<_>), SetupFailed(Vec<_>), HttpCleanupFailed(Vec<_>),
ConnectionCleanupFailed(Vec<_>), ManagerCleanupFailed(Vec<_>), … }` with `thiserror`, whose `Display`
renders the five exact upstream strings (they reach the user through `formatTerminalError`) and whose
`is_cleanup_failure()` is a structural match recursing through `std::error::Error::source()`. **Record
an intentional divergence**: upstream's regex would also match a *server-supplied* message containing
"cleanup failed"; the typed version does not.
**verify** — a 3-deep `source()` nest with the marker at the bottom returns true; a plain connect error
returns false; a cyclic chain terminates; a non-aggregate error whose message literally contains
"cleanup failed" returns false (the documented divergence).

**MCP-125 — `reconnect`: guards, single-flight, identity, in-flight preservation** · high · S · hand-written
**upstream** — `reconnect` and `doReconnect`.
**behavior** — §3.11. The disabled and stopped guards fire **before** the single-flight map is consulted
and are not inherited from `connect`. Concurrent callers racing the same stale connection share one
reconnect. A connection that is no longer the manager's current one is never torn down — the fresh one
is returned instead, and when the map holds nothing a plain `connect` runs. The stale connection's
`inFlight` is carried forward with `max`, so the idle sweep cannot close a server whose callers are
still waiting.
**cyrup** — `reconnect_promises: Mutex<HashMap<String, Shared<BoxFuture<..>>>>` with identity-matched
removal in a guard; `Arc::ptr_eq` for the staleness check.
**verify** — three concurrent `reconnect` calls with the same stale handle produce one close+connect
pair; a `reconnect` whose stale handle was already replaced returns the replacement without closing it;
a `reconnect` against a disabled definition throws before any close.

**MCP-126 — `close` / `closeAll`: generations, attempt aborts, late-name sweep** · high · M · hand-written
**upstream** — `close`, `disposeConnection`, `closeAll`.
**behavior** — §3.12. The connection is removed from the map **before** awaiting cleanup; a `close`
with no connection still awaits a pending close, or a pending connect re-throwing only cleanup
failures; `closeAll` re-reads the connection map after its first sweep and closes anything that
appeared during it; `samplingConfig`/`elicitationConfig` are nulled so a late callback cannot re-enter
a dead runtime; the trace writer is flushed last.
**cyrup** — Direct translation. `connect_attempts[name].cancel()` replaces the `AbortController`, with
the reason string carried alongside in the `AbortHandle` (MCP-100). Child teardown is
`TokioChildProcess::graceful_shutdown` — record the delta against the SDK escalation (§3.12).
**verify** — start 5 servers, call `close_all` while 2 are mid-connect, assert zero surviving child
processes by process-table check and that the returned error contains only genuine cleanup failures; a
unit test that a connect resolving *between* the two sweeps is still closed.

**MCP-127 — Idle and in-flight accounting** · medium · S · hand-written
**upstream** — `touch`, `incrementInFlight`, `decrementInFlight`, `isIdle`; consumed by `lifecycle.ts`
(30 s default health interval, 10 min `globalIdleTimeout`).
**behavior** — §3.13. `isIdle` requires `connected`, zero in-flight, and `now - lastUsedAt > timeoutMs`
(strict). `touch` runs both before and after every request. `decrementInFlight` never goes below zero.
All four are **public** — `lifecycle.ts` drives them.
**cyrup** — `AtomicU64` for `last_used_at` (epoch ms) and `AtomicU32` for `in_flight`, with an RAII
guard that increments on construction and decrements+touches on `Drop` so an early `?` cannot leak a
count. **Record as a divergence**: this is stricter than JS's `finally` (a panic also unwinds the
count) — equivalent on the happy path, safer on the unhappy one.
**verify** — `is_idle` is false while a guard is alive and true one tick after it drops past the
timeout; `decrement` on a zero counter is a no-op; `is_idle` on a `needs-auth` connection is false.

**MCP-128 — Request options: timeout normalisation and owned signal** · medium · S · rmcp + hand-written
**upstream** — `getRequestOptions`, `getResolvedRequestTimeoutMs`, `buildRequestOptions`,
`normalizeRequestTimeoutMs`; wired from `init.ts` with `config.settings?.requestTimeoutMs`.
**behavior** — §3.13. Per-server `requestTimeoutMs` overrides the global; an invalid per-server value
(`0`, negative, `NaN`, `Infinity`, non-number) resolves to **no timeout**, it does *not* fall back to
the global. When neither a signal nor a timeout exists, no request options object is produced at all.
The public `getRequestOptions(name, signal?)` resolves the definition **by name from the live
connection map**.
**cyrup** — `rmcp::service::PeerRequestOptions`. `reset_timeout_on_progress` and `max_total_timeout`
have no upstream analogue and stay at their defaults. Cancellation combines through the MCP-100
`combine` helper chained off the runtime token.
**verify** — a table over `{undefined, 0, -1, NaN, 5000}` × `{global unset, global 30000}`, asserting
`None` for every invalid per-server value even when a global is set.

**MCP-129 — `getPrompt` / `readResource` accounting and disabled re-check** · medium · S · rmcp + hand-written
**upstream** — `getPrompt` and `readResource`.
**behavior** — §3.13. Both require `status === "connected"`, else `` `Server "<name>" is not connected` ``.
Both touch+increment before and decrement+touch after, and both build options via
`getRequestOptions(name, signal)`. `readResource` additionally re-checks `isServerDisabled` on the
**live** definition first, throwing `` `MCP server "<name>" is disabled` `` — this catches a server
disabled in config *after* it connected. `getPrompt` omits `arguments` entirely when `args` is falsy.
**cyrup** — `Peer::{get_prompt, read_resource}` wrapped in the MCP-127 guard.
**verify** — disable a server in the live config after connecting and assert `read_resource` refuses
with the disabled message while `get_prompt` refuses only on not-connected; assert `get_prompt` with no
args sends no `arguments` key.

**MCP-130 — Startup connect concurrency limit** · medium · S · hand-written
**upstream** — `init.ts`'s two `parallelLimit(…, 10, …)` call sites over `utils.ts`'s `parallelLimit`:
`Math.min(limit, items.length)` workers pulling from one shared `items.entries()` iterator, results
written back by original index.
**behavior** — At most 10 MCP servers are connected concurrently at startup; results keep config order.
**cyrup** — `futures::stream::iter(..).map(..).buffered(10).collect()` preserves both the limit and the
input ordering. `init.ts` belongs to section 04; this item exists so the limit is not lost between
sections.
**verify** — an instrumented connect fn asserting peak concurrency ≤ 10 and output order.

**MCP-131 — Child-process cleanup and orphan avoidance** · high · S · rmcp
**upstream** — `client.close()` → the SDK's stdio close escalation; the non-orphan property depends on
npx resolution (MCP-103).
**behavior** — Closing a stdio connection leaves no surviving process, including no npm/npx grandchild.
**cyrup** — `TokioChildProcess::graceful_shutdown`: close the transport (dropping the child's stdin),
then `select!` the child's `wait()` against `MAX_WAIT_ON_DROP_SECS = 3`, killing on timeout;
`ChildWithCleanup::drop` spawns a `kill()` as the safety net. **Named delta**: one 3-second window and
a hard kill, versus the SDK's close-stdin → 2 s → SIGTERM → 2 s → SIGKILL — a server that ignores
stdin closure but would have honoured SIGTERM is hard-killed. Both signal a single pid rather than a
process group, which is correct *because* MCP-103 removes the npm launcher. The first pass filed a
registry-exhaustion prerequisite here (`ProcCaps`'s `MAX_SPAWNED_PROCESSES` never evicting); that path
is not on this transport at all — rmcp owns the child, `cyrup-mcp` owns the `TokioChildProcess`, and
no capability registry is involved.
**verify** — connect and close 300 stdio servers sequentially, asserting a stable process count and no
handle growth; a fixture that ignores stdin closure exits within ~3 s of `close`.

**MCP-132 — MCP endpoint probe (three-strategy ladder)** · medium · M · extension-owned
**upstream** — `mcp-probe.ts` `probeMcpEndpoint` and its helpers.
**behavior** — §3.14 in full: three request shapes, exact headers and bodies, exact status-set gates,
exact classification strings, 5 s per-request timeout, unauthenticated.
**cyrup** — `cyrup-mcp::probe` on `reqwest` directly with a 5 s timeout. Content-type parsing is plain
`split(';').next().map(str::trim).map(str::to_ascii_lowercase)`, **not** a MIME crate, to match
`responseKind`'s exact behaviour on malformed headers. The `www-authenticate` check is a `regex`
literal `(?i)(?:^|,)\s*Bearer\b`. Classification strings are user-visible inside connect-failure
messages and must be byte-exact, including the em-dash in `notMcp`. Add the one Cut-1 arm from §3.14.
**verify** — a table of 12 synthetic responses (SSE 200; JSON-RPC result with and without the modern
protocolVersion; JSON-RPC error; 401 with and without a Bearer challenge; HTML 404; untyped 500; …)
asserting the exact classification string and the exact ladder step reached — specifically that a
modern 200 whose `result.protocolVersion` is `"2025-06-18"` yields `unsupported-modern` and falls
through to legacy-post, and that a GET-only SSE endpoint yields the new legacy-transport diagnostic.

**MCP-133 — Probe-enriched HTTP connect failures** · medium · S · hand-written
**upstream** — the `definition.url ? attempt.catch(…)` wrapper in `connect`, and
`enrichHttpConnectionError`.
**behavior** — A URL server's connect failure is wrapped as `<original> — probe: <classification>` with
the original as `cause`. **Only** URL servers are wrapped. **Any** probe failure — including
`resolveServerUrl` throwing on the re-resolve — is swallowed and the original error returned unchanged.
**cyrup** — Wrap the connect future for URL servers only; the probe runs *after* the failure so it
costs nothing on the success path. Preserve the exact ` — probe: ` separator (space, em-dash, space).
**verify** — a connect failure against an HTML endpoint yields
`<msg> — probe: endpoint returned HTML (404) — this URL does not appear to speak MCP`; a probe that
itself times out yields the bare original message; a stdio server's failure is never wrapped.

**MCP-134 — `isTerminatedSession` predicate** · high · S · rmcp + hand-written
**upstream** — `session-recovery.ts`'s constants, `isTerminatedSession` and `hasSessionId`.
**behavior** — §3.15. The `hadSessionId` gate is absolute; a 400 qualifies only when its message body
literally contains both `"code": -32000` and `"message": "Bad Request: Server not initialized"`
(matched against the *serialised* error message, with flexible whitespace around the colons); a
`ProtocolError` qualifies only with code `-32000` and one of two exact messages — note the set has
**two** members while the 400 regex matches only the longer one. The four negatives in the module's
doc comment are part of the spec.
**cyrup** — The 404 arm is `StreamableHttpError::SessionExpired`, raised on exactly
`status == NOT_FOUND && session_was_attached` — the same predicate including the session gate, so
`hasSessionId` need not be reconstructed for it. The 400/`-32000` arm and the `ProtocolError` arm are
hand-written with two `regex` patterns in a `LazyLock`. **Record as a divergence**: JS duck-types
`transport.sessionId` and treats a missing transport as absent; in Rust the transport is a typed enum
and stdio is structurally session-less — stricter, not observably different.
**verify** — the four negatives (no session id, generic 400, generic -32000/ConnectionClosed, abort)
plus the four positives (404 with session; 400 with both markers; `ProtocolError` -32000 with each of
the two exact messages).

**MCP-135 — `withSessionRecovery` retry wrapper** · high · M · hand-written
**upstream** — `session-recovery.ts`'s `SessionRecoveryDeps`, `withSessionRecovery` and
`SessionRecoveryAuthRequiredError`.
**behavior** — §3.15 steps 1–16. Exactly one retry. The live config is re-read after the failure, not
the stale connection's snapshot. A 401 against an OAuth server invalidates the credential cache
**regardless** of whether the error is a terminated session (it runs before the `isTerminatedSession`
gate). A `needs-auth` result after reconnect goes through the caller's `onNeedsAuth` hook once, and if
still `needs-auth` raises `SessionRecoveryAuthRequiredError`; any other non-connected status re-raises
the **original** error, not a new one.
**cyrup** — `async fn with_session_recovery<T, F, Fut>(deps, server, f) -> Result<T>` where
`F: Fn(Arc<ServerConnection>) -> Fut` — must be `Fn`, not `FnOnce`, because it is called twice.
`SessionRecoveryDeps { manager, config, cancel: Option<CancelToken>, on_needs_auth }`. Leave
`StreamableHttpClientTransportConfig::reinit_on_expired_session` **off** so the two layers do not
double-retry — see *What does not fit cleanly*.
**verify** — a fixture that 404s the first call after issuing a session id and succeeds on the second:
assert exactly one reconnect and one retry, that a second 404 propagates, and that a 401 on a
non-terminated error still invalidates the credential cache.

**MCP-136 — Tracker: what survives a restart** · n/a · S · hand-written
**upstream** — `session-recovery.ts` as a whole, `metadata-cache.ts`'s path/save,
`npx-resolver.ts`'s cache path, `agent-dir.ts`.
**behavior** — Zero connection state is durable. The only cross-process artefacts are
`<agent_dir>/mcp-cache.json`, `<agent_dir>/mcp-npx-cache.json` and the OS-keychain OAuth entries.
Every `Mcp-Session-Id` is discarded on shutdown; `withSessionRecovery` covers the *remote* losing its
session, never the adapter restarting.
**cyrup** — No work of its own. Indexes MCP-104, MCP-139, MCP-141 and section 05's keychain items. The
one live issue is agent-dir resolution, owned by MCP-139.
**verify** — n/a.

**MCP-137 — Status snapshot construction** · medium · S · hand-written
**upstream** — `mcp-status.ts`'s `FAILURE_BACKOFF_MS`, `getActiveFailureAgeSeconds` and
`createMcpStatusSnapshot`; types in `types.ts`.
**behavior** — §3.16. Never connects or queries; six-way status precedence; 60 s failure backoff
window; `resourceCount`/`failedAgoSeconds` omitted rather than nulled while `disabled: boolean` is
**always** emitted; per-server order is **config order**.
**cyrup** — Serde structs with `#[serde(skip_serializing_if = "Option::is_none")]` on `resource_count`
and `failed_ago_seconds` only. The config server map **must** be insertion-ordered — a
`Vec<(String, ServerEntry)>` or an ordered map, never `BTreeMap`. Note the in-tree
`mcp_direct_tools` reader uses `BTreeMap` for its own server map, which is harmless there and must
**not** be copied here or the `/mcp` panel and the footer list servers alphabetically.
**verify** — a 6-server fixture covering every status arm, asserting field presence and absence
(including `disabled` present on an *enabled* server), the totals, and the exact ordering against a
config whose keys are not alphabetical.

**MCP-138 — Publish the status snapshot** · low · S · extension-owned
**upstream** — `publishMcpStatusSnapshot` / `publishMcpStatusShutdown` on `pi.events`, channel
`pi-mcp-adapter/status/v1`, payload version 1, with the one-method `McpStatusEventBus` contract. Both
emitters swallow every exception and both no-op on a missing bus.
**behavior** — Consumers observe a snapshot on every status change and an all-zero snapshot on
shutdown, and a throwing consumer can never interrupt MCP work. `publishMcpStatusSnapshot` accepts a
pre-built snapshot so a caller that already has one does not rebuild it.
**cyrup** — Keep the snapshot **in-crate** on a `tokio::sync::watch`, read by the `/mcp` panel, the
footer status segment (`HostServices::set_status`) and the proxy tool's `status` mode. The first pass
filed this as an open question needing a bus handle reachable from a native extension; there is **no
consumer for such a topic anywhere in cyrup**, and building the emit path would be a dead primitive.
The swallow-all discipline still applies to whatever the watch feeds.
**verify** — connect and disconnect a server and assert two snapshots reach the watch with the right
contents; assert the shutdown snapshot is published before teardown.

**MCP-139 — Metadata cache: path, schema, version, load and merge-save** · high · M · hand-written
**upstream** — `metadata-cache.ts`'s `CACHE_VERSION`, `getMetadataCachePath`, `loadMetadataCache`,
`saveMetadataCache`; schema in `types.ts`; path in `agent-dir.ts`.
**behavior** — §3.17: `<agent_dir>/mcp-cache.json`, `version: 1`, a load that is total (never throws,
returns `None` on any problem), and a save that is read-merge-**overlay**-write-`${path}.${pid}.tmp`-rename
with 2-space pretty JSON, no pruning, no per-entry validation on the merge, and no lock.
**cyrup** — `cyrup-mcp::cache` with `serde_json::to_string_pretty` (2-space by default), write to the
pid-suffixed temp then `fs::rename`, `create_dir_all` on the parent. Do **not** add a file lock —
upstream has none and adding one changes cross-process behaviour. The read side already exists and
agrees on version and TTL (`cyrup_ext_subagents::exec::mcp_direct_tools`'s `load_metadata_cache`,
`CACHE_VERSION`, `CACHE_MAX_AGE_MS`, and the entry structs). The **path does not agree**, on the three
axes in §3.17. Consolidate on one shared agent-dir helper — `cyrup-config`'s `ConfigDirs::agent_dir` or
a new `cyrup-core` one — and make `npx_resolver` and `mcp_direct_tools` call it, restoring
`resolve()`-equivalent absolutisation and a single home source. Do **not** add a fourth resolver. The
failure is silent: with `CYRUP_HOME` ≠ `HOME` (the configuration CI and subagent isolation use), the
cache is written where nobody reads it and the symptom is an empty tool list.
**verify** — save server A, save server B, assert both present and A untouched; save over a corrupt
file and assert it is replaced with a valid one containing only the new servers; assert the temp file
is gone; with `CYRUP_HOME` ≠ `HOME`, assert `mcp-cache.json` and `mcp-npx-cache.json` resolve to the
**same** directory.

**MCP-140 — Metadata cache: serialisers and reconstructors** · high · M · hand-written
**upstream** — `metadata-cache.ts`'s `serializeTools`/`serializeResources`/`serializePrompts`,
`reconstructToolMetadata`, `getOtherCurrentCandidates`, `reconstructPromptMetadata`.
**behavior** — §3.17. Serialisers filter on required fields and emit optional fields **only when
defined** (an explicit `null` is never written). `reconstructToolMetadata` applies, in order: the
`!tool?.name` filter; the `isUiToolVisibleToModel(tool.uiVisibility)` model-visibility filter;
`isToolAllowed` with cross-server collision candidates from `getOtherCurrentCandidates` (which applies
the same visibility filter and the same `exposeResources !== false` gate to *other* servers, scoped to
servers that are cache-valid and not disabled); `seenNames` first-wins dedup. Emitted metadata carries
`description ?? ""` — the **empty string**, not `undefined`. Resource tools are emitted only when
`exposeResources !== false`, named `read_<resourceNameToToolName(name)>`, with
`description ?? \`Read resource: ${uri}\``. `reconstructPromptMetadata` emits `arguments` **always as
an array**.
**cyrup** — `#[serde(skip_serializing_if = "Option::is_none")]` throughout. The name-formatting helpers
(`resolveToolPrefix`, `formatToolName`, `getToolNameCandidates`, `isToolAllowed`,
`formatPromptCommandName`) belong to section 02 and are consumed, not re-derived.
`isUiToolVisibleToModel` / `extractUiToolVisibility` survive Cut 2 (see *Out of scope*) and live in
`cyrup-mcp`.
**verify** — a golden-file round-trip of a 3-server cache; a collision test where two servers expose
the same tool name under `toolPrefix: "none"`; a tool whose `uiVisibility` excludes the model is absent
from the metadata **and** from the other-server candidate set; a server with `exposeResources: false`
contributes no `read_*` names to either list.

**MCP-141 — `computeServerHash` must hash all 14 fields; the in-tree reader hashes 11** · critical · M · hand-written
**upstream** — `metadata-cache.ts`'s `computeServerHash` — the 14-key identity object in §3.17,
including `socket`, `protocolVersion`, `includeTools`, and `url` via `resolveServerUrl` (which
interpolates and throws).
**behavior** — Changing a server's socket path, pinned protocol revision or `includeTools` must
invalidate its cached tool list. A URL server whose `${VAR}` changes must invalidate. A URL server
referencing a missing variable must never be cache-valid (the throw is caught by `isServerCacheValid`).
**cyrup** — `cyrup_ext_subagents::exec::mcp_direct_tools`'s `compute_mcp_server_hash` hashes 11 keys:
`socket`, `protocolVersion` and `includeTools` are absent, and so are the corresponding fields on its
`ServerEntry`; `url` is the **raw** string, never `resolveServerUrl`-interpolated, and therefore cannot
throw. Write the 14-key pre-image in `cyrup-mcp` and **upgrade the reader in the same change** — that
is the only option leaving the tree self-consistent and upstream-faithful, and its cost is an edit
outside `cyrup-mcp`. Keep `socket` and `protocolVersion` in the pre-image despite Cut 3 (§3.17). This
lands as **four** coordinated edits: the field set (this item), the `undefined` token (MCP-142), the
`{env:NAME}` pattern (MCP-143) and the `!`/`!!` semantics (MCP-144) — each of which independently
changes the digest for essentially every server. `sha2` is the in-tree hashing convention.

> **SATISFIED (and the instruction above was right).** The first wave shipped the pre-image without
> `socket` and recorded the omission as a deliberate Cut-3 divergence. It was not one: upstream's
> `stableStringify` walks `Object.keys()`, so a `socket` holding `undefined` is still emitted as
> `"socket":undefined` rather than dropped, and every cyrup digest therefore differed from pi's by
> exactly that member — for every server, in every config. Both pre-images now emit
> `"socket": undefined` unconditionally, which is correct **and** complete, because
> `to_server_entries` rejects any entry that configures a socket (MCP-054), so the value can only
> ever be `resolveConfigPath(undefined)`. Measured, not argued: upstream's digest for the plain
> stdio golden fixture is `2190558e470a75c0f992989bd1799b374e669deecb8093e4118a1a9419068cf4`, cyrup
> produced `4dd46c1f…`, and the two are now equal — pinned in
> `mcp_direct_tools::tests::the_socket_key_is_no_longer_a_divergence_from_upstream`. The same wave
> closed `protocolVersion` (and `auth`, and `env`/`headers`) on the writer side by giving the
> config types passthrough arms, so `lenient` no longer discards a value the digest depends on;
> `Invalid MCP protocolVersion` still throws, at connect, where `resolveVersionNegotiation` throws
> it.
**verify** — a golden-vector table of 8 server definitions with their expected SHA-256 hex, generated
by running the TypeScript at v2.25.0 once and committed as a fixture. This is the only way to prove
byte-compatibility and it must exist before either implementation is trusted.

**MCP-142 — `stableStringify` emits the bare token `undefined`, not `null`** · critical · S · hand-written
**upstream** — `metadata-cache.ts`'s `stableStringify`:
`const serialized = JSON.stringify(value); return serialized === undefined ? "undefined" : serialized;`.
Because the identity object always carries all 14 keys and a typical server sets two or three, most
keys hash as the 9-character token `undefined`.
**behavior** — The hash pre-image for a plain stdio server is the exact string in §3.17. Any
implementation substituting `null` produces a different digest for essentially **every** server.
**cyrup** — Introduce a three-state value in the hash builder: present / JSON-null / absent, where
absent serialises to the literal `undefined` and JSON-null to `null`. In Rust that is
`enum HashValue { Absent, Json(Value) }`, not an `Option<Value>` flattened to `Value::Null` — which is
exactly what `mcp_direct_tools`'s `opt_str_value`, its `interpolate_env_record` absent arm and its four
`.unwrap_or(Value::Null)` arms do today. Match upstream: a from-scratch "cleaner" pre-image silently
invalidates every cache written by any pi-era tooling the user still has, and makes MCP-141's golden
vectors worthless.
**verify** — `stable_stringify` of `{"a": Absent, "b": Json(Null)}` is exactly `{"a":undefined,"b":null}`;
plus the MCP-141 golden vectors.

**MCP-143 — `interpolateEnvVars` is missing its third pattern `{env:NAME}`** · high · S · hand-written
**upstream** — `utils.ts`'s `interpolateEnvVars`: three sequential `String.replace` passes,
`/\$\{(\w+)\}/g`, `/\$env:(\w+)/g`, `/\{env:(\w+)\}/g`, each falling back to `""`; `getMissingEnvVars`
matches the same three alternatives in one alternation, so a `{env:MISSING}` in a URL must also raise
the missing-variable throw.
**behavior** — `"{env:GITHUB_TOKEN}"` in an `env` value, a header, a URL, a `cwd` or a `bearerToken`
resolves to the environment variable's value.
**cyrup** — **Two** patterns in both in-tree implementations: `caps::proc`'s `interpolate_env_vars_with`
(= `interpolate_braces` + `interpolate_dollar_env`) and `mcp_direct_tools`'s `interpolate_env_vars`
(two `expand_pattern` passes); `resolve_config_path` inherits the gap in both. Add a third pass with
the same `[A-Za-z0-9_]+` name rule, the same empty-string fallback and the same "leave malformed input
byte-for-byte untouched" behaviour, in both files and in `cyrup-mcp`. Order matters: `${NAME}`, then
`$env:NAME`, then `{env:NAME}`, because each pass runs over the previous pass's output.
`mcp_direct_tools`'s `expand_pattern` already takes an open/close pair, so the third pass is
`expand_pattern(&s, "{env:", Some("}"), env)`. Two failure modes: the child gets a literal 18-character
placeholder, **and** the config hash differs — the second is silent and feeds MCP-141.
**verify** — in both crates: `"a${A}b$env:Bc{env:C}d"` with `A=1,B=2,C=3` → `"a1b2c3d"`; a missing var
yields `""`; `"{env:}"` and `"{env:-}"` are untouched.

**MCP-144 — `!`/`!!` secret-expression semantics in hashed values** · high · S · hand-written
**upstream** — `utils.ts`'s `interpolateSecretExpression` (`!!X` → `interpolateEnvVars(value.slice(1))`,
i.e. **one** `!` consumed so the result still begins with `!`; `!X` → **returned verbatim, never
executed**; otherwise `interpolateEnvVars(value)`), used by `interpolateEnvRecord` and
`resolveBearerToken`.
**behavior** — The config-identity hash must be computable without running any command, so a `!cmd`
value hashes as its literal text; but a `!!literal` escape must be un-escaped by exactly one `!` so it
hashes as the value the child will actually see.
**cyrup** — `mcp_direct_tools`'s `interpolate_env_record` and `resolve_bearer_token` call plain
`interpolate_env_vars`, so `!!X` is not un-escaped and `!cmd` **is** interpolated rather than passed
through. `interpolate_env_record` additionally **drops non-string values** where upstream would attempt
`.startsWith` on them. Add `fn interpolate_secret_expression(value, env) -> String` with the exact
three-arm rule and route both call sites through it, in `mcp_direct_tools` and in `cyrup-mcp::cache`.
**verify** — `"!!${HOME}"` hashes as `"!" + home`; `"!op read x"` hashes as itself; a plain value
interpolates; `"!!!x"` consumes exactly one `!`.

**MCP-145 — `isServerCacheValid` including the throw-to-false rule** · high · S · hand-written
**upstream** — `metadata-cache.ts`'s `isServerCacheValid`.
**behavior** — Hash computation happens inside a `try` and **any** throw means invalid — this is the
sole mechanism by which a URL server with a missing environment variable is kept out of the cold-start
tool surface. `cachedAt` must be present **and** numeric. A `maxAgeMs` of `0` disables the age check
entirely.
**cyrup** — `mcp_direct_tools`'s `is_server_cache_valid` implements hash equality, `cachedAt` presence
and the TTL, but its hash function cannot throw (raw `url`, no `resolve_server_url`), so the
missing-variable rule is absent; and there is no `max_age_ms` parameter — the TTL is the hard constant.
Once MCP-141 lands a `resolve_server_url` returning `Result`, thread the `Err` arm to `false`, and add
`max_age_ms: Option<i64>` with `Some(0)`/`None` meaning "no age check".
**verify** — a `url: "https://x/${MISSING}"` server is never cache-valid; `max_age_ms = 0` accepts a
year-old entry; a non-numeric `cachedAt` (arriving as a JSON string) is rejected.

**MCP-146 — Resource tool naming: `read_` upstream vs `get_` in the in-tree reader** · critical · S · hand-written
**upstream** — `metadata-cache.ts` forms `` const baseName = `read_${resourceNameToToolName(resource.name)}` ``
in both the collision-candidate scan and the emission loop, and `direct-tools.ts` does the same;
grepping `direct-tools.ts` at v2.25.0 for `get_` returns **zero** hits. `resourceNameToToolName`
(`resource-tools.ts`): non-alphanumerics → `_`, collapse runs, strip leading then trailing `_`,
lowercase; empty or digit-leading → `"resource"` / `"resource_" + result`.
**behavior** — A resource named `Project Readme` becomes the tool `read_project_readme` (then prefixed).
**cyrup** — `mcp_direct_tools`'s `resolve_direct_tool_names` builds
`format!("get_{}", resource_name_to_tool_name(name))`. `resource_name_to_tool_name` itself is a
faithful port; only the prefix differs — so the writer and the subagent resolver disagree on the name
of **every** resource-backed tool, and a subagent MCP allow-list entry naming a resource tool never
matches, silently. Change `get_` to `read_` in the same change as MCP-141 (same file, same contract).
Do **not** "support both": the allow-list is an exact string match, and accepting both would make two
distinct names resolve to one tool. The module documents itself as a port of pi-subagents'
`mcp-direct-tool-allowlist.ts` rather than of `metadata-cache.ts`; if pi-subagents genuinely uses
`get_`, pi itself is internally inconsistent and cyrup must still pick one — the adapter is the writer,
so `read_` wins.
**verify** — in `cyrup-ext-subagents`: a cache entry with one resource resolves to a `read_`-prefixed
direct tool name matching what `cyrup-mcp` writes (the existing test asserting
`browser_mcp_get_console_logs` must be updated in the same change); plus a cross-crate test that writes
a cache with `cyrup-mcp` and resolves it with `resolve_mcp_direct_tool_names`.

**MCP-147 — Direct-tool selector parsing and the missing-server gate** · medium · S · hand-written
**upstream** — `metadata-cache.ts`'s `parseDirectToolSelectors` and
`getMissingConfiguredDirectToolServers`.
**behavior** — §3.17. `parseDirectToolSelectors` strips trailing slashes and splits at the **first** `/`
only via `split("/", 2)`, which discards a third segment; a slash-less non-empty selector is a
whole-server selection. `getMissingConfiguredDirectToolServers` decides per server whether direct tools
are wanted (env override wins, then per-server `directTools`, then `settings.directTools`) and reports
those whose cache entry is missing or invalid under the default 7-day TTL — this is what makes startup
block until those servers connect.
**cyrup** — A different consumer's parser already exists (`mcp_direct_tools`'s `parse_selections`, for
the subagent allow-list) and already handles the discard correctly, documented in place. Reuse that
shape rather than writing a third one; note Rust's `splitn(2, '/')` gives `("a", "b/c")` where JS gives
`("a", "b")`, which `parse_selections` avoids by using `split('/')` and taking two. Port
`getMissingConfiguredDirectToolServers` into `cyrup-mcp::cache`.
**verify** — `["srv/", "srv/tool", "a/b/c", "", "/x"]` produces the exact expected server/tool sets
(`a/b/c` → server `a`, tool `b`); a server with `directTools: false` under a global `directTools: true`
is not reported missing.

**MCP-148 — The protocol layer is `rmcp`, client-only** · n/a · n/a · rmcp
**upstream** — the adapter delegates the whole protocol to `@modelcontextprotocol/client` and
hand-writes only the two transports the SDK lacks (the raw unix socket, and the SSE fallback *policy*).
**behavior** — Whatever supplies the protocol must cover the request families, bidirectional handlers,
pagination, `_meta` propagation, the protocol revisions, list-changed notifications, server
capabilities, server instructions, and the OAuth 2.1 + PKCE stack.
**cyrup** — Settled: `rmcp = { version = "3.1.2", default-features = false, features = ["client",
"transport-child-process", "transport-streamable-http-client-reqwest", "reqwest", "auth"] }`.
`default-features = false` is mandatory (`default = ["base64", "macros", "server"]`, and `server`
pulls `schemars`, `pastey` and `uuid` for a role the adapter never plays; `base64` returns
transitively through `client-side-sse`). The `reqwest` feature is what selects rustls — `__reqwest`
alone gives a TLS-less client. `elicitation` is **not** needed: it is `["dep:url"]` and every use is
server-side; the client elicitation types are unconditional under `client`. `which-command` stays off:
`npx_resolver` already does far more than `which` for the one command shape that needs it. New to the
lock file: `process-wrap` (via `transport-child-process`) and `sse-stream` (via the streamable-HTTP
client); `oauth2` 5.0.0 is already there. The first pass framed this as an open decision between rmcp
and hand-rolling on `HttpCaps`/`ProcCaps`; that tension does not exist — those are the **WASM-guest**
capability grants, and `cyrup-mcp` is a native built-in crate that links `rmcp`, `tokio` and `reqwest`
directly, exactly as `cyrup-ext-subagents` links `tokio::process` and `nix`.
**verify** — the `@modelcontextprotocol/conformance` client baseline the adapter already ships
(`conformance/baseline-client.yml`) run against `cyrup-mcp`.

**MCP-149 — Tracker: section 03 index and cross-section edges** · n/a · S · hand-written
**upstream** — n/a.
**behavior** — n/a.
**cyrup** — Cross-section edges that must not be dropped: MCP-118 and MCP-122 need section 05's
sampling/elicitation handlers; MCP-115/116 need section 05's OAuth provider and
`invalidateAuthEntryCache`; MCP-140 needs section 02's name-formatting helpers from `types.ts`;
MCP-130 lives in section 04's `init.ts`; the JSON-Schema validator passed into every client belongs to
the tool-execution section. Within cyrup, the only out-of-crate work this section requires is: one
`pub` promotion in `cyrup_ext::caps::proc` (MCP-103), one consolidated agent-dir resolver (MCP-139),
and the four coordinated edits to `cyrup_ext_subagents::exec::mcp_direct_tools` (MCP-141/142/143/144 +
MCP-146). None of it touches `HostServices`, `InitApi` or `ExtensionHost`.
**verify** — n/a.

---

### Out of scope

These are decisions by the project owner, recorded with their reasons so a later pass does not re-file
them as gaps.

**Cut 1 — the legacy HTTP+SSE transport.** The 2024-11-05 two-endpoint shape (GET `/sse` → `endpoint`
event → POST). Supported transports are exactly `stdio` and `streamable HTTP`. The reason is stronger
than a preference: **rmcp 3.1.2 ships no SSE client transport at all** — `crates/rmcp/src/transport/`
contains `async_rw.rs`, `auth.rs`, `child_process.rs`, `common/`, `io.rs`, `sink_stream.rs`,
`streamable_http_client.rs`, `streamable_http_server/`, `worker.rs`, `ws.rs` and nothing else, and the
`client-side-sse` feature is only the SSE *frame parser* the streamable-HTTP client consumes.
Supporting it would mean hand-writing a protocol transport, precisely what the dependency decision
exists to avoid. Removed with it: `shouldFallbackToSse` (the 404/405/406/415 downgrade probe), the
`SSEClientTransport` construction inside `connectHttpClient`, and the `httpTransport === "sse"`
branch. **Seams:** `ServerEntry.httpTransport` keeps only `"streamable-http"` and `"sse"` is rejected
at config load with a named diagnostic (MCP-113); the HTTP attempt loop terminates on the `throw err`
that preceded the fallback arm, with every earlier arm intact (§3.4); `ServerEntry.protocolVersion` is
**not** about this transport and stays in full (§3.6); the probe keeps all three strategies but gains
one arm so a legacy-SSE-only endpoint is diagnosed by name rather than reported as connectable
(§3.14); `mcp-trace.ts`'s `traceTransportKind` loses its `"sse"` variant.

**Cut 2 — MCP Apps / the UI extension, entirely.** In this section that removes
`attachAdapterNotificationHandlers`' `SERVER_STREAM_RESULT_PATCH_METHOD` registration, the
`uiStreamListeners` map keyed by stream token, and `registerUiStreamListener`/`removeUiStreamListener`
(MCP-121). **Seams:** `ServerConnection` and the manager lose one map and two public methods and
nothing else; the metadata cache keeps the field *names* `uiResourceUri`, `uiStreamMode` and
`uiVisibility` reserved in the on-disk schema (absent and ignored) because
`cyrup-ext-subagents` reads this file and `CACHE_VERSION` must not move; and `uiVisibility` is not
merely reserved but **still written and still read**, because `reconstructToolMetadata` filters on
`isUiToolVisibleToModel` and dropping that would expose to the model tools the server explicitly
marked app-only (§3.17, MCP-140). Consequences propagated from elsewhere: no `axum`, no local HTTP
server.

**Cut 3 — the raw unix-socket transport.** `unix-socket-transport.ts` and `ServerEntry.socket`. rmcp
ships `UnixSocketHttpClient` (`transport/common/unix_socket.rs`, feature
`transport-streamable-http-client-unix-socket`), but that is **streamable HTTP over a UDS** — a
different wire shape from the adapter's raw framed socket, which targets `rmcp-mux`. rmcp does not
ship the adapter's shape, and stdio plus streamable HTTP cover the field. **Seams:** the
"exactly one of command, url, or socket" invariant becomes "exactly one of `command` or `url`", and a
config carrying `socket` produces a named diagnostic rather than a silent skip (MCP-113); the
`tokio` `net` feature is no longer needed by `cyrup-mcp`; **the `socket` key stays in the config-hash
pre-image** (§3.17), because dropping it changes the digest for every server while keeping it costs
one always-absent field.

**Cut 4 — `mcpScript` / the JavaScript worker.** Nothing in this section's seven files references it;
recorded here only so the boundary is explicit. It removes the only JS-engine question in the port —
no `rquickjs`, no vendored C, no `boa`, and `node` is not a production dependency.

Two further consequences of the cuts land inside this section and are worth stating as decisions
rather than omissions. **The framing question is gone**: with the socket transport cut and stdio
delegated to `TokioChildProcess`/`AsyncRwTransport`, no NDJSON reader is written and the unresolved
`\r`-handling question from the first pass has nothing to attach to (MCP-112). And **`node` never
appears**: `npm exec --yes --package <spec> -- node -e 1` is the force-cache argv the npx resolver
already ports verbatim — the adapter authoring a JS expression whose body is the literal no-op `1`,
solely to make npm populate its own cache before a third-party Node MCP server launches. That is
inside the "spawning a third-party Node server is fine" boundary, and it is already in the shipped
Rust.

---

### What does not fit cleanly

Three genuine open decisions. **No host addition survives in this section** — nothing here needs a new
`HostServices`, `InitApi` or `ExtensionHost` verb, and the two the first pass filed (a bus-emit route
for the status snapshot, and a resolution to the `ProcCaps`-vs-rmcp spawn tension) both dissolve on
inspection: there is no consumer for the bus topic, and `ProcCaps` is the WASM-guest grant, not a
native crate's spawn path.

**1. `Auto` negotiation against a stdio server that exits on `server/discover`.** SDK v2 runs
`server/discover` on a **disposable sibling process**, so a legacy stdio server that exits on the
unknown method still connects; rmcp runs discover and the legacy fallback on the **same** transport,
and returns `DiscoverOutcome::Legacy` only when the probe produced a correlated JSON-RPC error — a
child that exits produces a transport error and there is no fallback. Upstream ships a fixture for
exactly this case. Options: **(a)** adopt `ClientLifecycleMode` as-is and record the loss —
`protocolVersion: "auto"` against a discover-intolerant stdio server fails where pi succeeded;
**(b)** adopt it for HTTP and, for stdio + `auto` only, spawn a disposable sibling child, run
`ClientLifecycleMode::Discover` on it, then open the real child with `Initialize` or `Discover` pinned
to the negotiated revision — the literal upstream mechanism, at the cost of one extra short-lived
child on one config path; **(c)** hand-roll negotiation, which the dependency decision rules out.
**Recommend (b)**: it is the only option preserving upstream's observable behaviour, the cost is
bounded and confined to one arm, and it needs nothing from cyrup's core — `cyrup-mcp` spawns its own
children already. **(a)** is acceptable only with explicit sign-off that
`legacy-exits-on-discover`-shaped servers may break.

**2. `reinit_on_expired_session` — off, and `withSessionRecovery` ported literally.** rmcp's
streamable-HTTP transport can transparently recover an expired session: replay `initialize`,
re-establish streaming, retry the in-flight request, bounded to one attempt. It covers upstream's
404-with-session arm and nothing else — not the 400/`-32000` arm, not the `ProtocolError` arm, and not
the manager-level reconnect that upstream's `onNeedsAuth` hook and `SessionRecoveryAuthRequiredError`
hang off. Turning it on **and** keeping `withSessionRecovery` means two independent one-shot retries
stacking on the same failure, with the transport's silent retry hiding the reconnect the adapter
wanted. **Recommend: leave it off** and port `withSessionRecovery` as specified; revisit only if the
adapter-level wrapper proves it cannot see a 404 the transport already swallowed.

**3. The client identity string.** Upstream announces `{ name: "pi-mcp-<server>", version: "1.0.0" }`
and the server sees it. `cyrup-mcp-<server>` is the obvious rename, but any MCP server that
allow-lists or fingerprints the pi client name will not recognise cyrup. **Recommend the rename** —
misrepresenting the client to a remote server to inherit an allow-list is worse than being refused —
and record it in the porting notes so a "why does this server reject us" report has an answer.

Two items are *not* open decisions and are recorded so nobody reopens them. The `read_` vs `get_`
resource-tool prefix has one correct answer (`read_`, because the adapter is the writer) and is filed
as MCP-146. The `undefined`-vs-`null` hash token has one correct answer (`undefined`, because a
"cleaner" pre-image invalidates every pi-era cache and makes the golden vectors meaningless) and is
filed as MCP-142.

---

### Coverage

**Read**

*Upstream at `v2.25.0`, in full:* `server-manager.ts`, `npx-resolver.ts`, `metadata-cache.ts`,
`mcp-probe.ts`, `session-recovery.ts`, `mcp-status.ts`, `unix-socket-transport.ts`, `utils.ts`,
`agent-dir.ts`, `resource-tools.ts`. *Partial:* `types.ts` (the status-event, `ServerEntry` and cache
schema declarations this section cites), `mcp-trace.ts` (the transport-wrap call sites and the
disposable-sibling rationale in `wrapTransportWithMcpTrace`'s doc comment), `lifecycle.ts` (constants
only), `init.ts` (the manager-wiring facts MCP-118/128/130 depend on), `direct-tools.ts` (grepped for
`read_`/`get_`), `package.json` (dependency pins), and the fixture and conformance file listings.

*cyrup, branch `david/cyrup`, by symbol:* `cyrup_ext::caps::proc::npx_resolver` (`resolve_npx_binary`,
`NpxResolution`, `NpxCacheEntry`, `cache_key`, `extract_package_name`, `find_cached_package_dir`,
`load_cache_at`, `save_cache_entry_at`, `SAVE_CACHE_LOCK`, `agent_dir`, `npx_cache_path`,
`force_npx_cache`, `detect_js_binary`), `cyrup_ext::caps::proc` (`host_home_dir`,
`interpolate_env_vars_with`, `resolve_config_path`, `apply_npx_resolution`, `ProcCaps::spawn`),
`cyrup_ext_subagents::exec::mcp_direct_tools` (`CACHE_VERSION`, `CACHE_MAX_AGE_MS`, `CachedTool`,
`CachedResource`, `ServerCacheEntry`, `MetadataCache`, `McpDirs`, `load_metadata_cache`,
`is_server_cache_valid`, `compute_mcp_server_hash`, `stable_stringify`, `opt_str_value`,
`interpolate_env_record`, `interpolate_env_vars`, `expand_pattern`, `resolve_config_path`,
`resolve_bearer_token`, `resolve_direct_tool_names`, `parse_selections`, `resource_name_to_tool_name`,
`home_dir`, `resolve_agent_dir`), `cyrup_core::cancel` (`CancelToken`, `RunCancel`),
`cyrup_ext::host::services` (the full `HostServices` method list), `cyrup_ext::facade`
(`ExtensionHost::{register_late_tool, refresh_tools, active_tools, bus}`), `cyrup_ext::native`
(`NativeExtension::{set_host_services, is_ambient, decides_project_trust}`), `cyrup-ext`'s feature
table.

*rmcp, from the checkout at `rmcp-v3.1.2-7-gf713ebd`:* `crates/rmcp/Cargo.toml`'s feature table;
`transport.rs` (the client-side export list); `transport/child_process.rs`
(`TokioChildProcess`, `TokioChildProcessBuilder`, `ChildWithCleanup`, `graceful_shutdown`,
`MAX_WAIT_ON_DROP_SECS`, `ConfigureCommandExt`, `which_command`);
`transport/streamable_http_client.rs` (`StreamableHttpClientTransportConfig`, `StreamableHttpError`
including `SessionExpired` / `AuthRequired` / `InsufficientScope` and `auth_challenge`, the
`reinit_on_expired_session` recovery path); `transport/common/reqwest/streamable_http_client.rs` (the
`NOT_FOUND && session_was_attached` gate); `transport/common/unix_socket.rs`;
`service/client.rs` (`ClientLifecycleMode`, `ClientServiceExt::serve_with_lifecycle`,
`serve_client_with_lifecycle_and_ct`, `discover_startup`, `legacy_startup`, `DiscoverOutcome`,
`DEFAULT_AUTO_DISCOVER_TIMEOUT`, `ClientInitializeError` and its `auth_challenge`, `Peer::list_all_*`);
`service.rs` (`PeerRequestOptions`, `RunningService::{peer_info, peer, cancellation_token, waiting,
cancel}`, `RequestHandle::cancel`); `handler/client.rs` (the full `ClientHandler` method set);
`model.rs` (`ProtocolVersion` and `LATEST`); `model/capabilities.rs` (`ServerCapabilities`).

**Excluded**

- `lifecycle.ts` — the health-check timer, keep-alive reconnect loop and idle-shutdown sweep. It
  *calls* `manager.connect`/`isIdle`/`close` and owns no transport logic; section 04's file. Its
  constants are cited only as inputs to MCP-127.
- `init.ts`, `index.ts`, `runtime-owner.ts`, `abort.ts`, `state.ts` — session lifecycle and ownership;
  section 04. `init.ts` was grepped only for the four manager-wiring facts MCP-118/128/130 depend on.
- `mcp-trace.ts` — JSONL protocol tracing. Read only for the transport-wrap call sites and the
  sibling-process rationale MCP-117 turns on; the writer, rotation and event schema are a separate
  concern. In cyrup the tracer becomes an `rmcp::transport::Transport` decorator.
- `mcp-auth.ts`, `mcp-auth-flow.ts`, `mcp-oauth-provider.ts`, `mcp-callback-server.ts`, `oauth.ts`,
  `oauth-handler.ts`, `mcp-keyring-helper.cjs` — the OAuth/keychain subsystem; section 05. Referenced
  here only through the four symbols `server-manager.ts` imports: `McpOAuthProvider`,
  `extractOAuthConfig`, `supportsOAuth`, `invalidateAuthEntryCache`.
- `sampling-handler.ts`, `elicitation-handler.ts` — referenced through `registerSamplingHandler`,
  `registerElicitationHandler` and `handleUrlElicitation`; section 05.
- `json-schema-validator.ts` — `createJsonSchemaValidator()` is passed into every client; rmcp does no
  client-side validation, so this is real hand-written work on `jsonschema`, owned by the
  tool-execution section.
- `types.ts`'s name-formatting helpers (`resolveToolPrefix`, `formatToolName`, `getToolNameCandidates`,
  `isToolAllowed`, `formatPromptCommandName`) — consumed by MCP-140, specified in section 02.
- `__tests__/**` and `conformance/**` — the test surface. Individual fixture filenames and
  `conformance/baseline-client.yml` are named in `verify` lines as the concrete things to port; the
  tests themselves are out of scope for a spec.
- `unix-socket-transport.ts`, the SSE branch of `connectHttpClient`, `shouldFallbackToSse`,
  `attachAdapterNotificationHandlers`' stream-patch registration and `registerUiStreamListener` /
  `removeUiStreamListener` — cut; see *Out of scope*.

**Corrections to the first pass**

- **The `rmcp`-vs-hand-roll question is not open.** It was filed as `MCP-148 · critical · open-question`,
  with three sub-decisions turning on whether rmcp's transports "bypass `ProcCaps` and `HttpCaps`". They
  are the **WASM-guest** capability grants; a native built-in crate links `rmcp`, `tokio` and `reqwest`
  directly, as `cyrup-ext-subagents` already does. Verdict `rmcp`, severity n/a.
- **Streamable HTTP is not a hand-write.** Filed as `critical · L · not-ported` with a "hand-write on
  `HttpCaps::request_stream` + `eventsource-stream`" option. It is `StreamableHttpClientTransport`
  plus a config struct: `high · S · rmcp`.
- **The NDJSON framing question dissolves.** Filed as `MCP-112 · open-question · unverified`, blocked on
  obtaining `@modelcontextprotocol/client@2.0.0` to settle `\r` handling. With the socket transport cut
  and stdio delegated to `TokioChildProcess`/`AsyncRwTransport`, no framing code is written.
- **The unix-socket transport and the SSE ladder are cuts, not `high`-severity work.** Filed as
  `MCP-111 · high · M` and `MCP-110 · high · L`, together with a request to add `net` to the workspace
  `tokio` features.
- **`isUnauthorizedHttpError` is not hand-written.** `ClientInitializeError::auth_challenge()` walks the
  `source()` chain for `AuthRequiredError` / `InsufficientScopeError` and returns the `WWW-Authenticate`
  header — strictly more informative, and it adds a 403 `insufficient_scope` arm upstream lacks.
- **The 404 arm of `isTerminatedSession` is not hand-written.** `StreamableHttpError::SessionExpired` is
  raised on exactly `NOT_FOUND && session_was_attached`, the same predicate including the session gate.
- **The stderr mode divergence does not exist.** The first pass built the stderr story on `ProcCaps`
  (`capture_stderr`, a 16 MiB `PipeBuf`, no `inherit` mode, and a proposed workaround forwarding debug
  stderr through `HostServices::notify`). `TokioChildProcessBuilder` defaults to `Stdio::inherit()` and
  accepts `Stdio::piped()`, returning the `ChildStderr` handle — upstream's `debug ? "inherit" : "pipe"`
  exactly.
- **The `ProcCaps` registry-exhaustion prerequisite is not on this path.** MCP-131 was filed with a
  follow-up to add eviction to `ProcCaps` because `MAX_SPAWNED_PROCESSES` never evicts. `cyrup-mcp` owns
  `TokioChildProcess` values directly; no capability registry participates.
- **The status-bus prerequisite dissolves.** MCP-138 was `open-question`, requiring `cyrup-mcp` to be
  handed an `Arc<SharedBus>` or `Arc<ExtensionHost>` at construction plus a channel-name decision. No
  consumer for the topic exists in cyrup; the snapshot stays in-crate on a `tokio::sync::watch`.
- **The `pi-mcp-adapter-port.md` blocker dissolves.** MCP-101 and MCP-148 were both blocked on a
  document cited by `caps/proc.rs` and `caps/http.rs` as "the locked WIT shape" but absent from `docs/`.
  Nothing in this section modifies `ProcSpawnSpec`, so the question does not arise.
- **`combineAbortSignals` needs no new primitive.** `cyrup_core::CancelToken` **is**
  `tokio_util::sync::CancellationToken`, the exact type `rmcp::serve_client_with_ct` takes, so the
  combined token binds a connection with no adapter layer; only the two-parent `combine` helper is new.
- **Severity recalibration.** The first pass rated ten of fifty items `critical`, most of them
  prerequisite-shaped ("without this the subsystem is inert"). Three survive under the house scale —
  MCP-141, MCP-142 and MCP-146 — because each produces a silently wrong result: a digest or a tool name
  that the already-shipped in-tree reader rejects, with no error anywhere. Blocking-ness now lives in
  the item bodies.
- **`socket` and `protocolVersion` stay in the hash pre-image despite Cut 3.** The first pass had no
  occasion to consider this; dropping the keys would change the digest for every server and break the
  golden-vector fixture MCP-141 depends on.
- **The probe gains one arm rather than losing one.** The first pass specified the ladder faithfully but
  did not consider Cut 1's effect: a legacy-SSE-only endpoint would be classified "responded with an MCP
  event stream" while the connect failed. All three strategies survive; the classification does not.
