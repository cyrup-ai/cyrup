---
stage: aug
status: done
updated: 2026-08-27 06:00
---

# MCP-135: `with_session_recovery` — the one-retry session-recovery wrapper

## Objective

Land `with_session_recovery` in
[`crates/cyrup-mcp/src/server_manager.rs`](../../crates/cyrup-mcp/src/server_manager.rs), directly
below the MCP-134 predicate it was written for. **Order is the specification.** A version that runs
the same steps in a different order passes a happy-path test and either never recovers, or recovers
while leaving a known-bad credential in the cache so the server is pinned at `needs-auth` for the
life of the process.

Verified absent: `grep -rn "with_session_recovery" crates/cyrup-mcp/src` returns nothing. Every
other seam the wrapper needs is already landed — nothing below the wrapper has to be invented.

## Corrections to the previous augmentation — read this section first

The prior pass was written against a tree that has since changed, and four of its claims are now
false. Each was checked by grep, not by reading.

| Prior claim | Reality (verified 2026-08-27) |
| --- | --- |
| Cites `proxy.rs:1285`, `:1458`, `:1465`, `:1478`, `:2911`, `:2953`, `:3580`, `:3711`, `:1664` | **`crates/cyrup-mcp/src/proxy.rs` no longer exists.** It is a directory of 14 files. Every one of those line citations is dead. New homes are in the table below. |
| "`ProxyEnv::call_tool` and `ProxyEnv::read_resource` are the production callers; **their implementations** call `with_session_recovery`" | Those are **trait method declarations** ([`proxy/env.rs:274`](../../crates/cyrup-mcp/src/proxy/env.rs), `:287`). The crate has **exactly one** `impl ProxyEnv` and it is `FakeEnv` in [`proxy/testsupport.rs:91`](../../crates/cyrup-mcp/src/proxy/testsupport.rs), `#[cfg(test)]`. There is **no production implementor anywhere in the workspace**. This unit cannot wire itself to a production caller, and the previous DoD item 14 was unachievable. |
| "`reinit_on_expired_session` is still not set anywhere in the crate" (prior DoD 15), quoting 13c's *"defaults off and stays off"* | **Backwards, and the crate already got it right.** rmcp defaults it to **`true`**; [`runtime.rs:898`](../../crates/cyrup-mcp/src/runtime.rs) explicitly sets `config.reinit_on_expired_session = false`, documented at `runtime.rs:862-868` and pinned by an assertion at `runtime.rs:3394-3397` whose message reads *"rmcp defaults this ON; MCP-135 owns session recovery, not the transport"*. 13c`:1255` is wrong on this point. **Do not touch it. Do not "restore the default".** |
| "`invalidate_auth_entry_cache` … `credentials.rs:2180`" | The credential-store primitive is named [`McpAuthStore::invalidate_cache`](../../crates/cyrup-mcp/src/credentials.rs) at `credentials.rs:2180` (doc `:2171-2178`), and it has **no production caller** — only `credentials.rs:4186`, a test. The seam this wrapper uses is the *trait* method [`HttpAuthProvider::invalidate_auth_entry_cache`](../../crates/cyrup-mcp/src/runtime.rs) at `runtime.rs:1905` (not `:1898`), whose sole production call site is `runtime.rs:2686` (not `:2679`). |

Smaller drifts, corrected throughout below: `ConnectionBuilder::with_auth_provider` is
`runtime.rs:2303`; `reconnect`'s detached driver is `server_manager.rs:2059-2098`; the
dropped-future sentence is `server_manager.rs:1236-1242`; `DisposeGuard` is
`server_manager.rs:1021-1038`; `SessionIdProbe`'s hardcoded-`true` bug note is `runtime.rs:956-961`.
`server_manager.rs:4035`/`:4057` do use the string `"session-reconnect"`, but through
`queue_metadata_publication`, not `publish_metadata_changed`.

**Also stale, in the other direction:** [`13-cyrup-mcp-STATUS.md`](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)`:645`
still rules MCP-134 **missing**. It landed. The file's own header says the census is *"as of the
audit"* (2026-08-21) and is not rewritten by later work.

### Where the proxy citations moved

