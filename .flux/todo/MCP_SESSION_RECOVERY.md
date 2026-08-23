---
stage: aug
status: done
updated: 2026-08-22 15:09
---

# MCP-135: `withSessionRecovery` Retry Wrapper

## Description

The wrapper is absent from the crate. Ledger row: `MCP-135` · high · `hand-written` · **missing** —
[13-cyrup-mcp-STATUS.md:646](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md), which names the gap as
"the disabled/not-connected preconditions, `hadSessionId` captured before the call, the **live**
config re-read after the failure, the 401 credential-cache invalidation running **before** the
`isTerminatedSession` gate, the exactly-one retry, the `onNeedsAuth` hook".

**Order is the specification.** A version that runs the same six things in a different order passes
a happy-path test and silently never recovers — or worse, recovers but leaves a known-bad credential
in the cache so the server is stuck at `needs-auth` for the life of the process.

Its predicate half, MCP-134, **has landed**:
[`is_terminated_session`](../../crates/cyrup-mcp/src/server_manager.rs) at
`server_manager.rs:2769` with `TerminatedSessionEvidence` at `server_manager.rs:2738`. This unit is
the caller that predicate was written for, and it is the only production caller it will have until
MCP-120 lands.

## Sources this augmentation read

| Source | What it settled |
| --- | --- |
| [session-recovery.ts](../../tmp/pi-mcp-adapter/session-recovery.ts) (158 lines, present) | The real order of operations, transcribed below. |
| [13c-mcp-servers.md](../../docs/gap-analysis/13c-mcp-servers.md) §3.15 (`:675-745`) and the MCP-135 block (`:1663-1680`) | The 16-step obligation list, verbatim. |
| [13-cyrup-mcp-STATUS.md](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) `:646` | The ledger row. |
| [server_manager.rs](../../crates/cyrup-mcp/src/server_manager.rs), [proxy.rs](../../crates/cyrup-mcp/src/proxy.rs), [runtime.rs](../../crates/cyrup-mcp/src/runtime.rs), [credentials.rs](../../crates/cyrup-mcp/src/credentials.rs), [lifecycle.rs](../../crates/cyrup-mcp/src/lifecycle.rs) | Every seam the wrapper needs already exists; nothing new has to be invented below the wrapper itself. |

## The obligation list, reproduced exactly

From [13c-mcp-servers.md](../../docs/gap-analysis/13c-mcp-servers.md)`:1663-1680`:

> **MCP-135 — `withSessionRecovery` retry wrapper** · high · M · hand-written
> **upstream** — `session-recovery.ts`'s `SessionRecoveryDeps`, `withSessionRecovery` and
> `SessionRecoveryAuthRequiredError`.
> **behavior** — §3.15 steps 1–16. Exactly one retry. The live config is re-read after the failure,
> not the stale connection's snapshot. A 401 against an OAuth server invalidates the credential
> cache **regardless** of whether the error is a terminated session (it runs before the
> `isTerminatedSession` gate). A `needs-auth` result after reconnect goes through the caller's
> `onNeedsAuth` hook once, and if still `needs-auth` raises `SessionRecoveryAuthRequiredError`; any
> other non-connected status re-raises the **original** error, not a new one.
> **cyrup** — `async fn with_session_recovery<T, F, Fut>(deps, server, f) -> Result<T>` where
> `F: Fn(Arc<ServerConnection>) -> Fut` — must be `Fn`, not `FnOnce`, because it is called twice.
> `SessionRecoveryDeps { manager, config, cancel: Option<CancelToken>, on_needs_auth }`. Leave
> `StreamableHttpClientTransportConfig::reinit_on_expired_session` **off** so the two layers do not
> double-retry — see *What does not fit cleanly*.

And §3.15's step list, [13c-mcp-servers.md](../../docs/gap-analysis/13c-mcp-servers.md)`:709-724`:

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

`reinit_on_expired_session` "defaults off and stays off"
([13c-mcp-servers.md](../../docs/gap-analysis/13c-mcp-servers.md)`:1255`) — rmcp's in-transport
recovery would double-retry against step 16 and does not run the manager-level reconnect the
`onNeedsAuth` hook hangs off ([13c-mcp-servers.md](../../docs/gap-analysis/13c-mcp-servers.md)`:734-740`).

## Corrections to the step list, taken from the real TypeScript

