---
stage: aug
status: done
updated: 2026-08-27 06:00
---

# MCP-119: Paginated Discovery With Capability Gating

## Read this first — the filename is misleading

"Pagination" here is **MCP list-cursor pagination** (`tools/list` → `nextCursor` → `tools/list`),
which happens on the wire against a live server. It has **nothing to do with**
[`proxy::ranking::paginate`](../../crates/cyrup-mcp/src/proxy/ranking.rs) (`ranking.rs:401-420`) and
`Page<T>` (`ranking.rs:382-393`), which slice an already-in-memory `&[T]` for the `mcp({ offset,
limit })` proxy tool. That helper is 13d's, is already implemented, and must not be touched by this
unit. Likewise [`proxy/discovery.rs`](../../crates/cyrup-mcp/src/proxy/discovery.rs) — the
`status`/`list`/`instructions`/`describe`/`search` modes of the proxy tool — is a **consumer** of
discovery results, not the producer this unit builds.

## Verified state — re-checked 2026-08-27, all citations refreshed

`crates/cyrup-mcp` still performs **no discovery**: there is no `list_all_tools`, `list_all_prompts`,
`list_all_resources`, `list_tools`, `list_prompts` or `list_resources` call anywhere in the crate
(grep is empty outside the rmcp registry). The record that holds the results exists and its writers
exist; nothing calls them.

| claim | verified citation |
|---|---|
| the five record fields exist (`instructions` + the four discovery results), and their doc says they stay empty until this unit | [server_manager.rs:804-818](../../crates/cyrup-mcp/src/server_manager.rs) |
| `set_tools` / `set_resources` / `set_prompts(prompts, failed)` are already written | [server_manager.rs:942-945, :956-959, :970-974](../../crates/cyrup-mcp/src/server_manager.rs) |
| the factory seam cannot deliver them — `NewConnection` has three fields and none of them is a list | [server_manager.rs:1129-1137](../../crates/cyrup-mcp/src/server_manager.rs) |
| `ServerConnection::new` hardcodes `instructions: None` and three empty `Vec`s | [server_manager.rs:830-850](../../crates/cyrup-mcp/src/server_manager.rs) |
| the `Peer` is not reachable through `Arc<dyn ConnectionResource>` | [runtime.rs:2184-2193](../../crates/cyrup-mcp/src/runtime.rs) and the trait at [server_manager.rs:510-534](../../crates/cyrup-mcp/src/server_manager.rs) |
| the exact source location discovery must occupy | [runtime.rs:2999-3004](../../crates/cyrup-mcp/src/runtime.rs) |
| the region banner that names discovery as absent | [runtime.rs:1862-1865](../../crates/cyrup-mcp/src/runtime.rs) |

Downstream units blocked by name: `refresh_tools`
([lifecycle.rs:324-333](../../crates/cyrup-mcp/src/lifecycle.rs)), `begin_request`'s missing call
half ([server_manager.rs:2625-2627](../../crates/cyrup-mcp/src/server_manager.rs)),
`should_reconnect_after_refresh`'s second disjunct
([server_manager.rs:2792-2795](../../crates/cyrup-mcp/src/server_manager.rs)), and
`UnbuiltConnectionFactory`'s diagnostic string
([server_manager.rs:1166-1170](../../crates/cyrup-mcp/src/server_manager.rs)).

Spec: unit body [13c-mcp-servers.md:1430-1444](../../docs/gap-analysis/13c-mcp-servers.md); ordering
spec §3.9 at [13c-mcp-servers.md:423-446](../../docs/gap-analysis/13c-mcp-servers.md). Ledger rows
`MCP-119 | missing` at [13-cyrup-mcp-STATUS.md:436](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)
and [:630](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md).

Upstream is checked out at [tmp/pi-mcp-adapter](../../tmp/pi-mcp-adapter), tag `v2.26.1` =
`fafae21ad693ada4a7c8f6248176d4ba68ee6d2e`. Verified by `git log -1`. Do not re-clone.

### Four claims the previous augmentation got wrong — corrected

1. **`McpConnection::peer()` already exists.** It is an inherent `pub fn` at
   [runtime.rs:2184-2193](../../crates/cyrup-mcp/src/runtime.rs), it is `#[must_use]`, and a live
   fixture test already calls `connection.peer().peer_info()`
   ([runtime.rs:3769-3776](../../crates/cyrup-mcp/src/runtime.rs)) — which independently confirms
   that `peer_info().instructions` is the right field for `getInstructions()`. Seam 1 is therefore
   *not* "write a `peer()` method"; it is "add the trait method and delegate to the inherent one,
   and fix the inherent one's doc, which currently asserts **Not on the `ConnectionResource`
   trait**".