| Symbol | Now at |
| --- | --- |
| `ProxyCallError` / `SessionRecoveryAuthRequired` | [`proxy/env.rs:90`](../../crates/cyrup-mcp/src/proxy/env.rs) / `:94` |
| `ProxyEnv::call_tool` / `::read_resource` | `proxy/env.rs:274` / `:287` |
| `ProxyCtx::config()` (`pub(crate)`) | `proxy/env.rs:473` |
| `AutoAuthLatch` | [`proxy/call.rs:41`](../../crates/cyrup-mcp/src/proxy/call.rs) |
| `AuthRecovery` / `AuthRecovery::recover` | `proxy/call.rs:70` / `:83` |
| `execute_call`'s in-flight pair | `proxy/call.rs:710-711` and `:724-725` |
| `catch_arm`'s `auth_required` arm | `proxy/call.rs:841`, arm at `:850-858` |

`proxy/mod.rs:97-108` glob re-exports every submodule, so existing `crate::proxy::X` paths still
resolve; only `proxy.rs:NNN` *line* citations died.

## Sources this augmentation read

| Source | What it settled |
| --- | --- |
| [session-recovery.ts](../../tmp/pi-mcp-adapter/session-recovery.ts) (158 lines, `v2.26.1` = `fafae21`) | The real order of operations, transcribed below verbatim from `:93-158`. |
| [proxy-modes.ts](../../tmp/pi-mcp-adapter/proxy-modes.ts)`:1167-1189`, `:1197-1240`, `:1294-1302` | `recoverAuthConnection`, the two `withSessionRecovery` call sites, and the catch arm. |
| [13c-mcp-servers.md](../../docs/gap-analysis/13c-mcp-servers.md) §3.15 (`:675-745`), MCP-135 (`:1663-1681`), `:1245-1258` | The 16-step obligation list, and the one line (`:1255`) that is wrong. |
| [server_manager.rs](../../crates/cyrup-mcp/src/server_manager.rs), [runtime.rs](../../crates/cyrup-mcp/src/runtime.rs), [lifecycle.rs](../../crates/cyrup-mcp/src/lifecycle.rs), [config.rs](../../crates/cyrup-mcp/src/config.rs), [credentials.rs](../../crates/cyrup-mcp/src/credentials.rs), [proxy/](../../crates/cyrup-mcp/src/proxy) | Every signature quoted below was read, not inferred. |

## The upstream, transcribed

[session-recovery.ts](../../tmp/pi-mcp-adapter/session-recovery.ts)`:93-158`, with the step numbers
13c §3.15 (`:709-724`) assigns:

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
13   if fresh.status === "needs-auth" { fresh = await deps.onNeedsAuth?.(serverName) ?? fresh; throwIfAborted(signal) }
14   if fresh.status === "needs-auth"  throw new SessionRecoveryAuthRequiredError(serverName)
15   if fresh.status !== "connected"   throw err           // the ORIGINAL error, not a new one
15b  try { manager.publishMetadataChanged(serverName, fresh, "session-reconnect") } catch { log }
     throwIfAborted(deps.signal)
