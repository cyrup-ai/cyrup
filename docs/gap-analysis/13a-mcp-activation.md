# 13a · Activation, lifecycle and the host seam

> Part of **[13 — cyrup-mcp](13-cyrup-mcp.md)**, which holds the thesis, the seam map, the
> architecture and the one canonical table of every port unit. Method and phasing are in
> **[MCP-PORT-METHODOLOGY.md](MCP-PORT-METHODOLOGY.md)**.

Upstream is `pi-mcp-adapter` v2.25.0. cyrup is branch `david/cyrup`. rmcp is the checkout at
`/Users/davidmaple/cyrup.ai/rmcp` (`rmcp-v3.1.2-7-gf713ebd`). cyrup is referenced by **symbol and
file** only — it moves under this document and a line-anchored plan is stale on arrival.

This is the activation path: the code that decides *when* an MCP server process exists, *what the
model can see before one does*, and *who is allowed to touch a half-built runtime*. It is not the MCP
protocol and it is not the tool surface — it is the scaffolding those two hang from. Eleven upstream
files: `index.ts` (the extension factory body, run synchronously at module load), `init.ts`
(`initializeMcp`, which builds the whole live runtime and returns an `McpExtensionState`),
`runtime-owner.ts` + `abort.ts` + `error-signal.ts` (the concurrency and ownership primitives),
`lifecycle.ts` (the reconnect/idle-shutdown state machine), `state.ts` (the state record's type),
`agent-plugin-loader.ts` (a sandboxed config translator for a vendor-neutral plugin format),
`agent-dir.ts` and `onboarding-state.ts` (path and persistence helpers), and `cli.js` (a standalone
`pi-mcp-adapter` binary that scaffolds compatibility imports).

**The design's central trick is cache-first registration.** `installMcpAdapter` (`index.ts`) runs to
completion with **no `await` anywhere** and **no MCP server contacted**. It reads `mcp.json` off
disk, reads `<agent_dir>/mcp-cache.json` off disk, and from those two files alone registers the full
model-visible surface — one direct tool per cached MCP tool and resource, one slash command per
cached MCP prompt, the `mcp` gateway tool, `/mcp`, `/mcp-auth`, and the `--mcp-config` flag. Only
*then*, deferred by one macrotask tick, does it consider spawning anything, and only if some server
declares `lifecycle: "eager" | "keep-alive"`. Everything else connects lazily on first call. A
porter who implements "connect servers, then register tools" reproduces none of this: startup would
block on N subprocess handshakes, the system prompt would change shape between runs, and the
provider's prompt-cache prefix would be invalidated on every reconnect — which is exactly what
`settings.freezeDirectTools` exists to prevent.

**This lands in cyrup as extension-owned code, and the port changes essentially nothing in cyrup's
core.** `crates/cyrup-mcp` is a native built-in crate compiled into the binary, the same shape as
`crates/cyrup-ext-subagents` — attached through `SessionFactory::with_native_extension` /
`SessionBuilder::with_native_extension` and loaded by the session builder via
`ExtensionHost::load_native_with_services`. A native extension is **not** sandboxed: `HostServices`
is the capability surface a *WASM guest* is confined to, while a native crate links `rmcp`, `tokio`,
`keyring`, `reqwest` and the filesystem directly and reaches for `HostServices` only where it
genuinely touches the host — drawing UI, notifying, reading session state, honouring cancellation,
registering tools and commands. Every mechanism in this section maps onto something that already
exists: registration onto `InitApi::{register_tool, register_command, register_tool_renderer,
register_flag, subscribe}`, the session-lifecycle hooks onto `EventKind::{SessionStart,
SessionShutdown, ToolResult}`, the footer onto `HostServices::set_status`, notifications onto
`HostServices::notify`, `/reload` onto `HostServices::control(ControlOp::Reload)`, the ownership
token onto `tokio_util::sync::CancellationToken`. **Exactly one host addition is load-bearing for
this section** — a native has no handle to `ExtensionHost::register_late_tool` (HA-1) — and one more
is secondary (argument completions, HA-2).

**The one structural difference, and it is an ordering inversion rather than a naming one.** In pi
the extension factory runs **once per process** and `session_start` fires repeatedly on the same
closure, which is why `lifecycleGeneration`, `currentOwner` and `currentOAuthRuntime` are
module-scoped mutable slots. In cyrup a session replacement **builds the replacement first** and
only then tears the old one down: `AgentSessionRuntime::new_session_with` calls
`SessionFactory::build_with_parent` — which re-runs `ExtensionHost::load_native_with_services` →
`NativeExtension::init` on the **same `Arc<dyn NativeExtension>`** — and *then* calls `install`,
whose `install_inner` fires `SessionShutdown` on the outgoing session (via
`AgentSession::dispose_with`) and finally `AgentSession::emit_session_start` on the new one, which
itself fires at most once per session object. The real cyrup order on a replacement is therefore:

```
init()          for generation N+1   ← fresh ExtensionHost, fresh InitApi, fresh registry
SessionShutdown for generation N     ← the old runtime's teardown + metadata flush
SessionStart    for generation N+1
```

pi's "one factory, many `session_start`s" becomes cyrup's "one object, many `init`s, one
`SessionStart` each, and the new `init` runs *before* the old shutdown". That inversion settles
MCP-014 and is the single most likely source of a subtle port defect in this subsystem.

**Scope.** Four surfaces are cut from the whole port by decision (see *Out of scope*). Inside this
section they remove: the `mcpScript` tool registration and `McpSettings.scriptMode`; the
`action: "ui-messages"` dispatch arm of the `mcp` tool; the `uiServer` / `completedUiSessions` /
`uiResourceHandler` / `consentManager` fields of `McpExtensionState` and the two `uiServer.close`
call sites; and the `httpTransport: "sse"` acceptance in the Agent Plugin translator. Everything
else in these eleven files ports.

---

### How it lands

| adapter capability | upstream mechanism | cyrup mechanism | verdict |
|---|---|---|---|
| extension is present in every session, off under `--no-extensions` | `package.json`'s `"pi": {"extensions": ["./index.ts"]}`; `export default createMcpAdapter()` | new crate `cyrup-mcp`; `impl NativeExtension for McpExtension`; attached with `SessionFactory::with_native_extension`; `is_ambient() -> true` | `hand-written` |
| synchronous registration of the whole surface from disk caches | `installMcpAdapter`, no `await` in its body | `NativeExtension::init` + `InitApi::{register_tool, register_command, register_tool_renderer, register_flag}` | `host-verb` |
| `--mcp-config` known to `--help`, value read from argv | `pi.registerFlag("mcp-config", …)` + `getConfigPathFromArgv` reading `process.argv` | `InitApi::register_flag` + `std::env::args()` — the literal upstream mechanism, no flag read-back needed | `host-verb` |
| session start / shutdown / tool-result hooks | `pi.on("session_start" \| "session_shutdown" \| "tool_result")` | `InitApi::subscribe(&[EventKind::SessionStart, SessionShutdown, ToolResult])` → `NativeExtension::on_event` | `host-verb` |
| re-flag a returned MCP failure as an error | `error-signal.ts`'s `toolErrorOverride` returning `{isError:true}` | `HookOutcome::Mutate(EventPatch::ToolResult{ is_error: Some(true), ..None })`; `apply_patch` leaves `content`/`details` untouched when `None` | `host-verb` |
| ownership token, LIFO cleanups, idempotent stop | `runtime-owner.ts`'s `McpRuntimeOwner` over one `AbortController` | `tokio_util::sync::CancellationToken` + a cleanup stack in `cyrup-mcp` | `hand-written` |
| stale-context fencing (`createOwnedUi`) | a recursive `Proxy` making a dead `ctx.ui` inert | an `OwnedServices` newtype in `cyrup-mcp` delegating `HostServices` behind an owner check | `extension-owned` |
| run-scoped cancellation (`ctx.signal`) | `AbortSignal` on `ExtensionContext` | `HostServices::is_run_cancelled` (documented CYRUP-DELTA: a poll, not a wake). **Not overridden by `LiveHostServices` today** — see *What does not fit cleanly* | `host-verb` |
| eager/keep-alive pre-warm before the first prompt | `startLoadTimeInitialization` on `setImmediate` with a synthetic print-mode ctx | a `tokio::spawn` from `init()` using the `Arc<dyn HostServices>` stashed by `set_host_services` | `extension-owned` |
| long-lived server children and timers across turns | module-scoped runtime, `setInterval().unref()` | tokio tasks owned by the extension, selecting on the owner token; settled precedent in `cyrup-ext-subagents` | `extension-owned` |
| register direct tools **after** init (connect, `tools/list_changed`, `mcp({connect})`) | `syncDirectTools` → `pi.registerTool` from any live handler | `ExtensionHost::register_late_tool` + `refresh_tools` exist and propagate through `AgentSession::{refresh_extension_tools, next_turn_tools, push_active_tools}` — **but a native has no handle to `ExtensionHost`** | `host-addition` HA-1 |
| register prompt slash commands after init | `syncPromptCommands` → `pi.registerCommand` | no `register_late_command` sibling; same seam as HA-1 | `host-addition` HA-1 |
| remove tools when a server is disabled/disappears | optional `pi.unregisterTool`, else `setActiveTools(active \ removed)` | `HostServices::{active_tools, set_active_tools}` — cyrup lands on upstream's own documented no-`unregisterTool` branch | `host-verb` |
| `/mcp` and `/mcp-auth` | `pi.registerCommand` | `InitApi::register_command` + `NativeExtension::execute_command` at **command tier** (session mutation legal) | `host-verb` |
| `/mcp setup` and `/mcp status` → `ctx.reload()` | `ctx.reload` bound off the context | `HostServices::control(ControlOp::Reload)` | `host-verb` |
| `/mcp <TAB>` and `/mcp reconnect <server><TAB>` | `getArgumentCompletions` returning `{value,label}` | `InitApi::add_autocomplete` exists; `ExtensionHost::command_completions` is WASM-only with no TUI consumer | `host-addition` HA-2 |
| footer status segment | `ui.setStatus("mcp", ui.theme.fg("accent", text))` | `HostServices::set_status(key, Option<&str>)` — `None` clears. No `fg(role, text)` analog: the text goes out uncoloured | `host-verb` |
| notifications | `ctx.ui.notify(msg, "info"\|"warning"\|"error")` | `HostServices::notify` + `NotifyKind::{Info, Warning, Error}` | `host-verb` |
| open the OAuth/browser URL | `openUrl` → `execOpen` → `pi.exec` per platform | `opener` directly (the seam map's dependency decision), or `HostServices::exec` for the literal `$BROWSER` table | `extension-owned` |
| status snapshot published on the event bus | `mcp-status.ts` on `pi.events`, `MCP_STATUS_EVENT` v1 | no consumer exists in cyrup; keep the snapshot as an in-crate `tokio::sync::watch` | `extension-owned` |
| approval brokering to another extension | `MCP_TOOL_APPROVAL_REQUEST_EVENT` carrying a `claim()` callback | `ExtHooks::before_tool_call` + `cyrup-permission-system`'s existing MCP target derivation is the same gate, already wired and fail-closed | `host-verb` |
| agent directory | `getAgentDir()` — `PI_CODING_AGENT_DIR` else `~/.pi/agent` | `ConfigDirs::agent_dir` (`CYRUP_AGENT_DIR` → `PI_CODING_AGENT_DIR` → `<home>/.cyrup/agent`), passed into the extension the way `cyrup-ext-subagents` takes it | `extension-owned` |
| `npx`/`npm` binary pre-resolution | `npx-resolver.ts` | `cyrup_ext::caps::proc::npx_resolver` — **already a complete port**; currently a private `mod` inside `caps/proc.rs` and needs a `pub` promotion | `extension-owned` (reuse) |
| JSONC config reading | `strip-json-comments` | `cyrup_permission_system::jsonc` — the same parser `cyrup-permission-system` uses on `mcp.json` | `extension-owned` (reuse) |
| standalone `pi-mcp-adapter init` binary | npm `bin`, `cli.js` | a `cyrup mcp init` arm in `crates/cyrup/src/subcommands.rs`'s visible verb table | `hand-written` |
| `mcpScript` tool and `settings.scriptMode` | `index.ts` registration + `mcp-code.ts` | — | `cut` |
| `action: "ui-messages"` dispatch, UI server lifecycle, consent manager | `executeUiMessages`, `state.uiServer`, `state.consentManager` | — | `cut` |

---

### Behavioural specification

#### 1 · The load-time activation sequence

`installMcpAdapter(pi, options)` executes **synchronously and completely** when the module is
loaded. There is no `await` in its body. Exact order, with the cut surfaces removed:

| # | action |
|---|---|
| 1 | `sessionConfig = options.config !== undefined ? cloneMcpConfig(options.config) : undefined`; `programmaticConfig = sessionConfig !== undefined` |
| 2 | declare the five module-scoped mutable slots: `state`, `initPromise`, `currentOwner`, `currentOAuthRuntime`, `lifecycleGeneration = 0` |
| 3 | `earlyConfigPath = programmaticConfig ? undefined : (options.configPath ?? getConfigPathFromArgv())` |
| 4 | `earlyConfig = programmaticConfig ? cloneMcpConfig(sessionConfig) : loadMcpConfig(earlyConfigPath)` |
| 5 | `earlyCache = loadMetadataCache()` |
| 6 | read `process.env.MCP_DIRECT_TOOLS` into `envRaw`; `envDirectToolOverride = envRaw?.split(",").map(trim).filter(Boolean)` |
| 7 | `registeredDirectTools: Map<name, fingerprint>`; `fallbackDeactivatedTools: Set<name>` |
| 8 | resolve render options; `toolRenderShell = resultRendering === "compact" ? "self" : "default"`; build `renderMcpToolResult` |
| 9 | `proxyToolRegistered = false`; `proxyToolDescription = null`; `directToolsFrozen = false` |
| 10 | `registeredPromptCommands = new Set<string>()` |
| 11 | **one slash command per cached MCP prompt** — `registerPromptCommands(resolveCachedPrompts(earlyConfig))` |
| 12 | `getPiTools = () => pi.getAllTools()` |
| 13 | `pi.registerFlag("mcp-config", {description: "Path to MCP config file", type: "string"})` |
| 14 | `pi.on("session_start", …)` |
| 15 | `pi.on("session_shutdown", …)` |
| 16 | `pi.on("tool_result", (event) => toolErrorOverride(event.details))` |
| 17 | `pi.registerCommand("mcp", …)` — with `getArgumentCompletions` |
| 18 | `pi.registerCommand("mcp-auth", …)` |
| 19 | ~~`mcpScript` registration~~ — **CUT 4** |
| 20 | `initialDirectTools = syncDirectTools(earlyConfig, earlyCache).specs` — **registers N direct tools from cache** |
| 21 | `syncProxyTool(earlyConfig, earlyCache, initialDirectTools)` — registers the `mcp` gateway tool |
| 22 | `startLoadTimeInitialization()` |

Steps 11, 20 and 21 are the point of the design: **the model-visible tool and command surface exists
before any MCP server process starts**, reconstructed entirely from `<agent_dir>/mcp-cache.json`.

`getActiveTools()` cannot be called during this window. `getActiveToolsIfReady()` calls
`pi.getActiveTools?.()` inside a `try` and swallows **only** an `Error` whose `.message` *includes*
the substring `"Action methods cannot be called during extension loading"`, rethrowing anything
else. cyrup needs no port of that try/catch: `HostServices::active_tools()` returns
`Option<Vec<String>>` and `None` **is** the "not ready" arm.

The public entry points are `createMcpAdapter(options = {})`, which clones the programmatic config
once at factory time and again per `mcpAdapter(pi)` call, and `export default createMcpAdapter()`,
which is what pi's loader invokes.

#### 2 · `--mcp-config`: two readers, deliberately

The flag is registered once but read two different ways *in this section* (a third lives in
`commands.ts`), because of *when* each reader runs:

- **Load time** — `getConfigPathFromArgv()` (`utils.ts`): `process.argv.indexOf("--mcp-config")`,
  and if `idx >= 0 && idx + 1 < process.argv.length` return `process.argv[idx + 1]`, else
  `undefined`. Only the space-separated form; `--mcp-config=path` is **not** recognised here.
- **Init time** — `pi.getFlag("mcp-config") as string | undefined`, reached only when
  `options.config === undefined`, and only after `options.configPath` has been given priority.

cyrup has the identical ordering: extension flag values are applied by
`ExtensionHost::apply_extension_flag_values`, called by the session builder **after** the
native-load loop, so `init()` cannot read the flag store either. The Rust port therefore does the
same direct `std::env::args()` scan. **There is no flag-read-back gap here** — `register_flag` is
for `--help` and for not reporting `--mcp-config` as an unknown flag, exactly as upstream.

#### 3 · `startLoadTimeInitialization` — the eager/keep-alive pre-warm

1. `hasStartupServer` = any entry in `earlyConfig.mcpServers` where `definition.disabled !== true`
   **and** `definition.lifecycle === "eager" || definition.lifecycle === "keep-alive"`. If none,
   **return** — nothing connects at load.
2. `setImmediate(() => { … })` — deferred by exactly one macrotask tick, so `installMcpAdapter`
   returns first.
3. Inside: `if (lifecycleGeneration !== 0 || state || initPromise) return;` — a `session_start` that
   already fired wins the race and this pass is abandoned.
4. `generation = ++lifecycleGeneration` (so it becomes 1), fresh `createMcpRuntimeOwner()`, fresh
   `createOAuthRuntime(owner.signal)`, assign both to `currentOwner`/`currentOAuthRuntime`.
5. `startInitialization(ctx, owner, oauthRuntime, generation, "stale_load_time_initialization")` with
   a **synthetic** `ExtensionContext`: `{mode:"print", hasUI:false, cwd:process.cwd(),
   model:undefined, modelRegistry:undefined, signal:undefined}`.

The synthetic ctx is what makes this safe: `hasUI:false` means `ui` is `undefined` throughout
`initializeMcp`, so no notification, status-bar write, elicitation or sampling handler is wired for
this pass.

#### 4 · The generation protocol

`session_start`, in exact order:

1. `generation = ++lifecycleGeneration`
2. snapshot `previousState`, `previousOwner`, `previousOAuthRuntime`
3. build `owner = createMcpRuntimeOwner()` and `oauthRuntime = createOAuthRuntime(owner.signal)`;
   assign to `currentOwner`/`currentOAuthRuntime`
4. `state = null; initPromise = null`
5. **`const stopPrevious = previousOwner?.stop("MCP extension session restarted") ?? Promise.resolve();`**
   — the `stop()` call is made **before** the `await`, so the abort is synchronous. The source
   comment states why: *"Abort synchronously before awaiting cleanup so old callbacks and startup
   work cannot resume into a stale ExtensionContext."*
6. `await Promise.all([stopPrevious, shutdownState(previousState, "session_restart"),
   previousOAuthRuntime ? shutdownOAuth(previousOAuthRuntime) : Promise.resolve()])`, wrapped in a
   `try/catch` that logs `MCP: failed to shut down previous session state: ${formatTerminalError(error)}`
   and continues.
7. `if (generation !== lifecycleGeneration || !owner.isActive()) return;`
8. `initialization = startInitialization(ctx, owner, oauthRuntime, generation, "stale_session_start")`
9. **Conditional blocking wait**: if `envRaw !== undefined && envRaw !== "__none__"`, compute
   `getMissingConfiguredDirectToolServers(earlyConfig, loadMetadataCache(), envDirectToolOverride)`;
   if that list is non-empty, `await initialization`. Otherwise `session_start` returns immediately
   with initialization still in flight.

`session_shutdown` is the same shape without the rebuild: `++lifecycleGeneration`; snapshot
`state`/`currentOwner`/`currentOAuthRuntime`; null all four slots;
`owner?.stop("MCP extension session shutdown")` **before** the
`await Promise.all([stopOwner, shutdownState(currentState, "session_shutdown"), oauthRuntime ?
shutdownOAuth(oauthRuntime) : Promise.resolve()])`; catch logs
`MCP: session shutdown cleanup failed: …`.

#### 5 · `shutdownState` — ordering and the preserved flush error

1. If `currentState` is null: `publishMcpStatusShutdown(pi.events)` and return.
2. `publishMcpStatusShutdown(currentState.statusEvents)`
3. ~~If `currentState.uiServer`: `.close(reason)` then null the field.~~ — **CUT 2**
4. `try { flushMetadataCache(currentState) } catch (error) { flushError = error }` — captured,
   **not** rethrown yet.
5. `try { currentState.owner ? await currentState.owner.stop(reason) : await
   currentState.lifecycle.gracefulShutdown() } catch (error) { … }` — if a `flushError` was already
   captured, the shutdown error is only **logged** as
   `MCP: graceful shutdown failed after metadata flush error: ${formatTerminalError(error)}`;
   otherwise it is rethrown.
6. `if (flushError) throw flushError;`

The invariant: **a metadata-flush failure is never masked by a shutdown failure.** Losing the tool
cache silently is worse than a noisy shutdown, because the next launch would register an empty tool
surface.

#### 6 · `startInitialization` — the staleness triple-check

1. `owner.addCleanup(() => cleanupMaterializedBinaryResources(owner.signal))` — registered
   **first**, so it runs **last** (LIFO).
2. `promise = initializeMcp(pi, ctx, owner, {…, oauthRuntime, statusEvents: pi.events})`. The options
   spread passes `configPath`/`config` **only** when `programmaticConfig || options.configPath !==
   undefined`.
3. `initPromise = promise` (module slot).
4. On resolve:
   - **`if (!owner.isActive() || generation !== lifecycleGeneration || initPromise !== promise)`** →
     the freshly-built state is stale; `await shutdownState(nextState, staleReason)` (errors logged
     as `MCP: failed to clean stale initialization state: …`) and return **without** committing. All
     three conditions are needed: the owner catches an abort, the generation catches a session
     restart, and `initPromise !== promise` catches a *second* initialization having superseded this
     one within the same generation.
   - `state = nextState`
   - install `nextState.onToolMetadataUpdated = (_serverName, _reason) => { if (state !== nextState
     || !owner.isActive()) return; syncPromptCommands(); if (directToolsFrozen) { logger.debug(
     \`MCP: metadata update for ${_serverName} (${_reason}) skipped — directTools frozen\`); return; }
     syncToolSurface(ctx); }`
   - `syncPromptCommands(); syncToolSurface(ctx); updateStatusBar(nextState); initPromise = null;`
   - if `earlyConfig.settings?.freezeDirectTools === true`: set `directToolsFrozen = true` and
     `logger.info("MCP: direct tools frozen after initial sync — reconnects won't rebuild the system
     prompt; use mcp({ connect: \"server\" }) to rediscover")`
5. On reject: return early if `!owner.isActive() || generation !== lifecycleGeneration`; return early
   if `initPromise !== promise && initPromise !== null`; else
   `console.error("MCP initialization failed: " + formatTerminalError(err))`, `initPromise = null`,
   `if (state) return;`, then `await Promise.all([owner.stop("MCP initialization failed"),
   shutdownOAuth(oauthRuntime)])` with its own catch logging
   `MCP: failed to clean rejected initialization: …`.

#### 7 · `McpRuntimeOwner`, `combineAbortSignals`, `createOwnedUi`, `abort.ts`

`McpRuntimeOwner` is one `AbortController`, one `cleanups: Array<() => void | Promise<void>>`, one
memoised `stopPromise`, and one shared `reportCleanupFailure(error, late)` that logs
`` `MCP: ${late ? "late " : ""}runtime cleanup failed: ${formatTerminalError(error)}` ``.

| member | semantics |
|---|---|
| `signal` | `controller.signal` |
| `isActive()` | `!controller.signal.aborted` |
| `addCleanup(cb)` | if **already aborted**, run `cb` on a microtask (`Promise.resolve().then(cleanup)`) and report failures as `MCP: late runtime cleanup failed: …`; else push onto the stack |
| `stop(reason = "MCP extension runtime stopped")` | if `stopPromise` exists return it (**idempotent**); `controller.abort(new Error(reason))`; `cleanups.splice(0).reverse()` → **LIFO**; each wrapped in `Promise.resolve().then(cleanup)`; `Promise.allSettled` → collect rejections → if any, build `new AggregateError(failures, "MCP runtime cleanup failed")`, `console.error("MCP: runtime cleanup failed: " + formatTerminalError(aggregate))`, and **throw** it |
| `throwIfInactive()` | `controller.signal.throwIfAborted()` |

`combineAbortSignals(...signals)`: filter out `undefined`; 0 → `undefined`; 1 → return that signal
**unwrapped** (no allocation, and identity is preserved for `isAbortError`); ≥2 →
`AbortSignal.any(active)`.

`createOwnedUi(ui, owner)`: a recursive `Proxy` with a `WeakMap<object, object>` identity cache so
the same target always yields the same proxy. The `get` trap: if `!owner.isActive()` return
`undefined`; read the member; if it is a function return a wrapper that re-checks `owner.isActive()`
and returns `undefined` without calling through; otherwise
`return owner.isActive() ? wrap(member) : undefined`. Primitives pass through unwrapped. **The
point:** a stale `ctx.ui` becomes silently inert rather than throwing, so every `ui?.notify(...)`
scattered through the runtime needs no owner check of its own.

`isAbortError(error, signal?)`: `true` if `signal?.aborted`; else `error instanceof Error &&
(error.name === "AbortError" || error.message === "MCP extension runtime stopped")`. Note the
**literal string match** on the default `stop()` reason — a custom reason string will not match this
second arm, only the `signal.aborted` arm.

`abort.ts`: `throwIfAborted(signal)` — if not aborted return; else
`throw signal.reason instanceof Error ? signal.reason : new Error(String(signal.reason ?? "MCP request aborted"))`.
`abortable(promise, signal)` — no signal → return the promise unchanged; else `throwIfAborted` first,
then race, with a `settled` flag and a `cleanup()` that removes the `abort` listener on **every**
exit path (the listener is registered `{once: true}` and *still* explicitly removed, so a resolved
promise leaks no listener onto a long-lived signal).

#### 8 · `initializeMcp` — the runtime build

**Everything the ctx can supply is snapshotted before the first `await`**, with the source comment
*"Pi guards ExtensionContext getters after reload. Snapshot all values that can be used by
asynchronous work before the first await."* Snapshots: `configPath`, `cwd`, `hasUI`, `mode`, `rawUi`,
`modelRegistry`, `initialSignal`. Derived: `ui = rawUi ? createOwnedUi(rawUi, owner) : undefined`,
`runtimeSignal = combineAbortSignals(owner.signal, initialSignal)`.

Wiring, in order:

| # | action |
|---|---|
| 1 | `config = options.config !== undefined ? cloneMcpConfig(options.config) : loadMcpConfig(configPath, cwd)` |
| 2 | `authStorageOptions = getAuthStorageOptions(config.settings?.oauthDir, cwd)` |
| 3 | `ownsOAuthRuntime = options.oauthRuntime === undefined`; `oauthRuntime = options.oauthRuntime ?? createOAuthRuntime(owner.signal)` |
| 4 | `manager = new McpServerManager(cwd)`; `setRuntimeSignal(owner.signal)`, `setOAuthRuntime(oauthRuntime)`, `setDefaultRequestTimeoutMs(settings.requestTimeoutMs)`, `setTraceConfig(settings.trace)`, `setAuthStorageOptions(...)` |
| 5 | **sampling gate**: `samplingAutoApprove = settings.samplingAutoApprove === true`; wire `setSamplingConfig` only when `settings.sampling !== false && (hasUI \|\| samplingAutoApprove)`. `getCurrentModel: () => owner.isActive() ? ctx.model : undefined` and `getSignal: () => owner.isActive() ? combineAbortSignals(owner.signal, ctx.signal) : owner.signal` are **live closures over `ctx`**, owner-guarded on each call |
| 6 | **elicitation gate**: `settings.elicitation !== false && hasUI`, and only if `ui` exists; `allowUrl: mode === "tui"` |
| 7 | `lifecycle = new McpLifecycleManager(manager, serverName => hasPendingAuth(serverName, undefined, oauthRuntime))` |
| 8 | allocate the live maps/sets: `toolMetadata`, `resourceCounts`, `promptMetadata`, `promptMetadataLive`, `serverInstructions`, `failureTracker`, `failureMessages`, `approvedToolCalls` (~~`uiResourceHandler`, `consentManager`~~ — **CUT 2**) |
| 9 | build `state: McpExtensionState`. `openBrowser` is `owner.throwIfInactive(); await openUrl(pi, url, process.env.BROWSER, owner.signal); owner.throwIfInactive()` — guarded on **both** sides of the await. `sendMessage` is `if (!owner.isActive()) return;` then `pi.sendMessage(...)` — **v2.26.1 replaces this two-line body with a `triggerTurn` convergence gate; build MCP-027a, not this cell** |
| 10 | if `ownsOAuthRuntime`: `owner.addCleanup(() => shutdownOAuth(oauthRuntime))` |
| 11 | `manager.setMetadataListChangedListener((serverName, reason) => { if (!owner.isActive()) return; updateServerMetadata; updateMetadataCache(state, serverName, {preserveEmptyResources:false}); notifyToolMetadataUpdated(state, serverName, reason); updateStatusBar(state); })` |
| 12 | `owner.addCleanup(() => lifecycle.gracefulShutdown())` |
| 13 | ~~`owner.addCleanup(() => { if (state.uiServer) { … } })`~~ — **CUT 2** |

**Cleanup LIFO order on `stop()`** after the cut is: `lifecycle.gracefulShutdown()` →
`shutdownOAuth` → (from `startInitialization`) `cleanupMaterializedBinaryResources`.

`McpExtensionState` upstream has 25 fields. Five are cut — `approvalEvents` (the pi-bus approval
broker, A-4: subsumed by `ExtHooks::before_tool_call` + `cyrup-permission-system`),
`uiResourceHandler`, `consentManager`, `uiServer`, `completedUiSessions` (all Cut 2) — leaving
**twenty**: `owner`, `manager`, `lifecycle`, `toolMetadata`, `resourceCounts`, `promptMetadata`,
`promptMetadataLive`, `serverInstructions`, `config`, `programmaticConfig`, `oauthRuntime`,
`authStorageOptions`, `failureTracker`, `failureMessages`, `approvedToolCalls`, `openBrowser`, `ui`
(the fenced services handle), `sendMessage`, `onToolMetadataUpdated`, `statusEvents` (an in-crate
`tokio::sync::watch` sender).

`state.openBrowser`'s `openUrl` (`utils.ts`) wraps `execOpen` and throws
``result.stderr || `Failed to open browser (exit code ${result.code})` `` on a non-zero exit.
`execOpen`'s platform table is load-bearing:

| platform | `$BROWSER` set | argv |
|---|---|---|
| `darwin` | absolute path whose extension is not `.app` | `exec(browser, [target])` |
| `darwin` | otherwise set | `exec("open", ["-a", browser, target])` |
| `darwin` | unset | `exec("open", [target])` |
| `win32` | set | `exec("cmd", ["/c", "start", "", browser, target])` |
| `win32` | unset | `exec("cmd", ["/c", "start", "", target])` |
| other | set | `exec(browser, [target])` |
| other | unset | `exec("xdg-open", [target])` |

The dependency decision replaces npm `open` with `opener`, which implements the same platform
dispatch; the `$BROWSER` override arms are the part `opener` does not cover and must be kept in
`cyrup-mcp` ahead of the `opener::open` fallback.

`isTuiMode(ctx)` is exported from `init.ts` and is exactly `ctx.hasUI && ctx.mode === "tui"`.

Then:

- `allServerEntries = Object.entries(config.mcpServers)`;
  `serverEntries = allServerEntries.filter(([, d]) => !isServerDisabled(d))` where `isServerDisabled`
  is `definition?.disabled === true` (**only the literal boolean**, per its own doc comment: *"Only
  the literal boolean `true` disables a server."*). If `serverEntries` is empty: when
  `allServerEntries.length > 0 && hasUI`, `ui?.notify("MCP: All ${n} server(s) are disabled", "info")`;
  then `publishMcpStatusSnapshot(state)` and **return the state** — no cache work, no lifecycle, no
  health checks.
- `idleSetting = typeof settings.idleTimeout === "number" ? settings.idleTimeout : 10`;
  `lifecycle.setGlobalIdleTimeout(idleSetting)` (which stores `minutes * 60 * 1000`).

#### 9 · Metadata-cache bootstrap

```
cachePath       = getMetadataCachePath()          // <agent_dir>/mcp-cache.json
cacheFileExists = existsSync(cachePath)
cache           = loadMetadataCache()
bootstrapAll    = false

if (!cacheFileExists) { bootstrapAll = true; saveMetadataCache({version:1, servers:{}}) }
else if (!cache)      { cache = {version:1, servers:{}}; saveMetadataCache(cache) }
```

The distinction is load-bearing: **file absent** ⇒ `bootstrapAll = true` ⇒ connect *every* enabled
server once so the cache is populated for the next launch. **File present but unparseable / wrong
version** ⇒ rewrite it empty but **do not** bootstrap — that path deliberately avoids a connect storm
on a corrupt cache.

#### 10 · Per-server lifecycle registration and cache rehydration

`prefix = config.settings?.toolPrefix ?? "server"`. For each `[name, definition]` in `serverEntries`:

1. `lifecycleMode = definition.lifecycle ?? "lazy"` (values: `"keep-alive" | "lazy" |
   "lazy-keep-alive" | "eager"`)
2. `persistsAfterFirstSpawn = lifecycleMode === "eager" || lifecycleMode === "lazy-keep-alive"`
3. `idleOverride = definition.idleTimeout ?? (persistsAfterFirstSpawn ? 0 : undefined)` — an `eager`
   or `lazy-keep-alive` server with no explicit `idleTimeout` gets **0 (never idle out)**
4. `lifecycle.registerServer(name, definition, idleOverride !== undefined ? {idleTimeout:
   idleOverride} : undefined)`
5. if `lifecycleMode === "keep-alive"`: `lifecycle.markKeepAlive(name, definition)` — **only**
   `keep-alive` is marked here; `lazy-keep-alive` is marked *after its first successful connect* by
   `markKeepAliveAfterConnect`, which itself early-returns when the definition is missing **or
   disabled**
6. `cachedEntry = cache?.servers?.[name]`; if `cachedEntry && isServerCacheValid(cachedEntry,
   definition)`:
   - `toolMetadata.set(name, reconstructToolMetadata(name, cachedEntry, prefix, definition,
     config.mcpServers, cache ?? undefined))`
   - if `Array.isArray(cachedEntry.resources)`: `resourceCounts.set(name,
     cachedEntry.resources.length)`
   - if `cachedEntry.prompts?.length`: `promptMetadata.set(name, reconstructPromptMetadata(name,
     cachedEntry.prompts ?? [], prefix, definition))` — note `promptMetadataLive` is **not** added,
     so a rehydrated prompt list is flagged as non-live
   - if `cachedEntry.instructions`: `serverInstructions.set(name, cachedEntry.instructions)`

#### 11 · The startup connect pass

```
startupServers = bootstrapAll
  ? serverEntries
  : serverEntries.filter(([, d]) => { const m = d.lifecycle ?? "lazy"; return m === "keep-alive" || m === "eager" })
```

If `ui && startupServers.length > 0`:
`ui.setStatus("mcp", formatMcpStatus(state.config, "connecting to ${n} servers..."))` where
`formatMcpStatus` returns `undefined` when `settings.mcpFooterStatus === "off"`, else
`` `${settings.showStatusIcon === false ? "MCP: " : "🔌 MCP: "}${message}` ``.

`results = await parallelLimit(startupServers, 10, async ([name, definition]) => { … })`.
`parallelLimit` (`utils.ts`) is an index-preserving worker pool: an `items.entries()` iterator shared
by `Math.min(limit, items.length)` workers, each writing `results[index] = await fn(item)`.
**Concurrency 10.**

Per-item body:
- `connection = await manager.connect(name, definition, runtimeSignal)`
- `connection.status === "needs-auth"` ⇒ `{name, definition, connection: null, error: "OAuth
  authentication required. Run /mcp-auth ${name}."}` (byte-exact message)
- caught error and `isAbortError(error, runtimeSignal)`: if `owner.signal.aborted` **rethrow** (kills
  the whole pass), else `{…, connection: null, error: null}` (a ctx-signal abort is a silent skip,
  not a failure)
- other caught error ⇒ `{…, connection: null, error: message}`

Then `if (initialSignal?.aborted) return state;` followed by `owner.throwIfInactive()`.

#### 12 · The two-pass startup metadata build

`startupKnownMetadata: Map<string, ToolMetadata[]>` is built **first**, over every successful
connection, before any per-server `buildToolMetadata` call. Per connection:

- `effectivePrefix = resolveToolPrefix(definition, prefix)`
- tools: `connection.tools.filter(t => t?.name).map(t => ({name: formatToolName(t.name, name,
  effectivePrefix), originalName: t.name, description: t.description ?? ""}))`
- resources, only when `definition.exposeResources !== false`:
  `connection.resources.filter(r => r?.name && r?.uri).map(r => { const originalName =
  \`read_${resourceNameToToolName(r.name)}\`; return {name: formatToolName(originalName, name,
  effectivePrefix), originalName, description: r.description ?? \`Read resource: ${r.uri}\`,
  resourceUri: r.uri} })`

The second pass then calls `buildToolMetadata(connection.tools, connection.resources, definition,
name, prefix, config.mcpServers, startupKnownMetadata, true)` per server — the pre-built map is
passed in as the **collision universe**, so name-collision resolution sees every server's names at
once rather than only the servers processed so far. Building it in one pass would make collision
outcomes depend on `parallelLimit` completion order, and a remembered tool name could then route to
a different server between runs.

Per result in the second pass:
- `owner.throwIfInactive()` at the top of **every** iteration
- on `error || !connection`: `if (initialSignal?.aborted) continue;`; `if (error) recordFailure(state,
  name, error)`; `displayError = sanitizeTerminalText(error ?? "Unknown connection failure")`;
  `ui?.notify("MCP: Failed to connect to ${name}: ${displayError}", "error")`; and **always**
  `console.error("MCP: Failed to connect to ${name}: ${displayError}")`
- on success: set `toolMetadata`, `resourceCounts` (from `connection.resources.length`); if
  `!connection.promptDiscoveryFailed` set `promptMetadata` (via `reconstructPromptMetadata`) **and**
  add to `promptMetadataLive`; set or **delete** `serverInstructions` depending on
  `connection.instructions`; `updateMetadataCache(state, name)`; `notifyToolMetadataUpdated(state,
  name, "startup")`; `markKeepAliveAfterConnect(state, name)`; if `failedTools.length > 0 && ui`:
  `ui.notify("MCP: ${name} - ${failedTools.length} tools skipped", "warning")`

`sanitizeTerminalText` (`utils.ts`) is **four** steps, and only the middle two are regexes:

1. `stripOscSequences(text)` — a **hand-written scanner**, not a regex. It recognises both the
   `ESC ]` and the C1 `0x9D` OSC introducers, then consumes to `BEL` (`0x07`), `ST` (`0x9C`) or
   `ESC \`. **An OSC payload with no terminator is consumed to the end of the string.** No regex
   reproduces that arm; a regex port silently leaves an unterminated `\x1b]…` in the output.
2. `.replace(/(?:\x1b\[[0-?]*[ -/]*[@-~]|\x1b[@-Z\\-_])/g, "")`
3. `.replace(/[\u0000-\u001f\u007f-\u009f]+/g, " ")` — every C0/C1 control becomes one space
4. `.replace(/\s+/g, " ").trim()`

It is applied to **every** server-supplied error string that reaches a terminal — an MCP server's
stderr is untrusted input. `formatTerminalError` walks `AggregateError.errors` and `.cause` chains
with a `seen` set, dedupes the collected messages, joins them with `": "`, and passes the result
through `sanitizeTerminalText`.

#### 13 · Failure tracking

Constants: `FAILURE_BACKOFF_MS = 60 * 1000`, `MAX_FAILURE_MESSAGE_CHARS = 8 * 1024`.
`failureExpiryTimers` is a **module-level `WeakMap<McpExtensionState, Map<string, Timeout>>`** so
timers are keyed per state object and die with it; `getFailureExpiryTimers(state)` lazily creates the
inner map.

`clearFailure(state, serverName)` — `failureTracker.delete`, `failureMessages?.delete`,
`clearTimeout` on any armed timer, and delete the timer entry. It is idempotent and is the first
thing `recordFailure` calls.

`recordFailure(state, serverName, message)`:
1. `clearFailure(state, serverName)` first (idempotent replace)
2. `failedAt = Date.now()`; `failureTracker.set(serverName, failedAt)`;
   `failureMessages?.set(serverName, message.slice(0, MAX_FAILURE_MESSAGE_CHARS))`
3. `timer = setTimeout(() => { if (!state.owner.isActive()) { timers.delete(serverName); return; }
   if (failureTracker.get(serverName) === failedAt) { failureTracker.delete; failureMessages?.delete;
   publishMcpStatusSnapshot(state); } timers.delete(serverName); }, FAILURE_BACKOFF_MS)` — the
   `=== failedAt` check makes the timer a no-op if a newer failure replaced this one
4. `timer.unref?.()` — **must not hold the process open**
5. store in the per-state timer map

`getFailureAgeSeconds` returns `null` when there is no record **or** when
`Date.now() - failedAt > FAILURE_BACKOFF_MS`; otherwise `Math.round(ageMs / 1000)`.
`getFailureMessage` returns `null` unless `getFailureAgeSeconds` is non-null. `lazyConnect` refuses
to retry while a failure is inside the window.

#### 14 · Startup notification and the `MCP_DIRECT_TOOLS` bootstrap pass

`connectedCount = results.filter(r => r.connection).length`,
`failedCount = results.filter(r => r.error).length`. If `ui && connectedCount > 0 &&
settings.notifyOnStartupConnect !== false`, `totalTools = totalToolCount(state)` and notify
(`"info"`) with **exactly** one of:
- `MCP: ${connectedCount}/${startupServers.length} servers connected (${totalTools} tools)` when
  `failedCount > 0`
- `MCP: ${connectedCount} servers connected (${totalTools} tools)` otherwise

The `MCP_DIRECT_TOOLS` bootstrap is skipped entirely when the env var is `"__none__"`. Note it
re-reads `process.env.MCP_DIRECT_TOOLS` here rather than reusing `index.ts`'s closure value — this is
a different module.
1. re-read the cache (`loadMetadataCache()`), re-split the env var
2. `missingCacheServers = getMissingConfiguredDirectToolServers(config, currentCache,
   envDirectToolOverride)`
3. if non-empty, `parallelLimit(missingCacheServers.filter(n => !results.some(r => r.name === n &&
   r.connection)), 10, async name => …)` — servers already connected in the startup pass are excluded
4. per item: look up the definition (throw `MCP server "${name}" is not configured` if absent),
   connect; `needs-auth` ⇒ `{name, ok:false}`; on success `updateServerMetadata`,
   `updateMetadataCache`, `notifyToolMetadataUpdated(state, name, "direct-tools-bootstrap")`,
   `markKeepAliveAfterConnect`, `clearFailure`, `{name, ok:true}`; on abort with
   `owner.signal.aborted` rethrow, else `{ok:false}`; on other error `recordFailure` +
   `logger.debug("MCP: direct-tools bootstrap failed for ${name}: ${sanitizeTerminalText(message)}")`
5. `owner.throwIfInactive()`; if any bootstrapped and `ui`:
   `ui.notify("MCP: direct tools for ${names.join(", ")} will be available after restart", "info")`

The "after restart" wording is honest for pi: this pass populates the cache but does **not**
re-register tools in the current process. If HA-1 lands, cyrup could register them in-session — in
which case the message must change with the behaviour, not be left dangling.

#### 15 · Lifecycle callbacks and health-check start

- `setReconnectCallback` — `if (!owner.isActive()) return;` then `updateServerMetadata`,
  `updateMetadataCache`, `notifyToolMetadataUpdated(state, serverName, "lifecycle-reconnect")`,
  `clearFailure`, `updateStatusBar`
- `setReconnectFailureCallback` — owner guard, `recordFailure(state, serverName, message)`,
  `updateStatusBar`
- `setIdleShutdownCallback` — owner guard,
  `logger.debug("${serverName} shut down (idle ${idleMinutes}m)")` using
  `getEffectiveIdleTimeoutMinutes`, `updateStatusBar`
- `owner.throwIfInactive()`; `lifecycle.startHealthChecks(runtimeSignal)`
- if `settings.mcpFooterStatus === "off"`: `ui?.setStatus("mcp", undefined)`
- `publishMcpStatusSnapshot(state)`; `return state`

#### 16 · `McpLifecycleManager` — the reconnect/idle state machine

Fields: `keepAliveServers`, `allServers`, `serverSettings`, `globalIdleTimeout = 10 * 60 * 1000`,
`healthCheckInterval`, three callbacks, `activeHealthCheck`, `shutdownPromise`, `stopped`,
`removeHealthAbortListener`.

`markKeepAlive` and `registerServer` both **early-return on `isServerDisabled`**; `registerServer`
records `serverSettings` only when `settings?.idleTimeout !== undefined`.

`startHealthChecks(signalOrInterval?, maybeIntervalMs = 30000)`:
- overloaded: a number in the first position is the interval, otherwise it is the `AbortSignal`
- `this.stopped = false`; if `signal?.aborted` → `stopped = true` and **return without starting**
- `stop = () => { stopped = true; clearInterval(healthCheckInterval); healthCheckInterval = undefined }`
- `signal?.addEventListener("abort", stop, {once:true})`; `removeHealthAbortListener` remembers the
  removal
- `setInterval(..., intervalMs)` whose body is
  `if (stopped || signal?.aborted || activeHealthCheck) return;` — **single-flight**: a still-running
  check suppresses the next tick entirely; then
  `check = checkConnections(signal).catch(e => console.error("MCP: Health check failed: " +
  formatTerminalError(e))).finally(() => { if (activeHealthCheck === check) activeHealthCheck = undefined })`
- `this.healthCheckInterval.unref()` — **unconditional**, no `?.`

`checkConnections(signal)` opens with its own guard `if (this.stopped || signal?.aborted) return;`,
then runs two sequential passes:
1. **reconnect** — for each `[name, definition]` of `keepAliveServers`: skip if `isServerDisabled`;
   get the connection; if missing or `status !== "connected"`: if `hasPendingAuthForServer(name)` →
   `logger.debug("Skipping reconnect for ${name} while OAuth authorization is pending")` and
   `continue`; else `await manager.connect(name, definition, signal)`, then
   `if (stopped || signal?.aborted) return;`, `logger.debug("Reconnected to ${name}")`,
   `onReconnect?.(name)`. On error: `if (stopped || signal?.aborted) return;`,
   `onReconnectFailure?.(name, error)`, `console.error("MCP: Failed to reconnect to ${name}: " +
   sanitizeTerminalText(message))`
2. **idle close** — for each `[name]` of `allServers` **not** in `keepAliveServers`:
   `timeout = getIdleTimeout(name)`; if `timeout > 0 && manager.isIdle(name, timeout)`:
   `await manager.close(name)`, `if (stopped || signal?.aborted) return;`, `onIdleShutdown?.(name)`

`getIdleTimeout(name)`: per-server `serverSettings.get(name)?.idleTimeout` × 60000 if defined, else
`globalIdleTimeout`. **`0` disables the idle close** (the `timeout > 0` guard).

`gracefulShutdown()` memoises `shutdownOnce()`. `shutdownOnce`: `stopped = true`; clear the interval;
`removeHealthAbortListener?.()` and null it; **`await this.activeHealthCheck`** (waits for an
in-flight check to settle before closing connections — otherwise `closeAll` would race a `connect`);
null `activeHealthCheck`; null the three callbacks;
`if (typeof manager.closeAll === "function") await manager.closeAll()`.

`init.ts`'s separate `getEffectiveIdleTimeoutMinutes(state, serverName)` is used only for the debug
message: no definition ⇒ `settings.idleTimeout ?? 10`; explicit `definition.idleTimeout` wins;
`eager`/`lazy-keep-alive` ⇒ `0`; else `settings.idleTimeout ?? 10`.

#### 17 · Metadata cache write rules

`updateMetadataCache(state, serverName, {preserveEmptyResources = true})`:

1. bail unless the connection exists and `status === "connected"`
2. bail unless the definition exists and is not disabled
3. `configHash = computeServerHash(definition)`; `existing = loadMetadataCache()`;
   `existingEntry = existing?.servers?.[serverName]`
4. `tools = serializeTools(connection.tools)`
5. `resources = definition.exposeResources === false ? [] : serializeResources(connection.resources)`
6. `prompts = connection.promptDiscoveryFailed ? (existingEntry?.configHash === configHash ?
   existingEntry.prompts : undefined) : serializePrompts(connection.prompts ?? [])` — **a failed
   prompt discovery preserves the cached prompt list only when the config hash still matches**
7. **empty-resource preservation**: if `exposeResources !== false && resources.length === 0 &&
   existingEntry?.resources?.length && existingEntry.configHash === configHash &&
   options.preserveEmptyResources !== false` ⇒ `resources = existingEntry.resources`. This defends
   against a server that transiently returns an empty `resources/list`. The `list_changed` listener
   passes `preserveEmptyResources: false` because *that* empty list is authoritative.
8. `entry = {configHash, tools, resources, ...(prompts !== undefined ? {prompts} : {}),
   ...(connection.instructions !== undefined ? {instructions} : {}), cachedAt: Date.now()}`
9. `saveMetadataCache({version: 1, servers: {[serverName]: entry}})` — **a single-server write**;
   `saveMetadataCache` merges.

`flushMetadataCache(state)`: for every `[name, connection]` of `manager.getAllConnections()` with
`status === "connected"`, call `updateMetadataCache(state, name)`. Called from `shutdownState` step 4.

`notifyToolMetadataUpdated` calls the hook inside a `try`, and if the return value is thenable
attaches a `.catch`; both paths log `MCP: metadata update hook failed for ${serverName}: ${message}`
at debug. A throwing hook must never break a connect.

`updateServerMetadata(state, serverName)`: bail unless the connection exists and is `connected`; bail
unless the definition exists; **if the definition is now disabled, delete the entry from all five
maps (`toolMetadata`, `resourceCounts`, `promptMetadata`, `promptMetadataLive`, `serverInstructions`)
and return**; else `prefix = settings.toolPrefix ?? "server"`,
`buildToolMetadata(..., state.toolMetadata)` — **the current map is the collision universe here**,
not the startup snapshot — set `toolMetadata` and `resourceCounts`; set `promptMetadata` + add to
`promptMetadataLive` only when `!connection.promptDiscoveryFailed`; set **or delete**
`serverInstructions`.

**On-disk contract.** `<agent_dir>/mcp-cache.json` at `CACHE_VERSION = 1` is already **read** by
`cyrup_ext_subagents::exec::mcp_direct_tools`, with `CACHE_MAX_AGE_MS` = 7 days and
`compute_mcp_server_hash` already ported from `computeServerHash`. `cyrup-mcp` is the **writer** of a
file that already has a reader; the digests must be identical or every `mcp:` subagent tool selector
silently resolves to nothing. Do **not** bump `CACHE_VERSION` to drop the now-dead `uiResourceUri` /
`uiStreamMode` fields — leave them absent and ignored.

#### 18 · The footer status bar

`updateStatusBar(state)`:

1. `publishMcpStatusSnapshot(state)` — **always**, even without a UI
2. `if (!state.ui) return`
3. `entries = Object.entries(state.config.mcpServers)`;
   `disabledCount = entries.filter(([, d]) => isServerDisabled(d)).length`;
   `enabledCount = entries.length - disabledCount`
4. `if (entries.length === 0) { ui.setStatus("mcp", undefined); return }`
5. `connectedCount` = connections whose `status === "connected"` **and** whose definition exists
   **and** is not disabled
6. `footerStatus = settings.mcpFooterStatus ?? "full"`; if `"off"` ⇒ `ui.setStatus("mcp", undefined)`
   and return
7. `status = footerStatus === "compact" ? \`MCP ${connectedCount}/${enabledCount}\` :
   \`${enabledCount} ${enabledCount === 1 ? "server" : "servers"} enabled\``
8. if `footerStatus === "full"`: append `` ` (${connectedCount} connected)` `` when
   `connectedCount > 0`, then `` ` (${disabledCount} disabled)` `` when `disabledCount > 0`
9. `formattedStatus = footerStatus === "compact" ? status : formatMcpStatus(state.config, status)` —
   so the `🔌 MCP: ` prefix is applied in `full` mode only
10. `if (formattedStatus === undefined) { ui.setStatus("mcp", undefined); return }`
11. `ui.setStatus("mcp", ui.theme ? ui.theme.fg("accent", formattedStatus) : formattedStatus)`

Step 11's colouring is the one part that does not cross. `HostServices::set_status(key,
Option<&str>)` is an exact match for `setStatus(key, undefined)`, but there is no `fg(role, text)`
anywhere on the trait — `theme()` returns a theme *name*, and `LiveHostServices` does not override it.
The status text goes out uncoloured; the branch collapses to the `else` arm, which is upstream's own
no-theme branch. Recorded as an accepted delta.

#### 19 · `lazyConnect`

1. `ownedSignal = combineAbortSignals(state.owner?.signal, signal)`; `throwIfAborted(ownedSignal)`
2. connection `status === "needs-auth"` ⇒ **`false`**
3. connection `status === "connected"` ⇒ `updateServerMetadata`, `markKeepAliveAfterConnect`,
   **`true`**
4. `getFailureAgeSeconds(state, serverName) !== null` ⇒ **`false`** (inside the 60 s backoff)
5. definition missing or disabled ⇒ **`false`**
6. if `state.ui`: `setStatus("mcp", formatMcpStatus(config, "connecting to ${serverName}..."))`
7. `connect`; `needs-auth` ⇒ `false`; else `clearFailure`, `updateServerMetadata`,
   `updateMetadataCache`, `notifyToolMetadataUpdated(state, serverName, "lazy-connect")`,
   `markKeepAliveAfterConnect`, `updateStatusBar`, **`true`**
8. on error: `if (isAbortError(error, ownedSignal)) throwIfAborted(ownedSignal)` — note this
   **rethrows only when the signal is actually aborted**; a stray `AbortError` with a live signal
   falls through to the failure path — then `recordFailure`,
   `logger.debug("MCP: lazy connect failed for ${serverName}: ${sanitizeTerminalText(message)}")`,
   `updateStatusBar`, **`false`**

#### 20 · Tool-surface synchronisation

`directToolFingerprint(spec)` — `JSON.stringify` over exactly these eight keys in this order:
`serverName, originalName, prefixedName, description, inputSchema, resourceUri, uiResourceUri,
uiStreamMode`. Property order matters because it is a raw `JSON.stringify` of an object literal.
With Cut 2 the last two are always absent from the spec; keep them in the key list only if the
fingerprint is ever compared against a value produced by another writer — it is not, so the Rust
fingerprint covers the six live keys and the *stability* requirement (deterministic field order)
is what matters, not byte-parity with pi.

`registerDirectTool(spec)` registers:
`{name: spec.prefixedName, label: \`MCP: ${spec.originalName}\`, description: spec.description ||
"(no description)", promptSnippet: truncateAtWord(spec.description, 100) || \`MCP tool from
${spec.serverName}\`, parameters: toToolParameters(normalizeDirectToolInputSchema(spec.inputSchema)),
execute: createDirectToolExecutor(() => state, () => initPromise, spec), renderShell: toolRenderShell,
renderCall: createMcpDirectToolCallRenderer(spec.prefixedName, toolRenderOptions), renderResult:
renderMcpToolResult}`. The executor closes over **getters**, not values, so it always sees the
current `state`/`initPromise`.

`truncateAtWord(text, 100)`: if `!text || text.length <= target` return as-is; slice to `target`;
find `lastIndexOf(" ")`; if that index `> target * 0.6` return `slice(0, lastSpace) + "..."` else
`truncated + "..."`.

`normalizeDirectToolInputSchema(schema)`: non-object/array/nullish ⇒ `{type:"object", properties:{}}`;
then **destructure away `$schema` and `additionalProperties`** and return the rest.

`toToolParameters(schema)` prefers `Type.Unsafe(schema)` when the host TypeBox shim has it, else
passes the plain object through; `optionalNumber({minimum, description})` prefers
`Type.Optional(Type.Number(opts))` and falls back to a raw `{type:"number", ...opts}` — the source
comment explains that TypeBox 1.x's `~optional` key survives serialization and Gemini rejects it with
`400 INVALID_ARGUMENT`. **Both shims evaporate in Rust**, where `cyrup_core::Tool::parameters`
already returns raw JSON Schema.

`resolveCurrentDirectTools(config, cache)`: `envRaw === "__none__"` ⇒ `[]`; else
`resolveDirectTools(config, cache, config.settings?.toolPrefix ?? "server", envDirectToolOverride)`.

`syncDirectTools(config, cache)`:
1. `specs = resolveCurrentDirectTools(...)`; `nextNames = Set(specs.map(s => s.prefixedName))`
2. for each spec: compute the fingerprint; if it differs from `registeredDirectTools.get(name)`, call
   `registerDirectTool(spec)` and store the new fingerprint. If the name was in
   `fallbackDeactivatedTools`, `delete` it and — when `getActiveToolsIfReady()` returns a list not
   containing the name — `pi.setActiveTools([...activeTools, name])` to **re-activate** it. Push onto
   `updated` if it had a previous fingerprint, else onto `added`.
3. for each key of `registeredDirectTools` not in `nextNames`: delete it and push onto `deactivated`
4. `deactivateTools(deactivated)`; return `{specs, added, updated, deactivated}`

`deactivateTools(toolNames)` — **the removal mechanism**:
1. empty ⇒ `[]`
2. `unregisterTool = (pi as ExtensionAPI & {unregisterTool?: (name:string)=>boolean}).unregisterTool`
   — read through a cast because the ambient `ExtensionAPI` type does not declare it. It is an
   **optional** host API, probed at runtime: the package's own `README.md` and `CHANGELOG.md` describe
   it as a capability newer pi hosts expose (*"On Pi versions that expose `pi.unregisterTool()`,
   stale direct tools are removed from the registry during refresh; older Pi versions still deactivate
   them from the active tool set"*), and `__tests__/index-lifecycle.test.ts` exercises **both**
   branches.
3. `unregistered = toolNames.filter(n => unregisterTool?.(n) === true)` — the primary path
4. `fallbackNames = toolNames.filter(n => !unregistered.includes(n))`
5. `remove = new Set(toolNames)`; `activeTools = getActiveToolsIfReady()`; if absent or empty, add
   every `fallbackName` to `fallbackDeactivatedTools` and return `unregistered`
6. `nextActiveTools = activeTools.filter(n => !remove.has(n))`; if the length changed, add every
   `fallbackName` to `fallbackDeactivatedTools` and `pi.setActiveTools(nextActiveTools)`
7. return `unregistered`

cyrup has step 5-6 exactly (`HostServices::{active_tools, set_active_tools}`, routed by
`LiveHostServices` into the session's dynamic tool state) and has no `unregister_tool` on
`ExtensionRegistry`, so it lands on upstream's `unregisterTool === undefined` branch — **a supported
upstream configuration, not a gap.**

`syncProxyTool(config, cache, directSpecs)`:
- `missingConfiguredDirectToolServers = getMissingConfiguredDirectToolServers(config, cache,
  envRaw === undefined || envRaw === "__none__" ? undefined : envDirectToolOverride)`
- `shouldRegisterProxyTool = config.settings?.disableProxyTool !== true || directSpecs.length === 0 ||
  missingConfiguredDirectToolServers.length > 0` — the `mcp` gateway survives `disableProxyTool` when
  there are no direct tools at all, or when some configured direct-tool server is missing from the
  cache
- if it should register: `description = buildProxyDescription(config, cache, directSpecs)`; if
  `!proxyToolRegistered || proxyToolDescription !== description` → `registerProxyTool(description)`
  and return. Otherwise, if `getActiveToolsIfReady()` returns a list not containing `"mcp"`,
  `pi.setActiveTools([...activeTools, "mcp"])`
- else if `proxyToolRegistered`: `deactivateTools(["mcp"])`, and **only if** `"mcp"` was genuinely
  unregistered clear `proxyToolRegistered`/`proxyToolDescription`

`syncToolSurface(ctx?)`: `config = state?.config ?? earlyConfig`; **re-reads the cache from disk**;
`syncDirectTools`; `syncProxyTool`; and if `added+updated+deactivated > 0` and `ctx?.hasUI`,
`ctx.ui.notify("MCP: direct tools refreshed (+${a}, ~${u}, -${d})", "info")`.

`applyDirectToolConfigChanges(changes: Map<string, true | string[] | false>)` mutates
`state.config.mcpServers[serverName] = {...definition, directTools: value}` in place, skipping unknown
servers; it is the callback `openMcpPanel` invokes.

#### 21 · Prompt slash commands

`registeredPromptCommands: Set<string>`. `registerPromptCommands(specs)`: for each spec, if
`registeredPromptCommands.has(spec.commandName)` →
`logger.debug("MCP: prompt \"${spec.originalName}\" on ${spec.serverName} skipped;
/${spec.commandName} is already registered")` and skip; else add and
`pi.registerCommand(spec.commandName, createPromptCommand(pi, () => state, spec))`.
`syncPromptCommands()` flattens `state?.promptMetadata?.values()`. Called from the cache at load, and
from the metadata-update hook plus once at commit.

Commands are registered at arbitrary times after `init` and are never unregistered. The
register-from-cache half maps onto `InitApi::register_command`; the after-`init` half is HA-1's
sibling seam.

#### 22 · `/mcp` and `/mcp-auth`

`getArgumentCompletions(prefix)`:
1. `normalized = prefix.trimStart()`; `argumentMatch = normalized.match(/^(\S+)\s+(.*)$/)`
2. no match ⇒ filter the eight fixed subcommand rows by `value.startsWith(normalized)`; return them
   or `null`: `reconnect — Reconnect servers`, `tools — List all tools`,
   `prompts — List all MCP prompts`, `setup — Configure MCP servers`,
   `logout — Clear server credentials`, `disable — Disable a server`, `enable — Enable a server`,
   `status — Show server status`
3. match ⇒ if the subcommand is not one of `reconnect|logout|disable|enable`, or `argumentPrefix` is
   undefined, or `!state`, return `null`; else map `Object.keys(state.config.mcpServers)` filtered by
   `startsWith(argumentPrefix.trimStart())` into
   `{value: \`${subcommand} ${serverName}\`, label: serverName}`, returning `null` when that list is
   empty

The handler first builds a **fenced command context**:
```
commandOwner  = currentOwner
commandReload = typeof ctx.reload === "function" ? ctx.reload.bind(ctx) : async () => {}
commandHasUI  = ctx.hasUI
commandCtx    = { hasUI: commandHasUI,
                  ui: commandHasUI ? (commandOwner ? createOwnedUi(ctx.ui, commandOwner) : ctx.ui) : undefined,
                  cwd: ctx.cwd, mode: ctx.mode, signal: commandOwner?.signal ?? ctx.signal }
```
Then: if `!state && initPromise`, `await initPromise` (**no timeout here** — unlike the tool bodies),
`commandOwner?.throwIfInactive()`, `state = initialized`; on error notify
`MCP initialization failed: ${message}` (`"error"`) and return. If still `!state`, notify
`"MCP not initialized"` (`"error"`) and return.

Args: `parts = args?.trim()?.split(/\s+/) ?? []`; `subcommand = parts[0] ?? ""`;
`targetServer = parts[1]`; `rest = parts.slice(1).join(" ")`.

| subcommand | behaviour |
|---|---|
| `reconnect` | `throwIfInactive`; `await reconnectServers(state, commandCtx, targetServer)`; if `directToolsFrozen` also `syncToolSurface(commandCtx)` |
| `tools` | `await showTools(state, commandCtx)` |
| `prompts` | `await showPrompts(state, commandCtx)` |
| `setup` | `throwIfInactive`; if `programmaticConfig` notify `"MCP setup is unavailable when config is supplied by createMcpAdapter()."` (`"info"`) and break; `result = await openMcpSetup(state, pi, commandCtx, earlyConfigPath, "setup")`; if `result?.configChanged` → `throwIfInactive`, `await commandReload()`, **return** |
| `logout` | `serverName = rest`; empty ⇒ notify `"Usage: /mcp logout <server>"` (`"error"`, only when `hasUI`) and **return**; else `throwIfInactive`, `await logoutServer(serverName, state, commandCtx)` |
| `disable` / `enable` | `serverName = rest`; `programmaticConfig` ⇒ notify `` `/mcp ${subcommand} is unavailable when config is supplied by createMcpAdapter().` `` (`"info"`) and break; empty name ⇒ `` `Usage: /mcp ${subcommand} <server>` `` (`"error"`) and break; unknown server ⇒ `` `Server "${serverName}" not found in effective config` `` (`"error"`) and break; else `throwIfInactive` and `writeProjectServerDisabledOverride(earlyConfigPath, commandCtx.cwd, serverName, subcommand === "disable")`, then notify either `` `${Disabled\|Enabled} server "${serverName}" in ${result.path} — run /reload to apply` `` or `` `Server "${serverName}" is already ${disabled\|enabled}` `` (both `"info"`) |
| `status` / `""` / default | with UI: `throwIfInactive`; if `programmaticConfig` notify `"MCP status is shown from the in-memory SDK config; configuration discovery is unavailable."` (`"info"`) then `await showStatus(...)` and break; else `openMcpPanel(state, pi, commandCtx, earlyConfigPath, changes => { applyDirectToolConfigChanges(changes); syncToolSurface(commandCtx) })` and on `configChanged` → `throwIfInactive`, `await commandReload()`, **return**. Without UI: `await showStatus(state, commandCtx)` |

`/mcp-auth`: same fenced ctx; `serverName = args?.trim()`; **if `!serverName && !commandCtx.hasUI`
return silently** — *before* the init-await, so a headless `/mcp-auth` never blocks on
initialization; same init-await/`not initialized` preamble; no server name with UI ⇒ if
`programmaticConfig` notify
`"Use /mcp-auth <server> to authenticate a server from the in-memory SDK config."` (`"info"`) else
`await openMcpAuthPanel(state, pi, commandCtx, earlyConfigPath)`; with a name ⇒
`result = await authenticateServer(serverName, state.config, commandCtx, commandCtx.signal,
state.oauthRuntime)` and if `result.ok` → `throwIfInactive`,
`await reconnectServer(state, commandCtx, serverName)`.

Both commands land on `InitApi::register_command` + `NativeExtension::execute_command` at **command
tier**, where session mutation is legal — so `ControlOp::Reload` from `/mcp setup` and `/mcp status`
is permitted. `ExtensionRegistry::resolved_commands` assigns pi's `name:N` invocation names in load
order, so a name collision with another extension is handled by the host.

#### 23 · The `mcp` gateway tool

`INIT_WAIT_TIMEOUT_MS = 30_000`; `INIT_WAIT_TIMED_OUT` is a `unique symbol`. `awaitWithTimeout` races
the promise against a `setTimeout` whose handle is `.unref?.()`'d and always `clearTimeout`'d in a
`finally`.

The tool body's preamble:
1. `executeOwner = currentOwner`
2. if `!state && initPromise`: `initialized = await awaitWithTimeout(initPromise, INIT_WAIT_TIMEOUT_MS)`;
   on `INIT_WAIT_TIMED_OUT` return `{content:[{type:"text", text:"MCP initialization is still in
   progress. Try again shortly."}], details:{…, error:"init_timeout", timeoutMs: 30000}}`; else
   `executeOwner?.throwIfInactive()` and `state = initialized`. On a caught error: **if
   `executeOwner && isAbortError(error, executeOwner.signal)` rethrow**, else return
   `{content:[{type:"text", text:\`MCP initialization failed: ${message}\`}], details:{…,
   error:"init_failed", message}}`
3. if still `!state` return `{content:[{type:"text", text:"MCP not initialized"}], details:{…,
   error:"not_initialized"}}`
4. `executeOwner?.throwIfInactive()`

The tool additionally parses `params.args` **before** the init wait, and only when
`params.args !== undefined && params.args !== ""`: a string is `JSON.parse`d and a `SyntaxError` is
rethrown as `` new Error(`Invalid args JSON: ${error.message}`, {cause: error}) ``; a
non-object/null/array result throws `` `Invalid args: expected a JSON object, got ${gotType}` ``
where `gotType` is `"array"`/`"null"`/`typeof`.

Dispatch order — **first match wins**, with the Apps arm removed:

| # | condition | action / failure shape |
|---|---|---|
| 1 | ~~`action === "ui-messages"`~~ | **CUT 2** |
| 2 | `action === "auth-start"` | requires `server`, else `{text:"auth-start requires \`server\`. Example: mcp({ action: \"auth-start\", server: \"linear-server\" })", details:{mode:"auth-start", error:"missing_server"}}` |
| 3 | `action === "auth-complete"` | requires `server`, else `{text:"auth-complete requires \`server\`.", details:{mode:"auth-complete", error:"missing_server"}}`; then `input = parsedArgs?.redirectUrl ?? parsedArgs?.code ?? parsedArgs?.input` which must be a non-empty trimmed string, else `{text:"auth-complete requires args with \`redirectUrl\`, \`code\`, or \`input\`.", details:{mode:"auth-complete", error:"missing_input"}}` |
| 4 | `params.tool` | `executeCall(state, tool, parsedArgs, server, getPiTools, signal)` |
| 5 | `params.connect` | `executeConnect(...)` **then `syncToolSurface(_ctx)`** then return |
| 6 | `params.describe` | `executeDescribe` |
| 7 | `params.instructions` | `executeInstructions` |
| 8 | `params.search !== undefined` | `executeSearch(state, search, regex, server, includeSchemas, limit, offset)` |
| 9 | `params.server` | `executeList(state, params.server)` |
| 10 | — | `executeStatus(state)` |

The parameter object has **twelve** properties, all optional: `tool`, `args` (a
`Union[String, Object({}, {additionalProperties:true})]`), `connect`, `describe`, `instructions`,
`search`, `regex`, `includeSchemas`, `limit` (minimum 1), `offset` (minimum 0), `server`, `action`.
It also carries `label: "MCP"`,
`promptSnippet: "MCP gateway — status, search, describe, auth, and single MCP tool calls"`,
`renderShell: toolRenderShell`, `renderCall`, `renderResult`. `registerProxyTool` sets
`proxyToolRegistered = true` and `proxyToolDescription = description` after the `pi.registerTool`
call.

**Cut-2 seam inside the schema:** `action`'s description is literally
`"Action: 'ui-messages', 'auth-start', or 'auth-complete'"`. It becomes
`"Action: 'auth-start' or 'auth-complete'"`. Leaving the old string in would advertise a mode that no
longer dispatches, and the model would call it.

**Cross-crate contract:** the tool must be named literally `mcp`, and its argument names are fixed.
`cyrup_permission_system::manager`'s `create_mcp_permission_targets` reads exactly
`{tool, server, connect, describe, search}` off the call arguments, in that precedence, and falls
through to the `mcp_status` baseline otherwise; `MCP_BASELINE_TARGETS` is
`["mcp_status", "mcp_list", "mcp_search", "mcp_describe", "mcp_connect"]`. Renaming a parameter
silently changes which permission rules apply. The remaining parameters (`args`, `regex`,
`includeSchemas`, `limit`, `offset`, `instructions`, `action`) are not read by the target derivation
and are safe.

#### 24 · `tool_result` — the isError override

`pi.on("tool_result", (event) => toolErrorOverride(event.details))`. `toolErrorOverride(details)`
returns `{isError: true}` **only** when `details` is a non-null object with an `"error"` key whose
value is exactly `"tool_error"` or `"call_failed"`; otherwise `undefined`. The file's own doc
explains: a failed MCP tool call is *returned*, not thrown, so without this pi records it as a
success. Returning `{isError:true}` **and nothing else** lets pi's field-by-field merge keep
`content` and `details`. Deliberately excluded: `auth_required`, connection states, and
search/validation feedback are not failed calls.

#### 25 · `agent-plugin-loader.ts` — the sandbox

Constants: `PLUGIN_SCHEMA = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json"`,
`MCP_SCHEMA = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json"`,
`PLUGIN_NAME_PATTERN = /^(?!.*(?:--|\.\.))[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$/`,
`PLUGIN_MANIFEST_FIELDS = {$schema, name, version, description, author, homepage, repository, license,
keywords, extensions}`, `MCP_CONFIG_FIELDS = {$schema, mcpServers}`,
`STDIO_FIELDS = {type, command, args, env, cwd}`, `HTTP_FIELDS = {type, url, headers}`.

Entry points: `loadAgentPluginConfigs(paths, cwd = process.cwd())` and
`getAgentPluginSummaries(paths, cwd)`. `getPluginPaths` accepts only an array and keeps only `string`
members — a non-array `settings.agentPluginPaths` yields `[]` silently.

`resolvePluginPath(path, cwd)`: `"~"` → `resolve(process.env.HOME ?? "", ".")`; `"~/…"` →
`resolve(process.env.HOME ?? "", path.slice(2))`; absolute → `resolve(path)`; else `resolve(cwd, path)`.
**Note the home source: `process.env.HOME`, not `os.homedir()`** — this file resolves `~` differently
from `agent-dir.ts`, which uses `homedir()`. A port that unifies them changes behaviour on a machine
where `HOME` is unset or differs from the OS home.

`readPluginManifest(pluginRoot, report)`: `plugin.json` must exist, be a **regular file**, parse, and
be a non-array object. Unknown keys are **warned only**. `$schema` must equal `PLUGIN_SCHEMA` exactly.
`name` must be a string of length 1..64 matching `PLUGIN_NAME_PATTERN` — the negative lookahead bans
any `--` or `..` anywhere. A non-object `extensions` warns but does not fail. **Only `{name}` is
returned.**

`loadAgentPluginMcpConfig`: a missing `mcp.json` yields `{mcpServers:{}}`; a non-regular file or a
parse failure warns and yields `{mcpServers:{}}`.

`translateAgentPluginMcpConfig`: a non-object raw ⇒ warn + empty; **any unknown top-level key
discards the whole file** (a `return {mcpServers:{}}`, not a `continue`); `$schema` must match
`MCP_SCHEMA`; `mcpServers` must be a non-array object.

Dispatch: a non-object entry warns `entry must be an object`; `type === "stdio"` → stdio;
`type ∈ {"streamable-http","sse"}` → http; anything else warns
`` `Agent Plugin ${name} skips invalid MCP server ${serverName}: unsupported type` ``. Every skip goes
through `skipServer(manifest, serverName, reason)`, which formats
`` `Agent Plugin ${name} skips invalid MCP server ${serverName}: ${reason}` ``.

**Cut-1 seam:** the accepted `type` set becomes `{"streamable-http"}` alone. A plugin declaring
`type: "sse"` must be skipped with a **named** reason (`unsupported type` is already the right
string), not silently accepted and then connected over the wrong shape.

**stdio**:
- any key outside `STDIO_FIELDS` ⇒ skip with `` `unknown field ${key}` ``
- `command` must be a non-empty string, and must be **bare** or start with `"./"`. `isBareCommand` =
  contains no `/`, no `\`, no `${PLUGIN_ROOT}`, no `${PLUGIN_DATA}`
- `args` via `translateStringArray`: `undefined` ⇒ `[]`; must be an array of strings
- `env` via `translateEnv`: `undefined` ⇒ `{}`; must be a non-array object; **a key of `PLUGIN_ROOT`
  or `PLUGIN_DATA` rejects the whole server**; every value must be a string
- a `"./…"` command is resolved with `resolveContainedPath(pluginRoot, command, pluginRoot)`;
  `null` ⇒ skip with `"command must stay inside the plugin directory"`
- `pluginDataDir = getAgentPath("agent-plugin-data", manifest.name)`
- `cwd` via `resolvePluginCwd`: `undefined` ⇒ `pluginRoot`; non-string ⇒ `null`; `"./…"` ⇒ contained
  under `pluginRoot`; `"${PLUGIN_ROOT}"`/`"${PLUGIN_ROOT}/…"` ⇒ replace with `"."` and contain under
  `pluginRoot`; `"${PLUGIN_DATA}"`/`"${PLUGIN_DATA}/…"` ⇒ replace with `"."` and contain under
  `pluginDataDir`; anything else ⇒ `null` → skip with `"cwd must be plugin-relative, PLUGIN_ROOT-rooted,
  or PLUGIN_DATA-rooted"`
- result: `{command, args: args.map(expand), env: {...expanded, PLUGIN_ROOT: pluginRoot, PLUGIN_DATA:
  pluginDataDir}, cwd, pluginDataDir, literalEnv: true}`. `literalEnv: true` suppresses the normal
  `$VAR` interpolation — plugin env values are already-resolved literals.

`resolveContainedPath(root, value, containmentRoot)`: `resolved = resolve(root, value)`;
`rel = relative(containmentRoot, resolved)`; accept when `rel === ""` **or** (`!rel.startsWith("..")`
and `!rel.startsWith(sep)` and `!isAbsolute(rel)`); else `null`.

`expandPluginPlaceholders(value, pluginRoot, pluginDataDir)`: `replaceAll("${PLUGIN_ROOT}", …)` then
`replaceAll("${PLUGIN_DATA}", …)`.

**http**:
- any key outside `HTTP_FIELDS` ⇒ skip
- `url` must be a non-empty string and pass `isValidAgentPluginUrl`: **rejects any occurrence of
  `${`, `$env:` or `{env:`** (no interpolation at all in plugin URLs); must parse as a `URL`; protocol
  must be `http:` or `https:`; **no username, password or hash**; `https:` always allowed; `http:` only
  when `isLoopbackHost(hostname)` — lowercased `localhost`, `127.0.0.1`, `::1`, `[::1]`, or
  `/^127(?:\.\d{1,3}){3}$/`
- `headers` via `translateHeaders`: `undefined` ⇒ `undefined`; non-array object required; every value
  a string; **case-insensitive duplicate keys reject the whole server**; the map is validated by
  constructing a real `new Headers(headers)`; an empty map ⇒ `undefined`
- result: `{url: raw.url, httpTransport: type, ...(headers ? {headers} : {})}` — `httpTransport` is
  set explicitly so the client cannot fall back to the other transport

`formatAgentPluginServerName(pluginName, serverName)`: each half is
`.replace(/[^A-Za-z0-9_-]+/g, "_").replace(/^[_-]+|[_-]+$/g, "")`, defaulting to `"plugin"`/`"server"`
when empty; joined with a **double underscore**: `` `${pluginPart}__${serverPart}` ``.

Duplicate handling: intra-plugin warns
`` `… normalized server name ${normalizedName} already exists` `` and skips; cross-plugin warns
`` `Agent Plugin at ${root} skips duplicate normalized MCP server ${serverName}` `` and skips.
**First writer wins in both cases.**

**Failure policy throughout: `console.warn` and skip. Nothing here ever throws.**

#### 26 · `agent-dir.ts`

`getAgentDir()`: `PI_CODING_AGENT_DIR?.trim()`; falsy ⇒ `join(homedir(), ".pi", "agent")`; exactly
`"~"` ⇒ `homedir()`; `"~/…"` ⇒ `resolve(homedir(), configured.slice(2))`; else `resolve(configured)`.
`getAgentPath(...segments)` = `join(getAgentDir(), ...segments)`.

`readPiConfig()` reads `$PI_PACKAGE_DIR/package.json` (after `resolve(dir)`) and returns its
`piConfig` object; a missing/blank `PI_PACKAGE_DIR` returns `undefined` immediately, and every
read/parse failure yields `undefined`. `getAppName()` → trimmed `piConfig.name` else `"pi"`.
`getAppClientUri()` → trimmed `piConfig.clientUri` else `undefined`. The file's own comment states why
this is read directly rather than imported from pi: *"this package deliberately depends on pi-ai and
pi-tui only"*.

Consumers in this section: `onboarding-state.ts` (`getAgentPath("mcp-onboarding.json")`) and
`agent-plugin-loader.ts` (`getAgentPath("agent-plugin-data", manifest.name)`).

cyrup's analog is `ConfigDirs::agent_dir` (`CYRUP_AGENT_DIR` → `PI_CODING_AGENT_DIR` →
`<home>/.cyrup/agent`), and the extension takes it as a constructor argument the way
`cyrup_ext_subagents::subagent_extension_for_env` does. Two in-tree consumers already bind to it:
`cyrup_ext_subagents::exec::mcp_direct_tools` reads `<agent_dir>/mcp-cache.json`, and
`cyrup_permission_system::manager` reads `<agent_dir>/mcp.json` through
`read_configured_mcp_server_names` over `cyrup_permission_system::jsonc`, accepting either
`mcpServers` or `mcp-servers` and sorting names length-desc-then-lexicographic. Both the cache **and**
the config must resolve identically in `cyrup-mcp`.

#### 27 · `onboarding-state.ts`

Schema: `{version: 1, sharedConfigHintShown: boolean, setupCompleted: boolean,
lastDiscoveryFingerprint?: string}`. `DEFAULT_STATE` is
`{version:1, sharedConfigHintShown:false, setupCompleted:false}`. Path:
`<agent_dir>/mcp-onboarding.json`.

`loadOnboardingState()`: missing file ⇒ a **copy** of the default; parse failure or non-object ⇒ a
copy of the default; otherwise a **normalising** read — `version` is forced to `1`, both booleans are
`=== true` coercions, and `lastDiscoveryFingerprint` is kept only when it is a `string`. Unknown keys
are dropped.

`saveOnboardingState(state)`: `mkdirSync(dirname(path), {recursive:true})`; write
`` `${JSON.stringify(state, null, 2)}\n` `` to `` `${path}.${process.pid}.tmp` ``; `renameSync` onto
the real path. **Atomic, pid-scoped temp name** — two concurrent processes cannot collide on the temp
file.

`updateOnboardingState(updater)` is load → update → save → return.
`markSharedConfigHintShown(fingerprint?)` and `markSetupCompleted(fingerprint?)` each set their flag
and carry `fingerprint ?? state.lastDiscoveryFingerprint` forward, omitting the key entirely when both
are undefined.

Consumers (outside this section's file set, listed so the port wires them): `commands.ts` and
`mcp-setup-panel.ts`.

#### 28 · `cli.js` — what the standalone binary is for

`cli.js` is the `pi-mcp-adapter` npm `bin`. **It is not the extension**: it never loads pi and never
connects to an MCP server. Its whole purpose is one-time, out-of-band scaffolding of *compatibility
imports* — telling the adapter to also read the MCP configs written by other agent tools.

Paths computed at module load: `AGENT_DIR` from `PI_CODING_AGENT_DIR` (with `expandHome`, anchored on
`os.homedir()`) else `~/.pi/agent`; `PI_CONFIG_PATH = <AGENT_DIR>/mcp.json`;
`GENERIC_GLOBAL_CONFIG_PATH = ~/.config/mcp/mcp.json`; `AGENTS_GLOBAL_CONFIG_PATH = ~/.agents/mcp.json`;
`AGENTS_NESTED_GLOBAL_CONFIG_PATH = ~/.agents/mcp/mcp.json`; `PROJECT_CONFIG_PATH = <cwd>/.mcp.json`;
`PROJECT_PI_CONFIG_PATH = <cwd>/.pi/mcp.json`.

`IMPORT_PATHS` — **seven** families, first existing candidate wins per family:

| kind | candidates in order |
|---|---|
| `cursor` | `~/.cursor/mcp.json` |
| `claude-code` | `~/.claude/mcp.json`, `~/.claude.json`, `~/.claude/claude_desktop_config.json` |
| `claude-desktop` | `~/Library/Application Support/Claude/claude_desktop_config.json` |
| `codex` | `~/.codex/config.toml`, `~/.codex/config.json` |
| `opencode` | `~/.config/opencode/opencode.json`, `<cwd>/opencode.json` |
| `windsurf` | `~/.windsurf/mcp.json` |
| `vscode` | `<cwd>/.vscode/mcp.json` |

Commands (`main`): no command / `help` / `--help` / `-h` ⇒ `printHelp` and return 0. `install` ⇒
prints two errors (`"The custom downloader has been retired."` and *"Use `pi install
npm:pi-mcp-adapter` instead, then optionally run `pi-mcp-adapter init`."*) and returns **1**. `init` ⇒
`runInit(rest)`. Anything else ⇒ `` `Unknown command: ${command}` ``, `printHelp`, return 1. The
returned code is assigned to `process.exitCode`, not passed to `process.exit`. The module
self-executes only when `import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href`, and a
thrown error prints `` `\nHelper failed: ${message}` `` and `process.exit(1)`.

`runInit(argv, log = console.log)`:
1. `dryRun = argv.includes("--dry-run")`; `discoverHostConfigs = argv.includes("--discover-host-configs")`
2. `foundImports = findAvailableImports()`
3. `existingConfig = loadPiConfig()` — a missing file yields `{mcpServers:{}}`; otherwise reads
   `PI_CONFIG_PATH` through `JSON.parse(stripJsonComments(raw, {trailingCommas: true}))`; accepts
   `mcpServers` **or** `mcp-servers`; throws `` `Invalid MCP config at ${PI_CONFIG_PATH}: expected
   "mcpServers" to be an object` `` when it is not a non-array object; **deletes the legacy
   `mcp-servers` key** from the normalized copy; keeps `imports` only if it is an array, filtered to
   strings
4. `importsToAdd` = detected kinds not already in `existingConfig.imports`
5. `printDiscovery(log, foundImports)` — prints `Config discovery:` then one line per path as
   `` `${exists ? "✓" : "-"} ${label}: ${filePath}` `` over the six labels *User-global standard MCP*,
   *User-global .agents MCP*, *User-global .agents nested MCP*, *Pi global override*, *Project standard
   MCP*, *Project Pi override*; then `Compatibility imports:` and either
   `- No host-specific MCP configs detected` or one `` `✓ ${kind}: ${path}` `` per detected family
6. `discoverySettingChanged = discoverHostConfigs && existingConfig.settings?.hostConfigDiscovery !== "on"`
7. if nothing to add and no setting change: print `"\nNo Pi config changes needed."` and *"Standard MCP
   configs are discovered automatically, and host-specific imports are already configured or
   unavailable."*, return 0
8. `nextConfig = {...existingConfig, ...(discoverySettingChanged ? {settings: {...existingConfig.settings,
   hostConfigDiscovery: "on"}} : {}), ...(importsToAdd.length > 0 ? {imports: [...existingImports,
   ...importsToAdd]} : {}), mcpServers: existingConfig.mcpServers ?? {}}`
9. log `` `\nDetected host configs to import into Pi: ${importsToAdd.join(", ")}` `` when any, and/or
   *"Opting in to host-specific fallback discovery (standard and Pi-owned configs still take
   precedence)."* when the setting changed
10. `dryRun` ⇒ log `` `Dry run: would update ${PI_CONFIG_PATH}` `` and return 0
11. `writePiConfig(nextConfig)`: `mkdirSync(dirname, {recursive:true})` then
    `writeFileSync(PI_CONFIG_PATH, JSON.stringify(config, null, 2) + "\n", "utf-8")` — **a plain
    overwrite; all comments in the existing JSONC are destroyed**
12. log `` `Updated ${PI_CONFIG_PATH}` ``, then **one unconditional** explanatory line (*"Pi will now
    keep reading standard MCP configs automatically, while these imports cover host-specific config
    formats."*) and **one conditional** line gated on `discoverySettingChanged` (*"Host config
    discovery is explicit and does not write to or execute commands from external host files."*)

**Does cyrup need an equivalent? Yes, as a subcommand, not a second binary.** cyrup is one executable;
`crates/cyrup/src/subcommands.rs` already pre-dispatches a visible verb table (`install`, `remove`,
`uninstall`, `update`, `list`, `config`) from `main.rs` before clap parses, gated by
`first_subcommand`. An `mcp` verb slots into that table. The hidden pre-dispatch precedents
(`intercom_broker_cmd`, `subagent_runner_cmd`) are the wrong model here because this one is
user-facing. JSONC reading reuses `cyrup_permission_system::jsonc`, which the `cyrup` bin already
depends on.

---

### Port units

Verdicts: **`rmcp`** · **`host-verb`** (a named existing cyrup API) · **`extension-owned`** (the
native crate does it itself; no host involvement, no core change) · **`hand-written`** (new code in
`cyrup-mcp`) · **`host-addition`** (needs a new host surface) · **`open-decision`** · **`cut`**.
Severity is the house scale — `critical` means data loss, silent wrong output, a permission bypass,
or a crash on a normal path. Blocking-ness is stated in the body, not in the severity.

**MCP-001 — Stand up `crates/cyrup-mcp` and attach it at the session-build arms** · n/a · M · `hand-written`
**upstream** — `index.ts`'s `createMcpAdapter(options)` clones the programmatic config at factory time
and again per call, returns `mcpAdapter(pi)`, and `export default createMcpAdapter()` is what pi's
loader invokes; `package.json` declares `"pi": {"extensions": ["./index.ts"], "skills": ["./skills"]}`
and the `pi-mcp-adapter` bin.
**behavior** — the adapter is present in every session of every mode, and switching it off with the
host's "no extensions" flag actually switches it off.
**cyrup** — a new crate `cyrup-mcp` with modules `{config, dirs, owner, abort, state, runtime,
lifecycle, registration, extension, onboarding, agent_plugin}`; one `McpExtension` implementing
`cyrup_ext::NativeExtension`; a construction gate shaped like
`cyrup_ext_subagents::subagent_extension_for_env(agent_dir, config, cwd) -> Option<Arc<dyn NativeExtension>>`;
attached at the Interactive / Rpc / Print|Json arms of `crates/cyrup/src/main.rs` through
`SessionFactory::with_native_extension`. `is_ambient()` **must** return `true` — `pi-mcp-adapter` is
an *installed package* upstream, and `native_survives_no_extensions` consults exactly that method to
implement `--no-extensions`. `decides_project_trust()` **must stay `false`** (the default): a native
that opts in has its `init` run twice on the same object in the pre-trust bootstrap pass, and this
`init` is not idempotent in that sense. Follow the subagents Cargo.toml precedent — direct `ratatui`,
no `cyrup-tui` dependency. `state.ts`'s record becomes the plain `McpState` struct (twenty fields
after the cuts, §8).
**verify** — a `cyrup-it` test asserting the `mcp` tool appears in `all_tool_names()` for a session
built with a fixture `mcp.json`, and does **not** appear under `--no-extensions`.

**MCP-002 — Read `--mcp-config` from argv directly, and register the flag for `--help`** · low · S · `host-verb`
**upstream** — `utils.ts`'s `getConfigPathFromArgv` scans `process.argv` for the exact token
`--mcp-config` and takes the following element; `index.ts` registers the flag with
`{description: "Path to MCP config file", type: "string"}`; `init.ts` reads `pi.getFlag("mcp-config")`
at init time only when `options.config === undefined`.
**behavior** — `cyrup --mcp-config /path/to/mcp.json` loads that file instead of running discovery, and
`--mcp-config` is listed as a known flag rather than reported as unknown.
**cyrup** — `InitApi::register_flag("mcp-config", json!({"description":"Path to MCP config file",
"type":"string"}))` for the declaration; `std::env::args()` for the value. The double read is not a
workaround: `ExtensionHost::apply_extension_flag_values` runs *after* the native-load loop on both
sides, so `init` cannot see the flag store on either. Reproduce the space-separated-only parse
including its non-support of `--mcp-config=path`. Registering the flag is not optional — an
unreconciled `--flag` is itself a startup diagnostic.
**verify** — unit: `config_path_from_argv(["cyrup","--mcp-config","/x"]) == Some("/x")`;
`(["cyrup","--mcp-config=/x"]) == None`; `(["cyrup","--mcp-config"]) == None`.

**MCP-003 — Register the entire tool/command surface from disk caches inside `init()`, and never fail** · critical · L · `host-verb`
**upstream** — `installMcpAdapter`'s whole body is synchronous; direct tools, the `mcp` gateway,
`/mcp`, `/mcp-auth` and one slash command per cached MCP prompt are all registered from
`loadMcpConfig()` + `loadMetadataCache()` before anything connects.
**behavior** — a session opens instantly with the full MCP tool surface visible to the model, identical
to the previous session's, with no subprocess spawned; the system prompt does not change shape between
a cold and a warm start.
**cyrup** — `NativeExtension::init` is the registration window; `InitApi::{register_tool,
register_command, register_tool_renderer, register_flag, subscribe}` are its write paths. Perform the
disk reads with `std::fs` (matching upstream's `readFileSync`), not `tokio::fs`, so nothing blocks the
session build on the reactor. Call `api.register_tool_renderer(name)` once per registered tool name so
`NativeExtension::{render_call, render_result}` are reachable — cyrup splits what upstream passes as
per-tool `renderCall`/`renderResult` arguments into a declaration plus a name-keyed serve.
**Why `critical`:** a native extension's `init()` returning `Err` is marked a fatal startup diagnostic
by the session builder and every mode arm turns that into `dispose(); exit 1`. Upstream's
`installMcpAdapter` cannot fail — every disk read it performs is defensive. A malformed `mcp.json` or
`mcp-cache.json` must therefore degrade to an empty surface, never to an `Err`; otherwise a stray `{{{`
in a user's config crashes cyrup on a normal path.
**verify** — cyrup-it: build a session with a populated `mcp-cache.json` and a config whose only server
is `lifecycle: "lazy"`; assert the direct tool names are in `all_tool_names()` and that **no** child
process was spawned. Plus a unit asserting `init()` returns `Ok(())` for an `mcp.json` containing `{{{`.

**MCP-004 — Port `McpRuntimeOwner`** · high · M · `hand-written`
**upstream** — `runtime-owner.ts`'s `{signal, isActive(), addCleanup(cb), stop(reason?),
throwIfInactive()}` over one `AbortController`, with a memoised `stopPromise` (§7).
**behavior** — one call to `stop()` reliably ends every piece of work a generation owns; calling it
twice is free; work started after the stop is refused rather than run.
**cyrup** — `tokio_util::sync::CancellationToken` (re-exported as `cyrup_core::CancelToken`) is the
signal; the owner, the cleanup stack and the memoised stop are new code in `cyrup-mcp`. One forced
mechanism difference: `AbortController.abort(new Error(reason))` carries the reason *inside* the
signal and `throwIfAborted()` rethrows that exact object, while a `CancellationToken` carries no
payload. Store the reason in an `ArcSwapOption<String>` set immediately before `token.cancel()` so
`throw_if_inactive` reproduces `signal.reason`; without it, `isAbortError`'s literal
`error.message === "MCP extension runtime stopped"` arm and `abort.ts`'s reason-rethrow both stop
working. `arc-swap` is already in the tree as a crate-local pin in two crates — promote it to
`[workspace.dependencies]` rather than adding a third.
**verify** — unit: `stop()` twice returns the same result and runs cleanups once; `add_cleanup` after
stop runs immediately; `throw_if_inactive` reports the reason string passed to `stop`.

**MCP-005 — Reverse-order cleanup, the aggregate error, and the late-cleanup path** · medium · S · `hand-written`
**upstream** — `runtime-owner.ts`'s shared `reportCleanupFailure(error, late)` formatter, the late
`addCleanup` microtask arm, and `stop()`'s `cleanups.splice(0).reverse()` + `Promise.allSettled` +
`AggregateError(failures, "MCP runtime cleanup failed")` + `console.error` + rethrow.
**behavior** — cleanups run strictly last-registered-first, one failing cleanup never prevents the
others from running, and all failures are reported together rather than the first masking the rest.
The real LIFO order after Cut 2 is `lifecycle.gracefulShutdown()` → `shutdownOAuth` →
`cleanupMaterializedBinaryResources`.
**cyrup** — `cleanups.lock().drain(..).rev()`, each spawned as a future, joined with
`futures::future::join_all` (`futures` is a workspace dependency), failures collected into a
`CleanupErrors(Vec<McpError>)` whose `Display` joins deduplicated messages with `": "` — matching
`formatTerminalError`'s `AggregateError`/`cause` walk and `seen` set. Log
`MCP: runtime cleanup failed: {aggregate}` before returning. `add_cleanup` on an already-cancelled
owner spawns the cleanup immediately and logs failures as `MCP: late runtime cleanup failed: {…}` —
one formatter, one `late` flag, as upstream.
**verify** — unit: register three cleanups where the middle one errors; assert execution order 3,2,1,
that all three ran, and that the returned error names the middle one.

**MCP-006 — Port `createOwnedUi` as a fenced services handle** · medium · M · `extension-owned`
**upstream** — `runtime-owner.ts`'s recursive `Proxy` with a `WeakMap` identity cache; once the owner
is inactive every property read is `undefined` and every method call is a no-op that returns
`undefined` **without calling through**.
**behavior** — after a session restart, a stale reconnect callback that calls `ui.notify(...)` does
nothing at all — no crash, no toast in the new session's TUI.
**cyrup** — an `OwnedServices(Arc<dyn HostServices>, Arc<McpRuntimeOwner>)` newtype **inside
`cyrup-mcp`**, implementing `HostServices` by delegating each method behind
`if !self.1.is_active() { return Default::default() }`. No host change: `HostServices` is a public
trait any crate can implement. Rust has no `Proxy`, so the fence is an explicit delegating impl and
must be kept in sync with the trait by hand — write it through a declarative macro over the method
list so a new trait method is a compile error, not a silent hole. The `WeakMap` identity cache has no
analog and is unnecessary: the wrapper is constructed once per generation, not per property read.
**verify** — unit against a recording fake `HostServices`: call `notify` before and after
`owner.stop()`; assert exactly one recorded call.

**MCP-007 — Port the abort helpers (`combineAbortSignals`, `isAbortError`, `throwIfAborted`, `abortable`)** · medium · S · `hand-written`
**upstream** — `runtime-owner.ts`'s `combineAbortSignals`/`isAbortError` and `abort.ts`'s
`throwIfAborted`/`abortable` (§7).
**behavior** — an in-flight MCP request aborts promptly when either the runtime owner or the caller's
own signal fires; an abort is reported as an abort rather than as a connection failure, which would
poison the 60-second failure backoff.
**cyrup** — `combine(owner: &CancelToken, other: Option<&CancelToken>) -> CancelToken`: `None` ⇒
`owner.clone()`; `Some` ⇒ a fresh token plus one spawned task doing
`tokio::select!{ _ = a.cancelled() => {}, _ = b.cancelled() => {} }` then `child.cancel()`.
`tokio-util` offers `child_token()` but no any-of combinator, so the composition costs one task per
combined pair — bound it by combining once per generation rather than per request.
`abortable(fut, token)` is `tokio::select!` with `biased;` on the cancel arm. `is_abort_error` becomes
`matches!(e, McpError::Aborted(_)) || token.is_cancelled()`, replacing upstream's literal string
compare with a typed variant, paired with MCP-004's reason storage so the message still round-trips.
The caller-side half of the pair is `HostServices::is_run_cancelled` — a documented poll rather than a
wake, and see *What does not fit cleanly* for its production wiring.
**verify** — unit: a future racing a combined token cancels when either parent cancels, and the
spawned joiner task exits (no leak) when the combined token is dropped.

**MCP-008 — The `session_start` generation protocol, abort-before-await** · high · M · `hand-written`
**upstream** — `index.ts`'s `session_start` handler (§4): bump the generation; snapshot the previous
state/owner/oauth; build the new owner and OAuth runtime; null `state`/`initPromise`; call
`previousOwner.stop("MCP extension session restarted")` **synchronously before** the
`await Promise.all([...])`; re-check `generation !== lifecycleGeneration || !owner.isActive()` after
the await; then `startInitialization`.
**behavior** — restarting or reloading a session never lets the previous session's connect pass,
reconnect timer or OAuth callback write into the new session; a slow shutdown does not delay the abort.
**cyrup** — delivered as `HostEvent::SessionStart { reason, previous_session_file }` to
`NativeExtension::on_event` once `init` subscribed `EventKind::SessionStart`. `McpExtension` holds
`generation: AtomicU64`, `owner: Mutex<Option<Arc<McpRuntimeOwner>>>`,
`state: Mutex<Option<Arc<McpState>>>`, `init_task: Mutex<Option<Shared<…>>>`. Reproduce the ordering
exactly, including the synchronous stop: take the previous owner out of the mutex and call
`owner.begin_stop()` — which cancels the token *and* returns the cleanup future — before awaiting
anything. Do **not** collapse this into a single `stop().await`; the whole point is that the cancel is
observable before the cleanup completes. Re-check `generation.load(Acquire) == my_gen &&
owner.is_active()` after the join. **Ordering note:** under cyrup's replacement tail the previous
generation's `SessionShutdown` has *already* run when this fires (MCP-014), so `previous_state` /
`previous_owner` are normally `None`; the snapshot-and-stop arm is the defence for the paths where
they are not — a `SessionStart` with no preceding shutdown, or a build that skipped the install tail.
**verify** — cyrup-it: drive two `SessionStart` events back to back with a slow cleanup on the first;
assert the first generation's owner is cancelled before the second generation's initialization begins.

**MCP-009 — The `session_shutdown` handler** · high · S · `hand-written`
**upstream** — `index.ts`'s `session_shutdown` handler: `++lifecycleGeneration`; snapshot and null
`state`/`currentOwner`/`currentOAuthRuntime`/`initPromise`;
`owner?.stop("MCP extension session shutdown")` before the
`await Promise.all([stopOwner, shutdownState(currentState, "session_shutdown"), shutdownOAuth(...)])`;
catch logs `MCP: session shutdown cleanup failed: ${…}`.
**behavior** — quitting or switching a session closes every MCP server process, persists the metadata
cache, and never leaves an orphan.
**cyrup** — `HostEvent::SessionShutdown { reason, target_session_file }`, dispatched by
`AgentSession::dispose_with` as an **awaited** notify **before** the session cancel token fires — so
the handler genuinely gets to finish, which is better than pi and needs no compensation. One caveat:
`abort_and_settle()` runs *before* the notify, so any in-flight turn is already settled when the
handler sees the event.
**verify** — cyrup-it: assert every spawned MCP child process has exited after `AgentSession::dispose`.

**MCP-010 — `shutdownState`, preserving the metadata-flush error** · high · S · `hand-written`
**upstream** — `index.ts`'s `shutdownState` (§5): publish the empty status snapshot; `flushMetadataCache`
inside a `try` that **captures** the error; stop the owner (or `lifecycle.gracefulShutdown()` when
there is no owner) inside a `try` whose error is only *logged* as
`MCP: graceful shutdown failed after metadata flush error: ${…}` when a flush error was already
captured, and rethrown otherwise; finally rethrow the flush error.
**behavior** — a metadata-cache write failure is never swallowed by a concurrent shutdown failure, so
the next launch is never silently left with a stale or missing tool surface.
**cyrup** — direct port; `flush_error: Option<McpError>` held across the shutdown arm, with
`flush_error` winning. The `uiServer.close(reason)` step is **cut** (Cut 2). The null-state arm still
publishes the shutdown snapshot — in Rust, into the crate's own `tokio::sync::watch`, not a bus.
**verify** — unit: make both the flush and the cleanup fail; assert the returned error is the flush
error and that the cleanup failure was logged.

**MCP-011 — `startInitialization`'s triple staleness check and metadata-update hook install** · high · M · `hand-written`
**upstream** — `index.ts`'s `startInitialization` (§6), including the frozen-arm log
`` `MCP: metadata update for ${serverName} (${reason}) skipped — directTools frozen` `` and the
`freezeDirectTools` `logger.info` string.
**behavior** — a slow initialization that finishes after its session is gone tears itself down instead
of becoming the live runtime; two initializations racing within one generation cannot both commit.
**cyrup** — direct port. The `initPromise !== promise` arm becomes identity comparison of the stored
`Arc<Shared<…>>` (`Arc::ptr_eq`), not a value comparison. All three conditions must be kept: dropping
the identity check silently permits a double commit within one generation.
**verify** — unit: complete an initialization after bumping the generation; assert the built state's
`stop()` ran and the extension's `state` slot is still `None`.

**MCP-012 — `startLoadTimeInitialization` — the eager/keep-alive pre-warm** · medium · S · `extension-owned`
**upstream** — `index.ts`'s `startLoadTimeInitialization` (§3): connect at load only when some enabled
server declares `lifecycle: "eager" | "keep-alive"`; defer by `setImmediate`; re-check
`lifecycleGeneration !== 0 || state || initPromise`; use the synthetic print-mode context and the stale
reason `"stale_load_time_initialization"`.
**behavior** — an eager server is already warm by the time the first prompt is typed, and a lazy-only
configuration spawns nothing until a tool is called.
**cyrup** — a `tokio::spawn` from `init()`, guarded by the same three-way check on the atomics. This is
settled precedent, not a stretch: `NativeExtension::set_host_services` exists *specifically* so a
built-in can stash the `Arc` and reach the live session from a background task outside any `HostCtx`,
and `ExtensionHost::load_native_with_services` calls it **before** `init`, so an `init`-spawned task
already holds the backend and observes the later manager/ui/inject attachments through the `Arc`'s
interior mutability. `cyrup-ext-subagents` spawns detached OS processes from `init` and supervises
them for the process lifetime. Two notes: the synthetic ctx becomes `HostCtx::event(ExtMode::Print,
false, cwd)` **and no services handle** — `hasUI:false` upstream means `ui` is `undefined` throughout
`initializeMcp`, so the Rust pass must pass `None` rather than the live services; and because cyrup
re-runs `init()` per built session, the task must be fenced by the generation counter or a second
build starts a second pre-warm. `setImmediate` guarantees ordering before any I/O; `tokio::spawn` does
not, so the re-check at the top of the task is load-bearing here in a way it is merely defensive
upstream.
**verify** — cyrup-it: a config with one `lifecycle:"eager"` server spawns a child during session
build; the same config with `lifecycle:"lazy"` spawns none.

**MCP-013 — The `MCP_DIRECT_TOOLS` blocking wait at session start** · low · S · `hand-written`
**upstream** — `index.ts`'s `session_start` step 9: when `MCP_DIRECT_TOOLS` is set and is not the
sentinel `"__none__"`, compute `getMissingConfiguredDirectToolServers(earlyConfig, loadMetadataCache(),
envDirectToolOverride)` and, only if that list is non-empty, `await initialization`.
**behavior** — a CI/scripted run that pins its direct tools by env var does not start prompting until
those servers have been contacted at least once, so the very first turn already sees them.
**cyrup** — direct port. `AgentSession`'s extension dispatch on `SessionStart` is awaited, so blocking
inside `on_event(SessionStart)` genuinely delays the session — the same effect upstream gets. Env var
read via `std::env::var("MCP_DIRECT_TOOLS")`; the `__none__` sentinel and the
`split(',').map(trim).filter(non-empty)` normalisation are identical.
**verify** — unit on the predicate: `MCP_DIRECT_TOOLS=a,b` with an empty cache yields a non-empty
missing list; `MCP_DIRECT_TOOLS=__none__` yields an empty one regardless of cache state.

**MCP-014 — Re-`init` per session, and the build-before-dispose inversion** · high · M · `hand-written`
**upstream** — `installMcpAdapter`'s body runs **once per process**; `lifecycleGeneration`,
`currentOwner`, `currentOAuthRuntime`, `state`, `initPromise`, `registeredDirectTools`,
`fallbackDeactivatedTools`, `registeredPromptCommands`, `proxyToolRegistered`, `proxyToolDescription`
and `directToolsFrozen` are closure variables that persist across every `session_start`.
**behavior** — the fingerprint map, the prompt-command dedup set and the fallback-deactivated set
survive a session restart, so a restart re-registers only what genuinely changed.
**cyrup** — `SessionFactory::with_native_extension` stores the `Arc` and its doc says it is
"re-`init`-ed into each freshly built session"; `build_with_parent` / `build_from_manager` re-attach
it, and `ExtensionHost::load_native_body` calls `ext.init(&mut api)` against a **fresh
`ExtensionHost`, fresh `InitApi` and fresh registry** per build, so re-registration is mandatory, not
optional. `AgentSession::emit_session_start` fires at most once per session object.
**The settled rule, and the ordering fact that settles it:** `AgentSessionRuntime::new_session_with`
**builds the replacement first** and only then calls `install`, whose `install_inner` disposes the
outgoing session and finally emits `SessionStart` on the new one. So `init()` for generation N+1 runs
while generation N is *still the installed session*. Therefore:
- **`init()` performs registration only** — read the config and the cache, register the tools,
  commands, renderers and the flag, subscribe, spawn the pre-warm task. It performs **no teardown of a
  previous generation.**
- **`on_event(SessionShutdown)` is the only teardown point**, and it is where the metadata flush lives.
- **`on_event(SessionStart)` is the generation bump** and builds the new runtime.
Putting teardown in `init()` would kill generation N's MCP children before N's own shutdown flush ran,
and if the build then failed, generation N would stay live with a torn-down MCP runtime and no path
back. All persistent maps (`registered_direct_tools`, `registered_prompt_commands`,
`fallback_deactivated_tools`, `proxy_tool_description`, `direct_tools_frozen`) live on the
`McpExtension` struct and therefore survive re-`init` exactly as upstream's closure variables survive
`session_start`. One consequence must be handled explicitly: because the registry is fresh but
`registered_direct_tools` is not, `init()` must register **every** tool while still updating the
fingerprint map — the fingerprint diff (MCP-036) suppresses re-registration only *within* a session,
never across an `init()`.
**verify** — cyrup-it: build a session, force a session replacement, assert (i) the direct tools are
still present, (ii) no duplicate slash command was registered, (iii) exactly one MCP child process per
`keep-alive` server exists across the transition, and (iv) a session build that fails after `init()`
leaves the current session's MCP servers running.

**MCP-015 — Snapshot every context value before the first await in `initialize`** · medium · S · `extension-owned`
**upstream** — `init.ts`'s seven `const` reads plus two derivations under the comment *"Pi guards
ExtensionContext getters after reload. Snapshot all values that can be used by asynchronous work
before the first await."*
**behavior** — an initialization that outlives a reload never reads a revoked host getter and never
crashes the session.
**cyrup** — `HostCtx` is `Clone` with plain public `mode`/`has_ui`/`cwd` fields; it is **already** a
snapshot by value and there is no getter to revoke. The discipline still matters for a different
reason: the `Arc<dyn HostServices>` handed to `set_host_services` belongs to the session that
installed it and a later session overwrites the facade's slot, so a generation must hold its **own**
`Arc` clone rather than re-reading a shared slot. Two upstream live-closure behaviours do **not**
survive by-value snapshotting: `getCurrentModel` becomes `HostServices::current_model()` (which
`LiveHostServices` does implement), and `getSignal` becomes MCP-007's combined token.
`HostCtx::model()` is a snapshot and is the wrong source for a long-lived closure.
**verify** — unit: an initialization holding an owned services `Arc` still notifies through the
*original* sink after the extension's services slot has been replaced.

**MCP-016 — The sampling and elicitation wiring gates** · medium · M · `hand-written`
**upstream** — `init.ts`: `samplingAutoApprove = settings.samplingAutoApprove === true`; sampling is
wired **only** when `settings.sampling !== false && (hasUI || samplingAutoApprove)`; elicitation
**only** when `settings.elicitation !== false && hasUI` and `ui` exists, with `allowUrl: mode === "tui"`.
`isTuiMode(ctx)` is the same predicate exported for other callers: `ctx.hasUI && ctx.mode === "tui"`.
**behavior** — a headless run never blocks on an MCP server's `sampling/createMessage` or
`elicitation/create` prompt; URL-mode elicitation only opens a browser from a real TUI.
**cyrup** — the gates are pure boolean logic over `HostCtx::{has_ui, mode}` and port directly;
`allow_url` is `mode == ExtMode::Tui`. The handlers themselves are other sections' work — and note
that neither needs a new host surface: rmcp's `ClientHandler::{create_message, create_elicitation}`
carry the protocol, `cyrup-provider` is reached **directly** for the nested completion exactly as
upstream reaches `pi-ai/compat` directly, and the dialogs are `HostServices::{confirm, input, select}`
under `HostServices::human_interaction_lock`. Only the gating belongs here, because getting it wrong
silently hangs a `cyrup -p` run.
**verify** — unit: a `HostCtx` with `has_ui:false` and `samplingAutoApprove` unset produces no sampling
config; with `samplingAutoApprove:true` it does; `ExtMode::Print` yields `allow_url == false`.

**MCP-017 — Register owner cleanups in the exact LIFO order, plus the list-changed listener** · medium · S · `hand-written`
**upstream** — `init.ts` registers `shutdownOAuth` (only when `ownsOAuthRuntime`) then
`lifecycle.gracefulShutdown()`, with `index.ts` having registered
`cleanupMaterializedBinaryResources` first so it runs last. The listener:
`setMetadataListChangedListener((serverName, reason) => { owner-guard; updateServerMetadata;
updateMetadataCache(state, serverName, {preserveEmptyResources:false});
notifyToolMetadataUpdated(state, serverName, reason); updateStatusBar(state) })`.
**behavior** — on teardown the MCP servers close before the OAuth runtime, so an in-flight callback can
still be refused cleanly.
**cyrup** — direct port onto MCP-005's cleanup stack. The `uiServer` cleanup arm is **cut** (Cut 2),
which shortens the chain to lifecycle → oauth → binaries. The `preserveEmptyResources: false` on the
list-changed path is the load-bearing detail: an authoritative empty `resources/list` must overwrite
the cache, whereas a transient empty list during a normal refresh must not (MCP-029). rmcp delivers
the trigger as `ClientHandler::on_{tool,prompt,resource}_list_changed`, a bare notification on which
`Peer<RoleClient>` invalidates its own response cache, so the handler re-calls `list_all_*` itself.
**verify** — unit: assert the recorded cleanup execution order is lifecycle → oauth → binaries.

**MCP-018 — The zero-enabled-servers early return** · low · S · `hand-written`
**upstream** — `init.ts` filters on `!isServerDisabled(d)` where `isServerDisabled` is
`definition?.disabled === true` (its own doc: *"Only the literal boolean `true` disables a server."*).
When empty: if `allServerEntries.length > 0 && hasUI`, notify
`` `MCP: All ${allServerEntries.length} server(s) are disabled` `` (`"info"`); then
`publishMcpStatusSnapshot(state)` and **return the state** — no cache work, no lifecycle registration,
no health-check timer.
**behavior** — a config where every server is disabled costs nothing at startup and says so once.
**cyrup** — direct port. `disabled: 1`, `disabled: "true"` and `disabled: null` must all mean
*enabled*. A `serde` `Option<bool>` reproduces this only if a non-boolean JSON value is tolerated
rather than rejected — use `Option<Value>` compared against `Value::Bool(true)`, or a custom
deserializer, so a malformed `disabled` does not fail the whole config parse (which under MCP-003
would be a fatal `init()`).
**verify** — unit: `is_server_disabled` is true only for `{"disabled": true}`; false for `1`, `"true"`,
`null`, absent.

**MCP-019 — Metadata-cache bootstrap: file-absent means connect everything once** · medium · S · `hand-written`
**upstream** — `init.ts` (§9): `!cacheFileExists` ⇒ `bootstrapAll = true` **and**
`saveMetadataCache({version:1, servers:{}})`; file present but `loadMetadataCache()` returned null ⇒
rewrite it empty but **do not** set `bootstrapAll`.
**behavior** — the very first run after installation contacts every enabled server once so the next
launch has an instant tool surface; a corrupt cache is repaired without triggering a connect storm.
**cyrup** — direct port at `<agent_dir>/mcp-cache.json`, `CACHE_VERSION = 1`. The two-way split on
*exists* vs *parses* is the whole item: collapsing it into "no usable cache ⇒ bootstrap" changes the
corrupt-cache path from cheap to expensive.
**verify** — unit with a temp agent dir: no file ⇒ `bootstrap_all == true`; a file containing `{}` ⇒
`bootstrap_all == false` and the file is rewritten to `{"version":1,"servers":{}}`.

**MCP-020 — Per-server lifecycle registration and idle-override derivation** · medium · S · `hand-written`
**upstream** — `init.ts` (§10): `lifecycleMode = definition.lifecycle ?? "lazy"`;
`persistsAfterFirstSpawn = mode === "eager" || mode === "lazy-keep-alive"`;
`idleOverride = definition.idleTimeout ?? (persistsAfterFirstSpawn ? 0 : undefined)`; `markKeepAlive`
**only** for `"keep-alive"`; `markKeepAliveAfterConnect` marks a `lazy-keep-alive` server only after
its first successful connect and early-returns when the definition is missing **or disabled**. Both
`registerServer` and `markKeepAlive` early-return on `isServerDisabled`, and `registerServer` stores
`serverSettings` only when `idleTimeout !== undefined`.
**behavior** — an `eager` or `lazy-keep-alive` server never idles out by default, a `keep-alive` server
is reconnected by the health check from the very first tick, and a `lazy-keep-alive` server is not
reconnected until it has been used once.
**cyrup** — direct port; the `?? 0` default for `persistsAfterFirstSpawn` and the deferred
`markKeepAlive` for `lazy-keep-alive` carry the whole four-mode semantics.
**verify** — unit table over the four lifecycle values × `{idleTimeout absent, idleTimeout: 5}`
asserting the derived override and whether the server is in `keep_alive_servers` before and after a
connect.

**MCP-021 — Rehydrate tool/resource/prompt/instruction metadata from a hash-valid cache entry** · medium · M · `hand-written`
**upstream** — `init.ts` (§10 step 6), including the deliberate omission of `promptMetadataLive` for a
rehydrated prompt list.
**behavior** — the tool list, prompt list, resource count and server instructions shown before any
connection are exactly what the last successful session saw, and the UI can still distinguish "from
cache" from "freshly discovered".
**cyrup** — the validity rule is already implemented on the *reader* side in
`cyrup_ext_subagents::exec::mcp_direct_tools`'s `is_server_cache_valid`: `configHash` match **and**
`cachedAt` present **and** `now - cached_at <= CACHE_MAX_AGE_MS` (7 days). Port the writer side to
match. `promptMetadataLive` becomes a `HashSet<String>` populated only from live discovery. The
subagents reader's `ServerCacheEntry` models only `configHash`/`tools`/`resources`/`cachedAt`, so the
writer's `prompts`/`instructions` fields are ignored harmlessly by it.
**verify** — unit: a cache entry whose `configHash` no longer matches the definition is ignored
entirely.

**MCP-022 — The bounded startup connect pass** · medium · M · `hand-written`
**upstream** — `init.ts` (§11): the `bootstrapAll ? all : keep-alive|eager` selection, the
`connecting to ${n} servers...` status write, `parallelLimit(startupServers, 10, …)`, the byte-exact
`` `OAuth authentication required. Run /mcp-auth ${name}.` `` message, and the abort split (rethrow
when `owner.signal.aborted`, silent skip otherwise).
**behavior** — startup contacts at most ten servers at once; a cancelled startup stops the whole pass
rather than recording ten spurious failures.
**cyrup** — port `parallelLimit` literally: a `Vec<Option<R>>` of `items.len()`, an
`Arc<Mutex<std::vec::IntoIter<(usize, T)>>>`, and `min(10, len)` worker tasks each looping
`next() → results[i] = f(item).await`. **Do not** substitute `buffer_unordered`: results are read back
by position in the second pass and the two-pass collision universe depends on all results being
present, not on their arrival order. The connect itself is `rmcp` — `TokioChildProcess` for stdio,
`StreamableHttpClientTransport` for HTTP — behind the server-manager section's `connect`.
**verify** — unit: with 25 items and a semaphore-counting `f`, assert peak concurrency is exactly 10
and `results[i]` corresponds to `items[i]`.

**MCP-023 — The two-pass startup metadata build** · high · M · `hand-written`
**upstream** — `init.ts` (§12): pass one builds `startupKnownMetadata` over **every** successful
connection; pass two calls `buildToolMetadata(..., startupKnownMetadata, true)` per server, with
`owner.throwIfInactive()` at the top of every iteration and the
`` `MCP: ${name} - ${failedTools.length} tools skipped` `` warning.
**behavior** — cross-server tool-name collisions resolve identically regardless of the order the
servers happened to finish connecting. A single-pass port makes prefixed tool names non-deterministic
across runs, which means a name the model learned in one session can address a *different server's*
tool in the next.
**cyrup** — direct port. Building the collision universe in a separate pass is the entire point.
**verify** — unit: two servers exposing the same tool name, connected in either completion order,
produce the same final pair of prefixed names.

**MCP-024 — Failure tracking with a 60-second backoff** · medium · S · `hand-written`
**upstream** — `init.ts` (§13): `FAILURE_BACKOFF_MS = 60 * 1000`, `MAX_FAILURE_MESSAGE_CHARS = 8 * 1024`,
the per-state `WeakMap` of timers, the `=== failedAt` generation check inside the timer, and
`timer.unref?.()`.
**behavior** — a server that fails to start is not retried on every tool call for the next minute, its
error text is shown for that minute and then disappears, and the timer never keeps the process alive.
**cyrup** — `HashMap<String, Instant>` + `HashMap<String, String>` on the state; the expiry timer is a
`tokio::spawn` + `sleep(60s)` holding a `Weak<McpState>` (the `WeakMap` analog) and the owner token,
selecting on `owner.cancelled()`. `timer.unref()` needs no analog — a tokio task does not keep the
process alive — but it *does* delay a clean runtime shutdown, so the select on the owner token is
required rather than optional. Keep the `=== failedAt` check: a re-insert must not be cleared by the
older timer.
**verify** — unit with `tokio::time::pause`: record a failure, advance 59 s (still recorded), advance to
61 s (cleared, snapshot republished); record a second failure at 30 s and assert the first timer does
not clear it.

**MCP-025 — Startup connect notifications, terminal sanitising, and skipped-tool warnings** · high · S · `hand-written`
**upstream** — `init.ts` (§12, §14): the per-failure
`` `MCP: Failed to connect to ${name}: ${sanitizeTerminalText(error ?? "Unknown connection failure")}` ``
notified as `"error"` **and** always `console.error`'d; the per-success
`` `MCP: ${name} - ${failedTools.length} tools skipped` `` as `"warning"`; and the summary line in its
two exact forms.
**behavior** — an MCP server cannot smuggle escape sequences into the user's terminal through its error
message; the startup summary is one line, not one per server.
**cyrup** — `HostServices::notify(&str, NotifyKind)` with `NotifyKind::{Info, Warning, Error}` is a 1:1
match for upstream's three literals; port the double reporting (notify **and** stderr) of connect
failures. **`sanitize_terminal_text` is NOT a pure regex port.** Its first step, `stripOscSequences`,
is a hand-written scanner that recognises both the `ESC ]` and C1 `0x9D` introducers and consumes to
`BEL`/`ST`/`ESC \` — **or to end-of-string when the payload is never terminated**. Port it as a
`char`-indexed state machine; a regex cannot express the unterminated arm, and dropping it is exactly
the hole a hostile server would use. Steps 2-4 are two regexes plus a whitespace collapse, ASCII-only
with no lookaround, so the `regex` crate suffices — note `regex` is currently a crate-local pin in
`cyrup-permission-system` and should be promoted to `[workspace.dependencies]` rather than pinned a
third time. **Why `high` and not `critical`:** the consequence is forged terminal content from an
untrusted server, which is a real security control, but it is not one of the four canonical clauses.
**verify** — unit: a message containing a terminated OSC sequence, an **unterminated** OSC introducer,
a CSI colour sequence and a raw BEL all sanitise to the plain text with single spaces.

**MCP-026 — The `MCP_DIRECT_TOOLS` cache-bootstrap pass inside `initialize`** · low · S · `hand-written`
**upstream** — `init.ts` (§14): skipped entirely for `"__none__"`; **re-reads
`process.env.MCP_DIRECT_TOOLS` itself** rather than reusing the factory's closure value, and re-reads
the cache; connects the subset **not** already connected in the startup pass at concurrency 10; per
success `updateServerMetadata` → `updateMetadataCache` →
`notifyToolMetadataUpdated(state, name, "direct-tools-bootstrap")` → `markKeepAliveAfterConnect` →
`clearFailure`; a missing definition throws `` `MCP server "${name}" is not configured` ``; then
`owner.throwIfInactive()` and, when anything was bootstrapped and there is a UI,
`` `MCP: direct tools for ${bootstrapped.join(", ")} will be available after restart` `` (`"info"`).
**behavior** — a `MCP_DIRECT_TOOLS`-pinned run populates the cache for the named servers even when they
are `lazy`, and honestly says the tools appear next launch rather than this one.
**cyrup** — direct port, sharing MCP-022's `parallel_limit`. If HA-1 (MCP-037) lands, the "after
restart" message becomes false for cyrup and must be changed **together with** an actual late
registration — pick one deliberately rather than leaving the message and adding the registration.
**verify** — unit: with `MCP_DIRECT_TOOLS=srv` and an empty cache, `srv` is connected exactly once even
when it also appeared in `startupServers`.

**MCP-027 — Lifecycle callbacks (reconnect, reconnect-failure, idle shutdown)** · medium · S · `hand-written`
**upstream** — `init.ts` (§15): each callback opens with the owner guard, then
`updateServerMetadata`/`updateMetadataCache`/`notifyToolMetadataUpdated(…, "lifecycle-reconnect")`/
`clearFailure`/`updateStatusBar`; the failure callback does `recordFailure` + `updateStatusBar`; the
idle callback logs `` `${serverName} shut down (idle ${idleMinutes}m)` `` using
`getEffectiveIdleTimeoutMinutes`.
**behavior** — the footer and the model's tool list stay correct across a background reconnect without
any user action.
**cyrup** — direct port. Every callback body opens with the owner guard, which is what keeps a
generation-N timer from writing into generation N+1.
**verify** — unit: stop the owner, then fire each callback; assert no state mutation and no notify.

**MCP-027a — `sendMessage`'s `triggerTurn` pre-turn convergence gate** · medium · S · `hand-written`
*Filed 2026-08-20 by the v2.25.0 → v2.26.1 retarget. NOT implemented.*
**upstream** — `init.ts:181-195` at v2.26.1 (commit `48799fa`). At v2.25.0 `state.sendMessage` was two
lines — `if (!owner.isActive()) return; pi.sendMessage(message, options)` — which is what §8 step 9
above still describes. `48799fa` replaced it: the owner-guarded send becomes a `deliver` closure, and
when `options?.triggerTurn` is set the send is **deferred behind
`lifecycle.ensureConverged(owner.signal)`**, delivering on success and, on failure, logging
`` `MCP: pre-turn keep-alive convergence failed: ${sanitizeTerminalText(detail)}` `` at debug and
delivering **anyway**. An abort or an inactive owner is swallowed silently.
**behavior** — a message that starts a turn must not start it against a stale keep-alive tool catalog.
This is the same defect `pi.on("input")` closes for user-typed turns (already ported —
`registration.rs` `SUBSCRIBED_EVENTS` carries `EventKind::Input`, handler at `extension.rs:344`);
this is its half for turns the extension itself triggers. Note the failure mode is deliberately
fail-*open*: convergence failing must not swallow the message.
**cyrup** — this is currently **inexpressible**. `pub type SendMessage = Arc<dyn Fn(String) + Send +
Sync>` (`state.rs:55`) takes no options at all, so there is no `triggerTurn` to branch on. The type
alias must grow the flag — `Arc<dyn Fn(String, bool) + Send + Sync>` or a small `SendMessageOptions`
struct — which touches the alias, both `state.rs` structs that hold it (`:113`, `:144`), the builder
at `runtime.rs:189`, and every call site. The fenced host handle already carries the flag
(`owner.rs:423`), so nothing new is needed from the host. Deliver-on-failure must be a real arm, not a
`?`: swallowing the message on a convergence error is the one outcome upstream rules out.
**verify** — unit: with `triggerTurn` unset the send is synchronous and does **not** await
convergence; with it set, a convergence that resolves delivers after it, and a convergence that
**rejects** still delivers, exactly once, with one debug line. Stopping the owner between the await
and the delivery drops the message silently.
*Note for whoever fills `runtime::initialize_mcp`:* §8 step 9's `sendMessage` row above is the
v2.25.0 shape and is correct as such — implement **this** unit's shape, not that row's.

**MCP-028 — `updateServerMetadata`** · medium · S · `hand-written`
**upstream** — `init.ts` (§17): the two bail guards; **if the definition is now disabled, delete every
map entry for that server and return**; else rebuild with `state.toolMetadata` as the collision
universe; set `promptMetadata` + `promptMetadataLive` only when `!connection.promptDiscoveryFailed`;
set **or delete** `serverInstructions`.
**behavior** — a server disabled while connected disappears from the tool surface on the next metadata
refresh instead of lingering.
**cyrup** — direct port; note the collision universe here is `state.toolMetadata` (all servers'
*current* names), not the startup snapshot (MCP-023).
**verify** — unit: disable a connected server in the in-memory config, call `update_server_metadata`,
assert all five maps lost the entry.

**MCP-029 — `updateMetadataCache` write rules** · high · M · `hand-written`
**upstream** — `init.ts` (§17): the four guards; `resources = exposeResources === false ? [] : …`; the
`promptDiscoveryFailed` preservation gated on a matching `configHash`; the empty-resource preservation
rule; the entry shape; and the single-server **merge** write.
**behavior** — a server that transiently returns an empty `resources/list` does not erase its cached
resources, but an authoritative `list_changed` empty list does; a failed `prompts/list` keeps the
previously cached prompts as long as the config has not changed.
**cyrup** — direct port. The merge write must be a read-modify-write of the whole file under a lock;
`cyrup-config`'s `FileSettingsStore::with_lock` (over `cyrup_config::lock::FileLock`) is the in-tree
cross-process advisory-lock pattern to follow. **The schema is a fixed cross-crate contract** — see
MCP-021 and §17: `cyrup_ext_subagents::exec::mcp_direct_tools` is an existing reader, and the
`compute_mcp_server_hash` digests must match or every `mcp:` subagent tool selector resolves to
nothing. Do not bump `CACHE_VERSION`.
**verify** — unit: write an entry with two resources; write again with zero resources and the same
`configHash` and default options ⇒ two resources survive; repeat with `preserve_empty_resources: false`
⇒ zero.

**MCP-030 — `notifyToolMetadataUpdated` must never let a hook break a connect** · low · S · `hand-written`
**upstream** — `init.ts` (§17): the hook is called inside a `try`; if it returns a thenable a `.catch`
is attached; both paths log `` `MCP: metadata update hook failed for ${serverName}: ${message}` `` at
debug and swallow.
**behavior** — a rendering bug in the tool-surface sync cannot abort a server connection.
**cyrup** — the hook becomes an `Arc<dyn Fn(&str, &str) + Send + Sync>` slot on the state; wrap the
call in `std::panic::catch_unwind(AssertUnwindSafe(...))` **as well as** an error match, because a
Rust panic is the closer analog of a thrown JS exception and `ExtensionHost` already establishes panic
containment for handlers.
**verify** — unit: a hook that panics does not propagate out of `notify_tool_metadata_updated`.

**MCP-031 — `flushMetadataCache` on shutdown** · medium · S · `hand-written`
**upstream** — `init.ts` (§17): for every connected connection, `updateMetadataCache(state, name)`.
Called from `shutdownState` step 4, whose error is preserved (MCP-010).
**behavior** — everything learned during a session is on disk before the process exits, so the next
launch starts warm.
**cyrup** — direct port. It is synchronous upstream and must stay synchronous-or-awaited here: it runs
inside the shutdown path before the owner stops, and a fire-and-forget write would race process exit.
cyrup's `SessionShutdown` dispatch is awaited and precedes the session cancel, so an awaited flush
genuinely completes.
**verify** — cyrup-it: connect a fixture server, dispose the session, assert the on-disk cache contains
its tools.

**MCP-032 — `updateStatusBar` — the three footer verbosities** · low · S · `host-verb`
**upstream** — `init.ts` (§18): the eleven-step algorithm, including `publishMcpStatusSnapshot` running
**before** the `!ui` early return; `entries.length === 0` clearing the key; `off` clearing the key;
`compact` being `` `MCP ${connected}/${enabled}` `` with **no** icon prefix; `full` being
`` `${n} server(s) enabled` `` plus the optional connected/disabled suffixes passed through
`formatMcpStatus`.
**behavior** — the MCP footer segment reads identically to upstream in all three verbosities and clears
itself when there is nothing to say.
**cyrup** — `HostServices::set_status(&self, key: &str, text: Option<&str>)`, implemented by
`LiveHostServices`; `None` clears, an exact match for `setStatus(key, undefined)`. The string
construction ports exactly. The colouring does not: `ui.theme.fg("accent", …)` has no analog —
`HostServices::theme()` returns a theme *name* and `LiveHostServices` does not override it, so a native
reads `None`. **Accept the loss**: the branch collapses to upstream's own no-theme arm and the text is
uncoloured. A `HostServices::style(role, text)` backed by the TUI theme would fix it, but that is a
theme-seam question for every extension, not an MCP prerequisite, and it must not hold up this port.
**verify** — unit table over `{off, compact, full} × {0 servers, 1 enabled, 3 enabled/1 connected/1
disabled} × {showStatusIcon true, false}` asserting the exact string or `None`.

**MCP-033 — `lazyConnect`** · medium · M · `hand-written`
**upstream** — `init.ts` (§19), the eight-step algorithm, with the four `false`-returning guards in
order: `needs-auth`, already-connected (returns `true` after refreshing metadata), inside the failure
backoff, missing-or-disabled definition.
**behavior** — the first model call on a `lazy` server spawns it, refreshes its metadata, writes the
cache and updates the footer; a server that failed a minute ago is not retried; a server needing OAuth
returns cleanly so the caller can emit the auth-required message.
**cyrup** — direct port; this is on the hot path for the `mcp` gateway and every direct tool. Note the
subtle error arm: `if (isAbortError(error, ownedSignal)) throwIfAborted(ownedSignal)` rethrows **only
if the signal is actually aborted**, so a stray `AbortError` from a live signal falls through to
`recordFailure`. Reproduce that exactly; collapsing it to "any abort error rethrows" would let a
server-side cancellation poison the call instead of being recorded as a failure.
**verify** — unit: a server inside the 60 s backoff returns `false` without attempting a connect; a
`needs-auth` connection returns `false` without recording a failure.

**MCP-034 — `McpLifecycleManager` — the health-check state machine** · medium · M · `hand-written`
**upstream** — `lifecycle.ts` (§16) in full: the signal/interval overload, the aborted-signal early
return, the once-abort listener, the `stopped || signal?.aborted || activeHealthCheck` **single-flight**
guard, the `` `MCP: Health check failed: ${formatTerminalError(error)}` `` catch, the identity-checked
`.finally`, the unconditional `unref()`, the two sequential passes, and `getIdleTimeout`'s
minutes-×-60000 per-server preference with `0` disabling the idle close.
**behavior** — a `keep-alive` server that dies is silently restarted within 30 seconds; an unused lazy
server's process is reaped after its idle timeout; the checks never overlap; a session abort stops them
immediately.
**cyrup** — a `tokio::spawn`'d loop over `tokio::time::interval(Duration::from_secs(30))` with
`MissedTickBehavior::Delay`, selecting on `owner.cancelled()`. The single-flight guard is still
required — `interval` fires on schedule regardless of how long the body took — and
`MissedTickBehavior::Delay` is the closest match to `setInterval`'s behaviour when a tick is skipped by
the guard. `unref()` needs no analog (see MCP-024) but the select on the owner token does.
**verify** — unit with `tokio::time::pause`: a check taking 45 s causes exactly one tick to be skipped,
not queued; aborting the owner mid-check stops the loop.

**MCP-035 — `gracefulShutdown` — memoised, and it waits for the in-flight check** · high · S · `hand-written`
**upstream** — `lifecycle.ts` (§16): `gracefulShutdown` memoises `shutdownOnce`, which sets
`stopped = true`, clears the interval, removes the abort listener, **awaits `this.activeHealthCheck`**,
nulls it, nulls the three callbacks, then `await manager.closeAll()` when it exists.
**behavior** — shutting down while a health check is mid-`connect` does not race a `closeAll` against a
just-opened connection, which would leave an orphaned child process.
**cyrup** — `OnceCell<Shared<BoxFuture<()>>>`; the `await this.activeHealthCheck` becomes joining the
health-check task's `JoinHandle` after cancelling its token. Dropping that join is the classic way to
leak an MCP child process on quit. rmcp's `TokioChildProcess::graceful_shutdown` (close-then-kill with
a 3 s grace) is the per-connection half; this is the manager-level half.
**verify** — unit: start a health check whose `connect` takes 200 ms, call `graceful_shutdown` at 50 ms,
assert `close_all` is observed **after** the connect completed and that no connection remains.

**MCP-036 — `syncDirectTools`: the fingerprint diff, the re-activation path, and the renderer declaration** · medium · M · `hand-written`
**upstream** — `index.ts`'s `directToolFingerprint`, `registerDirectTool`, `syncDirectTools` and
`syncToolSurface` (§20), the last of which **re-reads the cache from disk** and notifies
`` `MCP: direct tools refreshed (+${a}, ~${u}, -${d})` `` when anything changed and `ctx?.hasUI`.
**behavior** — a reconnect that discovers an unchanged tool list re-registers **nothing**, so the system
prompt bytes are identical and the provider's prompt cache is not invalidated; a tool that reappears
after being deactivated is put back into the active set.
**cyrup** — the fingerprint diff, the two maps and the notification all port directly. `label`,
`promptSnippet` (via `truncateAtWord(desc, 100)`) and `normalizeDirectToolInputSchema` map onto
`cyrup_core::Tool::{label, prompt_snippet, parameters}`, and `renderShell` onto
`Tool::render_kind() -> ToolRenderKind::{SelfRendered, Default}`. The `JSON.stringify` fingerprint
becomes a deterministic serialisation — an explicit field-ordered `serde_json::to_string` over a
struct, not a `BTreeMap` (which would sort); either is correct as long as it is stable, because the
string is never persisted. **The renderer seam:** upstream passes `renderCall`/`renderResult` as
per-tool arguments; cyrup splits them into `InitApi::register_tool_renderer(tool_name)` (declare) plus
`NativeExtension::{render_call, render_result}` (serve, keyed by name). So every direct tool needs a
matching `register_tool_renderer` at `init` — and a tool registered mid-session under MCP-037 has no
way to declare its renderer, so whatever HA-1 adds must cover renderers too or the loss must be
recorded. The *registration call itself* for a mid-session tool is MCP-037.
**verify** — unit: sync twice with identical cache content ⇒ zero re-registrations; change one tool's
description ⇒ exactly one `updated`; plus a cyrup-it assertion that a direct tool's `render_call` is
invoked.

**MCP-037 — HA-1: a native extension has no handle to `ExtensionHost::register_late_tool`** · high · M · `host-addition`
**upstream** — `registerDirectTool(spec)` is called from `syncDirectTools`, reached from the
`onToolMetadataUpdated` hook and from `syncProxyTool`; pi's `api.registerTool` is legal from any live
handler.
**behavior** — connecting a new MCP server mid-session (`/mcp reconnect`, `mcp({connect:"x"})`, a lazy
first call, a `tools/list_changed` notification) makes its tools callable **in the same session**,
without a restart. The `mcp` gateway's description also refreshes mid-session when the direct-tool set
changes.
**cyrup** — **the handle is missing AND the propagation is broken in the default build.** Two
additions, not one; the second is `MCP-037a` below, and building only this one ships a subsystem that
compiles, runs and shows nothing. `ExtensionHost::register_late_tool(owner, tool)` writes into
`ExtensionRegistry` and raises the dirty flag; `ExtensionHost::refresh_tools` consumes it;
`ExtensionHost::active_tools` re-materialises;
`AgentSession::refresh_extension_tools` merges into the session's dynamic tool state and
`AgentSession::push_active_tools` calls `Agent::set_tools`, rewrites the base system prompt and pushes
to the live agent — driven from `AgentSession::next_turn_tools`, i.e. **at every turn boundary within a
live run**, with new names auto-activated. What is missing is that a native extension's only host
handles are the `Arc<dyn HostServices>` late-bound by `NativeExtension::set_host_services` and the
per-dispatch `HostCtx`, and `HostServices` exposes `active_tools`, `all_tool_names`,
`set_active_tools`, `all_tools` and `commands` — five tool-shaped verbs, all read-or-restrict, none
that *adds*. `set_active_tools` cannot activate a name that is not registered. The WASM tier reaches
the same registry through its `registration` WIT import, so this is a **two-tier asymmetry in one
verb**, not an absent capability. `register_late_tool` has zero callers anywhere in the workspace —
production or test — which is why nobody has yet discovered `MCP-037a`.
**Why it passes the host-addition test:** tool registration mutates the agent's live tool array and the
system prompt — the definition of a host concern; the extension owns the `Arc<dyn Tool>` but only the
host can install it. **Size: M, across two crates** — this unit is small on its own, but it is
inseparable from `MCP-037a` and neither half is worth building alone. Two acceptable shapes: (i) a defaulted
`NativeExtension::set_ext_host(&self, host: Weak<ExtensionHost>)` called from
`load_native_with_services` beside the existing `set_host_services` — one method, one call site,
mirroring a pattern already in the tree, with `Weak` avoiding the cycle; or (ii) two defaulted
`HostServices` methods, `register_late_tool` and `register_late_command`, implemented by
`LiveHostServices` against the ext host through a late-attach sink, the same shape
`set_overlay_sink`/`set_inject_sink` already use. **Who else benefits:** every native extension. The
precedents are already in the tree — `execute_shortcut` and `on_bus_event` were both added to
`NativeExtension` for exactly this reason, because `InitApi::register_shortcut` was a write-only
surface. **Take MCP-039 with it** (`register_late_command` for MCP prompts) and MCP-036's renderer
declaration.
**Why `high`, not `critical`** (the previous edition rated this critical): nothing is lost, nothing is
wrong, nothing crashes, no permission is bypassed. On a cold `mcp-cache.json` the first session exposes
only the `mcp` proxy tool and direct tools appear next session; upstream itself registers direct tools
*from the cache* synchronously at load, so the warm path is identical either way. The degradation is
graceful. That is scheduling information, not severity — but it is real: without HA-1,
`McpSettings.disableProxyTool` must be treated as unsupported, because the proxy tool becomes the only
in-session path to a newly connected server.
**verify** — cyrup-it: connect a second MCP server mid-session and assert its prefixed tool name appears
in the next turn's tool list.

**MCP-037a — HA-1b: `refresh_tools` discards the native tier's dirty flag in the `wasm-host` build** · critical · S · `host-addition`
**upstream** — no upstream analogue. This is a latent defect in cyrup found while specifying `MCP-037`,
not a parity gap; it is filed here because the MCP port would be its first caller and therefore its
first victim.
**behavior** — a tool registered after `init` from a live handler must reach the running agent at the
next turn boundary. Today it does not, in the build that ships.
**cyrup** — the chain breaks one link after `ExtensionHost::register_late_tool`, and only across a
`cfg` boundary, which is why reading each function on its own suggests it works. In
`crates/cyrup-ext/src/facade.rs`, `refresh_tools` early-returns unless `take_tools_dirty()` and then
returns **`materialize_guest_tools()` as its tail expression** — i.e. it reports the *materializer's*
verdict rather than the flag's. `materialize_guest_tools` has two arms: the native-only arm
(`cfg(not(feature = "wasm-host"))`) is an unconditional `Ok(true)`, while the `wasm-host` arm iterates
`ExtensionRegistry::guest_tool_entries`, which reads `guest_tools`/`guest_tool_order` — **a different
map from the `tools` map that `register_tool_inner` just wrote the native tool into**. With no new
*guest* descriptor there is nothing to materialise, so it reports `false`. `crates/cyrup-ext`'s
manifest declares `default = ["wasm-host"]`, so production takes that arm.
`AgentSession::refresh_extension_tools` (`crates/cyrup-session-svc`) then hard-gates on the bool and
returns on `Ok(false)`, never reaching `active_tools` / `merge_registered` / `push_active_tools`. And
because `take_tools_dirty` is a `swap(false)`, **the signal is destroyed rather than deferred** — a
later turn cannot recover it. Session *build* is unaffected: that path goes through
`active_tools(&base)` directly, so init-time registration works and only the in-run path is broken —
exactly the path MCP tools need. The fix is to run the guest materializer for its side effects and
return `true` on the strength of the flag that was actually raised, since the flag is set by **both**
tiers' `register_tool_inner` while the materializer only ever reports on guests.
**Why `critical`:** silent wrong output on a normal path. The tool is registered, the registry holds
it, the extension and the `/mcp` panel both believe it is live, and the model is never told it exists —
no error, no warning, no log. `refresh_extension_tools` warns on the `Err` arm ("the late tool stays
invisible") but the `Ok(false)` arm is silent. `register_late_tool`'s own doc comment asserts the
behaviour that does not happen: "Marks the tool set dirty so the next `refresh_tools`/`active_tools`
surfaces it."
**verify** — a unit test in `cyrup-ext` that calls `register_late_tool` from a live handler and asserts
the tool reaches `agent.tools()` on the next turn, run **twice**: with `--features wasm-host` and with
`--no-default-features`. Today the second passes and the first fails; both must pass. The double run is
the point — a single default-feature run is what would have caught this, and a single
`--no-default-features` run is what would hide it again.

**MCP-038 — `deactivateTools`: the optional `unregisterTool` primary path and the `setActiveTools` fallback** · medium · S · `host-verb`
**upstream** — `index.ts`'s `deactivateTools` (§20): `pi.unregisterTool` is read through a cast because
the ambient `ExtensionAPI` type does not declare it, probed per name, and whatever it did not remove
goes into `fallbackDeactivatedTools` and is filtered out of `pi.setActiveTools(nextActiveTools)` —
called **only** when the filtered list is actually shorter. Re-activation lives in `syncDirectTools`.
The package's `README.md`/`CHANGELOG.md` document it as a capability *newer pi hosts expose*, and its
own test suite exercises both branches.
**behavior** — a removed MCP server's tools stop being offered to the model, and they come back
correctly if the server returns.
**cyrup** — `HostServices::{active_tools, set_active_tools}` is the fallback path exactly, and
`active_tools()` returning `Option<Vec<String>>` with `None` for "no live backend" is the precise
analog of `getActiveToolsIfReady()` returning `undefined`, so upstream's try/catch needs no port.
`ExtensionRegistry` has no `unregister_tool` (only `clear`), so cyrup lands on upstream's
`unregisterTool === undefined` branch — **a supported upstream configuration, not a gap, and not a
host addition.** Record the accepted delta: a deactivated MCP tool stops being callable but its name
remains in the registry for the session, exactly as upstream behaves against a host without
`unregisterTool`. Keep `fallback_deactivated_tools: HashSet<String>` on the `McpExtension` so it
survives re-`init` (MCP-014). `set_active_tools`'s documented timing — effective on the next agent turn
— matches pi.
**verify** — unit against a recording fake `HostServices`: deactivate two of five tools ⇒ one
`set_active_tools` call with three names; re-add one ⇒ one call with four; plus an assertion that the
deactivated name is still present in `all_tool_names()`, documenting the delta rather than hiding it.

**MCP-039 — MCP prompts as slash commands registered after `init`** · medium · S · `host-addition`
**upstream** — `index.ts`'s `registeredPromptCommands` dedup (§21), logging
`` `MCP: prompt "${spec.originalName}" on ${spec.serverName} skipped; /${spec.commandName} is already
registered` `` at debug for a collision. Called from the cache at load **and** on every metadata update.
Commands are never unregistered.
**behavior** — each MCP prompt a connected server advertises is invokable as its own slash command, and
a prompt discovered on a mid-session reconnect becomes available immediately.
**cyrup** — the load-time half is `InitApi::register_command`, which reads the same cache file, and is
fine. The after-`init` half has no analog: `ExtensionHost` has no `register_late_command` sibling to
`register_late_tool`, and `InitApi` is `&mut` only during `init`. **This is the same seam as HA-1 and
must not be filed as a second host addition** — solve it with HA-1's shape (either the `set_ext_host`
handle plus a new `ExtensionHost::register_late_command`, or a second defaulted `HostServices`
method). If HA-1 is not built, accept the delta: a prompt from a server that was not in the cache is
not a slash command until the next session. The failure mode is bounded — upstream already de-dupes by
command name and *skips* already-registered ones — and is identical in shape to the cold-cache tool
case.
**verify** — cyrup-it: connect a server advertising a prompt mid-session and assert the slash command is
in the session's command catalog on the next read.

**MCP-040 — The `/mcp` command handler** · medium · L · `host-verb`
**upstream** — `index.ts`'s `/mcp` handler (§22): the fenced `commandCtx`, the un-timed `await
initPromise` preamble with `MCP initialization failed: ${message}` / `"MCP not initialized"`, the
argument split, and the eight-way switch with every message string tabulated in §22.
`applyDirectToolConfigChanges` is the panel callback.
**behavior** — `/mcp`, `/mcp status`, `/mcp reconnect [server]`, `/mcp tools`, `/mcp prompts`,
`/mcp setup`, `/mcp logout <server>`, `/mcp disable|enable <server>` behave identically, including the
`programmaticConfig` refusals and the `— run /reload to apply` hint.
**cyrup** — `NativeExtension::execute_command(&self, name, args, ctx)`, routed by invocation name
through `ExtensionHost::execute_native_command` from `AgentSession::try_execute_extension_command`,
with a **command-tier** `HostCtx` — so session mutation is allowed and `ctx.reload()` maps to
`HostServices::control(ControlOp::Reload)`. `Ok(Some(text))` surfaces as an info notification; a handler
needing `Warning`/`Error` calls `HostServices::notify` itself and returns `Ok(None)`, which is how the
`"error"`-kind messages in §22 are emitted. **One cancellation note:** `execute_command` receives no
cancellation *handle* — the facade races the whole handler future against the dispatch token and
returns `ExtError::Cancelled` while the handler keeps running detached. The run-scoped substitute is
`HostServices::is_run_cancelled` (documented as the deliberate CYRUP-DELTA for `AbortSignal`), and its
production wiring is the one-line item in *What does not fit cleanly*. Until it lands, use the runtime
owner's token alone and record the loss: a `/mcp reconnect` cancelled by the user's own Esc keeps
connecting in the background until the owner stops.
**verify** — cyrup-it: `/mcp disable srv` writes the project override and emits the exact
`` `Disabled server "srv" in {path} — run /reload to apply` `` notice.

**MCP-041 — HA-2: `/mcp`'s dynamic argument completions have no native path and no TUI consumer** · medium · M · `host-addition`
**upstream** — `getArgumentCompletions(prefix)` (§22) returns the eight static subcommand rows filtered
by prefix, then — for `reconnect|logout|disable|enable` — live server names from
`state.config.mcpServers` filtered by the argument prefix, formatted as
`{value: \`${subcommand} ${serverName}\`, label: serverName}`, or `null`.
**behavior** — typing `/mcp rec<TAB>` completes to `reconnect`, and `/mcp reconnect li<TAB>` completes
to the user's actual `linear-server`.
**cyrup** — the declaration half exists (`InitApi::add_autocomplete`,
`ExtensionRegistry::{add_command_autocomplete, command_autocomplete}`) and the guest dispatch half
exists (`ExtensionHost::command_completions` → the live WASM instance's `argument_completions`). The
**native dispatch arm** and the **TUI consumer** are both missing: `command_completions` is
`#[cfg(feature = "wasm-host")]` and resolves out of the live-WASM map, and `cyrup-tui`'s
`autocomplete::slash_context` completes command *names* only, rendering `argument_hint` as static
description text and never calling `command_completions`, which has zero production consumers. No
native in the workspace calls `add_autocomplete` today, so nothing has noticed.
**Why it passes the host-addition test:** the TUI owns the editor buffer and the completion popup; an
extension cannot draw into them. **Size: medium.** (a) a defaulted
`NativeExtension::argument_completions(&self, command, prefix) -> Vec<(String, String)>` plus a
non-`wasm-host` arm on `ExtensionHost::command_completions` routing through the native map the way
`execute_native_command` already does; (b) `cyrup-tui`'s `slash_context` calling it once the cursor is
past `/<name> `. The value/label pair matters — upstream returns `{value, label}` and the label carries
the description. **Who else benefits:** any extension command with arguments; this is the surface that
would make `add_autocomplete` real. **Consequence if not built:** `/mcp reconnect|logout|disable|enable
<TAB>` do not complete server names and `/mcp <TAB>` does not list the eight subcommands. The commands
all still work when typed in full — a UX fidelity gap, not a functional one.
**verify** — cyrup-it: the `/` menu offers `reconnect`/`tools`/… as completions for `/mcp `, and
`/mcp reconnect ` offers the configured server names.

**MCP-042 — The `/mcp-auth` command handler** · medium · M · `host-verb`
**upstream** — `index.ts`'s `/mcp-auth` handler (§22): the same fenced ctx; **`if (!serverName &&
!commandCtx.hasUI) return;`** silently, *before* the init-await; the same init-await preamble; no name
+ UI ⇒ the `programmaticConfig` notice or `openMcpAuthPanel(...)`; with a name ⇒ `authenticateServer`
and, on `ok`, `throwIfInactive` then `reconnectServer`.
**behavior** — `/mcp-auth linear` runs the OAuth flow and reconnects on success; `/mcp-auth` with no
argument in a headless run is a silent no-op rather than an error.
**cyrup** — same seam as MCP-040. The ordering detail that must survive is the pre-await bail. The
no-argument server picker is `HostServices::oauth_select` or the panel overlay
(`HostServices::open_overlay` + `cyrup_ext::InteractiveOverlay`), both of which belong to the OAuth and
TUI sections; this item owns only the dispatch and the bail.
**verify** — cyrup-it: `/mcp-auth` with no argument and `has_ui == false` produces no output, no error,
and no initialization wait.

**MCP-043 — The `mcp` gateway tool: registration, the init wait, and the dispatch order** · high · L · `hand-written`
**upstream** — `index.ts`'s `INIT_WAIT_TIMEOUT_MS = 30_000`, the `unique symbol` sentinel,
`awaitWithTimeout` with `timer.unref?.()` and a `finally { clearTimeout(timer) }`, `registerProxyTool`
(the twelve-property schema and the execute body) and `syncProxyTool`'s registration gate (§20, §23).
The dispatch order, first match wins, is the table in §23; the args pre-parse runs **before** the init
wait.
**behavior** — one tool named `mcp` covers status, search, describe, instructions, connect, auth and
single tool calls; while initialization is still running it returns a structured `init_timeout` after
30 s instead of hanging the turn.
**cyrup** — `cyrup_core::Tool::parameters` returns raw JSON Schema, so upstream's whole TypeBox shim
evaporates; `renderShell` maps to `Tool::render_kind()`; `renderCall`/`renderResult` need
`InitApi::register_tool_renderer("mcp")` plus `NativeExtension::{render_call, render_result}`.
`awaitWithTimeout` becomes `tokio::time::timeout(Duration::from_secs(30), fut)`. The three failure
shapes (`init_timeout` with `timeoutMs: 30000`, `init_failed` with `message`, `not_initialized`) must
be reproduced exactly in the result `details` because they are asserted upstream and read by the model.
**Cut 2 lands here twice:** the `action === "ui-messages"` arm is removed, and the `action` parameter's
description drops `'ui-messages'` from its enumeration. **Cross-crate contract:** the tool name `mcp`
and the parameter names `{tool, server, connect, describe, search}` are read by
`cyrup_permission_system::manager`'s `create_mcp_permission_targets` to derive per-server permission
targets; renaming any of them silently changes which permission rules apply. The `connect` arm's
`syncToolSurface` is the primary consumer of HA-1.
**verify** — unit: an initialization that never completes yields
`details == {"error":"init_timeout","timeoutMs":30000}` after 30 virtual seconds; a table setting all
nine surviving dispatch keys simultaneously asserts `auth-start` wins.

**MCP-044 — The `mcpScript` tool** · n/a · S · `cut`
**upstream** — `index.ts` registers `mcpScript` only when `earlyConfig.settings?.scriptMode !== false`;
parameters `{code: string, timeoutMs?: number ≥ 1}`; the same init-wait preamble with `mode: "script"`
added to every failure `details`; body `runMcpScript(state, params.code, params.timeoutMs, getPiTools,
signal)` over `mcp-code.ts` + `mcp-script-worker.mjs`.
**cyrup** — **not ported.** See *Out of scope*, Cut 4. `settings.scriptMode` is not read at all; the
tool never appears; `McpToolApprovalOrigin`'s `"script"` variant disappears and `executeCall`'s
`origin?: "proxy" | "script"` parameter keeps only its `"proxy"` default. Do **not** register a stub —
the model would call it and get an error, which is worse than the tool not existing.
**verify** — unit: `mcpScript` is absent from the registered tool names for every value of
`settings.scriptMode`, including `true`.

**MCP-045 — The `tool_result` `isError` override** · medium · S · `host-verb`
**upstream** — `error-signal.ts`'s `toolErrorOverride` returning `{isError: true}` **and nothing else**
when `details.error` is exactly `"tool_error"` or `"call_failed"`; `undefined` for every other value
including `auth_required` (§24).
**behavior** — a failed MCP tool call is recorded as an error in the transcript and in any downstream
consumer, while its `content` and `details` survive intact.
**cyrup** — subscribe `EventKind::ToolResult`; from `on_event` return
`HookOutcome::Mutate(EventPatch::ToolResult { is_error: Some(true), ..Default })`. `apply_patch` sets
`is_error` only when `Some` and leaves `content`/`details` untouched when `None` — an exact match for
pi's field-by-field merge. Return `HookOutcome::Noop` for every other case, **not** an all-`None`
patch: this handler sees every tool result in the session, not just MCP ones.
**verify** — unit: `details = {"error":"tool_error"}` ⇒ `Mutate(ToolResult{is_error:Some(true), ..None})`;
`{"error":"auth_required"}` ⇒ `Noop`; `details = null` ⇒ `Noop`.

**MCP-046 — The abort call-site discipline inside the runtime** · medium · S · `hand-written`
**upstream** — `abort.ts` consumed at the two `throwIfAborted` sites in `lazyConnect`, and
`owner.throwIfInactive()` at four points in `initializeMcp`: after the startup connect pass, at the top
of **every** pass-two iteration, after the `MCP_DIRECT_TOOLS` bootstrap, and before
`startHealthChecks`.
**behavior** — every long-running MCP operation is interruptible at a defined point and reports the
interruption as the owner's reason rather than a generic error.
**cyrup** — MCP-007 supplies the primitives; this item is the audit that every one of those points is
guarded identically. rmcp contributes the per-request half for free —
`PeerRequestOptions { timeout, reset_timeout_on_progress, max_total_timeout }`,
`RequestHandle::cancel(reason)` emitting `notifications/cancelled`, and `serve_client_with_ct` binding
a connection to a token — so the discipline here is about the *adapter's* checkpoints, not the wire.
The one non-obvious call site is `lazyConnect`'s catch arm (MCP-033).
**verify** — unit: cancel the owner mid-`initialize` at each of the four `throwIfInactive` points and
assert the partially-built state is torn down.

**MCP-047 — Port `agent-plugin-loader.ts`** · critical · M · `hand-written`
**upstream** — the full ruleset in §25: the two `$schema` equality checks; the plugin-name regex with
its 1..64 length bound; four field allowlists; an unknown top-level key in `mcp.json` discarding the
whole file; bare-or-`./` commands with `resolveContainedPath` containment; `cwd` restricted to
`./…`/`${PLUGIN_ROOT}…`/`${PLUGIN_DATA}…`; `env` forbidden from defining `PLUGIN_ROOT` or
`PLUGIN_DATA`, which the loader injects itself together with `literalEnv: true`; URLs rejecting any
`${`/`$env:`/`{env:` and any userinfo or fragment, with plain `http:` allowed only for loopback;
case-insensitive header dedup validated by constructing a real `Headers`; `` `${plugin}__${server}` ``
namespacing; and warn-and-skip everywhere.
**behavior** — a third-party Agent Plugin directory can contribute MCP servers without being able to run
an arbitrary binary from outside its own directory, read an env var through interpolation, or reach a
non-loopback plaintext HTTP endpoint.
**cyrup** — direct port into `cyrup_mcp::config::agent_plugin`. Four Rust-specific care points:
1. `resolveContainedPath`: Node's `path.relative` plus the `..`/sep/absolute test must be reproduced by
   normalising `..` components **lexically** before `Path::strip_prefix`, because Rust's `Path` does not
   normalise and `strip_prefix` would accept `root/../../etc`. Do **not** resolve symlinks — upstream
   does not, so resolving them would be a stricter and therefore divergent check.
2. The plugin-name regex uses a **negative lookahead**, which the `regex` crate rejects; express it as
   `!name.contains("--") && !name.contains("..")` plus the plain character-class regex.
3. URL validation uses the workspace `url` crate. Header validation needs `HeaderName`/`HeaderValue`
   parsers — the `http` crate is **not** a dependency anywhere in this workspace; the in-tree source is
   `reqwest::header::{HeaderName, HeaderValue}`.
4. `resolvePluginPath` anchors `~` on **`std::env::var_os("HOME")`**, a different source from
   `agent-dir.ts`'s `homedir()`. Reproduce the split or record the unification as a deliberate
   behaviour change.
Cut 1 narrows the accepted `type` set to `{"stdio", "streamable-http"}`; a `type: "sse"` entry is
skipped with the existing `unsupported type` reason.
**Why `critical`:** every one of these checks is a containment boundary around third-party plugin
content. A lexical-normalisation slip in `resolveContainedPath` lets a plugin execute a binary outside
its own directory — a permission bypass under the house scale.
**verify** — unit table: a `command` of `../evil` is skipped; `cwd: "${PLUGIN_DATA}/x"` resolves under
the data dir; `env: {"PLUGIN_ROOT": "x"}` rejects the server; `url: "http://example.com"` is rejected
but `http://127.0.0.1:8080` is accepted; a plugin named `a--b` is rejected; an unknown top-level
`mcp.json` key discards every server in the file; a `command` of `./bin/../../evil` is rejected by the
lexical normaliser; and `type: "sse"` is skipped with a named reason.

**MCP-048 — Agent-directory resolution, and whether `~/.pi/agent` is a migration source** · high · S · `open-decision`
**upstream** — `agent-dir.ts` (§26): `getAgentDir()` reads `PI_CODING_AGENT_DIR` (trimmed, with `~`
expansion) defaulting to `join(homedir(), ".pi", "agent")`; `getAgentPath(...segments)`;
`getAppName()`/`getAppClientUri()` read `piConfig.{name,clientUri}` from `$PI_PACKAGE_DIR/package.json`
with every failure yielding `undefined` and `getAppName` falling back to `"pi"`.
**behavior** — every adapter-owned file (`mcp.json`, `mcp-cache.json`, `mcp-onboarding.json`,
`agent-plugin-data/`, `npx-cache.json`) lands in one directory the user can point elsewhere, and the
OAuth dynamic-client-registration payload names the host correctly for a rebranded distribution.
**cyrup** — **do not re-implement.** Take the `agent_dir` the extension is constructed with — already
`ConfigDirs::agent_dir`, resolved from `CYRUP_AGENT_DIR` → `PI_CODING_AGENT_DIR` → `<home>/.cyrup/agent`
— exactly as `cyrup_ext_subagents::subagent_extension_for_env` takes it. That settles the *resolution*.
**What is open:** cyrup's agent dir is `~/.cyrup/agent` and upstream's is `~/.pi/agent`, and two
in-tree consumers already bind to the cyrup one — `cyrup_ext_subagents::exec::mcp_direct_tools` reads
`<agent_dir>/mcp-cache.json`, and `cyrup_permission_system` resolves `<agent_dir>/mcp.json`
**independently** as its manager's global MCP config path. So a `~/.pi/agent` fallback living only
inside `cyrup-mcp` would make the permission gate enumerate a different (empty) MCP server set than the
extension actually runs — permissions too permissive or too strict, with no error. Options:
(a) read `~/.cyrup/agent` only and require migration; (b) read `~/.cyrup/agent` and fall back to
`~/.pi/agent` **in a shared resolver both crates call**; (c) add `~/.pi/agent/mcp.json` as a permanent
additional discovery source. Recommendation: **(a)**, with the one-way move handled where cyrup already
handles migrations. If (b) is chosen, the fallback must not live inside `cyrup-mcp`.
Separately, `getAppName()`/`getAppClientUri()` need a source: recommend a compile-time constant pair
(`"cyrup"` and the project URL) rather than inventing a manifest-reading mechanism — `ConfigDirs`
exposes a *package install root*, not a distribution manifest. Record it as a mechanism substitution.
**verify** — unit: with `CYRUP_AGENT_DIR` set to a `~`-prefixed path, `agent_path("mcp-cache.json")`
resolves under the expanded home; plus a cyrup-it assertion that `cyrup-mcp` and
`cyrup-permission-system` resolve the **same** `mcp.json` path for the same `agent_dir`.

**MCP-049 — Port `cli.js init` as a `cyrup mcp init` subcommand** · medium · M · `hand-written`
**upstream** — `cli.js` (§28) in full: the seven `IMPORT_PATHS` families (first existing candidate per
family), the six-row discovery table with `✓`/`-` prefixes, the JSONC read with trailing commas
allowed, the `mcpServers`/`mcp-servers` acceptance with the legacy key **deleted** from the normalised
copy, the `imports` array filtered to strings, `--dry-run` and `--discover-host-configs`, the no-op
message pair, the merge, the 2-space-indented overwrite with a trailing newline, and the trailing
output — one unconditional line plus one gated on `discoverySettingChanged`. The `install` verb prints
two errors and returns 1; an unknown verb prints `` `Unknown command: ${command}` `` plus help and
returns 1; the code becomes `process.exitCode`.
**behavior** — a user who already configured MCP in Cursor, Claude Code, Codex, opencode, Windsurf or
VS Code runs one command and their servers become visible, with a printed table showing exactly which
files were found.
**cyrup** — add `"mcp"` to the visible `SUBCOMMANDS` table in `crates/cyrup/src/subcommands.rs` and an
`mcp init [--dry-run] [--discover-host-configs]` arm in its `dispatch`. It is **user-facing**, so it
belongs in the visible table rather than the hidden pre-dispatch used by the intercom broker and the
subagent runner. JSONC reading reuses `cyrup_permission_system::jsonc`, which the `cyrup` bin already
depends on — the same parser `cyrup-permission-system` uses on `mcp.json`, so both read that file
identically by construction. The `install` verb has no cyrup analog and is not ported.
**One divergence to decide** (also listed under open decisions): upstream's `writePiConfig` is a plain
`JSON.stringify` overwrite that **destroys every comment** in the user's JSONC `mcp.json`.
`cyrup-permission-system`'s `ext_config` already establishes the merge-preserving alternative — read,
merge, write back, refuse to clobber an unparseable file, write through symlinks. Preserving comments
is the better behaviour and is a visible divergence either way.
**verify** — unit: a temp HOME with `~/.cursor/mcp.json` and `~/.codex/config.toml` present yields
`imports: ["cursor","codex"]`; `--dry-run` writes nothing; a second run reports
`"No Pi config changes needed."`; an existing `mcp-servers` key is normalised to `mcpServers` in the
written file; and the `--discover-host-configs`-only run emits exactly two trailing lines while a plain
run emits one.

---

### Out of scope

These are **decisions by the project owner**, recorded with their reasons so a later pass does not
re-file them as gaps. Each entry names what this section stops doing.

**CUT 1 — the legacy HTTP+SSE transport.** The 2024-11-05 two-endpoint shape (GET `/sse` → `endpoint`
event → POST), the `shouldFallbackToSse` 404/405/406/415 downgrade probe, and every
legacy-protocol-revision code path. **Supported transports are exactly `stdio` and `streamable HTTP`.**
*Reason:* rmcp 3.1.2 ships no SSE client transport at all — `crates/rmcp/src/transport.rs` exports
`TokioChildProcess`, `StreamableHttpClientTransport` and `UnixSocketHttpClient` and nothing else on the
client side (`client-side-sse` is only the SSE *frame parser* the streamable-HTTP client uses).
Supporting it would mean hand-writing a protocol transport, which is precisely what the dependency
decision exists to avoid. *In this section:* `agent-plugin-loader.ts`'s http translator accepts
`type ∈ {"streamable-http","sse"}`; it keeps only `"streamable-http"` and skips `"sse"` through the
existing `skipServer(..., "unsupported type")` path — **a named diagnostic, not a silent acceptance**,
because a plugin declaring `type: sse` would otherwise connect over the wrong shape.
`ServerEntry.protocolVersion` (`"legacy" | "auto" | "2026-07-28"`) is *not* about the SSE transport and
**stays** — it maps onto rmcp's `ClientLifecycleMode`.

**CUT 2 — MCP Apps / the UI extension, entirely.** *Reason:* out of scope by decision; with it go
`axum`, the local HTTP *server*, the iframe bridge and app-initiated tool calls. *In this section, the
seam falls in four places, and the surviving half must still do its job:*
- **`state.ts`** loses `uiResourceHandler`, `uiServer`, `completedUiSessions` — and, on the evidence,
  `consentManager` with them: `ConsentManager` is constructed in `init.ts` and stored on the state, but
  its only production consumer is `ui-server.ts` (reached from `ui-session.ts`), which sets
  `requireToolConsent`/`cacheToolConsent`, calls `ensureApproved` and records the iframe's decision.
  With no apps there is no caller, and `errors.ts`'s `ConsentError` becomes unreachable from this path.
  The **surviving** approval surface is `tool-approval.ts`'s local gate — the `approveTools` config, the
  session `approvedToolCalls` cache and the three-way select — which is untouched by this cut.
- **`init.ts`** drops the `UiResourceHandler`/`ConsentManager` constructions and the uiServer owner
  cleanup. The remaining cleanup LIFO is lifecycle → oauth → binaries; **it must still run in that
  order**, because closing the servers before the OAuth runtime is what lets an in-flight callback be
  refused cleanly.
- **`index.ts`'s `shutdownState`** drops the `uiServer.close(reason)` step. **The remaining sequence is
  unchanged and load-bearing:** publish the shutdown snapshot → flush the metadata cache capturing its
  error → stop the owner (or `lifecycle.gracefulShutdown()`) → rethrow the flush error in preference to
  the shutdown error.
- **The `mcp` tool** drops the `action === "ui-messages"` dispatch arm **and** the `'ui-messages'`
  token from the `action` parameter's description string. **The remaining dispatch is the same
  first-match-wins ladder** with nine arms (§23).
`McpToolApprovalOrigin`'s `"iframe"` variant disappears with the cut.

**CUT 3 — the raw unix-socket transport.** `unix-socket-transport.ts` and `ServerEntry.socket`.
*Reason:* rmcp ships `UnixSocketHttpClient`, but that is streamable-HTTP-over-a-UDS — a different wire
shape from the adapter's raw framed socket, which targets `rmcp-mux`. rmcp does not ship the adapter's
shape, and stdio plus streamable HTTP cover the field. *In this section:* nothing in these eleven files
constructs a socket transport; the consequence lands one layer down, where `createConnection`'s
"exactly one of command, url, or socket" invariant becomes "exactly one of `command` or `url`" and a
config carrying `socket` produces a named load-time diagnostic rather than a silent skip.

**CUT 4 — `mcpScript` / the JavaScript worker.** `mcp-code.ts`, `mcp-script-worker.mjs`, the
`skills/mcp-scripting` skill, the `mcpScript` registration in `index.ts`, `McpSettings.scriptMode`, and
`McpToolApprovalOrigin`'s `"script"` variant. *Reason:* out of scope by decision. **This removes the
only JS-engine question in the entire port** — no `rquickjs`, no vendored C, no `boa`, no JS-in-WASM,
and `node` is not a production dependency of `cyrup-mcp`. The remaining proxy modes cover the same
ground: `mcp({search})` → `mcp({describe})` → `mcp({tool, args})` is the same discover/inspect/call
loop, one call per turn instead of batched. *In this section:* activation step 19 disappears entirely
and `settings.scriptMode` is not read; MCP-044 is the `cut` record. The tool must **not** be registered
with a stub body — a registered tool that always errors is worse than an absent one.

**Not cuts, but nothing to port** (recorded so a later pass does not file them as gaps): a `grep` over
the upstream package finds **zero** occurrences of `roots`, `logging/setLevel` / `notifications/message`,
and `completion/complete`. The adapter implements none of them. rmcp ships all three
(`ClientHandler::list_roots` defaulting to an empty result, `Peer::set_level` +
`ClientHandler::on_logging_message`, and the `Peer::complete_*` family), and wiring them would be **new
functionality, not a port**.

---

### What does not fit cleanly

Three things. Two are host additions already named above; one is a one-line wiring gap that this
section is the first to need.

**1 · HA-1 (MCP-037 + MCP-039) — the late-registration handle.** The only load-bearing residual in this
section. Full statement in MCP-037: the registry write, the dirty flag, the refresh and the live-agent
push all exist and are driven every turn; a native extension simply has no handle to `ExtensionHost`
where a WASM guest has one. **Recommendation:** shape (i) — a defaulted
`NativeExtension::set_ext_host(&self, host: Weak<ExtensionHost>)` called from
`ExtensionHost::load_native_with_services` beside the existing `set_host_services`, plus an
`ExtensionHost::register_late_command` sibling to `register_late_tool` so MCP-039 comes with it. One
method, one call site, mirroring a pattern already in the tree, and it closes a two-tier asymmetry that
is backwards on its face. Shape (ii) — defaulted `HostServices` methods behind a late-attach ext-host
sink — is equally acceptable and keeps the one-handle-per-native story intact. **If neither is built,**
the port still works: the proxy tool is the documented single-tool path, cold-cache sessions expose it
alone, and `McpSettings.disableProxyTool` must be treated as unsupported.

**2 · HA-2 (MCP-041) — argument completions for native slash commands.** Medium, spans `cyrup-ext` and
`cyrup-tui`, and benefits every extension command with arguments. **Recommendation:** build it, but it
does not block anything — `/mcp reconnect linear-server` typed in full works today. Do not substitute a
static-completions-only half-measure that carries `CommandDescriptor.completions` into the catalog
without the dynamic arm: the valuable half here is the *server names*, which are only knowable at
runtime.

**3 · `LiveHostServices` does not override `HostServices::is_run_cancelled`.** The verb exists and its
doc names pi's `ctx.signal` explicitly, calling poll-instead-of-wake a deliberate CYRUP-DELTA — but the
production implementor does not override it, so it returns the trait default `false` forever. This is
**not** a new host surface and **not** a design gap: it is one method body on `LiveHostServices`
returning the live run token's state. It matters here because MCP-007's combined signal, MCP-033's
`lazyConnect` guards, MCP-040's `/mcp reconnect` and MCP-046's checkpoints all read the caller-side
half of the abort pair. Until it lands the port uses the runtime owner's token alone, and a user's Esc
during `/mcp reconnect` does not stop the connect — it keeps running until the owner stops. Size: S.
Who else benefits: every extension, native and guest, that reads `ctx.signal`. **This is the one item in
this section a reader would otherwise trip over,** because `is_run_cancelled()` compiles, returns
`false`, and looks correct.

Everything else the first pass filed as a cyrup-side prerequisite dissolved — see *Corrections to the
first pass*.

---

### Coverage

**Read — upstream, in full at `v2.25.0`** (via `git show v2.25.0:<path>`): `index.ts`, `init.ts`,
`agent-plugin-loader.ts`, `cli.js`, `lifecycle.ts`, `runtime-owner.ts`, `onboarding-state.ts`,
`agent-dir.ts`, `state.ts`, `abort.ts`, `error-signal.ts`.

**Read — upstream, targeted regions at `v2.25.0`**: `utils.ts` (`execOpen`/`openUrl`, `parallelLimit`,
`getConfigPathFromArgv`, `interpolateEnvVars`, `stripOscSequences`, `sanitizeTerminalText`,
`formatTerminalError`, `truncateAtWord`, `normalizeDirectToolInputSchema`, `formatMcpStatus`);
`types.ts` (`ServerEntry`'s `lifecycle`/`idleTimeout`/`directTools`/`httpTransport`/`pluginDataDir`/
`literalEnv`/`protocolVersion`/`disabled`, `isServerDisabled`); `config.ts` (`IMPORT_PATHS`,
`ConfigSourceSpec`); `consent-manager.ts` (in full, to settle its only consumer); `package.json`;
`__tests__/index-lifecycle.test.ts` (the `unregisterTool` primary/fallback pair); `README.md` and
`CHANGELOG.md` (the `unregisterTool` capability note).

**Read — cyrup, on branch `david/cyrup`, by symbol**: `cyrup-ext` — `ExtMode`, `HostCtx` (+`HostCtxRich`,
`begin_human_wait`, `require_command_tier`), `InitApi` (the complete method list), `NativeExtension`
(the complete method list incl. `set_host_services`, `is_ambient`, `decides_project_trust`,
`render_call`/`render_result`), `ExtensionHost::{load_native_with_services, load_native_inner,
load_native_body, execute_native_command, refresh_tools, register_late_tool, command_completions,
apply_extension_flag_values, bus}`, `EventKind`/`HostEvent`, `HookOutcome`/`EventPatch`/`apply_patch`,
`ExtensionRegistry::{register_tool, resolved_commands, add_command_autocomplete, command_autocomplete,
unregister_provider}`, `CommandDescriptor`, the whole `HostServices` trait (incl. `is_run_cancelled`,
`theme`, `human_interaction_lock`, `open_overlay`, `control`, `set_status`, `notify`,
`active_tools`/`all_tool_names`/`set_active_tools`/`all_tools`/`commands`), `InteractiveOverlay`,
`caps::proc::npx_resolver` (visibility). `cyrup-session-svc` — `LiveHostServices` and its **complete
`impl HostServices` method list** (establishing that `is_run_cancelled` and `theme` are *not*
overridden), the builder's native-load loop and its fatal-diagnostic containment,
`native_survives_no_extensions`/`natives_to_load`, `SessionFactory::{with_native_extension,
build_with_parent, build_from_manager}`, `AgentSession::{emit_session_start, dispose_with,
slash_command_catalog, refresh_extension_tools, next_turn_tools, push_active_tools}`,
`AgentSessionRuntime::{new_session_with, install, install_inner}`. `cyrup-config` — `ConfigDirs`,
`EnvVars`, `FileSettingsStore::with_lock`. `cyrup-core` — `CancelToken`, `Tool`, `ToolRenderKind`.
`cyrup-tui` — `SlashCommand`, `autocomplete::slash_context`, `dynamic_commands_from_catalog_gated`,
`ExtensionOverlay`. `cyrup` — the subcommand pre-dispatch table, `intercom_broker_cmd`,
`subagent_runner_cmd`, the three session-registration arms, the runtime-diagnostics gate.
`cyrup-ext-subagents` — `subagent_extension_for_env`, `impl NativeExtension`,
`exec::mcp_direct_tools` (`CACHE_VERSION`, `CACHE_MAX_AGE_MS`, `ServerCacheEntry`, `MetadataCache`, the
cache path, `is_server_cache_valid`, `compute_mcp_server_hash`). `cyrup-permission-system` —
`MCP_BASELINE_TARGETS`, `create_mcp_permission_targets`, `read_configured_mcp_server_names`,
`jsonc`, `ext_config`'s merge-preserving save.

**Read — rmcp at `rmcp-v3.1.2-7-gf713ebd`**: `crates/rmcp/src/transport.rs`'s client exports (to
confirm no SSE client transport exists), `transport/child_process.rs` (`TokioChildProcess`,
`graceful_shutdown`), `handler/client.rs` (`ClientHandler`'s method set), `service/client.rs`
(`PeerRequestOptions`, `RequestHandle::cancel`, `serve_client_with_ct`).

**Excluded — one reason per entry**

- `mcp-setup-panel.ts`, `mcp-panel.ts`, `panel-keys.ts`, `commands.ts` (the bulk) — the interactive
  panels and their command bodies; named here only where `/mcp setup` and `/mcp status` reach them.
  TUI/panels section.
- `config.ts` (discovery, merge, imports, write-back, `getMcpDiscoverySummary`,
  `writeProjectServerDisabledOverride`, `ensureCompatibilityImports`) — the config section. Read only
  `IMPORT_PATHS` and the `ConfigSourceSpec` head so `cli.js`'s seven families and six discovery rows
  could be cross-checked.
- `metadata-cache.ts` (`computeServerHash`, `isServerCacheValid`, `serialize*`, `reconstruct*`,
  `getMetadataCachePath`, `saveMetadataCache`) — the metadata-cache section. This section specifies only
  *when* those functions are called and with what options, plus the fixed on-disk contract.
- `direct-tools.ts`, `tool-registrar.ts`, `tool-metadata.ts`, `tool-result-renderer.ts`,
  `resource-tools.ts` — the tool-surface section. This section specifies the sync *protocol* around
  them and the renderer-declaration seam (MCP-036).
- `proxy-modes.ts` (the surviving nine `execute*` functions) — the proxy-modes section. This section
  specifies only the dispatch order and the argument pre-parse.
- `server-manager.ts`, `npx-resolver.ts`, `mcp-probe.ts`, `mcp-trace.ts` — transport internals. This
  section calls `manager.connect/close/closeAll/getConnection/getAllConnections/isIdle` and specifies
  the setter calls in `initializeMcp`, nothing more.
- `mcp-auth.ts`, `mcp-auth-flow.ts`, `mcp-oauth-provider.ts`, `mcp-callback-server.ts`, `oauth.ts`,
  `mcp-keyring-helper.cjs` — the OAuth section. This section specifies only
  `createOAuthRuntime(owner.signal)` / `shutdownOAuth(runtime)` / `hasPendingAuth(...)` /
  `getAuthStorageOptions(...)` as lifecycle participants.
- `prompts.ts` (`createPromptCommand`, `resolveCachedPrompts`) — the prompts section. This section
  specifies the registration/dedup protocol only.
- `mcp-status.ts` — the status-snapshot payload. Called at seven points here; its shape is not this
  section's, and its cyrup home is an in-crate `tokio::sync::watch` because no bus consumer exists.
- `tool-approval.ts`, `session-recovery.ts`, `json-schema-validator.ts`, `ts-shape.ts`,
  `mcp-output-guard.ts`, `search-ranking.ts`, `errors.ts`, `logger.ts` — no activation-path role beyond
  being imported.
- `state.ts` **gets no port unit**: it declares types only, with zero executable logic. Its content is
  specified inline in §8 and becomes a plain Rust struct under MCP-001. Filing a `struct` definition as
  work would be padding.
- `ui-server.ts`, `ui-session.ts`, `ui-resource-handler.ts`, `ui-stream-types.ts`,
  `host-html-template.ts`, `app-bridge.bundle.js`, `glimpse-ui.ts`, `consent-manager.ts`,
  `mcp-code.ts`, `mcp-script-worker.mjs`, `unix-socket-transport.ts` — **cut** (see *Out of scope*),
  not excluded. `consent-manager.ts` was read in full precisely to establish that `ui-server.ts` is its
  only production consumer.
- `package.json`, `tsconfig.json`, `vitest.config.ts`, `conformance/**`, `examples/**` — packaging and
  fixtures.
- **cyrup `crates/cyrup-ext-sdk`** — the WASM *guest* SDK. A native built-in never crosses the component
  boundary; opened only to confirm `argument_completions` lives there and therefore does not exist for
  natives.
- **The pi host's own `ExtensionAPI` type** — not present in this checkout (no `node_modules`), so the
  exact text of the "extension loading" error and which pi versions ship `unregisterTool` are reported
  as *unverified* rather than asserted.

**Corrections to the first pass**

1. **"There is no way for a native extension to register a tool after `init`" (rated `critical`) is half
   wrong, and the correct half is small.** `ExtensionHost::register_late_tool` + `refresh_tools` exist,
   work, and propagate to a live agent through `AgentSession::{refresh_extension_tools,
   next_turn_tools, push_active_tools}` at every turn boundary, auto-activating new names. Only the
   *handle* is missing. Re-filed as HA-1 at `high` with the degradation named (MCP-037).
2. **"No tool unregistration anywhere" is not a gap.** `pi.unregisterTool` is an **optional** upstream
   API the adapter probes at runtime, with a documented `setActiveTools` fallback that the package's own
   tests exercise. cyrup lands on upstream's `unregisterTool === undefined` branch — a supported
   upstream configuration. Re-verdicted `host-verb` with an accepted delta (MCP-038).
3. **"No `CancelToken` reaches a handler" overstated the gap.** Cancellation is applied by the facade
   *racing* the handler future; the run-scoped substitute is `HostServices::is_run_cancelled`, whose doc
   names pi's `ctx.signal` and calls the poll a deliberate CYRUP-DELTA. What actually survives is a
   one-method wiring gap in `LiveHostServices`, sized S — not a missing design.
4. **"No theme access of any kind" is not a prerequisite.** `ui.theme.fg("accent", …)` is cosmetic; the
   footer text goes out uncoloured through `HostServices::set_status` and the branch collapses to
   upstream's own no-theme arm. Accepted delta (MCP-032), not a host addition.
5. **"No `~/.pi/agent` semantics" is served.** `ConfigDirs::agent_dir` resolves `CYRUP_AGENT_DIR` →
   `PI_CODING_AGENT_DIR` → `<home>/.cyrup/agent`, and the extension takes the agent dir as a
   constructor argument the way `cyrup-ext-subagents` does. What remains open is only the *migration*
   question, and its real content is the permission-gate desync (MCP-048).
6. **"`state.approvalEvents` needs an extension-bus route" dissolves.** `ExtHooks::before_tool_call` +
   `cyrup-permission-system`'s existing `create_mcp_permission_targets` is the same gate, already wired
   and fail-closed; cyrup's `SharedBus` is JSON-only and deferred and structurally cannot carry
   upstream's `claim(handler)` callback. The bus event does not port and does not need to.
7. **"The MCP status snapshot needs a bus-publish verb" dissolves.** No consumer exists in cyrup;
   building the route would be a dead primitive. Keep the snapshot in-crate.
8. **"Sampling has no landing spot" dissolves.** Upstream reaches the provider layer directly
   (`complete` from `pi-ai/compat` + `ModelRegistry`); the port reaches `cyrup-provider` directly, which
   `cyrup-ext-subagents` already establishes as normal. Adding a host verb would *diverge* from
   upstream.
9. **The `--mcp-config` flag read-back is not a gap.** Upstream reads `process.argv` directly;
   `registerFlag` is only for `--help`. `std::env::args()` is the literal mechanism port (MCP-002).
10. **`npx-resolver.ts` does not need re-porting.** `cyrup_ext::caps::proc::npx_resolver` is already a
    complete port with the same cache version, TTL and force-cache timeout. It is a private `mod` inside
    `caps/proc.rs`; reuse is a one-line `pub` promotion — a visibility chore, not a host concern, and
    strictly better than copying the file.
11. **MCP-014 was framed as an open question; it is settled.** The decisive fact is ordering:
    `AgentSessionRuntime::new_session_with` builds the replacement — re-running `init()` — *before*
    `install_inner` disposes the outgoing session. Teardown in `init()` would kill generation N's MCP
    children before N's own shutdown flush, and a failed build would strand N. `init()` registers;
    `SessionShutdown` tears down; `SessionStart` rebuilds.
12. **`consent-manager.ts` was listed as another section's work; it is cut.** Its only production
    consumer is `ui-server.ts`, so Cut 2 removes it along with `state.consentManager` and makes
    `errors.ts`'s `ConsentError` unreachable from this path. The surviving approval surface is
    `tool-approval.ts`'s local gate.
13. **`sanitizeTerminalText` is not three regexes** — `stripOscSequences` is a hand-written scanner that
    also consumes an *unterminated* OSC payload to end-of-string. Retained from the first pass because it
    is correct and load-bearing; re-rated `high` rather than treated as an ordinary port (MCP-025).
14. **Every cyrup line number and commit sha is gone.** 293 line-anchored cyrup citations and 17
    commit/tag-sha references were removed and replaced with symbol-and-file references; three
    revision-provenance paragraphs, the batch/ADR scaffolding, the `depends` edges and the
    `Kind`/`Confidence` fields were dropped. Upstream `:NNN` anchors were reduced to symbol names
    throughout.