2. **`ServerConnection::instructions()` has zero callers.** The previous text claimed "`close`'s
   status snapshot and the metadata cache read it". They do not — `grep -rn '\.instructions()'
   crates/` is empty. The real consumer is
   [`proxy::env::ConnectOutcome::instructions`](../../crates/cyrup-mcp/src/proxy/env.rs)
   (`env.rs:52-53`), fed by a bridge that does not exist yet. `instructions` must still land on the
   record — it is `createConnection`'s output and MCP-100's contract — but it is **write-only until
   that bridge lands**, and the code must say so rather than claim a reader it does not have.
3. **`ListFailure`'s "`ServiceError::Timeout` cannot be constructed" rationale was right for the
   wrong reason.** `ServiceError::Timeout { timeout: Duration }` does exist
   (`rmcp-3.1.4/src/service.rs:92-93`), and the enum is `#[non_exhaustive]`
   (`service.rs:78`) so it cannot be constructed out-of-crate. But the sharper fact is that rmcp
   **never produces it on this path at all**: `list_tools` calls `send_request`, which hardcodes
   `PeerRequestOptions::no_options()` (`service.rs:835-840`), so no timeout is ever armed inside
   rmcp. The local variant is not a workaround for a construction restriction; it is the only
   representation of a timeout that exists here.
4. **All runtime.rs line numbers in the previous draft were stale** (the file grew by ~7 lines on
   2026-08-27). Every citation in this document was re-read today.

## The obligation, reproduced exactly

`createConnection`'s `try` is
[server-manager.ts:543-586](../../tmp/pi-mcp-adapter/server-manager.ts):

```
543  try {
544    throwIfAborted(signal)
545-547 connect (skipped when already connected)
548    attachAdapterNotificationHandlers(name, client)        ← MCP-120/121, NOT this unit
550    const instructions = client.getInstructions?.()
551-565 build the ServerConnection record (instructions spread only when !== undefined)
566-572 client.onclose = identity-guarded status flip          ← MCP-120/121, NOT this unit
576-580 const [tools, resources, promptResult] = await Promise.all([
           fetchAllTools(client, requestOptions),
           fetchAllResources(client, requestOptions),
           fetchAllPrompts(client, requestOptions)])
581-584 connection.tools = tools; connection.resources = resources
        connection.prompts = promptResult.prompts
        connection.promptDiscoveryFailed = promptResult.failed
586    return connection
587  } catch (error) { … }
```

`requestOptions` is built **once**, before any transport exists, at
[server-manager.ts:471](../../tmp/pi-mcp-adapter/server-manager.ts), from
`buildRequestOptions(definition, requestSignal)`
([server-manager.ts:240-255](../../tmp/pi-mcp-adapter/server-manager.ts)) — the **request** signal,
not the attempt signal, plus `getResolvedRequestTimeoutMs(definition)`. The port already computes it
once in the manager and threads it through `CreateConnection::request_options`
([server_manager.rs:1123-1126](../../crates/cyrup-mcp/src/server_manager.rs)).

### The per-list failure matrix

Three different policies, not one. Source:
[server-manager.ts:972-1031](../../tmp/pi-mcp-adapter/server-manager.ts) and
[13c-mcp-servers.md:430-438](../../docs/gap-analysis/13c-mcp-servers.md). All three re-verified
against the checkout today.

| | `fetchAllTools` (`:972-983`) | `fetchAllResources` (`:1010-1031`) | `fetchAllPrompts` (`:985-1008`) |
|---|---|---|---|
| **capability gate** | **none** — always listed | `capabilities?.resources` absent → `[]` **with no request on the wire** (`:1011-1012`) | `capabilities?.prompts` absent → `{prompts: [], failed: false}`, **no request** (`:989-990`) |
| **`try`/`catch`** | **no `catch` at all** | yes (`:1015`) | yes (`:992`) |
| **abort** | propagates | `if (requestOptions?.signal?.aborted) throwIfAborted(...)` (`:1025-1027`) | same (`:1001`) |
| **401** | propagates | `if (isUnauthorizedHttpError(error)) throw error` (`:1029`) | same (`:1003`) |
| **any other error** | propagates | **swallow → `[]`, NO log** (`:1030`) | ``logger.debug(`MCP: prompts/list failed: ${message}`)`` → `{prompts: [], failed: true}` (`:1005-1006`) |
| **pagination** | `do { listTools(cursor) } while (cursor)`, `result.tools ?? []` | same over `listResources` | same over `listPrompts` |
| **request options** | the single `requestOptions` object | same object | same object |

Three details a paraphrase loses:

1. The abort arm tests **the signal**, at catch time — not whether the caught error *is* an abort. A
   `resources/list` that failed with `ECONNRESET` while the signal happened to be aborted re-throws
   the **abort reason**, not `ECONNRESET`.
2. `isUnauthorizedHttpError` is
   `error instanceof UnauthorizedError || (error instanceof SdkHttpError && error.status === 401)`
   ([server-manager.ts:73-75](../../tmp/pi-mcp-adapter/server-manager.ts)) — **401 only**. A 403
   falls to the swallow arm.
3. `failed` is not cosmetic. It is what distinguishes "this server has no prompts" from "we could
   not ask"; the port already has a consumer waiting for it at
   [proxy/env.rs:54-56](../../crates/cyrup-mcp/src/proxy/env.rs) and
   [proxy/auth.rs:363](../../crates/cyrup-mcp/src/proxy/auth.rs).

## What rmcp 3.1.4 supplies — verified in the registry

Registry source: `/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rmcp-3.1.4/`.
The crate depends on `rmcp = "3.1.2"` ([crates/cyrup-mcp/Cargo.toml:83](../../crates/cyrup-mcp/Cargo.toml))
and the lock resolves to 3.1.4.