16   return fn(fresh)                                      // retried EXACTLY once
```

Three things the doc's list gets wrong or omits, all three load-bearing and all three visible in the
`.ts`:

1. **Step 2 has no status check.** `session-recovery.ts:101-104` is `getConnection(serverName)` then
   `if (!connection) throw`. A `needs-auth` or `closed` record passes and reaches `fn`. The stronger
   `status === "connected"` test belongs to the *manager's* request path, which the Rust already
   has: `begin_request` at `server_manager.rs:2634-2658` (the status test is `:2648-2650`). Two
   different checks, both upstream, both must survive. **Do not merge them, and do not call
   `begin_request` from the wrapper.**

2. **Step 1 reads LIVE config; `begin_request` reads the connection's SNAPSHOT.**
   `session-recovery.ts:98` is `isServerDisabled(deps.config.mcpServers[serverName])`, while
   `server_manager.rs:2639-2643` reads `connection.definition().is_disabled()` — deliberately, and
   the field doc at `server_manager.rs:788-790` says so: *"Deliberately a snapshot:
   `withSessionRecovery` re-reads the live config precisely because this one is stale by then."*

3. **Step 15b is missing from the doc's list.** `session-recovery.ts:149-155` publishes
   `"session-reconnect"` inside a `try/catch`, then aborts, then retries. In Rust the listener is
   `Fn(&str, &str)` and cannot throw (`server_manager.rs:2496-2499`), so the `try/catch` collapses
   to an ignored `bool`; the identity guard *inside* `publish_metadata_changed`
   (`Arc::ptr_eq` + `status() == Connected`, `server_manager.rs:2506-2513`) is the part that must be
   reached.

**One more, and the previous augmentation's code got it wrong:** in step 13 the `throwIfAborted`
sits **inside the `needs-auth` branch but outside the hook call**. `deps.onNeedsAuth?.(...)` on an
absent hook yields `undefined`, `?? freshConnection` restores the reconnect's answer, and the abort
check still runs. Nesting it under `if let Some(hook)` — which the prior code block did — silently
drops one of upstream's four cancellation points.

Also settled by reading `types.ts:445-447`: `isServerDisabled(undefined) === false`, so step 1's
Rust is `.is_some_and(ServerEntry::is_disabled)` and a *missing* server is not "disabled" — it falls
through to step 2's not-connected message.

## The Rust-only hazard, stated as the bug it produces

A JS promise runs to completion regardless of who awaits it. A dropped Rust future runs **nothing**
— it stops at whatever await point it reached, and every statement after that await never executes.
This file has already paid for that twice:

* `ServerConnection::dispose` (`server_manager.rs:1003`) once set its once-only flag *before* the
  await, so a cancelled close left `disposed == true` with a live child. Fixed by `DisposeGuard`
  (`server_manager.rs:1021-1038`).
* `reconnect_inner` (`server_manager.rs:1991`) handed its shared future straight to `Self::race`
  (i.e. `abortable`), whose cancel arm **drops** it, leaving `do_reconnect` dead after `close_inner`
  had already removed the connection from the map. Fixed by the detached driver at
  `server_manager.rs:2059-2098`.
* The sentence itself is at `server_manager.rs:1236-1242`: *"in node that body is a live promise
  that runs to completion, where a dropped Rust future runs nothing at all."*

**The danger here is not the `had_session_id` comparison.** That is safe for free: a Rust future can
only be dropped at an `.await`, and there is no await between the error arriving from `call` and the
classification. The danger is **step 6's side effect**. If the wrapper's frame is dropped while
`call` is in flight — which is exactly what `abortable(with_session_recovery(..), cancel)` does —
step 6 never runs, the next connect reads the same known-bad credential back out of the cache, and
the server stays at `needs-auth` until the process restarts. Upstream cannot express this bug.

### The two rules that prevent it

1. **Steps 5–8 live in a synchronous `fn`, not an `async fn`.** A body with no `.await` has no
   cancellation point, so the compiler guarantees nothing interleaves between the capture at step 3
   and the comparison at step 7, and guarantees step 6's eviction cannot be skipped once the error
   is in hand. `fn` rather than `async fn` **is** the assertion. Do not make it `async` to tidy a
   caller.
2. **The wrapper owns its own cancellation and is never itself wrapped.** It takes
   `cancel: &CancelToken` (required, not `Option`) and applies `throw_if_aborted`
   ([`abort.rs:95`](../../crates/cyrup-mcp/src/abort.rs)) at steps 9, 12, 13 and 15b — upstream's
   four `throwIfAborted` sites. It must never be an argument to `abortable` (`abort.rs:111`), a
   `select!` arm, or `tokio::time::timeout`. The cancellation seam belongs **inside** `call`, which
   is where `proxy/env.rs:265-270` already puts it: *"The cancellation wrapper belongs on **this**
   side"*. `read_resource` is the asymmetric case and stays asymmetric (`proxy/env.rs:282-286`,
   *"Deliberately not wrapped in `abortable`"*) — which changes nothing here, because the wrapper's
   own four abort checks cancel the recovery path either way.

A third exposure is already closed: the reconnect at step 11 survives this frame being dropped,
because `McpServerManager::reconnect` detaches its own driver. **Do not add a second detachment.**

And one Rust-only trap: **`had_session_id` must be a plain `bool` local, never re-read at catch
time.** After step 11 the map holds a *different* `Arc`, and the flag is per-transport — written
from the wire by `SessionIdProbe` (`runtime.rs:963-1001`), one `AtomicBool` per client. A catch-time
read is not merely late; it can read a different transport's flag. The hardcoded-`true` version of
that same field is the bug `runtime.rs:956-961` documents.

## Where it goes

[`crates/cyrup-mcp/src/server_manager.rs`](../../crates/cyrup-mcp/src/server_manager.rs), inserted
**between line 2802** (the closing `}` of `should_reconnect_after_refresh`) **and line 2804** (the
`// Tests` banner), under its own section header matching the MCP-134 one at
`server_manager.rs:2703-2705`.

