---
stage: aug
status: done
updated: 2026-08-22 15:07
---

# MCP-119: Paginated Discovery With Capability Gating

## Where this stands

`crates/cyrup-mcp` performs **no discovery**. The record that holds the results already exists —
`ServerConnection` carries `tools` / `resources` / `prompts` / `prompt_discovery_failed` with
setters — but nothing ever writes them. The crate says so in five places:

| claim | citation |
|---|---|
| the fields exist and are explicitly left empty for this unit | [server_manager.rs:806-812](../../crates/cyrup-mcp/src/server_manager.rs) |
| `set_tools` / `set_resources` / `set_prompts(prompts, failed)` are already written | [server_manager.rs:942-947, :957-961, :971-976](../../crates/cyrup-mcp/src/server_manager.rs) |
| the factory seam cannot deliver them: `NewConnection` has no field for them | [runtime.rs:1855-1857](../../crates/cyrup-mcp/src/runtime.rs) |
| the `Peer` is unreachable from behind `Arc<dyn ConnectionResource>` | [runtime.rs:2178-2186](../../crates/cyrup-mcp/src/runtime.rs) |
| the exact source location discovery must occupy | [runtime.rs:2992-2997](../../crates/cyrup-mcp/src/runtime.rs) |

Downstream units are blocked on it by name: `refresh_tools`
([lifecycle.rs:330-332](../../crates/cyrup-mcp/src/lifecycle.rs)), `begin_request`'s missing call
half ([server_manager.rs:2626](../../crates/cyrup-mcp/src/server_manager.rs)),
`should_reconnect_after_refresh`'s second disjunct
([server_manager.rs:2792-2795](../../crates/cyrup-mcp/src/server_manager.rs)), and
`McpError::SetupFailed`'s only *upstream* producer
([13-cyrup-mcp-STATUS.md:179](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)).

Ledger row: `MCP-119 | high | rmcp | missing` at
[13-cyrup-mcp-STATUS.md:630](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) (and again at `:436`).
Unit body: [13c-mcp-servers.md:1430-1444](../../docs/gap-analysis/13c-mcp-servers.md); the ordering
spec is §3.9 at [13c-mcp-servers.md:423-445](../../docs/gap-analysis/13c-mcp-servers.md).

Upstream **is** checked out at [tmp/pi-mcp-adapter](../../tmp/pi-mcp-adapter) — `package.json`
reports `2.26.1`. Do not re-clone it.

## The obligation, reproduced exactly

Upstream order (§3.9, [13c-mcp-servers.md:425-429](../../docs/gap-analysis/13c-mcp-servers.md)),
which is [server-manager.ts:544-585](../../tmp/pi-mcp-adapter/server-manager.ts):

```
throwIfAborted → connect → attachAdapterNotificationHandlers → instructions = client.getInstructions?.()
  → build the ServerConnection (instructions present only when !== undefined) → install client.onclose
  → Promise.all([fetchAllTools, fetchAllResources, fetchAllPrompts])
  → assign tools / resources / prompts / promptDiscoveryFailed
```

### The per-list failure matrix

This is the part that gets flattened by mistake. It is **three different policies**, not one.
Source: [server-manager.ts:972-1031](../../tmp/pi-mcp-adapter/server-manager.ts) and
[13c-mcp-servers.md:430-438](../../docs/gap-analysis/13c-mcp-servers.md).

| | `fetchAllTools` (`:972`) | `fetchAllResources` (`:1010`) | `fetchAllPrompts` (`:985`) |
|---|---|---|---|
| **capability gate** | **none** — always listed | `capabilities?.resources` absent → return `[]` **with no request on the wire** | `capabilities?.prompts` absent → return `{prompts: [], failed: false}`, **no request** |
| **`try`/`catch`** | **no `catch` at all** | yes | yes |
| **abort** (`requestOptions?.signal?.aborted` at catch time) | propagates | `throwIfAborted(signal)` — re-throws the **abort**, not the list error | `throwIfAborted(signal)` — same |
| **401** (`isUnauthorizedHttpError`) | propagates | `throw error` — the original error | `throw error` |
| **any other error** | propagates | **swallow → `[]`, and NO log** | ``logger.debug(`MCP: prompts/list failed: ${message}`)`` → `{prompts: [], failed: true}` |
| **pagination** | `do { listTools(cursor) } while (nextCursor)`, `result.tools ?? []` | same over `listResources` | same over `listPrompts` |
| **request options** | the single `requestOptions` built once, before the transport | same object | same object |