The doc's 16 steps are faithful but **incomplete in three places**. All three are visible in
[session-recovery.ts](../../tmp/pi-mcp-adapter/session-recovery.ts) and all three are load-bearing:

1. **Step 2 has no status check.** `session-recovery.ts:101-104` is `getConnection(serverName)` then
   `if (!connection) throw`. A `needs-auth` or `closed` record passes this precondition and reaches
   `fn`. The stronger `status === "connected"` test lives in the *manager's* request path, which the
   Rust already has: `begin_request` at `server_manager.rs:2634-2657` (`server_manager.rs:2648-2650`
   is the status test). Two different checks, both upstream, both must survive — do not merge them.

2. **Step 1's disabled check reads LIVE config; `begin_request`'s reads the connection's SNAPSHOT.**
   `session-recovery.ts:98` is `isServerDisabled(deps.config.mcpServers[serverName])`, while
   `server_manager.rs:2639-2643` reads `connection.definition().is_disabled()` — deliberately, and
   the field doc at `server_manager.rs:789-793` says so: *"Deliberately a snapshot:
   `withSessionRecovery` re-reads the live config precisely because this one is stale by then."*

3. **There is a step 15b the doc omits.** `session-recovery.ts:149-155`:

   ```ts
   try {
     deps.manager.publishMetadataChanged(serverName, freshConnection, "session-reconnect");
   } catch (publicationError) { logger.debug(...); }
   throwIfAborted(deps.signal);
   return fn(freshConnection);
   ```

   The reason string `"session-reconnect"` is already reserved in-tree — the manager's own tests use
   it verbatim at `server_manager.rs:4035` and `server_manager.rs:4057`, and
   `publish_metadata_changed` is landed at `server_manager.rs:2500`. In Rust the listener is
   `Fn(&str, &str)` and cannot throw (`server_manager.rs:2496-2499`), so the `try/catch` collapses to
   an ignored `bool` return; the identity guard inside it (`Arc::ptr_eq` +
   `status() == Connected`, `server_manager.rs:2508-2513`) is the part that must be reached.

Everything else in the doc's list matches the source line for line.

## The Rust/JS divergence, stated as the bug it produces

A JS promise runs to completion regardless of who is awaiting it. A dropped Rust future runs
**nothing** — it stops at whatever await point it had reached, and every statement after that await
never executes. This crate has already paid for that twice, and both fixes are in the same file the
wrapper goes into:

* `ServerConnection::dispose` (`server_manager.rs:1003`) once set its once-only flag *before* the
  await, so a cancelled close left `disposed == true` with a live child and the drop-net declined to
  fire. The fix is `DisposeGuard` (`server_manager.rs:1016-1055`), which re-arms the flag if the
  future dies before `resource.close()` returns.
* `reconnect_inner` (`server_manager.rs:1991`) handed its shared future straight to
  `Self::race` (`server_manager.rs:1668`, i.e. `abortable`), whose cancel arm **drops** it. An owner
  stop during `/mcp reconnect` left `do_reconnect` dropped after `close_inner` had already removed
  the connection from the map: upstream ends connected, this port ended closed and not reconnected.
  The fix is the detached driver at `server_manager.rs:2074-2101` — the reconnect is spawned, and
  `race` only decides what *this caller observes*.
* The same sentence is written out at `server_manager.rs:1235-1243`: *"in node that body is a live
  promise that runs to completion, where a dropped Rust future runs nothing at all."*

**The task's original framing of the hazard is slightly off, and the correction matters.** The
danger is not the `hadSessionId` comparison — that is safe for free, because a Rust future can only
be dropped at an `.await`, and there is no await between the error arriving from `fn` and the
classification. The danger is **step 6's side effect**. `invalidateAuthEntryCache` is the *only*
eviction path for the credential cache (`credentials.rs:2180`, whose doc at `credentials.rs:2171-2179`
names this wrapper as one of exactly three call sites). If the wrapper's frame is dropped while `fn`
is in flight — which is exactly what `abortable(with_session_recovery(...), cancel)` does — step 6
never runs, the next connect reads the same known-bad credential back out of the cache, and the
server is pinned at `needs-auth` until the process restarts. Upstream cannot express this bug.

### The structure that prevents it

Two rules, one machine-checked and one reviewed:

1. **Steps 5–8 live in a synchronous `fn`, not an `async fn`.** A body with no `.await` in it has no
   cancellation point, so the compiler guarantees nothing can interleave between the capture at step
   3 and the comparison at step 7, and guarantees the 401 eviction at step 6 cannot be skipped once
   the error is in hand. `fn` rather than `async fn` is the assertion.