Not a new `session_recovery.rs`: MCP-134's predicate already landed *inside* this file, and every
collaborator is here too — `server_disabled_message` (`:100`), `server_not_connected_message`
(`:130`), `ServerConnection` (`:787`), `ServerConnection::status` (`:866`),
`ServerConnection::has_session_id` (`:985`), `get_connection` (`:1467`), `reconnect` (`:1981`),
`publish_metadata_changed` (`:2500`), `is_terminated_session` (`:2769`),
`TerminatedSessionEvidence` (`:2738`). A separate module would force `TERMINATED_400_MARKERS`'
helpers public for no gain.

No `lib.rs` change: `server_manager` is already `pub mod`, so `crate::server_manager::with_session_recovery`
is public API and cannot trip `dead_code` while it waits for its caller.

## Imports

Two edits at the top of the file.

`server_manager.rs:86`:

```rust
use crate::config::{McpConfig, ServerEntry};
```

`server_manager.rs:89-90`:

```rust
use crate::runtime::{append_stderr_tail, build_request_options, normalize_request_timeout_ms,
    stderr_tail_detail, HttpAuthProvider};
```

`Arc`, `BoxFuture`, `CancelToken`, `McpError`, `ConnectionHandle`, `ConnectionStatus` and
`throw_if_aborted` are already imported (`:71`, `:76`, `:75`, `:87`, `:88`, `:85`).

## The code