Two details inside that table that a paraphrase loses:

1. The abort arm checks **the signal**, at catch time — not whether the caught error *is* an abort.
   A `resources/list` that failed with `ECONNRESET` while the signal happened to be aborted
   re-throws the **abort reason**, not `ECONNRESET`.
2. `isUnauthorizedHttpError` is
   `error instanceof UnauthorizedError || (error instanceof SdkHttpError && error.status === 401)`
   ([server-manager.ts:73-75](../../tmp/pi-mcp-adapter/server-manager.ts)) — **401 only**. A 403
   falls to the swallow arm.
3. `failed` is not cosmetic. It is what distinguishes "this server has no prompts" from "we could
   not ask", which the prompt-command layer reads
   ([13c-mcp-servers.md:436-438](../../docs/gap-analysis/13c-mcp-servers.md)).

## What rmcp already supplies

Registry source: `/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rmcp-3.1.4/`.

* **The cursor loops are written.** `Peer<RoleClient>::list_all_tools` / `list_all_prompts` /
  `list_all_resources` at `rmcp-3.1.4/src/service/client.rs:1727`, `:1746`, `:1765` are exactly
  upstream's `do…while(cursor)`: they issue `list_*(Some(PaginatedRequestParams { meta: None,
  cursor }))`, `extend` the accumulator, and stop when `next_cursor` is `None`. **Do not
  reimplement a pagination loop.**
* **Cache coherence with MCP-120 comes free through them.** `list_tools` / `list_prompts` /
  `list_resources` (`client.rs:1666`, `:1503`, `:1560`) read and write rmcp's own list-response
  cache, and rmcp invalidates that cache itself when a `*_list_changed` notification arrives
  (`client.rs:341-347`). Hand-rolling the requests would opt out of both halves.
* **Capability gate.** `Peer::peer_info()` (`rmcp-3.1.4/src/service.rs:1018`) →
  `Option<Arc<ServerPeerInfo>>`, and `ServerPeerInfo::capabilities: ServerCapabilities`
  (`rmcp-3.1.4/src/model.rs:1095-1109`) with `prompts: Option<PromptsCapability>` and
  `resources: Option<ResourcesCapability>` (`rmcp-3.1.4/src/model/capabilities.rs:223-243`).
* **401 evidence.** A failed list surfaces as `ServiceError::TransportSend(DynamicTransportError)`
  (`rmcp-3.1.4/src/service.rs:82-83`, `rmcp-3.1.4/src/transport.rs:242-247`), whose
  `error: Box<dyn Error + Send + Sync>` is the chain to walk. This crate already has both leaf
  predicates: the `AuthRequiredError` downcast and
  [`bare_unauthorized`](../../crates/cyrup-mcp/src/runtime.rs) (runtime.rs:2052-2070), which covers
  the 401 rmcp reports as `StreamableHttpError::UnexpectedServerResponse("HTTP 401 …")`.

### Two corrections to the plan's `cyrup` note

* **`RunningService::peer_info()` does not return `InitializeResult`.** In rmcp 3.1.4
  `RoleClient::PeerInfo = ServerPeerInfo` (`rmcp-3.1.4/src/service/client.rs:270`), a distinct type
  from `InitializeResult` whose `server_info` is `Option`. The field this unit needs —
  `capabilities: ServerCapabilities` — is the same, so the gate is unaffected, but the type name in
  the plan is wrong and a port that writes `InitializeResult` will not compile. Also read the peer
  off `Peer<RoleClient>`, not `RunningService`: the service is behind a
  `tokio::sync::Mutex<Option<..>>` in `McpConnection`
  ([runtime.rs:2110](../../crates/cyrup-mcp/src/runtime.rs)) while the `Peer` is a plain field
  ([runtime.rs:2112](../../crates/cyrup-mcp/src/runtime.rs)).
* **`list_all_*` cannot carry the shared `requestOptions`.** They call `send_request`, which is
  `send_request_with_option(.., PeerRequestOptions::no_options())`
  (`rmcp-3.1.4/src/service.rs:835-840`) — the per-server `requestTimeoutMs` is dropped. §3.9's "all
  three run concurrently sharing the single `requestOptions`" therefore has to be re-imposed by the
  caller. See *Named divergences*.

## Where the code goes

**Discovery lives in [runtime.rs](../../crates/cyrup-mcp/src/runtime.rs), inside
`ConnectionBuilder::post_handshake` (`runtime.rs:2981-3020`), at the marker on
`runtime.rs:2992-2997`.** Not in the manager, for a reason that is load-bearing: upstream's
discovery is inside `createConnection`'s `try`, so everything it raises reaches the shared catch and
becomes `MCP connection setup failed` when the teardown after it also fails
([runtime.rs:2933-2951](../../crates/cyrup-mcp/src/runtime.rs)). Running it in the manager after
`factory.create()` returns would put it outside that catch and leave `McpError::SetupFailed`
without its upstream producer — the exact gap
[13-cyrup-mcp-STATUS.md:179-183](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) records.

Three seams have to widen first. All three are named in the crate as this unit's work.

### Seam 1 — reach the `Peer` from behind the trait object

[server_manager.rs:510-533](../../crates/cyrup-mcp/src/server_manager.rs), add to
`trait ConnectionResource` (imports on `server_manager.rs:79` become
`use rmcp::service::{Peer, PeerRequestOptions, RoleClient};`):

```rust
    /// The live `Peer` every request goes through, when this resource owns one.
    ///
    /// `runtime.rs:2178-2186` names the two ways to expose it — a trait method or a typed field on
    /// `NewConnection` — and this is the trait method, because it also unblocks the *other*
    /// consumer: `begin_request` (`server_manager.rs:2626`) has the accounting and no call.
    ///
    /// `None` for a resource with no live client: the `needs-auth` arm's [`InertResource`]
    /// (`server_manager.rs:730`), `StdioChildConnection` (`server_manager.rs:641`, which never
    /// handed its child to rmcp), and the test doubles.
    fn peer(&self) -> Option<&Peer<RoleClient>> {
        None
    }