2. **The wrapper owns its own cancellation and is never itself wrapped.** It takes
   `cancel: &CancelToken` (required, not `Option`) and applies `throw_if_aborted`
   (`abort.rs:95`) at steps 9, 12, 13 and 15b — the four places upstream calls `throwIfAborted`. It
   must never be an argument to `abortable` (`abort.rs:111`) or a `select!` arm. The cancellation
   seam belongs **inside** `call`, which is where [proxy.rs](../../crates/cyrup-mcp/src/proxy.rs)
   already puts it: `ProxyEnv::call_tool`'s doc at `proxy.rs:1458-1460` says *"The cancellation
   wrapper belongs on **this** side"*, i.e. `abortable(peer.send_request(..), cancel)` inside the
   closure. `read_resource` is the asymmetric case and stays asymmetric — `proxy.rs:1475-1477`,
   *"**Deliberately not wrapped in `abortable`**"* — which changes nothing here, because the
   wrapper's own four `throw_if_aborted` sites are what cancel the recovery path either way.

There is a third exposure that is already closed: the reconnect at step 11 survives this frame being
dropped, because `McpServerManager::reconnect` detaches its own driver (`server_manager.rs:2074-2101`).
Do not add a second detachment on top of it.

And one Rust-only trap the task did not name: **`had_session_id` must be a plain `bool` local, never
re-read off the connection at catch time.** After step 11 the map holds a *different* `Arc`, and the
flag is per-transport — it is written by `SessionIdProbe` (`runtime.rs:964-1000`) from the wire, one
`AtomicBool` per client. A catch-time read is not merely late, it can read a different transport's
flag. The hardcoded-`true` version of this same field is the bug `runtime.rs:956-962` documents.

## Where it goes

`crates/cyrup-mcp/src/server_manager.rs`, immediately after `should_reconnect_after_refresh` ends at
`server_manager.rs:2802` and before the `// Tests` banner at `server_manager.rs:2804`, under its own
section header matching the MCP-134 one at `server_manager.rs:2703-2705`.

Not a new `session_recovery.rs`: upstream's module split does not survive the port because MCP-134's
predicate already landed *inside* this file, and every other collaborator the wrapper needs is here
too — `get_connection` (`:1467`), `reconnect` (`:1981`), `publish_metadata_changed` (`:2500`),
`server_disabled_message` (`:100`), `server_not_connected_message` (`:130`), `ServerConnection`
(`:787`), `has_session_id` (`:985`). A separate module would force `TERMINATED_400_MARKERS`' helpers
public for no gain.

## The code

Add to the imports at `server_manager.rs:86`: `use crate::config::{McpConfig, ServerEntry};`
(currently `ServerEntry` only).