```rust
// =================================================================================================
// MCP-135 — `withSessionRecovery` (`session-recovery.ts:93-158`)
// =================================================================================================

/// The two facts the wrapper needs out of `call`'s error, and the only two.
///
/// Upstream branches on `instanceof SdkHttpError` / `UnauthorizedError` / `ProtocolError`. Those
/// live behind rmcp's transport types, which this file deliberately does not depend on — the same
/// seam reasoning as [`TerminatedSessionEvidence`]: the caller that issued the request owns the
/// classification, the predicate owns the policy.
pub trait SessionRecoveryFailure {
    /// `err instanceof UnauthorizedError || (err instanceof SdkHttpError && err.status === 401)`
    /// (`session-recovery.ts:113`).
    fn is_unauthorized(&self) -> bool;

    /// The evidence [`is_terminated_session`] classifies (MCP-134).
    fn terminated_session_evidence(&self) -> TerminatedSessionEvidence<'_>;
}

/// `` `MCP server "${serverName}" requires OAuth authentication after reconnect.` `` —
/// `SessionRecoveryAuthRequiredError`'s default message (`session-recovery.ts:70`). Byte-exact,
/// trailing period included.
#[must_use]
pub fn session_auth_required_message(server: &str) -> String {
    format!("MCP server \"{server}\" requires OAuth authentication after reconnect.")
}

/// What the wrapper adds to whatever `call` already raises.
#[derive(Debug)]
pub enum SessionRecoveryError<E> {
    /// `SessionRecoveryAuthRequiredError` (`session-recovery.ts:68-73`) — step 14, and the hook's
    /// own failure. Maps 1:1 onto [`crate::proxy::env::ProxyCallError::SessionRecoveryAuthRequired`].
    AuthRequired {
        /// `error.serverName`.
        server: String,
        /// `error.authMessage`, when the raiser had one. `None` at step 14, which is why
        /// [`session_auth_required_message`] is the [`std::fmt::Display`] fallback.
        auth_message: Option<String>,
    },
    /// A precondition (steps 1-2), an abort (steps 9/12/13/15b), or the reconnect at step 11.
    Manager(McpError),
    /// `call`'s own failure, propagated **unchanged** — steps 7, 8, 15, and a second failure at 16.
    Call(E),
}

impl<E: std::fmt::Display> std::fmt::Display for SessionRecoveryError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AuthRequired {
                server,
                auth_message,
            } => match auth_message {
                Some(message) => formatter.write_str(message),
                None => formatter.write_str(&session_auth_required_message(server)),
            },
            Self::Manager(error) => write!(formatter, "{error}"),
            Self::Call(error) => write!(formatter, "{error}"),
        }
    }
}

/// `onNeedsAuth?: (serverName) => Promise<ServerConnection | undefined>`
/// (`session-recovery.ts:79`).
///
/// `Ok(None)` is upstream's `undefined`, which `?? freshConnection` turns back into the connection
/// the reconnect produced. The hook may also *fail* the whole call — `recoverAuthConnection` throws
/// `SessionRecoveryAuthRequiredError` carrying its own message (`proxy-modes.ts:1175`) — which is
/// why the error type is the wrapper's own rather than [`McpError`].
pub type NeedsAuthHook<'a, E> = dyn Fn(
        &str,
    )
        -> BoxFuture<'a, Result<Option<Arc<ServerConnection>>, SessionRecoveryError<E>>>
    + Send
    + Sync
    + 'a;

/// `SessionRecoveryDeps` (`session-recovery.ts:75-80`).
pub struct SessionRecoveryDeps<'a, E> {
    /// `deps.manager`.
    pub manager: &'a Arc<McpServerManager>,
    /// `deps.config` — the **live** handle. Held as a config, never as a resolved `&ServerEntry`:
    /// resolving early is step 5's whole failure mode.
    pub config: &'a McpConfig,
    /// `deps.signal`. Required, not `Option`: see this function's *Cancellation* section.
    pub cancel: &'a CancelToken,
    /// `invalidateAuthEntryCache(serverName)` — the same seam the connect path's 401 arm uses at
    /// `runtime.rs:2686`, so both 401 sites share one eviction primitive. Note the crate's default
    /// [`crate::runtime::NoStoredCredentials`] implements it as a **no-op**; a caller that passes
    /// the default gets no eviction, exactly as on the connect path.
    pub auth: &'a dyn HttpAuthProvider,
    /// `deps.onNeedsAuth`. `None` reproduces the optional call `deps.onNeedsAuth?.(...)`.
    pub on_needs_auth: Option<&'a NeedsAuthHook<'a, E>>,
}

/// Steps 5–8, as a **synchronous** function — and `fn` rather than `async fn` is the point.
///
/// A Rust future can only be dropped at an await point, so a body with no `.await` cannot be
/// interrupted between the capture at step 3 and the comparison at step 7, and step 6's eviction
/// cannot be skipped by a cancellation landing on the failure path. In JS that property is free;
/// here it has to be built, and this signature is what builds it. Do not make this `async`.
///
/// `None` means *propagate the original error* — steps 7 and 8 give the same answer for different
/// reasons and upstream re-raises the same `err` for both.
fn classify_session_failure<E: SessionRecoveryFailure>(
    deps: &SessionRecoveryDeps<'_, E>,
    server: &str,
    error: &E,
    had_session_id: bool,
) -> Option<ServerEntry> {
    // 5 — the LIVE config, resolved HERE and nowhere earlier. The connection's own `definition()`
    //     (`server_manager.rs:788-791`) is a snapshot and is the wrong answer.
    let definition = deps.config.mcp_servers.get(server);

    // 6 — BEFORE the gate below, and independent of its verdict: a 401 on an error that is NOT a
    //     terminated session still evicts. Ordering this after step 7 is the silent failure this
    //     unit exists to prevent.
    if definition.is_some_and(crate::oauth::supports_oauth) && error.is_unauthorized() {
        deps.auth.invalidate_auth_entry_cache(server);
    }

    // 7 — the gate. `had_session_id` is the bool captured at step 3, never a fresh read.
    if !is_terminated_session(&error.terminated_session_evidence(), had_session_id) {
        return None;
    }

    // 8 — removed from config since connect: nothing to reconnect to.
    definition.cloned()
}

/// `withSessionRecovery(deps, serverName, fn)` (`session-recovery.ts:93-158`) — **MCP-135**.
///
/// `call` is `Fn`, not `FnOnce`, because step 16 invokes it a second time against the *fresh*
/// record. There is no loop in this body and there must never be one: exactly one retry.
///
/// # Cancellation
///
/// This function takes the token and applies it itself, at steps 9, 12, 13 and 15b. It must
/// **never** be handed to [`crate::abort::abortable`], a `select!` arm or `tokio::time::timeout` —
/// dropping this frame while `call` is in flight skips step 6 and leaves a known-bad credential
/// cached. Put the cancellation seam inside `call` (`proxy/env.rs:265-270`).
///
/// # Errors
///
/// [`SessionRecoveryError::Manager`] for the two preconditions, an abort, or a failed reconnect;
/// [`SessionRecoveryError::AuthRequired`] for step 14; [`SessionRecoveryError::Call`] carrying
/// `call`'s own error, unchanged, for steps 7, 8, 15 and a second failure at 16.
pub async fn with_session_recovery<T, E, F, Fut>(
    deps: &SessionRecoveryDeps<'_, E>,
    server: &str,
    call: F,
) -> Result<T, SessionRecoveryError<E>>
where
    E: SessionRecoveryFailure,
    F: Fn(Arc<ServerConnection>) -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    // 1 — LIVE config, before anything touches the connection map. `isServerDisabled(undefined)` is
    //     `false` (`types.ts:445-447`), so a missing server falls through to step 2.
    if deps
        .config
        .mcp_servers
        .get(server)
        .is_some_and(ServerEntry::is_disabled)
    {
        return Err(SessionRecoveryError::Manager(McpError::Other(
            server_disabled_message(server),
        )));
    }

    // 2 — presence only. A `needs-auth` or `closed` record passes; the `status == connected` test
    //     belongs to `begin_request` (`server_manager.rs:2648`), a different precondition.
    let Some(connection) = deps.manager.get_connection(server) else {
        return Err(SessionRecoveryError::Manager(McpError::Other(
            server_not_connected_message(server),
        )));
    };

    // 3 — captured BEFORE the call, into a plain `bool` in this frame. Never re-read at catch time:
    //     after step 11 the map holds a different `Arc` with a different transport flag
    //     (`runtime.rs:963-1001`).
    let had_session_id = connection.has_session_id();

    // 4 — the call. Everything from here to `classify_session_failure`'s return is await-free.
    let error = match call(Arc::clone(&connection)).await {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };

    // 5, 6, 7, 8 — synchronous by construction; see `classify_session_failure`.
    let Some(definition) = classify_session_failure(deps, server, &error, had_session_id) else {
        return Err(SessionRecoveryError::Call(error));
    };

    // 9
    throw_if_aborted(deps.cancel, None).map_err(SessionRecoveryError::Manager)?;

    // 10
    tracing::debug!(%server, "MCP session for \"{server}\" expired; reconnecting");

    // 11 — one reconnect. Single-flight, identity-guarded, and it already survives this frame being
    //      dropped via its own detached driver (`server_manager.rs:2059-2098`). Do NOT add a second
    //      detachment on top of it.
    let stale: ConnectionHandle = connection as ConnectionHandle;
    let mut fresh = deps
        .manager
        .reconnect(server, &definition, &stale, Some(deps.cancel))
        .await
        .map_err(SessionRecoveryError::Manager)?;

    // 12
    throw_if_aborted(deps.cancel, None).map_err(SessionRecoveryError::Manager)?;

    // 13 — the hook fires on the `needs-auth` arm ONLY, at most once, and `Ok(None)` is
    //      `?? freshConnection`: keep what the reconnect produced. The abort check is inside the
    //      branch but OUTSIDE the hook call — upstream's `?.` on an absent hook still reaches
    //      `throwIfAborted` (`session-recovery.ts:137-140`).
    if fresh.status() == ConnectionStatus::NeedsAuth {
        if let Some(hook) = deps.on_needs_auth {
            if let Some(replacement) = hook(server).await? {
                fresh = replacement;
            }
        }
        throw_if_aborted(deps.cancel, None).map_err(SessionRecoveryError::Manager)?;
    }

    // 14 — still needs auth after the hook: a NEW error.
    if fresh.status() == ConnectionStatus::NeedsAuth {
        return Err(SessionRecoveryError::AuthRequired {
            server: server.to_string(),
            auth_message: None,
        });
    }

    // 15 — any OTHER non-connected status re-raises the ORIGINAL error, not a new one.
    if fresh.status() != ConnectionStatus::Connected {
        return Err(SessionRecoveryError::Call(error));
    }

    // 15b — `session-reconnect`, identity-guarded inside. Upstream's try/catch has no counterpart:
    //        the listener is `Fn(&str, &str)` and cannot throw (`server_manager.rs:2496-2499`), so
    //        the catch arm collapses to an ignored `bool`.
    let _published = deps
        .manager
        .publish_metadata_changed(server, &fresh, "session-reconnect");
    throw_if_aborted(deps.cancel, None).map_err(SessionRecoveryError::Manager)?;

    // 16 — EXACTLY one retry, against the FRESH record, result returned whatever it is.
    call(fresh).await.map_err(SessionRecoveryError::Call)
}
```