```

and in [runtime.rs:2195](../../crates/cyrup-mcp/src/runtime.rs), `impl ConnectionResource for McpConnection`:

```rust
    fn peer(&self) -> Option<&Peer<RoleClient>> {
        Some(&self.peer)
    }
```

The defaulted method keeps every other `impl ConnectionResource` (there are ten, all in-crate)
source-compatible.

### Seam 2 — `NewConnection` gains somewhere to put the results

[server_manager.rs:1130-1137](../../crates/cyrup-mcp/src/server_manager.rs):

```rust
pub struct NewConnection {
    pub resource: Arc<dyn ConnectionResource>,
    pub status: ConnectionStatus,
    pub credentials_invalidated: bool,
    /// `client.getInstructions?.()` — present only when the server sent one (§3.9: the field is
    /// spread into the record only when `!== undefined`).
    pub instructions: Option<String>,
    /// `Promise.all([fetchAllTools, fetchAllResources, fetchAllPrompts])`'s three results.
    pub tools: Vec<Tool>,
    pub resources: Vec<Resource>,
    pub prompts: Vec<Prompt>,
    /// `promptDiscoveryFailed` — the `prompts` capability was advertised and `prompts/list` threw.
    pub prompt_discovery_failed: bool,
}

impl NewConnection {
    /// A connection with nothing discovered — the base every existing literal updates from, so the
    /// ten in-crate construction sites do not each grow five lines.
    #[must_use]
    pub fn bare(resource: Arc<dyn ConnectionResource>, status: ConnectionStatus) -> Self {
        Self {
            resource,
            status,
            credentials_invalidated: false,
            instructions: None,
            tools: Vec::new(),
            resources: Vec::new(),
            prompts: Vec::new(),
            prompt_discovery_failed: false,
        }
    }
}
```

The ten literals (`runtime.rs:2968`, `:3001`; `server_manager.rs:2910`, `:2915`, `:2942`, `:3185`,
`:3534`, `:3624`, `:4231`, `:4619`) become
`NewConnection { credentials_invalidated: …, ..NewConnection::bare(resource, status) }`.

### Seam 3 — `ServerConnection` accepts them

`ServerConnection::new` ([server_manager.rs:830-851](../../crates/cyrup-mcp/src/server_manager.rs))
hardcodes `instructions: None` and three empty `Vec`s, and has nine *test* call sites. Leave it
alone; add a second constructor and use it at the one production site
([server_manager.rs:1826](../../crates/cyrup-mcp/src/server_manager.rs)):

```rust
    /// Build the record from what `createConnection` returned, discovery included.
    #[must_use]
    pub fn from_created(definition: Arc<ServerEntry>, created: NewConnection) -> Arc<Self> {
        let connection = Self::new(
            definition,
            created.resource,
            created.status,
            created.credentials_invalidated,
        );
        // Upstream assigns these onto the record it just built, in this order
        // (`server-manager.ts:580-583`).
        connection.set_tools(created.tools);
        connection.set_resources(created.resources);
        connection.set_prompts(created.prompts, created.prompt_discovery_failed);
        connection
    }