* **The cursor loops are written.** `Peer<RoleClient>::list_all_tools` / `list_all_prompts` /
  `list_all_resources` at `service/client.rs:1727-1741`, `:1746-1760`, `:1765-1779` are exactly
  upstream's `do…while(cursor)`: `list_*(Some(PaginatedRequestParams { meta: None, cursor }))`,
  `extend`, stop when `next_cursor` is `None`. **Do not reimplement a pagination loop.** This also
  keeps the unit clear of `clippy::indexing_slicing` (workspace `deny`): there is no slicing to do.
* **Cache coherence with MCP-120 comes free.** `list_tools` / `list_prompts` / `list_resources`
  (`client.rs:1668-1725`, `:1504`, `:1554`) read and write rmcp's own list-response cache, and rmcp
  invalidates it itself on a `*_list_changed` notification
  (`client.rs:334-350`). Hand-rolling the requests opts out of both halves.
* **Capability gate.** `Peer::peer_info()` (`service.rs:1018-1020`) → `Option<Arc<R::PeerInfo>>`;
  for `RoleClient`, `PeerInfo = ServerPeerInfo` (`service/client.rs:270`), whose fields are
  `protocol_version`, `capabilities: ServerCapabilities`, `server_info: Option<Implementation>`,
  `instructions: Option<String>`, `meta` (`model.rs:1088-1109`). `ServerCapabilities` has
  `prompts: Option<PromptsCapability>` and `resources: Option<ResourcesCapability>`
  (`model/capabilities.rs:223-243`). `set_peer_info` runs immediately after a successful
  `initialize` (`service/client.rs:902`), so `peer_info()` is `Some` for every connection that
  reaches `post_handshake`.
* **`getInstructions()` is `peer_info().instructions`** — already exercised by
  [runtime.rs:3769-3776](../../crates/cyrup-mcp/src/runtime.rs).
* **401 evidence.** A failed list surfaces as `ServiceError::TransportSend(DynamicTransportError)`
  (`service.rs:82-83`, `transport.rs:239-247`), whose `error: Box<dyn Error + Send + Sync>` is the
  chain to walk. `ServiceError::TransportSend` carries **no** `#[source]` attribute, so a walk that
  starts at the `ServiceError` finds nothing — root it at `dynamic.error.as_ref()`, exactly as
  [`unauthorized_challenge`](../../crates/cyrup-mcp/src/runtime.rs) (`runtime.rs:2000-2032`) already
  does and for the reason its own comment states. Both leaf predicates already exist in-crate: the
  `AuthRequiredError` downcast and
  [`bare_unauthorized`](../../crates/cyrup-mcp/src/runtime.rs) (`runtime.rs:2058-2073`).

### Two corrections to the plan's `cyrup` note (13c:1437-1438)

* **`RunningService::peer_info()` does not return `InitializeResult`.** In rmcp 3.1.4
  `RoleClient::PeerInfo = ServerPeerInfo` — a distinct, `#[non_exhaustive]` type whose `server_info`
  is `Option` (`model.rs:1088-1109`). `capabilities: ServerCapabilities` is the same field, so the
  gate is unaffected, but a port that writes `InitializeResult` will not compile. Also read the peer
  off `Peer<RoleClient>`, **not** `RunningService`: the service is behind a
  `tokio::sync::Mutex<Option<..>>` ([runtime.rs:2117](../../crates/cyrup-mcp/src/runtime.rs)) while
  the `Peer` is a plain field ([runtime.rs:2119](../../crates/cyrup-mcp/src/runtime.rs)).
* **`list_all_*` cannot carry the shared `requestOptions`.** They call `send_request` =
  `send_request_with_option(.., PeerRequestOptions::no_options())` (`service.rs:835-840`), so the
  per-server `requestTimeoutMs` is dropped and no signal is threaded. §3.9's "all three run
  concurrently sharing the single `requestOptions`" has to be re-imposed by the caller. See *Named
  divergences*.

## Where the code goes

**Discovery lives in [runtime.rs](../../crates/cyrup-mcp/src/runtime.rs), inside
`ConnectionBuilder::post_handshake` (`runtime.rs:2988-3030`), replacing lines 2994-3013.** Not in
the manager, for a load-bearing reason: upstream's discovery is inside `createConnection`'s `try`,
so everything it raises reaches the shared catch and becomes `MCP connection setup failed` when the
teardown after it also fails ([runtime.rs:3015-3029](../../crates/cyrup-mcp/src/runtime.rs)).
Running it in the manager after `factory.create()` returns would put it outside that catch and leave
`McpError::SetupFailed` without its upstream producer — the gap
[13-cyrup-mcp-STATUS.md:174-183](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) records.

Both transports reach `post_handshake` holding an `Arc<McpConnection>`: `connect_stdio` returns
`McpResult<Arc<McpConnection>>` ([runtime.rs:2386](../../crates/cyrup-mcp/src/runtime.rs)) and the
HTTP `Connected` arm returns `HttpAttempt::Connected(Arc<McpConnection>)`
([runtime.rs:2332](../../crates/cyrup-mcp/src/runtime.rs)). The only `NewConnection` that carries an
`InertResource` is the `needs-auth` early return
([runtime.rs:2972-2979](../../crates/cyrup-mcp/src/runtime.rs)), which never reaches
`post_handshake`. So `resource.peer()` is `Some` on every path discovery runs on.