```rust
// =================================================================================================
// MCP-135 — `withSessionRecovery` (`session-recovery.ts:93-158`)
// =================================================================================================

/// `onNeedsAuth?: (serverName) => Promise<ServerConnection | undefined>`
/// (`session-recovery.ts:79`).
///
/// `Ok(None)` is upstream's `undefined`, which `?? freshConnection` turns back into the connection
/// the reconnect produced. The hook may also *fail* the whole call — `recoverAuthConnection` throws
/// `SessionRecoveryAuthRequiredError` carrying its own message — which is why the error type is the
/// wrapper's own rather than [`McpError`].
pub type NeedsAuthHook<'a, E> = dyn Fn(&str) -> BoxFuture<'a, Result<Option<Arc<ServerConnection>>, SessionRecoveryError<E>>>
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
    /// `deps.signal`. Required, not `Option`: see "the wrapper owns its own cancellation".
    pub cancel: &'a CancelToken,
    /// `deps.onNeedsAuth`. `None` reproduces the optional-call `deps.onNeedsAuth?.(...)`.
    pub on_needs_auth: Option<&'a NeedsAuthHook<'a, E>>,
    /// `invalidateAuthEntryCache(serverName)` — the same seam the connect path's 401 arm uses at
    /// `runtime.rs:2679`, so the two 401 sites share one eviction primitive
    /// (`credentials.rs:2180`).
    pub auth: &'a dyn crate::runtime::HttpAuthProvider,
}

/// What the wrapper adds to whatever `call` already raises.
#[derive(Debug)]
pub enum SessionRecoveryError<E> {
    /// `SessionRecoveryAuthRequiredError` (`session-recovery.ts:68-73`) — step 14, and the hook's
    /// own failure. Maps onto [`crate::proxy::ProxyCallError::SessionRecoveryAuthRequired`]
    /// (`proxy.rs:1285`).
    AuthRequired {
        /// `error.serverName`.
        server: String,
        /// `error.authMessage`, when the raiser had one.
        auth_message: Option<String>,
    },
    /// A precondition (steps 1-2), an abort (steps 9/12/13/15b), or the reconnect at step 11.
    Manager(McpError),
    /// `call`'s own failure, propagated **unchanged** — steps 7, 8, 15, and a second failure at 16.
    Call(E),
}

impl<E> SessionRecoveryError<E> {
    /// `authMessage ?? `MCP server "${serverName}" requires OAuth authentication after reconnect.``
    /// (`session-recovery.ts:70`). Byte-exact, including the trailing period.
    #[must_use]
    pub fn auth_required_text(server: &str, auth_message: Option<&str>) -> String {
        auth_message.map_or_else(
            || format!("MCP server \"{server}\" requires OAuth authentication after reconnect."),
            str::to_string,
        )
    }
}

/// The two facts the wrapper needs out of `call`'s error, and the only two.
///
/// Upstream branches on `instanceof SdkHttpError` / `UnauthorizedError` / `ProtocolError`. Those
/// live behind rmcp's transport types, which this file deliberately does not depend on — the same
/// seam reasoning as [`TerminatedSessionEvidence`] (`server_manager.rs:2731-2737`): the caller that
/// issued the request owns the classification, the predicate owns the policy.
pub trait SessionRecoveryFailure {
    /// `err instanceof UnauthorizedError || (err instanceof SdkHttpError && err.status === 401)`
    /// (`session-recovery.ts:113`).
    fn is_unauthorized(&self) -> bool;
    /// The evidence [`is_terminated_session`] classifies (MCP-134).
    fn terminated_session_evidence(&self) -> TerminatedSessionEvidence<'_>;
}

/// Steps 5–8, as a **synchronous** function — and `fn` rather than `async fn` is the point.
///
/// A Rust future can only be dropped at an await point, so a body with no `.await` cannot be
/// interrupted between the capture at step 3 and the comparison at step 7, and step 6's eviction
/// cannot be skipped by a cancellation landing on the failure path. In JS that property is free
/// (`withSessionRecovery`'s promise runs to completion whoever abandons it); here it has to be
/// built, and this signature is what builds it. Do not make this `async` to "tidy up" a caller.
///
/// `None` means *propagate the original error* — steps 7 and 8 give the same answer for different
/// reasons and upstream re-raises the same `err` for both.
fn classify_session_failure<E: SessionRecoveryFailure>(
    deps: &SessionRecoveryDeps<'_, E>,
    server: &str,
    error: &E,
    had_session_id: bool,
) -> Option<ServerEntry> {
    // 5 — the LIVE config, resolved HERE and nowhere earlier. The connection's own
    //     `definition()` (`server_manager.rs:791`) is a snapshot and is the wrong answer.
    let definition = deps.config.mcp_servers.get(server);

    // 6 — BEFORE the gate below, and independent of its verdict: a 401 on an error that is not a
    //     terminated session still evicts. Ordering this after step 7 is the silent-failure the
    //     unit exists to prevent.
    if definition.is_some_and(crate::oauth::supports_oauth) && error.is_unauthorized() {
        deps.auth.invalidate_auth_entry_cache(server);
    }

    // 7 — the gate. `had_session_id` is the bool captured at step 3, never a fresh read.
    let terminated = is_terminated_session(&error.terminated_session_evidence(), had_session_id);
    if !terminated {
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
/// **never** be handed to [`crate::abort::abortable`] or a `select!` arm — dropping this frame
/// while `call` is in flight skips step 6 and leaves a known-bad credential cached. Put the
/// cancellation seam inside `call` (`proxy.rs:1458-1460`).
///
/// # Errors
///
/// [`SessionRecoveryError::Manager`] for the two preconditions, an abort, or a failed reconnect;
/// [`SessionRecoveryError::AuthRequired`] for step 14; [`SessionRecoveryError::Call`] carrying
/// `call`'s own error, unchanged, for steps 7, 8, 15 and a second failure at 16.
pub async fn with_session_recovery<T, E, F, Fut>(
    deps: SessionRecoveryDeps<'_, E>,
    server: &str,
    call: F,
) -> Result<T, SessionRecoveryError<E>>
where
    E: SessionRecoveryFailure,
    F: Fn(Arc<ServerConnection>) -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    // 1 — LIVE config, and before anything touches the connection map.
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

    // 2 — presence only. A `needs-auth` or `closed` record passes; the status test belongs to
    //     `begin_request` (`server_manager.rs:2648`), which is a different precondition.
    let Some(connection) = deps.manager.get_connection(server) else {
        return Err(SessionRecoveryError::Manager(McpError::Other(
            server_not_connected_message(server),
        )));
    };

    // 3 — captured BEFORE the call, into a plain `bool` in this frame. Never re-read at catch
    //     time: after step 11 the map holds a different `Arc` with a different transport flag
    //     (`runtime.rs:964-1000`).
    let had_session_id = connection.has_session_id();

    // 4 — the call. Everything from here to the `classify_session_failure` return is await-free.
    let error = match call(Arc::clone(&connection)).await {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };

    // 5, 6, 7, 8 — synchronous by construction; see `classify_session_failure`.
    let Some(definition) = classify_session_failure(&deps, server, &error, had_session_id) else {
        return Err(SessionRecoveryError::Call(error));
    };

    // 9
    throw_if_aborted(deps.cancel, None).map_err(SessionRecoveryError::Manager)?;
    // 10
    tracing::debug!(server, "MCP session for \"{server}\" expired; reconnecting");

    // 11 — one reconnect. Single-flight, identity-guarded, and it already survives this frame
    //      being dropped via its own detached driver (`server_manager.rs:2074-2101`); do not add
    //      a second detachment.
    let stale: ConnectionHandle = Arc::clone(&connection) as ConnectionHandle;
    let mut fresh = deps
        .manager
        .reconnect(server, &definition, &stale, Some(deps.cancel))
        .await
        .map_err(SessionRecoveryError::Manager)?;

    // 12
    throw_if_aborted(deps.cancel, None).map_err(SessionRecoveryError::Manager)?;

    // 13 — the hook fires on the `needs-auth` arm ONLY, and at most once. `Ok(None)` is
    //      `?? freshConnection`: keep what the reconnect produced.
    if fresh.status() == ConnectionStatus::NeedsAuth {
        if let Some(hook) = deps.on_needs_auth {
            if let Some(replacement) = hook(server).await? {
                fresh = replacement;
            }
            throw_if_aborted(deps.cancel, None).map_err(SessionRecoveryError::Manager)?;
        }
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
    //       the listener is `Fn(&str, &str)` and cannot throw (`server_manager.rs:2496-2499`).
    let _ = deps
        .manager
        .publish_metadata_changed(server, &fresh, "session-reconnect");
    throw_if_aborted(deps.cancel, None).map_err(SessionRecoveryError::Manager)?;

    // 16 — EXACTLY one retry, against the FRESH record, result returned whatever it is.
    call(fresh).await.map_err(SessionRecoveryError::Call)
}
```