```

`instructions` is not behind a lock (`server_manager.rs:800`), so it has to move into
`ServerConnection::new`'s signature or become the fifth field of a private struct-literal
constructor — either is fine, but it must end up on the record: `close`'s status snapshot and the
metadata cache read it.

## The discovery code

New section in [runtime.rs](../../crates/cyrup-mcp/src/runtime.rs), immediately before
`impl ConnectionFactory for ConnectionBuilder` (`runtime.rs:2913`).

### The failure carrier and the 401 predicate

```rust
/// What one `*/list` walk can fail with.
///
/// It is not `McpError` because the 401 arm has to inspect the *original* transport error and
/// `McpError` has already flattened it to a string, and it is not `ServiceError` because that enum
/// is `#[non_exhaustive]` (`rmcp-3.1.4/src/service.rs:77-78`) so its `Timeout` variant cannot be
/// constructed outside rmcp. Same shape and same reason as [`HttpAttempt::TimedOut`]
/// (`runtime.rs:2335-2339`).
enum ListFailure {
    Service(rmcp::service::ServiceError),
    /// `requestOptions.timeout` elapsed — upstream's
    /// `SdkError(RequestTimeout, "Request timed out")`.
    TimedOut,
}

impl ListFailure {
    /// `error.message` — what upstream interpolates into the prompts debug line. Deliberately NOT
    /// `into_mcp(..).to_string()`, which `McpError::Server`'s `Display` prefixes with
    /// `"<server>: "` (`errors.rs:183-189`).
    fn message(&self) -> String {
        match self {
            Self::Service(error) => error.to_string(),
            Self::TimedOut => HANDSHAKE_TIMED_OUT.to_string(),
        }
    }

    fn into_mcp(self, server: &str) -> McpError {
        McpError::Server { server: server.to_string(), message: self.message() }
    }