Three seams widen first.

### Seam 1 — reach the `Peer` from behind the trait object

In [server_manager.rs](../../crates/cyrup-mcp/src/server_manager.rs), extend the import at line 79
to `use rmcp::service::{Peer, PeerRequestOptions, RoleClient};` and append to
`trait ConnectionResource` (after `stderr_detail`, i.e. at `server_manager.rs:533`):

```rust
    /// The live `Peer` every request goes through, when this resource owns one.
    ///
    /// Defaulted to `None` so the ten in-crate `impl ConnectionResource` blocks stay
    /// source-compatible. `None` is the honest answer for [`InertResource`]
    /// (`server_manager.rs:730`, the `needs-auth` arm, which has no client at all),
    /// [`StdioChildConnection`] (`server_manager.rs:641`, which never handed its child to rmcp) and
    /// the test doubles.
    ///
    /// This also unblocks the *other* consumer named in-crate: [`McpServerManager::begin_request`]
    /// (`server_manager.rs:2625-2627`) has the in-flight accounting and no call to wrap.
    fn peer(&self) -> Option<&Peer<RoleClient>> {
        None
    }
```

In [runtime.rs](../../crates/cyrup-mcp/src/runtime.rs), add the override inside
`impl ConnectionResource for McpConnection` (`runtime.rs:2202-2247`), next to `stderr_detail`.
`Peer` and `RoleClient` are already imported at `runtime.rs:463-466`, so no import edit is needed
here:

```rust
    fn peer(&self) -> Option<&Peer<RoleClient>> {
        Some(McpConnection::peer(self))
    }
```

The explicit `McpConnection::peer(self)` disambiguates: the inherent method and the trait method
share a name, and inherent methods win on a concrete receiver, so `self.peer()` inside the trait
impl would recurse into itself only if the inherent one were removed — spelling it out makes the
resolution unambiguous to a reader as well as to the compiler.

Finally, **rewrite the inherent method's doc at `runtime.rs:2185-2189`**, which currently asserts
the opposite of what the code will then say. Replace those five doc lines with:

```rust
    /// The `Peer` every request goes through. Also exposed through
    /// [`ConnectionResource::peer`], which is how a holder of `Arc<dyn ConnectionResource>` —
    /// `post_handshake`'s discovery, and `begin_request`'s `tools/call` — reaches it. The inherent
    /// method stays because `connect_stdio` returns a concrete `Arc<McpConnection>` and its callers
    /// should not need the trait in scope.
```

Do **not** delete the inherent method: it is called at `runtime.rs:3770`.

### Seam 2 — `NewConnection` gains somewhere to put the results

Replace [server_manager.rs:1129-1137](../../crates/cyrup-mcp/src/server_manager.rs):

```rust
/// What `createConnection` returns.
pub struct NewConnection {
    /// `{ client, transport }`.
    pub resource: Arc<dyn ConnectionResource>,
    /// `"connected"` or `"needs-auth"` — `createConnection` never returns `"closed"`.
    pub status: ConnectionStatus,
    /// `credentialsInvalidated`, possibly set by this attempt's own 401 handling (MCP-116).
    pub credentials_invalidated: bool,
    /// `client.getInstructions?.()` (`server-manager.ts:550`) — present only when the server sent
    /// one, because §3.9 spreads the key into the record only when `!== undefined`.
    pub instructions: Option<String>,
    /// `fetchAllTools`' result (`server-manager.ts:577`, assigned at `:581`).
    pub tools: Vec<Tool>,
    /// `fetchAllResources`' result (`:578`, assigned at `:582`).
    pub resources: Vec<Resource>,
    /// `fetchAllPrompts`' `prompts` half (`:579`, assigned at `:583`).
    pub prompts: Vec<Prompt>,
    /// `promptDiscoveryFailed` — the `prompts` capability was advertised and `prompts/list` threw.
    pub prompt_discovery_failed: bool,
}

impl NewConnection {
    /// A connection with nothing discovered — the base every non-discovering construction site
    /// updates from, so the `needs-auth` arm and the eight test doubles do not each grow five
    /// lines.
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

`Tool`, `Resource` and `Prompt` are already imported at
[server_manager.rs:78](../../crates/cyrup-mcp/src/server_manager.rs).

The nine literals that are **not** the discovery site — `runtime.rs:2975`;
`server_manager.rs:2910`, `:2915`, `:2942`, `:3185`, `:3534`, `:3624`, `:4231`, `:4619` — become

```rust
NewConnection {
    credentials_invalidated: /* whatever it was */,
    ..NewConnection::bare(resource, status)
}
```

(drop the `credentials_invalidated` line entirely at the sites that pass `false`).

### Seam 3 — `ServerConnection` accepts them

`instructions` is a plain `Option<String>` field, not behind a lock
([server_manager.rs:805](../../crates/cyrup-mcp/src/server_manager.rs)), so it cannot be written by
a setter after construction. `ServerConnection::new` has nine test call sites
(`server_manager.rs:3419`, `:3776`, `:3798`, `:3823`, `:4028`, `:4072`, `:4099`, `:4148`, `:4712`)
and one production one (`:1826`). Keep `new`'s signature and make it delegate.

Replace [server_manager.rs:828-850](../../crates/cyrup-mcp/src/server_manager.rs) with:

```rust
    /// Build the record around a freshly created live half, with nothing discovered.
    ///
    /// Kept at four arguments because nine test call sites construct records that never ran
    /// discovery; the production path goes through [`Self::from_created`].
    #[must_use]
    pub fn new(
        definition: Arc<ServerEntry>,
        resource: Arc<dyn ConnectionResource>,
        status: ConnectionStatus,
        credentials_invalidated: bool,
    ) -> Arc<Self> {
        Self::from_created(
            definition,
            NewConnection {
                credentials_invalidated,
                ..NewConnection::bare(resource, status)
            },
        )
    }

    /// Build the record from what `createConnection` returned, discovery included —
    /// `server-manager.ts:551-584`, where the record is built and the four discovery results are
    /// then assigned onto it.
    #[must_use]
    pub fn from_created(definition: Arc<ServerEntry>, created: NewConnection) -> Arc<Self> {
        Arc::new(Self {
            definition,
            resource: created.resource,
            status: AtomicU8::new(status_code(created.status)),
            credentials_invalidated: AtomicBool::new(created.credentials_invalidated),
            last_used_at: AtomicU64::new(now_ms()),
            in_flight: AtomicU32::new(0),
            instructions: created.instructions,
            tools: Mutex::new(created.tools),
            resources: Mutex::new(created.resources),
            prompts: Mutex::new(created.prompts),
            prompt_discovery_failed: AtomicBool::new(created.prompt_discovery_failed),
            disposed: AtomicBool::new(false),
        })
    }