### Why each signature is what it is

* `deps: &SessionRecoveryDeps<'_, E>` — by reference, because `execute_call` builds one deps set and
  runs it against either `read_resource` or `call_tool` (`proxy/call.rs:750` / `:773`).
* `auth: &dyn HttpAuthProvider` rather than `&McpAuthStore` — `HttpAuthProvider::invalidate_auth_entry_cache`
  (`runtime.rs:1905`) is *sync*, which is what lets step 6 be await-free, and it is the same object
  the connect path's 401 arm already evicts through (`runtime.rs:2686`). It is installed via
  `ConnectionBuilder::with_auth_provider` (`runtime.rs:2303`).
* `connection as ConnectionHandle` — `Arc<ServerConnection>` upcasts into
  `Arc<dyn ServerConnectionRef>` by unsizing coercion, exactly as `lifecycle.rs:289-293` does. The
  move is safe: step 4 already cloned for `call`, and `connection` is not read after step 11.
* `McpError::Other` (`errors.rs:286`) carries the two byte-exact precondition strings from
  `server_manager.rs:100` and `:130`.
* `tracing::debug!(%server, ..)` matches the crate's field convention (`runtime.rs:1632`,
  `:1646`).

## Wiring — what is reachable today, and what is not

**The production caller does not exist yet, and this unit does not build it.** `ProxyEnv` has no
production implementor; `execute_call` (`proxy/call.rs:750`, `:773`) reaches `call_tool` /
`read_resource` only through `ctx.env`, and the only `impl` is the `#[cfg(test)]` `FakeEnv`. So the
deliverable is the wrapper itself, shaped to drop into that implementor unchanged when it lands.