## Wiring

* **`ProxyEnv::call_tool` (`proxy.rs:1465`) and `ProxyEnv::read_resource` (`proxy.rs:1478`)** are the
  production callers; both doc comments already read `withSessionRecovery(..., conn => ...)`. Their
  implementations call `with_session_recovery` with the request future as `call`, and map
  `SessionRecoveryError::AuthRequired { server, auth_message }` onto
  `ProxyCallError::SessionRecoveryAuthRequired` (`proxy.rs:1285`), which `catch_arm`
  (`proxy.rs:3711`) already renders as `auth_required` with `details.autoAuthAttempted`.
* **`on_needs_auth`** is `AuthRecovery::recover` (`proxy.rs:2953`) and nothing else — the doc at
  `proxy.rs:1462-1464` says to call it rather than re-derive the ladder, so the single-shot
  `AutoAuthLatch` (`proxy.rs:2911`) is honoured. Adapter: call `recover()`, and on `Ok(_)` return
  `deps.manager.get_connection(server)` (the ladder replaces the map entry via `close` + `connect`,
  so the status it hands back belongs to a record that must be re-fetched by name); on
  `Err(ProxyCallError::SessionRecoveryAuthRequired { server, auth_message })` return
  `SessionRecoveryError::AuthRequired` with the same fields, so the auto-auth message survives into
  `details.message`.