```

Then the production site
([server_manager.rs:1824-1831](../../crates/cyrup-mcp/src/server_manager.rs)) becomes:

```rust
                        let created = factory.create(request).await.map_err(ManagerError::mcp)?;
                        Ok(ServerConnection::from_created(definition_for_record, created))
```

Also correct the now-false doc on the fields at
[server_manager.rs:806-810](../../crates/cyrup-mcp/src/server_manager.rs) — "**Populated by
MCP-119**, which is not this unit … it stays empty until that unit lands" — to state that
`from_created` writes them, and add to `instructions` at `:804`:

```rust
    /// `connection.instructions` — present only when the server sent one
    /// (`server-manager.ts:550`, `:561`). **Write-only today**: the reader is
    /// `proxy::env::ConnectOutcome::instructions` (`proxy/env.rs:52-53`) and the bridge that fills
    /// it is 13d's, not this unit's.
    instructions: Option<String>,
```

## The discovery code

New section in [runtime.rs](../../crates/cyrup-mcp/src/runtime.rs), immediately **before**
`impl ConnectionFactory for ConnectionBuilder` (`runtime.rs:2920`).

Two import edits first:

* `runtime.rs:463-466` — add `ServiceError` to the `use rmcp::service::{…}` list.
* `runtime.rs:467` — widen to
  `use rmcp::model::{ClientJsonRpcMessage, Prompt, Resource, ServerCapabilities, Tool};`.
  Keep it separate from the `#[allow(deprecated)]` block at `:456-462`; none of these four is
  deprecated.

### The failure carrier and the 401 predicate

```rust
/// What one `*/list` walk can fail with.
///
/// Not `McpError`, because the 401 arm has to inspect the *original* transport error and `McpError`
/// has already flattened it to a string. Not `ServiceError`, because rmcp never produces a timeout
/// on this path — `list_*` goes through `send_request`, which is
/// `send_request_with_option(.., PeerRequestOptions::no_options())`
/// (`rmcp-3.1.4/src/service.rs:835-840`) — so the timeout upstream *does* apply has no rmcp
/// representation and needs a local variant. (`ServiceError` is also `#[non_exhaustive]`
/// (`service.rs:78`), so `Timeout` could not be constructed here even if it were produced.) Same
/// shape and same reason as [`HttpAttempt::TimedOut`] (`runtime.rs:2342-2346`).
#[derive(Debug)]
enum ListFailure {
    Service(ServiceError),
    /// `requestOptions.timeout` elapsed — upstream's
    /// `SdkError(RequestTimeout, "Request timed out")`.
    TimedOut,
}

impl ListFailure {
    /// `error.message` — what upstream interpolates into the prompts debug line. Deliberately NOT
    /// `into_mcp(..).to_string()`, whose `Display` is `"{server}: {message}"`
    /// (`errors.rs:181-189`) and would put the server name inside the message twice.
    fn message(&self) -> String {
        match self {
            Self::Service(error) => error.to_string(),
            Self::TimedOut => HANDSHAKE_TIMED_OUT.to_string(),
        }
    }

    fn into_mcp(self, server: &str) -> McpError {
        McpError::Server {
            server: server.to_string(),
            message: self.message(),
        }
    }