When it does, the adapter is mechanical, and these are the facts it must respect:

* **`on_needs_auth` is [`AuthRecovery::recover`](../../crates/cyrup-mcp/src/proxy/call.rs) and
  nothing else** (`proxy/call.rs:83`) — `proxy/env.rs:271-273` says to call it rather than
  re-derive the ladder, so the single-shot `AutoAuthLatch` (`proxy/call.rs:41`) is honoured.
* **`recover` returns `Result<Option<ConnectionStatus>, ProxyCallError>` — a *status*, not a
  connection.** Its ladder replaces the map entry via `close` + `connect` (`proxy/call.rs:107-118`)
  and ends with `Ok(self.ctx.env.get_connection(&self.server))`. So the adapter maps
  `Ok(_) => deps.manager.get_connection(server)` (re-fetch by name, never reuse a captured `Arc`),
  `Err(ProxyCallError::SessionRecoveryAuthRequired { server, auth_message }) =>
  SessionRecoveryError::AuthRequired { server, auth_message }`, and
  `Err(ProxyCallError::Other(e)) => SessionRecoveryError::Manager(e)`.
* **`AuthRecovery`'s fields are private** (`proxy/call.rs:70-75`); it is constructed only at
  `proxy/call.rs:706-707`. An out-of-crate implementor can call `.recover()` on the `&AuthRecovery`
  it is handed and nothing more.
* **`SessionRecoveryError::AuthRequired` maps onto `ProxyCallError::SessionRecoveryAuthRequired`**
  (`proxy/env.rs:94-99`), which `catch_arm` (`proxy/call.rs:850-858`) already renders as
  `auth_required` with `details.autoAuthAttempted`. Note that arm falls back to
  `get_auth_required_message(..)` when `auth_message` is `None` — matching
  `proxy-modes.ts:1296` — so `session_auth_required_message` is the error's `Display`, not the
  rendered `details.message`.
* **`config` is `state.config`** (`state.rs:101`), reached through `ProxyCtx::config()`
  (`proxy/env.rs:473`, `pub(crate)`).