    /// `isUnauthorizedHttpError(error)` (`server-manager.ts:73-75`), rooted at a `ServiceError`.
    ///
    /// **401 only, and this is why rmcp's own helper is not used.** `ClientInitializeError::
    /// auth_challenge` (`rmcp-3.1.4/src/service/client.rs:110-131`) matches `AuthRequiredError`
    /// (401) *and* `InsufficientScopeError` (403); a 403 that re-threw here would turn a
    /// permission error into a failed connect where upstream degrades `resources` to `[]`. The
    /// arms below are exactly [`unauthorized_challenge`]'s (`runtime.rs:1993-2024`) minus the
    /// challenge string, rooted at `DynamicTransportError::error` for the reason that function
    /// states: `ClientInitializeError::TransportError` generates no `source()` edge, so a walk
    /// that starts at the wrapper finds nothing.
    fn is_unauthorized(&self) -> bool {
        let Self::Service(rmcp::service::ServiceError::TransportSend(dynamic)) = self else {
            return false;
        };
        let mut source: Option<&(dyn std::error::Error + 'static)> = Some(dynamic.error.as_ref());
        while let Some(current) = source {
            if current
                .downcast_ref::<rmcp::transport::streamable_http_client::AuthRequiredError>()
                .is_some()
                || bare_unauthorized(current)
            {
                return true;
            }
            source = std::error::Error::source(current);
        }
        false
    }
}
```

### One list walk, under the shared options

```rust
/// `client.list*(cursor, requestOptions)`'s cursor loop, with the two halves of `requestOptions`
/// re-imposed around rmcp's `list_all_*`.
///
/// The outer `Err` is upstream's `throwIfAborted(signal)`; the inner `Err` is the list's own
/// failure, which each caller's own arm of the matrix then classifies.
async fn run_list<T, F>(
    list: F,
    timeout: Option<Duration>,
    signal: &CancelToken,
) -> McpResult<Result<Vec<T>, ListFailure>>
where
    F: Future<Output = Result<Vec<T>, rmcp::service::ServiceError>>,
{
    let walk = async move {
        match timeout {
            Some(limit) => match tokio::time::timeout(limit, list).await {
                Ok(result) => result.map_err(ListFailure::Service),
                Err(_elapsed) => Err(ListFailure::TimedOut),
            },
            None => list.await.map_err(ListFailure::Service),
        }
    };
    // `PeerRequestOptions` has no signal field — rmcp cancels a request by dropping its future, so
    // the `ownedSignal` half lives in this wrapper (`runtime.rs:1826-1830`).
    crate::abort::abortable(walk, signal).await
}

/// `client.getServerCapabilities?.()` (`server-manager.ts:989`, `:1012`).
///
/// `peer_info()` is `Option` only because it is `None` before the handshake; after a successful
/// `initialize` it is always `Some`. `None` therefore reads as "advertised nothing", which is the
/// same answer upstream's `?.()` gives for a client that never connected.
fn advertises(peer: &Peer<RoleClient>, has: impl Fn(&rmcp::model::ServerCapabilities) -> bool) -> bool {
    peer.peer_info().is_some_and(|info| has(&info.capabilities))
}
```

### The three fetchers — the matrix as three explicit bodies

```rust
/// `fetchAllTools` (`server-manager.ts:972-983`) — **unconditional, and every error propagates.**
///
/// There is no capability gate and no `catch`. Upstream reads `getServerCapabilities()?.tools` in
/// `refreshTools` (`server-manager.ts:363`, MCP-120) and **nowhere else**; a gate added here would
/// leave a tools-only server that omits the capability block silently toolless.
async fn fetch_all_tools(
    peer: &Peer<RoleClient>,
    server: &str,
    timeout: Option<Duration>,
    signal: &CancelToken,
) -> McpResult<Vec<Tool>> {
    run_list(peer.list_all_tools(), timeout, signal)
        .await?
        .map_err(|failure| failure.into_mcp(server))
}

/// `fetchAllResources` (`server-manager.ts:1010-1031`).
async fn fetch_all_resources(
    peer: &Peer<RoleClient>,
    server: &str,
    timeout: Option<Duration>,
    signal: &CancelToken,
) -> McpResult<Vec<Resource>> {
    // `if (!capabilities?.resources) return []` — and NOTHING goes on the wire.
    if !advertises(peer, |capabilities| capabilities.resources.is_some()) {
        return Ok(Vec::new());
    }
    match run_list(peer.list_all_resources(), timeout, signal).await? {
        Ok(resources) => Ok(resources),
        Err(failure) => {
            // `if (requestOptions?.signal?.aborted) throwIfAborted(requestOptions.signal)`.
            // The test is on the SIGNAL at catch time, not on the error, and what it raises is the
            // ABORT — not the list failure. `abortable` above covers only the case where the abort
            // won the race; this covers the case where it arrived a moment later.
            crate::abort::throw_if_aborted(signal, None)?;
            // `if (isUnauthorizedHttpError(error)) throw error`.
            if failure.is_unauthorized() {
                return Err(failure.into_mcp(server));
            }
            // "The server advertises resources but the listing failed" — swallowed, `[]`, NO LOG.
            Ok(Vec::new())
        }
    }
}

/// `fetchAllPrompts` (`server-manager.ts:985-1008`) — the only arm that logs, and the only one that
/// reports *that* it failed.
async fn fetch_all_prompts(
    peer: &Peer<RoleClient>,
    server: &str,
    timeout: Option<Duration>,
    signal: &CancelToken,
) -> McpResult<(Vec<Prompt>, bool)> {
    // `if (!capabilities?.prompts) return { prompts: [], failed: false }` — `failed` is FALSE here:
    // "the server has no prompts" is not "we could not ask".
    if !advertises(peer, |capabilities| capabilities.prompts.is_some()) {
        return Ok((Vec::new(), false));
    }
    match run_list(peer.list_all_prompts(), timeout, signal).await? {
        Ok(prompts) => Ok((prompts, false)),
        Err(failure) => {
            crate::abort::throw_if_aborted(signal, None)?;
            if failure.is_unauthorized() {
                return Err(failure.into_mcp(server));
            }
            // Byte-exact: `` logger.debug(`MCP: prompts/list failed: ${message}`) ``.
            tracing::debug!("MCP: prompts/list failed: {}", failure.message());
            Ok((Vec::new(), true))
        }
    }
}
```

### The concurrent reduce

```rust
/// `Promise.all([fetchAllTools, fetchAllResources, fetchAllPrompts])` (`server-manager.ts:577-579`).
///
/// **`join!`, never `try_join!`.** `try_join!` drops the sibling futures on the first `Err`. That
/// would cancel a `prompts/list` that was about to record `prompt_discovery_failed = true` and
/// cancel a `resources/list` that was about to degrade to `[]`, so the per-list policy would
/// survive only for whichever list happened to settle first — the failure mode
/// `13c-mcp-servers.md:1440-1441` calls out by name.
async fn discover(
    peer: &Peer<RoleClient>,
    server: &str,
    timeout: Option<Duration>,
    signal: &CancelToken,
) -> McpResult<(Vec<Tool>, Vec<Resource>, Vec<Prompt>, bool)> {
    let (tools, resources, prompts) = tokio::join!(
        fetch_all_tools(peer, server, timeout, signal),
        fetch_all_resources(peer, server, timeout, signal),
        fetch_all_prompts(peer, server, timeout, signal),
    );
    // All three have already run to completion; the `?`s only choose which surviving error wins.
    let tools = tools?;
    let resources = resources?;
    let (prompts, prompt_discovery_failed) = prompts?;
    Ok((tools, resources, prompts, prompt_discovery_failed))
}
```

### Wiring it into `post_handshake`

Replace `runtime.rs:2992-3007` — the marker comment, the lone `throw_if_aborted` and the `let Err`
early return. Everything below it (the `Promise.allSettled` cleanup and the
`McpError::SetupFailed` wrapper, `runtime.rs:3009-3019`) stays exactly as it is; discovery simply
gives it the producer it was written for.

```rust
        // §3.2: the timeout half of `buildRequestOptions(definition, requestSignal)`, computed once
        // by the manager (`server_manager.rs:1755-1757`) and reused for all three list calls.
        // `PeerRequestOptions` is not `Clone` (`rmcp-3.1.4/src/service.rs:759-768`), so what travels
        // through `CreateConnection` and into each call is the `Duration`.
        let timeout = request.request_options.as_ref().and_then(|options| options.timeout);