    /// `isUnauthorizedHttpError(error)` (`server-manager.ts:73-75`), rooted at a `ServiceError`.
    ///
    /// **401 only, and this is why rmcp's own helper is not used.**
    /// `ClientInitializeError::auth_challenge` (`rmcp-3.1.4/src/service/client.rs:110-131`) matches
    /// `AuthRequiredError` (401) *and* `InsufficientScopeError` (403); a 403 that re-threw here
    /// would turn a permission error into a failed connect where upstream degrades `resources` to
    /// `[]`. The arms below are exactly [`unauthorized_challenge`]'s (`runtime.rs:2000-2032`) minus
    /// the challenge string, rooted at `DynamicTransportError::error` for the reason that function
    /// states: `ServiceError::TransportSend(DynamicTransportError)` carries no `#[source]`
    /// attribute (`rmcp-3.1.4/src/service.rs:82-83`), so a walk that starts at the wrapper finds
    /// nothing.
    fn is_unauthorized(&self) -> bool {
        let Self::Service(ServiceError::TransportSend(dynamic)) = self else {
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
/// `client.list*(cursor, requestOptions)`'s cursor loop, with both halves of `requestOptions`
/// re-imposed around rmcp's `list_all_*`.
///
/// The outer `Err` is the `signal` half — upstream's `throwIfAborted`. The inner `Err` is the list's
/// own failure, which each caller's arm of the matrix then classifies.
async fn run_list<T>(
    list: impl std::future::Future<Output = Result<Vec<T>, ServiceError>>,
    timeout: Option<Duration>,
    signal: &CancelToken,
) -> McpResult<Result<Vec<T>, ListFailure>> {
    let walk = async move {
        match timeout {
            Some(limit) => match tokio::time::timeout(limit, list).await {
                Ok(result) => result.map_err(ListFailure::Service),
                Err(_elapsed) => Err(ListFailure::TimedOut),
            },
            None => list.await.map_err(ListFailure::Service),
        }
    };
    // `PeerRequestOptions` has no signal field — rmcp cancels a request by dropping its future — so
    // the `ownedSignal` half of `buildRequestOptions` (`server-manager.ts:245`) lives in this
    // wrapper.
    crate::abort::abortable(walk, signal).await
}

/// `client.getServerCapabilities?.()` (`server-manager.ts:989`, `:1011`).
///
/// `peer_info()` is `Option` only because it is `None` before the handshake; rmcp sets it in
/// `initialize`'s success path (`rmcp-3.1.4/src/service/client.rs:902`), so after the handshake it
/// is always `Some`. `None` therefore reads as "advertised nothing", which is the same answer
/// upstream's `?.()` gives for a client that never connected.
fn advertises(peer: &Peer<RoleClient>, has: fn(&ServerCapabilities) -> bool) -> bool {
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
            // `if (requestOptions?.signal?.aborted) throwIfAborted(requestOptions.signal)`
            // (`:1025-1027`). The test is on the SIGNAL at catch time, not on the error, and what it
            // raises is the ABORT — not the list failure. `run_list`'s `abortable` covers only the
            // case where the abort won the race; this covers the case where it arrived a moment
            // later.
            crate::abort::throw_if_aborted(signal, None)?;
            // `if (isUnauthorizedHttpError(error)) throw error` (`:1029`).
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
            // Byte-exact: `` logger.debug(`MCP: prompts/list failed: ${message}`) `` (`:1005`).
            tracing::debug!("MCP: prompts/list failed: {}", failure.message());
            Ok((Vec::new(), true))
        }
    }
}
```

### The concurrent reduce

```rust
/// `Promise.all([fetchAllTools, fetchAllResources, fetchAllPrompts])`
/// (`server-manager.ts:576-580`).
///
/// **`join!`, never `try_join!`.** `try_join!` drops the sibling futures on the first `Err`. That
/// would cancel a `prompts/list` about to record `prompt_discovery_failed = true` and a
/// `resources/list` about to degrade to `[]`, so the per-list policy would survive only for
/// whichever list happened to settle first — the failure mode `13c-mcp-servers.md:1439-1440` names.
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

Replace [runtime.rs:2994-3013](../../crates/cyrup-mcp/src/runtime.rs) — the two comment blocks, the
lone `throw_if_aborted` and the `let Err(error) = outcome else` early return. Everything below
(`runtime.rs:3015-3029`, the `Promise.allSettled` cleanup and the `McpError::SetupFailed` wrapper)
stays byte-for-byte as it is; discovery simply gives it the producer it was written for.

```rust
        // §3.2: the timeout half of `buildRequestOptions(definition, requestSignal)`
        // (`server-manager.ts:240-255`), computed once by the manager
        // (`server_manager.rs:1123-1126`) and reused for all three list calls.
        // `PeerRequestOptions` is neither `Clone` nor constructible field-by-field out of crate
        // (`rmcp-3.1.4/src/service.rs:759-768`, `#[non_exhaustive]`), so what travels through
        // `CreateConnection` and into each call is the `Duration`.
        let timeout = request
            .request_options
            .as_ref()
            .and_then(|options| options.timeout);