* **In-flight accounting is NOT this wrapper's.** `begin_request` + `InFlightGuard`
  (`server_manager.rs:2634`, `:2689`) own `touch → incrementInFlight → … → decrementInFlight →
  touch`, and `execute_call` does its own pair at `proxy/call.rs:710-711` / `:724-725`. A third pair
  here would double-count; `InFlightGuard`'s by-name decrement (`server_manager.rs:2694-2699`) is
  already what makes a request straddling step 11 land on the fresh record.
* **`reinit_on_expired_session` stays `false`** at `runtime.rs:898`. It is already correct; two
  layers of single-attempt recovery is two retries.

## Adjacent, not in scope

`ManagerSupervisor::should_reconnect_after_refresh` (`lifecycle.rs:347-352`) still returns a
hardcoded `false`, and `lifecycle.rs:1090` captures `had_session_id` for it. It is blocked on
MCP-120 (no live `Peer` to refresh from), not on this unit — but it needs the *same* typed evidence,
so `SessionRecoveryFailure` is the trait it should implement when MCP-119/MCP-120 land. **Leave
`lifecycle.rs:347` alone**, and do not substitute a message match, which
[session-recovery.ts](../../tmp/pi-mcp-adapter/session-recovery.ts)`:17-22` forbids by name.

Likewise out of scope: `McpAuthStore::invalidate_cache`'s missing production binding
(`credentials.rs:2180`), and the absent production `impl ProxyEnv`.

## Definition of done

The wrapper exists in [server_manager.rs](../../crates/cyrup-mcp/src/server_manager.rs) between
`should_reconnect_after_refresh` and the `// Tests` banner, `cargo clippy --workspace
--all-targets` is clean under the workspace's `deny` set (`unwrap_used`, `expect_used`, `panic`,
`indexing_slicing`, `rustdoc::broken_intra_doc_links`), `cargo nextest run --workspace` still
reports **7862 passing**, and every one of the following is true of the source as written:

1. The step markers `1` … `16` (with `15b`) appear in ascending source order, `5`–`8` inside
   `classify_session_failure`.
2. `classify_session_failure` is declared `fn`, not `async fn`, and its body contains no `.await`.
3. There is **no `.await` between** the `Err(error)` binding at step 4 and the
   `invalidate_auth_entry_cache` call at step 6.
4. `deps.auth.invalidate_auth_entry_cache` is called **above** the `is_terminated_session` call in
   the same function, guarded only by `supports_oauth(definition) && error.is_unauthorized()`.
5. `deps.config.mcp_servers.get(server)` appears **twice** — once at step 1, once at step 5 — and no
   `&ServerEntry` / `Arc<ServerEntry>` is bound before step 4 and reused after it.
6. `had_session_id` is a `bool` local assigned exactly once, before step 4; `has_session_id()` is
   called exactly once in the function.
7. The body contains no `loop`, `while` or `for`; `call` is invoked exactly twice, textually, and
   the second invocation is passed `fresh`, never `connection`.
8. `deps.on_needs_auth` is invoked inside an `if fresh.status() == ConnectionStatus::NeedsAuth`
   block and nowhere else.
9. Step 13's `throw_if_aborted` is a sibling of the `if let Some(hook)` block, **not** nested inside
   it — it runs when `on_needs_auth` is `None`.
10. Steps 14 and 15 are distinct arms: `NeedsAuth` yields `AuthRequired`, every other
    non-`Connected` status yields `SessionRecoveryError::Call(error)` carrying the **original**
    error value.
11. `throw_if_aborted(deps.cancel, None)` appears exactly **four** times — steps 9, 12, 13, 15b.
12. `publish_metadata_changed(server, &fresh, "session-reconnect")` runs after step 15 and before
    step 16, reason string byte-exact.
13. The two precondition strings come from `server_disabled_message` and
    `server_not_connected_message`; `SessionRecoveryError`'s `Display` renders the `AuthRequired`
    default as `MCP server "<name>" requires OAuth authentication after reconnect.` via
    `session_auth_required_message`.
14. `grep -rn "with_session_recovery" crates/` shows the definition and **no** call site that is an
    argument to `abortable`, `select!`, `tokio::time::timeout`, or any other combinator that can
    drop it.
15. `runtime.rs:898`'s `config.reinit_on_expired_session = false;` is **unchanged**, and
    `grep -rn "reinit_on_expired_session" crates/` still returns exactly three hits
    (`runtime.rs:863`, `:898`, `:3395`).
16. No file other than `crates/cyrup-mcp/src/server_manager.rs` is modified.