        let outcome = async {
            // `throwIfAborted(signal)` — the ATTEMPT signal, as before.
            crate::abort::throw_if_aborted(&request.attempt, None)?;
            let peer = resource.peer().ok_or_else(|| McpError::Server {
                server: request.name.clone(),
                message: "MCP connection has no peer to discover against".to_string(),
            })?;
            // `const instructions = client.getInstructions?.()`.
            let instructions = peer.peer_info().and_then(|info| info.instructions.clone());
            // The list calls race the REQUEST signal, not the attempt one: upstream builds
            // `requestOptions` from `requestSignal` (`server-manager.ts:471`), which is the
            // caller-plus-runtime signal without the attempt controller
            // (`server_manager.rs:1117-1121`).
            let (tools, resources, prompts, prompt_discovery_failed) =
                discover(peer, &request.name, timeout, &request.request).await?;
            Ok::<_, McpError>((instructions, tools, resources, prompts, prompt_discovery_failed))
        }
        .await;

        let error = match outcome {
            Ok((instructions, tools, resources, prompts, prompt_discovery_failed)) => {
                return Ok(NewConnection {
                    resource,
                    status: ConnectionStatus::Connected,
                    credentials_invalidated,
                    instructions,
                    tools,
                    resources,
                    prompts,
                    prompt_discovery_failed,
                });
            }
            Err(error) => error,
        };
```

## Named divergences — record these in the code, not here

1. **The shared timeout becomes per-list, not per-page.** rmcp's `list_all_*` call `send_request`,
   which is `PeerRequestOptions::no_options()` (`rmcp-3.1.4/src/service.rs:835-840`), so the only
   way to keep `requestTimeoutMs` is `tokio::time::timeout` around the whole walk. Upstream applies
   it to each page. This is **stricter, never looser** — an N-page list gets N×timeout upstream and
   1×timeout here. The alternative, hand-rolling `send_request_with_option` per page, opts out of
   rmcp's list-response cache and its `*_list_changed` invalidation (`client.rs:341-347`), which is
   what MCP-120 is specified against.
2. **Which error wins a multi-failure `join!` is deterministic here, timing-dependent upstream.**
   `Promise.all` rejects with the first rejection *in time*; the reduce above picks tools →
   resources → prompts by position. Only observable when two lists fail in the same connect with
   different errors.
3. **A 403 is not a 401.** Upstream's predicate is 401-only and so is `is_unauthorized`; rmcp's
   `auth_challenge` would also match `InsufficientScopeError`. The existing note at
   `runtime.rs:1950-1956` already records this choice for the connect path — discovery repeats it.
4. **rmcp may answer a first-page failure from a stale cache.** `list_tools` with no cursor sets
   `uses_cursor = false`, and on error returns `stale_cached_response` if one exists
   (`client.rs:1683-1698`). At connect time the cache is empty so this cannot fire for MCP-119; it
   can for MCP-120's refresh. Caching is opt-in per response (`ttl_ms` / `cache_scope`), so it only
   applies to servers that ask for it.

## Out of scope — do not let these in

* `attachAdapterNotificationHandlers` and the `client.onclose` identity guard
  ([server-manager.ts:559-575](../../tmp/pi-mcp-adapter/server-manager.ts)) are MCP-120/MCP-121.
  They sit in the same region of §3.9; only discovery belongs to this unit.
* `refreshTools` ([server-manager.ts:357-395](../../tmp/pi-mcp-adapter/server-manager.ts)) — the
  `cacheMode: "refresh"` re-list and its `tools` capability gate — is MCP-120.
* `getPrompt` / `readResource` calling through the new `peer()` is MCP-129. This unit only makes
  the peer reachable.
* MCP-133's `enrichHttpConnectionError` wrapper (`server_manager.rs:1759-1766`) stays absent.

## Definition of done

1. `ConnectionResource::peer()` exists with a `None` default and `McpConnection` returns `Some`.
2. `NewConnection` carries `instructions`, `tools`, `resources`, `prompts`,
   `prompt_discovery_failed`; `ServerConnection::from_created` writes all five onto the record; the
   manager's production call site (`server_manager.rs:1826`) uses it.
3. `fetch_all_tools` is unconditional and has no `catch`: an abort, a 401 and any other error all
   leave through `?`.
4. `fetch_all_resources` and `fetch_all_prompts` return their empty value **without issuing a
   request** when `peer_info().capabilities.{resources,prompts}` is `None`.
5. Each of the two gated fetchers implements the three arms in this order: `throw_if_aborted` on the
   request signal → `is_unauthorized` re-throw → degrade. Resources degrades to `[]` silently;
   prompts degrades to `([], true)` after
   `tracing::debug!("MCP: prompts/list failed: {message}")` with that exact prefix.
6. The three run under `tokio::join!`; `try_join!` appears nowhere in the unit.
7. Pagination is rmcp's `list_all_*`; no hand-written cursor loop is introduced.
8. Discovery runs inside `ConnectionBuilder::post_handshake`, above the existing cleanup arm, so a
   discovery failure whose `resource.close()` also fails produces `McpError::SetupFailed`.
9. `crates/cyrup-mcp` compiles with no new `#[allow]`, and the four "blocked on MCP-119" comments
   (`runtime.rs:1855-1857`, `runtime.rs:2992-2997`, `server_manager.rs:806-812`,
   `server_manager.rs:2626`) are updated or removed rather than left contradicting the code.
10. The `MCP-119` rows in
    [13-cyrup-mcp-STATUS.md](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) (`:436` and `:630`)
    move off `missing`, and the open-item bullet at `:179-183` is corrected to say `SetupFailed`
    now has its upstream producer.