        let outcome = async {
            // `throwIfAborted(signal)` (`server-manager.ts:544`) — the ATTEMPT signal. Upstream's
            // sits at the top of the try, before the connect; the one that matters after a
            // successful handshake is this: a close that raced the handshake must not leave a live
            // child behind just because it arrived a microsecond late.
            crate::abort::throw_if_aborted(&request.attempt, None)?;
            let peer = resource.peer().ok_or_else(|| McpError::Server {
                server: request.name.clone(),
                message: "MCP connection has no peer to discover against".to_string(),
            })?;
            // `const instructions = client.getInstructions?.()` (`server-manager.ts:550`).
            let instructions = peer.peer_info().and_then(|info| info.instructions.clone());
            // The list calls race the REQUEST signal, not the attempt one: upstream builds
            // `requestOptions` from `requestSignal` (`server-manager.ts:471`), the
            // caller-plus-runtime signal *without* the attempt controller
            // (`server_manager.rs:1116-1119`).
            let (tools, resources, prompts, prompt_discovery_failed) =
                discover(peer, &request.name, timeout, &request.request).await?;
            Ok::<_, McpError>((instructions, tools, resources, prompts, prompt_discovery_failed))
        }
        .await;

        let error = match outcome {
            Ok((instructions, tools, resources, prompts, prompt_discovery_failed)) => {
                // `connection.tools = tools; … connection.promptDiscoveryFailed = promptResult.failed`
                // (`server-manager.ts:581-584`) — carried on the return value here, and written onto
                // the record by `ServerConnection::from_created`.
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

Note the `async { … }.await` block: `resource` is moved into the `Ok` arm below, so the borrow of
`resource` taken by `resource.peer()` must end before that. The block is what ends it.

Finally, delete the stale marker comment now that the code is there, and update the two doc claims
that will otherwise contradict it:

* [runtime.rs:2944-2955](../../crates/cyrup-mcp/src/runtime.rs) — `create_connection`'s doc says
  "this builder does not have that one, because `NewConnection` has nowhere to put the results".
  Rewrite so it says discovery is now the ordinary producer and the abort race is the secondary one.
* [runtime.rs:1862-1865](../../crates/cyrup-mcp/src/runtime.rs) — the region banner's
  "**Discovery (MCP-119).** `NewConnection` has nowhere to put tools/resources/prompts" bullet.
  Remove it.
* [server_manager.rs:1166-1170](../../crates/cyrup-mcp/src/server_manager.rs) — drop ", with
  discovery pending MCP-119" from `UnbuiltConnectionFactory`'s message.
* [server_manager.rs:2625-2627](../../crates/cyrup-mcp/src/server_manager.rs) — `begin_request`'s
  "it needs the connection's `Peer`, which the `ConnectionFactory` does not yet produce (MCP-119's
  plumbing)" becomes "the `Peer` is reachable through `ConnectionResource::peer()`; the call itself
  is MCP-129's".
* [server_manager.rs:2792-2795](../../crates/cyrup-mcp/src/server_manager.rs) — leave
  `should_reconnect_after_refresh` alone. Its second disjunct needs `ServiceError` discriminants
  surfaced through `refreshTools`, which is MCP-120; this unit does not change that predicate.

## Named divergences — record these in the code, not only here

1. **The shared timeout becomes per-list, not per-page.** rmcp's `list_all_*` call `send_request` =
   `PeerRequestOptions::no_options()` (`service.rs:835-840`), so the only way to keep
   `requestTimeoutMs` is `tokio::time::timeout` around the whole walk. Upstream applies it to each
   page. This is **stricter, never looser** — an N-page list gets N×timeout upstream and 1×timeout
   here. The alternative, hand-rolling `send_request_with_option` per page, opts out of rmcp's
   list-response cache and its `*_list_changed` invalidation (`client.rs:334-350`), which is what
   MCP-120 is specified against.
2. **Which error wins a multi-failure `join!` is deterministic here, timing-dependent upstream.**
   `Promise.all` rejects with the first rejection *in time*; the reduce above picks tools →
   resources → prompts by position. Only observable when two lists fail in the same connect with
   different errors.
3. **A 403 is not a 401.** Upstream's predicate is 401-only and so is `is_unauthorized`; rmcp's
   `auth_challenge` would also match `InsufficientScopeError`. The note at
   [runtime.rs:1954-1998](../../crates/cyrup-mcp/src/runtime.rs) already records this choice for the
   connect path — discovery repeats it, so cross-reference rather than restate.
4. **rmcp may answer a first-page failure from a stale cache.** `list_tools` with no cursor sets
   `uses_cursor = false` and on error returns `stale_cached_response` if one exists
   (`client.rs:1686-1698`). At connect time the cache is empty, so this cannot fire for MCP-119; it
   can for MCP-120's refresh. Caching is opt-in per response (`ttl_ms` / `cache_scope`), so it only
   applies to servers that ask for it.
5. **A discovery 401 fails the connect here; upstream can downgrade it to `needs-auth`.** This unit
   makes upstream's catch-path downgrade
   ([server-manager.ts:605-622](../../tmp/pi-mcp-adapter/server-manager.ts)) reachable for the first
   time — until now nothing post-handshake could raise a 401. That downgrade is **MCP-116's second
   exit**, explicitly ([13c-mcp-servers.md:1387-1399](../../docs/gap-analysis/13c-mcp-servers.md):
   "There are **two** needs-auth exits: the HTTP ladder's own and `createConnection`'s catch-path
   downgrade"), and only the ladder's half has landed
   ([runtime.rs:2681-2699](../../crates/cyrup-mcp/src/runtime.rs)). Do **not** build it here — the
   gate is `isUnauthorizedHttpError(error) && supportsOAuth(definition) && cleanupFailures.length ===
   0`, which needs the cleanup outcome and the OAuth predicate, and belongs with the other half.
   Record it with a comment at the `let error = match outcome` site naming
   `server-manager.ts:605-622` and MCP-116.
6. **A discovery failure gets no stderr-tail suffix.** Upstream's catch appends the child's last
   three non-empty stderr lines to *any* error the try raised
   ([server-manager.ts:624-632](../../tmp/pi-mcp-adapter/server-manager.ts)); here that enrichment
   lives inside `initialize_error` ([runtime.rs:2893-2917](../../crates/cyrup-mcp/src/runtime.rs))
   and so only covers handshake failures. Porting it into `post_handshake` would have to flatten the
   typed `McpError::SetupFailed(CleanupErrors)` into a string, which is worse than the divergence.
   Bounded: by discovery time the handshake has succeeded, so the tail is a startup banner rather
   than a failure reason. Record it in the same comment.

## Out of scope — do not let these in

* `attachAdapterNotificationHandlers` and the `client.onclose` identity guard
  ([server-manager.ts:548](../../tmp/pi-mcp-adapter/server-manager.ts),
  [:566-572](../../tmp/pi-mcp-adapter/server-manager.ts)) are MCP-120/MCP-121. They sit inside the
  region this unit edits; only discovery belongs here.
* `refreshTools` ([server-manager.ts:357-395](../../tmp/pi-mcp-adapter/server-manager.ts)) — the
  `cacheMode: "refresh"` re-list and its `tools` capability gate — is MCP-120, including
  `lifecycle.rs`'s unbound `refresh_tools` (`lifecycle.rs:324-333`).
* `getPrompt` / `readResource` / `tools/call` calling through the new `peer()` is MCP-129. This unit
  only makes the peer reachable.
* MCP-133's `enrichHttpConnectionError`
  ([server-manager.ts:637-645](../../tmp/pi-mcp-adapter/server-manager.ts)) stays absent.
* `proxy/ranking.rs`, `proxy/discovery.rs` and every other file under `proxy/` — untouched.
* No new tests, benchmarks or `docs/` edits are part of this unit.

## Definition of done

1. `ConnectionResource::peer()` exists on the trait with a `None` default
   (`server_manager.rs`, after `stderr_detail`), `impl ConnectionResource for McpConnection`
   returns `Some`, and the inherent `McpConnection::peer`'s doc no longer says "Not on the
   `ConnectionResource` trait".
2. `NewConnection` carries `instructions`, `tools`, `resources`, `prompts`,
   `prompt_discovery_failed`, plus `NewConnection::bare`; the nine non-discovery literals use
   `..NewConnection::bare(..)`.
3. `ServerConnection::from_created` exists and writes all five onto the record including
   `instructions`; `ServerConnection::new` delegates to it and keeps its four-argument signature so
   the nine test call sites are untouched; `server_manager.rs:1826` calls `from_created`.
4. `fetch_all_tools` is unconditional and has no error-swallowing arm: an abort, a 401 and any other
   error all leave through `?`.
5. `fetch_all_resources` and `fetch_all_prompts` return their empty value **without issuing a
   request** when `peer_info().capabilities.{resources,prompts}` is `None`; the prompts gate returns
   `failed == false`.
6. Both gated fetchers implement three arms in this order: `throw_if_aborted` on the **request**
   signal → `is_unauthorized` re-throw → degrade. Resources degrades to `[]` with no log; prompts
   degrades to `([], true)` after `tracing::debug!("MCP: prompts/list failed: {}", …)` with that
   exact prefix and no server name inside it.
7. `is_unauthorized` matches only `ServiceError::TransportSend`, walks from
   `dynamic.error.as_ref()`, and accepts `AuthRequiredError` or `bare_unauthorized` — no use of
   rmcp's `auth_challenge`.
8. The three run under `tokio::join!` and are reduced by position; no `try_join!` invocation is
   introduced (the only existing mention is the cautionary comment at
   `server_manager.rs:2400`, which stays).
9. Pagination is rmcp's `list_all_tools` / `list_all_prompts` / `list_all_resources`; no
   hand-written cursor loop and no slice indexing is introduced (workspace denies
   `clippy::indexing_slicing`).
10. Discovery runs inside `ConnectionBuilder::post_handshake`, above the existing cleanup arm at
    `runtime.rs:3015-3029`, which is unchanged — so a discovery failure whose `resource.close()`
    also fails still produces `McpError::SetupFailed`.
11. Divergences 5 and 6 are recorded as a comment at the `let error = match outcome` site, citing
    `server-manager.ts:605-622` / MCP-116 and `server-manager.ts:624-632`.
12. The six stale "blocked on MCP-119" statements are corrected or removed:
    `runtime.rs:1862-1865`, `runtime.rs:2944-2955`, `runtime.rs:2999-3004`,
    `server_manager.rs:806-810`, `server_manager.rs:1166-1170`, `server_manager.rs:2625-2627`.
13. `cargo clippy -p cyrup-mcp --all-targets` is clean with **no new `#[allow]` or `#[expect]`**, and
    `cargo nextest run --workspace` still reports 7862 passing.