* **`auth`** is the `Arc<dyn HttpAuthProvider>` already installed via
  `ConnectionBuilder::with_auth_provider` (`runtime.rs:2296`); `invalidate_auth_entry_cache`
  (`runtime.rs:1898`) is its sync method, which is why step 6 can be await-free.
* **`config`** is `state.config` (`state.rs:101`), reached through `ProxyCtx::config()`
  (`proxy.rs:1664`).
* **In-flight accounting is NOT this wrapper's.** `begin_request` + `InFlightGuard`
  (`server_manager.rs:2634`, `:2689`) own `touch → incrementInFlight → … → decrementInFlight → touch`,
  and `execute_call` does its own pair at `proxy.rs:3580-3595`. Adding a third here would
  double-count; `InFlightGuard`'s by-name decrement (`server_manager.rs:2694-2699`) is already the
  piece that makes a request straddling step 11 land on the fresh record.
* **`reinit_on_expired_session` stays unset** on `StreamableHttpClientTransportConfig`
  ([13c-mcp-servers.md](../../docs/gap-analysis/13c-mcp-servers.md)`:1255`). Two layers of
  single-attempt recovery is two retries.

## Adjacent, not in scope

`ManagerSupervisor::should_reconnect_after_refresh` (`lifecycle.rs:347-352`) still returns a hardcoded
`false`, and `lifecycle.rs:1090` captures `had_session_id` for it. It is blocked on MCP-120 (no live
`Peer` to refresh from), not on this unit — but it needs the *same* typed evidence, so
`SessionRecoveryFailure` is the trait it should implement when MCP-119/MCP-120 land. Leave
`lifecycle.rs:347` alone here; do not substitute a message match, which
[session-recovery.ts](../../tmp/pi-mcp-adapter/session-recovery.ts)`:17-22` forbids by name.

## Definition of done

The wrapper exists in [server_manager.rs](../../crates/cyrup-mcp/src/server_manager.rs) between
`should_reconnect_after_refresh` and the tests banner, and all of the following are true of the
source as written:

1. The step markers `1` … `16` appear in the body in **ascending source order**, matching the block
   quoted above, with `5`–`8` inside `classify_session_failure`.
2. `classify_session_failure` is declared `fn`, not `async fn`, and its body contains no `.await`.
3. There is **no `.await` between** the `Err(error)` binding at step 4 and the
   `invalidate_auth_entry_cache` call at step 6.
4. `deps.auth.invalidate_auth_entry_cache` is called **above** the `is_terminated_session` call in
   the same function, guarded only by `supports_oauth(definition) && error.is_unauthorized()`.
5. `deps.config.mcp_servers.get(server)` appears **twice** — once at step 1, once at step 5 — and no
   `&ServerEntry` / `Arc<ServerEntry>` is bound before step 4 and reused after it.
6. `had_session_id` is a `bool` local assigned exactly once, before step 4; `has_session_id()` is
   called exactly once in the function.
7. The body contains no `loop`, `while`, or `for`; `call` is invoked exactly twice, textually, and
   the second invocation is passed `fresh`, never `connection`.
8. `on_needs_auth` is invoked inside an `if fresh.status() == ConnectionStatus::NeedsAuth` block and
   nowhere else.
9. Steps 14 and 15 are distinct arms: `NeedsAuth` yields `AuthRequired`, every other non-`Connected`
   status yields `SessionRecoveryError::Call(error)` carrying the **original** error value.
10. `throw_if_aborted(deps.cancel, ..)` appears at steps 9, 12, 13 and 15b — four sites.
11. `publish_metadata_changed(server, &fresh, "session-reconnect")` runs after step 15 and before
    step 16, with the reason string byte-exact.
12. The two precondition strings come from `server_disabled_message` (`server_manager.rs:100`) and
    `server_not_connected_message` (`server_manager.rs:130`); the auth-required default text is
    `MCP server "<name>" requires OAuth authentication after reconnect.`, produced by
    `SessionRecoveryError::auth_required_text`.
13. `with_session_recovery` is never an argument to `abortable`, `select!`, `tokio::time::timeout`,
    or any other combinator that can drop it — grep every call site.
14. `ProxyEnv::call_tool` and `ProxyEnv::read_resource` implementations route through it, with
    `AuthRecovery::recover` as `on_needs_auth`, and `SessionRecoveryError::AuthRequired` maps onto
    `ProxyCallError::SessionRecoveryAuthRequired`.
15. `reinit_on_expired_session` is still not set anywhere in the crate.
