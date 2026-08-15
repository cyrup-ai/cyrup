# 13g · The OAuth 2.1 flow and the callback server

> Part of **[13 — cyrup-mcp](13-cyrup-mcp.md)**, which holds the thesis, the seam map, the
> architecture and the one canonical table of every port unit. Method and phasing are in
> **[MCP-PORT-METHODOLOGY.md](MCP-PORT-METHODOLOGY.md)**.

**Provenance.** Upstream is `pi-mcp-adapter` v2.25.0. cyrup is branch `david/cyrup`. rmcp is the
checkout at `/Users/davidmaple/cyrup.ai/rmcp` (`rmcp-v3.1.2-7-gf713ebd`). cyrup is referenced by
symbol and file only.

This subsystem is what lets an HTTP MCP server answer "you are not authorized" and have the adapter
turn that into a browser round trip, a token in the OS credential store, and a working connection —
without the user configuring an endpoint. Upstream it is four files: `mcp-auth-flow.ts` (the
orchestrator, and the only module that owns flow state), `mcp-oauth-provider.ts` (an implementation
of the MCP TypeScript SDK's `OAuthClientProvider` interface — the storage/redirect/client-auth
adapter the SDK calls back into), `mcp-callback-server.ts` (a process-wide singleton `node:http`
listener keyed by `state` values), and `OAUTH.md` (the published spec). Token storage itself — the
OS keyring, the chunking manifest, the `keyctl` recovery — is `mcp-auth.ts` and belongs to the
storage section; this one consumes it.

**The shape of the answer: rmcp owns the protocol, `cyrup-mcp` owns the flow around it.** The four
RFCs in this section's title are not implemented in the adapter either — they live inside
`@modelcontextprotocol/client`'s `auth()` orchestrator, and the adapter's job is to prime it, hand
it a provider, and intercept the one place it hands control back. In the port that orchestrator is
`rmcp`'s `auth` feature, verified against `crates/rmcp/src/transport/auth.rs`: RFC 9728
protected-resource metadata and RFC 8414/OIDC authorization-server metadata discovery
(`AuthorizationManager::{resolve_metadata, resolve_metadata_from_challenge}`), RFC 7591 dynamic
client registration (`AuthorizationManager::register_client`), PKCE S256 (always — there is no
`plain` path), the RFC 8707 `resource` parameter on every authorization and token request, the RFC
9207 `iss` gate, SEP-2207 `offline_access` scope augmentation, SEP-991 CIMD, SEP-835 scope upgrade,
automatic refresh, and the client-credentials grant. **The port hand-writes no OAuth protocol code
at all.** What it does hand-write is everything the adapter itself wrote: flow ownership and
generation fencing, config validation and its exact messages, the loopback callback listener's
multiplexing and lifetime, the manual/headless paste leg, the stale-registration hygiene, the
approval and command surfaces, and the user-facing strings.

**The callback listener is a reuse, not a rebuild.** `cyrup_provider::auth::oauth::callback` already
ships a real loopback listener: `CallbackServer::start(CallbackServerConfig, handler)` binds a
`std::net::TcpListener`, runs a detached accept thread with a request-read timeout and a request-head
cap, routes one path, and hands each request to a `CallbackHandler` returning
`CallbackOutcome::{Complete, Failed, Continue}`. **`Continue` is the seam that makes it
multi-tenant**: a handler that never calls `CallbackControl::claim` and always returns `Continue`
never settles the server's own one-shot, so one listener serves N concurrent logins while the
adapter's `state`→waiter map does the routing inside the handler. `CallbackServerConfig` already
carries `port` (`0` = ephemeral, or `fixed(port, path)`), `path`, `host`, `advertise_host` (bind one
name, register another), `timeout` and `cancel`; `CallbackServer` exposes `redirect_uri()`,
`port()`, `path()` and `close()`; a bind failure is `OAuthError::Listen { address, source }`, whose
`source.kind()` is `AddrInUse` for the strict-port collision message. `cyrup-mcp` already depends on
`cyrup-provider` for sampling, so there is no layering question. The previous edition ruled this
"the wrong shape" on ten counts; on inspection eight of the ten are dissolved by the `Continue`
handler contract, and the two that survive are cosmetic (the foreign-path 404 is the HTML error page
rather than `text/plain Not found`, and there is an extra 400 branch for a malformed request).

**Everything here is `extension-owned`.** `crates/cyrup-mcp` is a native built-in crate compiled into
the binary, not a WASM guest; it links `rmcp`, `reqwest`, `keyring`, `opener` and `std::process`
directly. The only places it reaches `HostServices` are the ones that genuinely touch the host:
`confirm`/`input`/`oauth_prompt`/`oauth_select` for the manual paste and the server picker, `notify`
and `set_status` for the `/mcp-auth` surface, `human_interaction_lock` so an OAuth prompt and a
permission prompt cannot both be on screen, and `is_run_cancelled` for run-scoped abort. **This
section files zero host additions.** The previous edition's ambient-authority worries —
`HostServices::exec` being denied by default, no browser-open verb existing, `interpolate_env_vars`
being `pub(crate)` — all rest on the WASM-guest capability model and do not apply to a native crate.

---

### How it lands

| # | adapter capability | upstream mechanism | cyrup mechanism | verdict |
| --- | --- | --- | --- | --- |
| OA-a | RFC 9728 protected-resource metadata discovery, with the `.well-known` path-insertion and root fallbacks | SDK `discoverOAuthProtectedResourceMetadata` / `discoverMetadataWithFallback`, seeded by the adapter's `probeAuthDiscovery` | `AuthorizationManager::resolve_metadata` (`crates/rmcp/src/transport/auth.rs`), `well_known_paths` | **`rmcp`** |
| OA-b | `WWW-Authenticate` challenge parsing and `resource_metadata` seeding | `extractWWWAuthenticateParams` in the SDK; adapter supplies the header | `WWWAuthenticateParams::parse`, `AuthorizationManager::resolve_metadata_from_challenge`, `AuthorizationRequest::with_challenge` | **`rmcp`** |
| OA-c | RFC 8414 + OIDC AS metadata, three-URL priority order, issuer echo check | SDK `buildDiscoveryUrls` / `discoverAuthorizationServerMetadata` | `AuthorizationManager` discovery + `validate_authorization_metadata_issuer` / `issuer_identifiers_match` | **`rmcp`** |
| OA-d | RFC 7591 dynamic client registration | SDK `registerClient`; body from `McpOAuthProvider.clientMetadata` | `AuthorizationManager::register_client`, driven by `AuthorizationSession::new`'s priority order | **`rmcp`** (body fidelity delta — OA-13) |
| OA-e | PKCE S256, authorization URL construction, `offline_access` augmentation | SDK `startAuthorization` + transitive `pkce-challenge` | `AuthorizationManager::get_authorization_url` (`PkceCodeChallenge::new_random_sha256`), `add_offline_access_if_supported` | **`rmcp`** |
| OA-f | RFC 8707 `resource` on authorize and token requests | SDK `selectResourceURL` / `checkResourceAllowed` | `AuthorizationManager::oauth_resource`, `.add_extra_param("resource", …)` on authorize, exchange and refresh | **`rmcp`** |
| OA-g | code exchange, refresh (keeping an omitted refresh token), auto-refresh | SDK `exchangeAuthorization` / `refreshAuthorization` / `fetchToken` | `AuthorizationManager::{exchange_code_for_token_with_issuer, refresh_token, get_access_token}` | **`rmcp`** |
| OA-h | RFC 9207 `iss` validation bound to the flow's `state` | SDK `validateAuthorizationResponseIssuer` ×4 sites + the adapter's earlier gate in `completeAuth` | `AuthorizationManager::validate_authorization_response_issuer` over `StoredAuthorizationState::{expected_issuer, require_issuer}` | **`rmcp`** + **`hand-written`** (the friendlier message) |
| OA-i | discovery-state custody across the browser hop (SEP-2352) | `McpOAuthProvider.saveDiscoveryState`/`discoveryState` + the SDK's `AuthorizationServerMismatchError` | `StateStore` keyed by CSRF token, holding `pkce_verifier` + `expected_issuer` + `require_issuer` + `requested_scopes`; state is deleted **after** the issuer check | **`rmcp`** |
| OA-j | issuer binding of stored credentials (SEP-2352) | `assertStoredIssuerBindings`, latched `flowIssuerMismatch` | `AuthorizationManager::initialize_from_store`'s stored-vs-current issuer guard | **`rmcp`** (clear-vs-throw delta — OA-14) |
| OA-k | `client_credentials` grant | `prepareTokenRequest` + the SDK's non-interactive branch | `OAuthState::authenticate_client_credentials`, `ClientCredentialsConfig::ClientSecret`, `AuthorizationManager::{configure_client_credentials, exchange_client_credentials}` | **`rmcp`** |
| OA-l | reactive trigger: a 401 during connect means "authorize" | `server-manager.ts`'s `isUnauthorizedHttpError` + `HttpAuthProviderState` promotion | `ClientInitializeError::{auth_challenge, is_authorization_required}` (`service/client.rs`), then `AuthorizationRequest::with_challenge` | **`rmcp`** |
| OA-m | authorized transport | SDK transport `authProvider` option | `AuthClient::new(reqwest_client, manager)` → `StreamableHttpClientTransport::with_client` | **`rmcp`** |
| OA-1 | loopback callback listener: bind, ephemeral-vs-fixed port, accept loop, request read | `mcp-callback-server.ts` `ensureCallbackServerLocked` over `node:http` | `cyrup_provider::auth::oauth::callback::{CallbackServer, CallbackServerConfig}` | **`extension-owned` (reuse)** |
| OA-2 | multi-tenant routing by `state`, reservations, per-state waiters | `pendingAuths` / `reservedAuthStates` maps inside `handleRequest` | a `CallbackHandler` that always returns `CallbackOutcome::Continue` and routes into its own `Mutex<HashMap<state, oneshot::Sender>>` | **`hand-written`** on a reused primitive |
| OA-3 | rebind on host/port/path change, "cannot be switched while authorizations are pending" | `ensureCallbackServerLocked`'s `needsStrictRebind`/`needsHostSwitch`/`needsPathSwitch` | close and re-`start` the reused `CallbackServer` under a `tokio::sync::Mutex`, with the two refusal errors | **`hand-written`** |
| OA-4 | the three HTML pages, host-branded, self-contained | `htmlSuccess` / `htmlManualSuccess` / `htmlError` + `PAGE_STYLE` | a `cyrup-mcp` page module; only `cyrup_provider::auth::oauth::page::escape_html` transfers | **`hand-written`** |
| OA-5 | flow ownership, generation fencing, the four flow maps, refcounted listener shutdown | `mcp-auth-flow.ts` `createOAuthRuntime` / `shutdownOAuth` / `setPendingAuth` / `clearPendingAuth` | `Arc<McpOAuthRuntime>` holding a `cyrup_core::CancelToken` + `Mutex<RuntimeState>`, `AtomicU64` generation, a process-global live-runtime set | **`hand-written`** |
| OA-6 | `oauth` block validation and its twelve messages | `extractOAuthConfig` | a hand-written validator over `url::Url`; not `serde` (the messages are the contract) | **`hand-written`** |
| OA-7 | loopback-only `redirectUri` validation | `parseOAuthRedirectUri` | `url::Url` + the six ordered checks; note `cyrup_provider::auth::oauth::callback::bracket_host` is the inverse transform | **`hand-written`** |
| OA-8 | manual/headless paste: input parsing, the callback-vs-prompt race | `getSearchParamsFromInput`, `parseAuthorizationRedirectInput`, `waitForAuthorizationResponse` | `tokio::select!`; `HostServices::{confirm, input, oauth_prompt}` for the prompt; `rmcp::transport::auth::AuthorizationCallback::from_redirect_url` covers only the strict URL case | **`hand-written`** + **`host-verb`** |
| OA-9 | browser launch, with the URL printed first and a swallowed launch failure | `open` npm package | `opener` crate | **`extension-owned`** |
| OA-10 | `!command` client-secret resolution | `utils.ts` `resolveCommandSecret` (`spawnSync`, `shell: true`) | `std::process::Command` with `sh -c` / `cmd /C`, directly — a native crate does not go through `HostServices::exec` | **`extension-owned`** |
| OA-11 | token/credential persistence in the OS keychain | `mcp-auth.ts` over `@napi-rs/keyring` | a `rmcp::transport::auth::CredentialStore` impl over `keyring` 4.1.6, one instance per server | **`hand-written`** (thin) |
| OA-12 | PKCE/CSRF state persistence for the flow | `McpOAuthProvider`'s in-memory `flowCodeVerifier`/`flowState` | `rmcp::transport::auth::InMemoryStateStore` (in-process custody is sufficient and correct) | **`rmcp`** |
| OA-13 | client-metadata document: `client_uri`, `logo_uri`, `token_endpoint_auth_method` | `McpOAuthProvider.clientMetadata` | rmcp's `ClientRegistrationRequest` is fixed: `client_name`, `redirect_uris`, `grant_types`, `token_endpoint_auth_method: "none"`, `response_types`, `scope`, `application_type` | **`open-decision`** (see below) |
| OA-14 | client-auth method selection at the token endpoint | `McpOAuthProvider.addClientAuthentication` | `AuthorizationManager::configure_client` selects request-body auth only when the AS advertises `client_secret_post` **without** `client_secret_basic`; lever is `set_metadata` before `configure_client` | **`rmcp`** + **`hand-written`** (the lever) |
| OA-15 | `/mcp-auth`, `mcp({action:"auth-start"/"auth-complete"})`, auto-auth | `commands.ts` `authenticateServer`; `proxy-modes.ts` `executeAuthStart`/`executeAuthComplete`/`attemptAutoAuth`; `direct-tools.ts` | `InitApi::register_command` + `NativeExtension::execute_command`; `HostServices::{notify, set_status, confirm, input}` | **`host-verb`** + **`hand-written`** |
| OA-16 | the published OAuth documentation | `OAUTH.md` | a ported doc with eight corrections applied | **`hand-written`** |

---

### Behavioural specification

#### 1 · The callback listener

**Endpoint configuration.** `DEFAULT_OAUTH_CALLBACK_PORT = 19876`; `DEFAULT_OAUTH_CALLBACK_PATH =
"/callback"`. `mcp-oauth-provider.ts` reads `process.env.MCP_OAUTH_CALLBACK_PORT` **once at module
load**, accepting it only when it parses as an integer in `1..=65535`, and otherwise ignoring it
silently. Two live values, `oauthCallbackPort` and `oauthCallbackPath`, are module-global mutable
state that `mcp-callback-server.ts` writes and `McpOAuthProvider`'s constructor **snapshots**.

The ordering consequence is load-bearing: `startAuth` calls `ensureCallbackServer` **before**
constructing the provider, so the provider's redirect-URL snapshot sees the port the listener
actually bound. Reverse the two and every dynamically-registered client advertises
`http://localhost:19876/callback` while the listener sits on an OS-assigned port — a permanent,
provider-side, on-disk consequence. In the port the equivalent is: `CallbackServer::start` first,
then read `CallbackServer::redirect_uri()`, then build the `AuthorizationRequest`. A design that
"cleans this up" into a per-flow struct must still preserve *bind-then-advertise*.

**Strict versus ephemeral port.** `strictPort` is `Boolean(config.clientId) || config.redirectUri
!== undefined`. When false — the default, dynamic-registration case — the listener binds port `0`
and adopts whatever the OS assigns; **the default flow never binds 19876**. When true it binds the
required port exactly, and an `EADDRINUSE` is rewritten as:

> `OAuth callback port ${requiredPort} is already in use. Pre-registered OAuth clients require an exact redirect URI; set MCP_OAUTH_CALLBACK_PORT to your registered port or free port ${requiredPort}`

with the original error as its cause. Any other bind error propagates raw.

**Bind / rebind state machine.** `ensureCallbackServer` is a serializing wrapper: it refuses
outright while a stop is in flight (`OAuth callback server stopped`), snapshots a generation
counter, awaits any in-flight bind, re-checks the generation, then runs the locked routine as the
new in-flight bind and clears it in a `finally` only if it is still its own. The locked routine
computes three flags against the running listener — `needsStrictRebind` (strict and the port
differs), `needsHostSwitch` (host differs), `needsPathSwitch` (path differs) — and then:

* neither strict-rebind nor host-switch needed: if the path differs, refuse when
  `pendingAuths` or `reservedAuthStates` is non-empty with
  `OAuth callback server is using path ${current}, but callback path ${requested} is required and cannot be switched while authorizations are pending`,
  otherwise switch the path in place; reserve the state if asked; **return, reusing the listener**;
* rebind needed but authorizations are outstanding: refuse with
  `OAuth callback server is running on ${host}:${port}, but strict callback endpoint ${requestedHost}:${requiredPort} is required and cannot be switched while authorizations are pending`;
* otherwise bind a **new** listener first, adopt its port, and only then close the old one — a
  failed rebind must leave the existing listener serving.

`reserveState` requires an `oauthState` (`OAuth callback reservation requires an oauthState`) and
the reservation is registered **inside the same critical section as the bind**, so a subsequent
host or path switch is refused by it. On any failure inside the locked routine: release only the
state this call reserved, close the candidate listener, then apply the `EADDRINUSE` rewrite.

**Process-exit semantics.** Node calls `server.unref()` exactly once after a successful bind and
**not at all** when the bind fails. An abandoned OAuth flow must never hold the process open. The
Rust equivalent is the detached accept thread `cyrup_provider::auth::oauth::callback` already
spawns and never joins — but this is a *property to test*, not a line to port: the acceptance test
is that the process exits with a pending, never-completed callback registered.

**Stop and restart.** `stopCallbackServer` bumps the generation, drains any in-flight bind
(swallowing its rejection), closes and nulls the listener, resets the live port/host/path to their
configured defaults, snapshots and clears both maps, and then on a **deferred tick** clears each
timer and rejects each waiter with `OAuth callback server stopped` — deferred so a caller awaiting
the stop observes the reset state before the rejections land. It is idempotent while in flight via a
single shared stop future, and **it clears that future when it completes, so the listener is
restartable**: only calls queued before or during the stop are rejected, by the generation check. A
`OnceCell` gate would wedge it permanently; use a `Mutex<Option<Shared<…>>>` that is taken and
cleared, plus an `AtomicU64` generation.

**Refcounting across runtimes.** The listener is process-shared. Every `getRuntime` and
`initializeOAuth` **adds** its runtime to a live set (idempotently — an incrementing counter would
over-count and never reach zero); `shutdownOAuth` removes one and stops the listener **only when the
set empties**. Two sessions each with their own runtime must not tear down each other's listener.
Binding is lazy: `initializeOAuth` never starts the listener; only `startAuth` does.

`reserveCallbackServer(state)` / `releaseCallbackServer(state)` are the manual add/remove;
`startAuth` releases on its own failure path and after an `AUTHORIZED` short-circuit.
`isCallbackServerRunning()` and `getPendingAuthCount()` are status accessors.

#### 2 · The request handler — every branch

`handleRequest` parses `new URL(req.url || "/", "http://" + host)`. **There is no HTTP method
check** — a `POST` to the callback path is handled identically to a `GET`. Do not add one.

| # | condition | status | content-type | body | map effects |
| --- | --- | --- | --- | --- | --- |
| 1 | path is not the callback path | 404 | `text/plain` | `Not found` | — |
| 2 | no `state` | 400 | `text/html` | `htmlError("Missing required state parameter - potential CSRF attack")` | — |
| 3 | `error` present **and** state neither pending nor reserved | 400 | `text/html` | `htmlError("Invalid or expired state parameter - potential CSRF attack")` — **the provider's `error_description` is deliberately NOT reflected** | — |
| 4 | `error` present and state known | 200 | `text/html` | `htmlError(error_description ?? error)` | see below |
| 5 | state neither pending nor reserved, no `error` | 400 | `text/html` | `htmlError("Invalid or expired state parameter - potential CSRF attack")` | — |
| 6 | no `code` | 400 | `text/html` | `htmlError("No authorization code provided")` | the waiter is **left pending** |
| 7 | state *reserved but not yet awaited* | 200 | `text/html` | `htmlManualSuccess()` | **none — the reservation survives** |
| 8 | state pending | 200 | `text/html` | `htmlSuccess()` | timer cleared, entry removed, resolve `{ code, iss? }` |

Branches 2, 3 and 5 are the CSRF boundary and branch 3's suppression is deliberate: an attacker who
can drive the user's browser to the loopback endpoint must not be able to reflect arbitrary text
into the page for a `state` the adapter does not recognise.

**Branch 4's map effects are conditional and only its rejection is deferred.** The response is
written first; then, *synchronously and only when the state was pending*, the reservation is
removed, the timer cleared and the entry deleted; then the waiter is rejected on a later tick. If
the state was merely **reserved** and never awaited, **nothing is removed and the reservation
survives** — a user who gets `?error=access_denied` on the headless path can retry the same state
and still be served by branch 7. This is an explicit upstream contract, driven end to end by
`__tests__/mcp-callback-server-manual.test.ts`.

Branch 7 is the headless path: `startAuth` reserves the state *before* the browser opens, and
`waitForCallback` — called later, from `authenticate` — is what promotes a reservation to a pending
wait (it deletes from the reservation set first). A user who runs `mcp({action:"auth-start"})` on a
remote box and never calls `auth-complete` gets branch 7.

`waitForCallback(state)` arms a `CALLBACK_TIMEOUT_MS = 5 * 60 * 1000` timer that, **only if the
state is still pending**, deletes it and rejects with
`OAuth callback timeout - authorization took too long`. This timer is **not** unref'd, unlike the
flow-side abandon timer. `cancelPendingCallback(state)` deletes the reservation and, if pending,
clears the timer and rejects with `Authorization cancelled`.

**Mapping onto the reused listener.** The multiplexer is a `CallbackHandler` whose `handle` never
calls `CallbackControl::claim` and always returns `CallbackOutcome::Continue { reply }`, so the
server's own settle-once path is never taken and its 409 "already used" branch is unreachable. Two
behaviours come from the server rather than the handler and are named deltas: a request for a
foreign path is answered by `serve_connection` with the HTML error page at 404 rather than
`text/plain` `Not found`, and a malformed request gets a 400 HTML page that upstream has no branch
for (Node's parser handles it below the handler). Both are cosmetic; neither is worth forking the
listener over. Write the response and shut down the write half **before** settling any channel.

#### 3 · The three HTML pages

Built **per request** so a host that resolves its app name late is still named correctly.
`app = escapeHtml(getAppName())`. The page template emits, exactly:

```
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>${title}</title>
  <style>${PAGE_STYLE}
  </style>
</head>
<body>
  <main class="card">
    <div class="badge ${tone}">${icon}</div>
    <h1>${heading}</h1>
    <p>${body}</p>
    ${extra ?? ""}
  </main>
${autoClose ? "  <script>setTimeout(() => window.close(), 2000);</script>\n" : ""}</body>
</html>
```

`PAGE_STYLE` is inline CSS, dark by default (`background:#0f1117`, `color:#e6e8ee`, card
`#161922`/`#242938`, `.ok` `#4ade80`, `.bad` `#f87171`) with a `@media (prefers-color-scheme: light)`
override. The check and cross icons are inline `<svg viewBox="0 0 24 24">` **with no `xmlns`
attribute** — that omission is what makes the page contain zero `https?://` occurrences, which the
upstream test asserts.

| page | title | heading | body | icon/tone | extra | auto-close |
| --- | --- | --- | --- | --- | --- | --- |
| `htmlSuccess()` | `${app} — Authorization Successful` | `Authorization Successful` | `You can close this window and return to <span class="app">${app}</span>.` | check / `ok` | — | **yes, 2000 ms** |
| `htmlManualSuccess()` | `${app} — Authorization Received` | `Authorization Received` | `Copy the full callback URL from your browser address bar and paste it back into <span class="app">${app}</span> with auth-complete.` | check / `ok` | — | no |
| `htmlError(error)` | `${app} — Authorization Failed` | `Authorization Failed` | `Something went wrong during authorization. You can close this window and try again from <span class="app">${app}</span>.` | cross / `bad` | `<code>${escapeHtml(error)}</code>` | no |

The separator in every title is an em dash, U+2014, not a hyphen. `escapeHtml` replaces, in order,
`&`→`&amp;`, `<`→`&lt;`, `>`→`&gt;`, `"`→`&quot;`, `'`→`&#39;` — identical to
`cyrup_provider::auth::oauth::page::escape_html`, which is reusable verbatim and is the **only**
part of that module that transfers. The provider-controlled error text is escaped in the **served
HTML** but the **rejection message carries the raw original**; both halves are asserted upstream.

cyrup's own `oauth_success_html` / `oauth_error_html` are incompatible in both directions: they
embed a logo SVG carrying an `xmlns` URL (their own test asserts exactly one `http://` occurrence,
this page family requires zero) and their test asserts the page contains no `<script>`, while this
success page requires one. Different titles, headings, DOM and CSS, and no app-name interpolation.

#### 4 · Flow ownership, generations and the four maps

`McpOAuthRuntime` is `{ readonly signal: AbortSignal }`. All mutable flow state hangs off a
`WeakMap<McpOAuthRuntime, RuntimeState>` plus a `Set` of live runtimes:

| field | purpose |
| --- | --- |
| `controller` | aborted by `shutdownOAuth` with `new Error("OAuth runtime stopped")` |
| `generation` | bumped by `shutdownOAuth`; `setPendingAuth` refuses to publish if it moved |
| `pendingAuths` | keyed by `` `${serverName}|${authBaseDir}` `` |
| `pendingAuthStates` | same key → the flow's `state` nonce |
| `pendingAuthCleanupTimers` | same key → the 5-minute unref'd abandon timer |
| `pendingAuthentications` | keyed by `` `${serverName}|${serverUrl}|${authBaseDir}` `` — in-flight dedup |

`createOAuthRuntime(signal?)` combines the caller's signal with a fresh controller and registers the
runtime. A module-level legacy runtime is created at import and **removed from the live set
immediately**, so a process that never calls `createOAuthRuntime` still has a working default that
does not by itself keep the shared listener alive. `getRuntime(options)` either adopts an explicit
runtime (after `throwIfAborted`) or recreates the legacy one if it has been aborted; either way it
**adds** to the live set.

`shutdownOAuth(runtime)` runs in exactly this order: return early if already aborted; increment the
generation; abort the controller with `OAuth runtime stopped`; `cancelPendingCallback` every
recorded state; `clearPendingAuth` every pending auth; clear the dedup map; remove the runtime from
the live set; **and only if the set is now empty, stop the callback listener.**

`setPendingAuth` clears any prior pending auth for the key, re-checks the abort signal **and the
generation captured before the await** (`if (generation !== state.generation) throw new
Error("OAuth runtime stopped")`), publishes into both maps, and arms a
`MANUAL_AUTH_TIMEOUT_MS = 5 * 60 * 1000` timer whose failure path logs
`` `MCP Auth: Timed-out flow cleanup failed: ${formatTerminalError(error)}` ``. The timer is
**unref'd**. The generation check is why the counter must stay an explicit `AtomicU64` rather than
collapsing into a cancel token: it is compared against a value captured *before* an await point.

`clearPendingAuth(runtime, serverName, oauthState?, fallbackStorageOptions)`: key from the server
name plus the fallback options, but the **stored** `pendingAuth.authStorageOptions` wins for the
later store operations; **if `oauthState` is given and the recorded state differs, return without
doing anything** — this is the guard that stops a stale 5-minute timer from destroying a *newer*
flow for the same server. Then: clear and delete the timer, deactivate the provider, delete from
both maps, `cancelPendingCallback(recordedState ?? oauthState)`, and — only if the *persisted*
`getOAuthState(serverName)` equals that same state — clear the persisted state.

`hasPendingAuth(serverName, options?, runtime?)` does an exact key lookup with options and a linear
scan without. Its only consumer is the lifecycle manager's idle-shutdown suppression: a server with
an OAuth flow in flight must not be reaped.

#### 5 · `extractOAuthConfig` — the validation gate and its exact messages

Input is the `oauth` member of a `ServerEntry`: `grantType`, `clientId`, `clientSecret`, `scope`,
`authorizationParams`, `redirectUri`, `clientName`, `clientUri`, `logoUri`,
`skipIssuerMetadataValidation`. `oauth === false` yields `{}`. Evaluated in source order; every
check throws a bare `Error` whose message is the contract:

| field | checks, in order | message |
| --- | --- | --- |
| `grantType` | copied verbatim, **not validated** | — |
| `clientId` | must be a string; then env interpolation | `OAuth clientId must be a string` |
| `clientSecret` | must be a string; **`!`-prefixed values are preserved un-interpolated** so the command resolver can run them later | `OAuth clientSecret must be a string` |
| `scope` | string; interpolate | `OAuth scope must be a string` |
| `authorizationParams` | non-null non-array object; each key non-empty; each value a string; each value interpolated | `OAuth authorizationParams must be an object` · `OAuth authorizationParams keys must not be empty` · ``OAuth authorizationParams.${key} must be a string`` |
| `redirectUri` | string; interpolate then trim; non-empty | `OAuth redirectUri must be a string` · `OAuth redirectUri must not be empty` |
| `clientName` | string, interpolate, trim, non-empty | `OAuth clientName must be a string` · `OAuth clientName must not be empty` |
| `clientUri` | string, interpolate, trim, non-empty | `OAuth clientUri must be a string` · `OAuth clientUri must not be empty` |
| `logoUri` | string, interpolate, trim, non-empty, **URL-parses**, **scheme is `https:` or `http:`** | `OAuth logoUri must be a string` · `OAuth logoUri must not be empty` · `OAuth logoUri must be an absolute http(s) URL` (both the parse failure and the scheme failure emit the identical string) |
| `skipIssuerMetadataValidation` | must be a boolean | `OAuth skipIssuerMetadataValidation must be a boolean` |

The `logoUri` absoluteness rule exists because consent screens fetch the logo *server-side*, so a
relative or `file:` path renders nothing and the failure is otherwise invisible.

Env interpolation (`utils.ts` `interpolateEnvVars`) expands **three** placeholder forms, in order,
each falling back to the empty string on a missing variable:

```
1.  ${VAR}       /\$\{(\w+)\}/g
2.  $env:VAR     /\$env:(\w+)/g
3.  {env:VAR}    /\{env:(\w+)\}/g      ← the form both cyrup copies are missing
```

cyrup has two copies of this algorithm and **neither implements form 3**:
`cyrup_ext::caps::proc::interpolate_env_vars` (currently `pub(crate)`) and a private one in
`cyrup_ext_subagents::exec::mcp_direct_tools`. This is a pre-existing parity defect in both, not a
visibility problem — see MCP-342.

#### 6 · `parseOAuthRedirectUri` — loopback-only redirect validation

Returns `{ port, callbackHost, callbackPath }`, in this order:

1. parse the URL; on failure `` `Invalid OAuth redirectUri: ${redirectUri}` `` with the parse error
   as cause;
2. lowercase the hostname; `isLocalhost` is membership in exactly
   `{"localhost", "127.0.0.1", "[::1]", "::1"}`. A non-`http:` scheme **or** a non-loopback host ⇒
   `OAuth redirectUri must be an http:// localhost or loopback URI`. **Note the asymmetry:** only
   these four literals pass — `127.0.0.2`, `0.0.0.0` and every other `127/8` address are rejected,
   narrower than RFC 8252 §7.3. The TypeScript SDK's own `isLoopbackHost` uses the identical four
   literals; they agree;
3. userinfo present ⇒ `OAuth redirectUri must not include username or password`;
4. fragment present ⇒ `OAuth redirectUri must not include a fragment`;
5. empty port ⇒ `OAuth redirectUri must include an explicit numeric port`;
6. port not an integer, `<= 0`, or `> 65535` ⇒ **the same message as step 5**;
7. `callbackHost = hostname === "[::1]" ? "::1" : hostname`; `callbackPath = url.pathname`.

Five distinct messages across six throw sites. The order is observable: a non-loopback URL that also
carries a fragment reports the loopback error, not the fragment error. Step 7 strips the brackets
step 2 required, because the bind call wants a bare `::1`;
`cyrup_provider::auth::oauth::callback::bracket_host` does the inverse and the port needs both
directions.

#### 7 · The acquisition sequence, and the rmcp split

For a stock `{"url":"https://api.example.com/mcp"}` server with no configured `clientId`:

| # | actor | step | who owns it |
| --- | --- | --- | --- |
| 1 | adapter | provoke a 401 to read `WWW-Authenticate` (or, preferred, take the challenge off the failed connect — see MCP-309) | `cyrup-mcp` / `rmcp` |
| 2 | server | `401` + `WWW-Authenticate: Bearer resource_metadata="…", scope="…"` | — |
| 3 | adapter | start the callback listener with `strictPort:false`, reserving the state; **port now known** | `cyrup-mcp` |
| 4 | rmcp | `GET <resource_metadata URL>` (RFC 9728), with `.well-known` path-insertion and root fallbacks when no header was seen | `rmcp` |
| 5 | rmcp | `GET` the AS metadata in three-URL priority order (RFC 8414 then the two OIDC forms), enforcing the issuer echo | `rmcp` |
| 6 | rmcp | `POST <registration_endpoint>` (RFC 7591), skipped when a `client_id` is pre-registered or CIMD applies | `rmcp` |
| 7 | adapter | persist `{client_id, client_secret?, issuer}` in the keychain | `cyrup-mcp` `CredentialStore` |
| 8 | rmcp | build `<authorization_endpoint>?response_type=code&client_id=…&code_challenge=…&code_challenge_method=S256&redirect_uri=…&state=…&scope=…&resource=…`; store the verifier, expected issuer and requested scopes under the CSRF token | `rmcp` |
| 9 | adapter | publish the pending auth and return the URL | `cyrup-mcp` |
| 10 | adapter | **register the callback waiter, then surface the URL, then open the browser** — in that order, always | `cyrup-mcp` |
| 11 | browser | `GET http://localhost:<port>/callback?code=…&state=…[&iss=…]` → handler branch 8 | `cyrup-mcp` |
| 12 | adapter | the RFC 9207 pre-check with its friendlier message, then hand `(code, state, iss)` to rmcp | `cyrup-mcp` |
| 13 | rmcp | validate `iss` against the stored state, **then** delete the state, **then** `POST <token_endpoint>` with `grant_type=authorization_code`, `code_verifier`, `redirect_uri`, `resource` | `rmcp` |
| 14 | adapter | tokens land in the keychain through `CredentialStore::save`; the pending auth is cleared | `cyrup-mcp` |

**Order matters at step 13 and rmcp gets it right for free.** `exchange_code_for_token_with_issuer`
validates the issuer *before* consuming the stored state, so a callback bearing the correct `state`
but a forged or missing required `iss` does not discard the PKCE verifier the legitimate callback
still needs. That is precisely the property the adapter's `keepPendingForRetry` flag exists to
provide, and it means the port's own gate can be a pure message-quality wrapper.

**Client-registration priority.** `AuthorizationSession::new` implements the MCP spec order:
pre-registered client information first (`AuthorizationRequest::with_preregistered_client` +
`with_client_secret`), then CIMD when the AS advertises `client_id_metadata_document_supported` and
the caller supplied a `with_client_metadata_url`, then dynamic registration. Passing no scopes lets
rmcp auto-select from the challenge hint, protected-resource metadata, or AS metadata; passing
scopes explicitly still appends `offline_access` when the AS advertises it (SEP-2207). The adapter
does not publish a CIMD document, so the port supplies no `client_metadata_url`.

**Compatibility assertions.** rmcp's `validate_server_metadata("code")` refuses an AS whose
`response_types_supported` omits `code` (`AuthError::InvalidScope`) and whose
`code_challenge_methods_supported` is present without `S256` (`AuthError::PkceUnsupported`) — the
two "Incompatible auth server" assertions, one for one.

**`authorizationParams` and the reserved-key guard.** `addAuthorizationParams` clones the
authorization URL and, for each configured entry, throws
``OAuth authorizationParams.${key} cannot override an authorization flow parameter`` when the key is
in the reserved set **or already present on the URL**; otherwise it sets it. The reserved set has
**eight** members: `client_id`, `code_challenge`, `code_challenge_method`, `redirect_uri`,
`resource`, `response_type`, `scope`, `state`. Because rmcp builds the URL and the adapter decorates
it afterwards, the "already present" half of the check does all the work and must not be dropped.

**The `redirectToAuthorization` fence.** Upstream, the transport-attached provider is constructed
with no initial `state` and a no-op `onRedirect`, so when the SDK falls through a failed refresh into
a fresh authorization it throws
``UnauthorizedError(`Re-authentication required for MCP server: ${serverName}`)`` and the connection
is classified `needs-auth` instead of opening a browser from inside a tool call. A **stale on-disk
`oauthState` for a different server URL must not unblock it.** In the port there is no provider
callback to fence — the connect path never calls `start_authorization`; it inspects
`ClientInitializeError::auth_challenge()` and classifies. The invariant to preserve is behavioural:
**a 401 during a tool call never opens a browser.**

#### 8 · The manual / headless leg

`getSearchParamsFromInput(input)`: try to parse the whole string as a URL and take its query, then
merge in any fragment parameters not already present; on parse failure, treat the text after the
first `?` (or the whole string minus a leading `#`) as a query string and return it **only if it
contains `code`, `state` or `error`** — otherwise nothing.

`parseAuthorizationRedirectInput(input, expectedState?) -> {code, iss?}`:

1. trim; empty ⇒ `Authorization code or redirect URL is required`;
2. if parameters were found:
   * `error` present ⇒ throw ``${error}: ${description}`` when `error_description` exists, else the
     bare `error`;
   * `expectedState` set and no `state` ⇒ `OAuth state missing from redirect URL`;
   * `expectedState` set and `state` differs ⇒ `OAuth state mismatch - potential CSRF attack`;
   * `code` present ⇒ return it, with `iss` when non-null;
3. otherwise, if the whole trimmed input matches `/^[A-Za-z0-9._~+/=-]+$/`, treat it as a bare code;
4. else `Could not find an OAuth authorization code in the provided input`.

rmcp's `AuthorizationCallback::from_redirect_url` covers only the strict "a real URL with `code` and
`state`" case, with its own messages, and has no bare-code, fragment-merge or expected-state
comparison. The adapter's parser is the one that ports; rmcp's is not a substitute.

`waitForAuthorizationResponse(callbackPromise, url, expectedState, onAuthorizationInput?, signal?)`:
with no input prompt it simply awaits the callback tagged `"callback"`. Otherwise it races the
callback against the prompt, wrapped in the caller's abort, with the prompt's own controller
aborted in a `finally` **so the prompt is dismissed however the race ends**. A `"callback"` winner
returns immediately. A `"manual"` winner with empty or whitespace input ⇒
`OAuth authentication cancelled`. A `"manual"` winner whose input yields no parameters ⇒
`Paste the full OAuth callback URL, including its code and state parameters` — **a bare code is
rejected on this path**, even though `parseAuthorizationRedirectInput` would accept one. An external
abort must reject with the **identical reason value**, not a wrapped copy.

`completeAuthFromInput(serverName, input, options)` resolves the runtime, looks up the pending state
for the server, parses the input against it, and delegates.

`completeAuth(serverName, code | {code, iss?}, options)`:

1. resolve runtime, signal and key; a pending auth **must** exist, else
   ``No pending OAuth flow for server: ${serverName}``;
2. read the flow's discovery state; derive `expectedIssuer` and
   `requiresIssuer = authorization_response_iss_parameter_supported === true`;
3. **`expectedIssuer` known, no `iss` supplied, and the AS requires it** ⇒ set `keepPendingForRetry`
   and throw

   > ``The authorization server for ${serverName} requires the RFC 9207 "iss" parameter. Paste the full redirect URL from the browser address bar (not just the authorization code).``

   This is the **only** path that leaves the pending auth alive, so the user can paste again without
   restarting the flow. Model it as an explicit flag, not an error-type match;
4. `expectedIssuer` known, `iss` supplied and different ⇒ throw
   ``The OAuth authorization response issuer does not match the discovered issuer for ${serverName}.``
   and **do not** keep the flow pending;
5. exchange under the caller's abort; a non-`AUTHORIZED` result ⇒
   `UnauthorizedError("Failed to authorize")`; otherwise `"authenticated"`;
6. `finally`, unless `keepPendingForRetry`: clear the pending auth; a cleanup failure becomes an
   aggregate under `OAuth completion cleanup failed`, or is rethrown alone when the body succeeded.

`authenticate` wraps `startAuth`: disabled check; runtime; an in-flight promise for
`` `${serverName}|${serverUrl}|${authBaseDir}` `` is returned as-is; an empty `authorizationUrl`
means a live refresh already sufficed and the answer is `"authenticated"`; the pending `oauthState`
is read back **inside** the cleanup boundary (a missing one is
`OAuth state not found - this should not happen`); **the callback waiter is registered before the
browser opens**; the URL is surfaced through `options.onAuthorizationUrl` — or logged as
``MCP Auth: Open this URL to authenticate ${serverName}:\n${authorizationUrl}`` — **always before the
browser handoff**, so remote users are never stranded; a non-abort browser-launch failure logs
``MCP Auth: Failed to open browser for ${serverName}; waiting for manual callback`` and the flow
**continues**; a `"manual"` winner cancels the pending callback; failures cancel the callback and
clear the pending auth, aggregating a failing cleanup under `OAuth cancellation cleanup failed`; and
the dedup entry is removed in a `finally` **only if it is still the one this call installed**.

#### 9 · `startAuth` — ordering, stale registration, aggregate cleanup

1. disabled server ⇒ ``MCP server "${serverName}" is disabled``;
2. resolve runtime, config, storage options, combined abort signal, and capture the generation;
3. `client_credentials` short-circuit (§10);
4. **idempotency**: an existing pending auth for the key whose `serverUrl` matches returns its
   stored `authorizationUrl` unchanged; a *different* URL falls through and is replaced;
5. parse `config.redirectUri` if present;
6. `oauthState = generateState()` — 32 CSPRNG bytes rendered as **64 lowercase hex characters**.
   Nothing validates the format on the way back in (the callback matches by map lookup), so a port
   that substituted base64url would pass every test while emitting URLs some strict authorization
   servers reject on character class. rmcp generates its own CSRF token, so **this nonce is only
   the adapter's own map key** unless the port chooses to feed it to rmcp;
7. start the callback listener with `strictPort = Boolean(config.clientId) || config.redirectUri !== undefined`,
   reserving the state and passing the parsed host/port/path when a `redirectUri` was configured;
   then re-check the abort. On **any** throw: release the reserved state, clear the persisted OAuth
   state, and if *that* also throws, raise an aggregate under `OAuth startup cleanup failed`;
8. construct the flow's authorization request. **Step 7 must precede step 8** (§1);
9. **stale-registration checks**, only when stored client info exists **and no `config.clientId`**:
   * **no tokens** ⇒ clear client info, code verifier and OAuth state;
   * **tokens present** ⇒ if the stored `redirectUris` is not an array, or does not include the
     current redirect URL, clear client info **and tokens** and code verifier and OAuth state. A
     callback-endpoint change therefore forces re-registration *and* discards the tokens, because
     those tokens belong to a client whose registered redirect URI no longer matches. Changing
     `MCP_OAUTH_CALLBACK_PORT` or adding an `oauth.redirectUri` triggers exactly this;
10. probe/seed discovery, then run the acquisition under `abortable`;
11. an `AUTHORIZED` result (a live refresh sufficed) ⇒ deactivate, release the reserved state, clear
    the persisted state, return an **empty** `authorizationUrl`;
12. no captured URL ⇒ `UnauthorizedError("OAuth authorization URL was not provided")`;
13. publish the pending auth and return the URL;
14. catch-all: deactivate, then clear the pending auth; a failing cleanup becomes an aggregate under
    `OAuth startup cleanup failed`.

There are **four** aggregate-cleanup sites in total, with three distinct phase strings —
`OAuth startup cleanup failed` (twice), `OAuth completion cleanup failed`,
`OAuth cancellation cleanup failed` — plus the `completeAuth` case that rethrows the cleanup error
alone when the body succeeded. Every one of these surfaces to the user through
`Failed to authenticate "<n>": <message>`, so a port that collapses to the primary error makes the
secondary permanently invisible.

#### 10 · `client_credentials`, refresh, status, removal, and the OAuth predicate

**`client_credentials`.** No callback listener, no `state`, no browser. Stale-registration hygiene
first (clear client info, code verifier and OAuth state when client info exists without tokens and
no `config.clientId` is set) — the same rule as the interactive path but **without** the
redirect-URI check. Then acquire; a non-`AUTHORIZED` result is
`UnauthorizedError("Failed to authorize")`; the provider is deactivated in a `finally`. Three
methods throw grant-specific errors if reached:
`redirectToAuthorization is not used for client_credentials flow`,
`codeVerifier is not used for client_credentials flow`,
`state is not used for client_credentials flow`. The RFC 9207 check is skipped entirely on this
path. In the port this is `OAuthState::authenticate_client_credentials` with
`ClientCredentialsConfig::ClientSecret { client_id, client_secret, scopes, resource }`; rmcp selects
`client_secret_post` for it by default and switches to Basic only when the AS advertises
`client_secret_basic` alone. Machine-to-machine servers authenticate with no interaction, including
in `rpc`/`print` modes where auto-auth explicitly allows it.

**`getValidToken`.**

1. no tokens ⇒ `null`;
2. `isTokenExpired` is **tri-state**: `null` when there are no tokens, `false` when there is no
   `expiresAt` **or** it lies in the future, `true` otherwise. `false` ⇒ return the tokens;
3. expired **and** a refresh token exists ⇒ log
   ``MCP Auth: Token expired for ${serverName}, attempting refresh``, build the refresh context with
   an **empty config**, and:
   * no client info ⇒ log ``MCP Auth: No client info for refresh for ${serverName}`` and return
     `null`;
   * discovery runs with **no server definition, so no configured headers are sent on the refresh
     probe** — for a server that gates even its 401 behind a tenant header, login works and refresh
     silently fails. rmcp reproduces the asymmetry for a different reason: its discovery uses its own
     HTTP client, not the transport's configured headers. Name it either way;
   * a non-`AUTHORIZED` result ⇒ `null`; otherwise re-read and return the stored tokens;
   * abort errors and credential-store errors **rethrow**; everything else logs
     ``MCP Auth: Token refresh failed for ${serverName}`` and returns `null`;
4. fall-through — no expiry information, or expired with no refresh token — **returns the tokens
   anyway** ("assume valid").

rmcp's `AuthorizationManager::get_access_token` covers the mechanical half: it refreshes with a
30-second proactive buffer, maps an `invalid_grant` rejection to `AuthError::TokenRefreshRejected`
and everything else to `TokenRefreshFailed`, and **preserves the existing refresh token when the
response omits one** (RFC 6749 §6). It does not implement the tri-state or the fall-through, which
are the adapter's policy and stay hand-written.

**`getAuthStatus`** resolves the runtime for its side effect of resurrecting an aborted legacy
runtime, then answers `"not_authenticated"` when no tokens are stored, else `expired ? "expired" :
"authenticated"` — so a `null` expiry reads as `"authenticated"`.

**`removeAuth`**, in order: read the persisted `oauthState`; cancel the pending callback when
present; clear the pending auth; clear all credentials; clear the persisted state; log
``MCP Auth: Removed credentials for ${serverName}``. `throwIfAborted` is interleaved at four points.

**`supportsOAuth(definition)`** — the exact truth table, and the order is observable:

| condition, in order | result |
| --- | --- |
| no `url` | `false` |
| `auth === false` | `false` |
| `oauth === false` | `false` |
| `auth === "oauth"` | `true` — **even with custom headers** |
| `headers` present and non-empty | `false` |
| `auth === undefined` | `true` |
| otherwise (`auth === "bearer"`) | `false` |

The `auth === "oauth"` row **beats** the custom-headers row. This predicate decides whether a 401
becomes `needs-auth` or a hard error, whether `/mcp-auth` is offered, and whether auto-auth fires; a
wrong answer silently disables OAuth for a whole class of servers.

#### 11 · Storage: what this flow needs from the keychain

`getAuthForUrl(serverName, serverUrl, options)` returns nothing when the stored `serverUrl` is
absent or differs — that URL binding is the mechanism that invalidates credentials on a server-URL
change, and it must not be dropped. `getAuthBaseDir` reads `MCP_OAUTH_DIR` first. The stored shape
is `AuthEntry { tokens, clientInfo, codeVerifier, oauthState, serverUrl }`, with `StoredTokens`
carrying `expiresAt` as a **Unix timestamp in seconds**. Read/write helpers are **synchronous**
upstream even where callers await them; keep them synchronous so async does not propagate into the
credential-store implementation beyond what rmcp's trait requires.

Two expiry rules that are easy to lose: `expires_in` is written as `now_secs + expires_in` **even
when `expires_in === 0`**, so an already-expired token stays expired rather than becoming
never-expiring; and `token_type` is hardcoded `"Bearer"` on read.

`cyrup_provider::auth::store::CredentialStore` and `cyrup_config`'s `auth.json` are **not** this
model — one credential per `ProviderId`, millisecond expiry, no client registration, no URL
binding. `cyrup-mcp` owns its own store and does **not** reuse `Credential::Oauth`, and it must not
silently convert milliseconds to seconds.

**The rmcp seam.** `rmcp::transport::auth::CredentialStore` is `load`/`save`/`clear` with **no key**,
so `cyrup-mcp` instantiates one keyring-backed store per server, bound to that server's account key.
That is the natural shape, not a workaround. `StoredCredentials` is
`{ client_id, token_response, granted_scopes, token_received_at, issuer }` — **no client-secret
field** — so `initialize_from_store` restores only the client id, and does it through
`configure_client_id`, which additionally sets `redirect_uri` to the manager's base URL. A
confidential pre-registered client therefore loses both its secret and its redirect URI across a
restart unless the port re-applies them. The fix is ~20 lines and uses rmcp's public API: persist the
registration fields as a second keychain record and, after `initialize_from_store()`, call
`configure_client(OAuthClientConfig::new(client_id, redirect_uri).with_client_secret(secret))`.
`initialize_from_store` does **not** destroy credentials on an `invalid_client`, so the failure mode
is a refresh that fails loudly rather than silent credential loss.

`rmcp::transport::auth::StateStore` holds `{pkce_verifier, csrf_token, expected_issuer,
require_issuer, created_at, requested_scopes}` keyed by CSRF token. The adapter keeps the equivalent
**in memory only** — a deliberate change from an older on-disk model — so `InMemoryStateStore` is
the faithful choice and no keychain writes happen for verifiers. Do not "improve" this by persisting
it.

#### 12 · `!command` secret resolution

`resolveCommandSecret(value, context)`: `COMMAND_SECRET_TIMEOUT_MS = 10_000`,
`COMMAND_SECRET_MAX_OUTPUT_BYTES = 1024 * 1024`. Then:

* `!!X` ⇒ interpolate `X` with one `!` stripped, **no subprocess**;
* a value not starting with `!` ⇒ plain interpolation;
* otherwise `spawnSync(value.slice(1), { shell: true, timeout: 10_000, maxBuffer: 1 MiB,
  stdio: ["ignore","pipe","ignore"], windowsHide: true })`.

Five failure strings, all prefixed `Failed to resolve ${context}: ` — `command timed out after 10
seconds`, `command output exceeded 1 MiB`, `command failed to start`,
``command exited with code ${status ?? "unknown"}``, `command returned empty output`. Success is
stdout trimmed. **`shell: true` means the string goes to `/bin/sh -c` (or `cmd.exe`)** — a port that
spawns the argv directly changes which configs work. **stderr is discarded**, so a failing command's
diagnostics never reach the user; only the exit code does. The context string this flow supplies is
``MCP server "${serverName}" OAuth clientSecret`` and it appears verbatim in all five messages.

This is `extension-owned`: a native crate runs `std::process::Command` directly. It does **not** go
through `HostServices::exec` — that verb is the WASM-guest capability gate, and routing through it
would make an `!command` secret fail for reasons upstream has no analogue for.

Upstream calls `clientInformation()` — and therefore this resolver — up to three times per token
leg, so a single token request can spawn the user's secret command three times. Under rmcp the
secret is applied once at `configure_client` time and reused, so **the port naturally resolves it
once per configuration**. That is a divergence in observable subprocess count and should be recorded
rather than discovered.

#### 13 · Where this subsystem is consumed, and every user-facing string

| consumer | upstream symbol | behaviour |
| --- | --- | --- |
| `/mcp-auth <server>` | `commands.ts` `authenticateServer` | message table below |
| `mcp({action:"auth-start"})` | `proxy-modes.ts` `executeAuthStart` | returns `formatManualAuthInstructions` |
| `mcp({action:"auth-complete"})` | `proxy-modes.ts` `executeAuthComplete` | on success **closes the server connection** so the next `connect` uses the new token, clears the failure, updates the status bar |
| `settings.autoAuth` | `proxy-modes.ts` `attemptAutoAuth` | fires **only** when `autoAuth === true`; with no UI it refuses everything except `client_credentials` |
| direct tools | `direct-tools.ts` | same auto-auth shape, **different message literal** |
| transport | `server-manager.ts` `createAuthProvider`, `HttpAuthProviderState` | below |
| lifecycle | `init.ts` | `hasPendingAuth` suppresses idle shutdown |
| package export | `oauth.ts` | `getMcpOAuthTokensForUrl` → `getValidToken` (may refresh); `inspectMcpOAuthTokensForUrl` → `inspectAuthForUrl` (never refreshes, exposes **only** tokens — never client info, code verifier or OAuth state); `updateMcpOAuthTokensForUrl` → `updateTokens` |

**Transport integration.** `HttpAuthProviderState` is a four-variant union:
`disabled` / `implicit-deferred` / `implicit-challenged` / `explicit`. The assignment is
`supportsOAuth ? (definition.auth === undefined ? implicit-deferred : explicit{provider}) :
disabled` — **explicit OAuth touches the credential store at connect time; implicit OAuth defers
until the server proves it needs auth**, so a non-OAuth HTTP server never triggers a keyring read. On
a 401, `implicit-deferred` promotes to `implicit-challenged` and retries **once**; a second 401 with
`supportsOAuth` invalidates the auth-entry cache **once** and yields `needs-auth`.
`skipIssuerMetadataValidation` is forwarded into the transport only when a provider exists and the
config asked for it. In the port the 401 detection is
`ClientInitializeError::{auth_challenge, is_authorization_required}`, and the challenge string feeds
`AuthorizationRequest::with_challenge`.

**Message table.** Every string is a literal to reproduce.

| trigger | message |
| --- | --- |
| no interactive UI | `OAuth authentication requires an interactive session.` |
| unknown server | `Server "<n>" not found in config` |
| disabled server | `Server "<n>" is disabled. Run /mcp enable <n>, then /reload.` |
| not an OAuth server | `Server "<n>" does not use OAuth authentication. Set "auth": "oauth" or omit auth for auto-detection.` (the `notify` variant breaks after the first sentence with `\n`) |
| no URL | `Server "<n>" has no URL configured (OAuth requires HTTP transport)` |
| in progress | status key `mcp-auth` ← `Authenticating <n>...`, cleared in `finally` **unless the signal aborted** |
| URL surfaced (TUI) | `Open this URL to authenticate <n>:\n\n<hyperlink>\n\nAfter approving, Pi will complete automatically if the browser can reach its localhost callback. On a remote machine, copy the full localhost URL from the browser address bar and paste it into Pi.` |
| manual prompt | confirm titled `Authorize <n>`, body `Open this link in your browser:\n<hyperlink>\n\nAfter approving access, select Yes to paste the callback URL.`; then input titled `Complete <n> OAuth`, placeholder `Paste the full callback URL` |
| success | `OAuth authentication successful for "<n>".` |
| non-`authenticated` status | `OAuth authentication failed for "<n>".` |
| thrown | `Failed to authenticate "<n>": <message>` |
| `auth-start`/`auth-complete`, unknown server | `Server "<n>" not found. Use mcp({}) to see available servers.` |
| `auth-start`, not OAuth | `Server "<n>" is not configured for OAuth over HTTP.` |
| `auth-start`, already authorized | `OAuth authentication successful for "<n>".` |
| `auth-start` threw | `Failed to start OAuth for "<n>": <message>`, `details.error = "auth_start_failed"` |
| `auth-complete` incomplete | `OAuth authentication did not complete for "<n>".`, `details.error = "not_authenticated"` |
| `auth-complete` success | `OAuth authentication successful for "<n>". Run mcp({ connect: "<n>" }) to connect with the new token.` |
| `auth-complete` threw | `Failed to complete OAuth for "<n>": <message>`, `details.error = "auth_complete_failed"` |
| auto-auth, no UI (proxy) | `Server "<n>" requires OAuth authentication. Run mcp({ action: "auth-start", server: "<n>" }) to get a browser URL, or /mcp-auth <n> in an interactive local session.` (overridable by `settings.authRequiredMessage`, which templates `${server}`) |
| auto-auth, no UI (direct tools) | **`MCP server "<n>" requires OAuth authentication. …`** — the same sentence with an `MCP server` prefix, a genuinely different literal |
| auto-auth failed | `OAuth authentication failed for "<n>": <message>. ` + the auth-required text |

**`formatManualAuthInstructions` emits six lines, not nine.** The function builds a ten-element
array with `""` separators and then filters out every falsy element before joining on `\n`, so the
rendered block has **no blank lines**:

```
MCP OAuth required for "<n>".
Open this URL in your local browser:
<authorizationUrl>
After approving, copy the full redirected localhost URL from your browser address bar and send it back with:
mcp({ action: "auth-complete", server: "<n>", args: { redirectUrl: "PASTE_REDIRECT_URL_HERE" } })
You can also pass just the `code` query parameter as `args: { code: "PASTE_CODE_HERE" }`. JSON-string args remain supported.
```

When `getRedirectPort` (re-parsing `redirect_uri` out of the authorization URL) yields a port, the
port note begins with a literal `\n`, so joining produces one blank line followed by
`The redirect URL will use local port <p>. On a remote server it is expected for that localhost page to fail locally; copy the address bar URL anyway.`

#### 14 · `OAUTH.md` versus the code — eight divergences the port must fix, not reproduce

1. **`oauth.logoUri` is undocumented.** The options list omits it, though the code validates it,
   declares it, advertises it in both client-metadata shapes and tests it.
2. **The rebranding rules are missing.** The doc says `clientName` defaults to `Pi Coding Agent` and
   `client_uri` to the adapter repository URL. Under a rebranded host the actual defaults are the
   host's own name and **no `client_uri` at all**.
3. **Discovery order is stated backwards.** The doc lists RFC 9728 well-known first and
   `WWW-Authenticate` second. In the code the adapter reads `WWW-Authenticate` **first** and the
   resulting `resource_metadata` URL then **suppresses** the well-known path and its root fallback
   entirely. The `.well-known` probe is the fallback, not the primary.
4. **RFC 9207 is absent from the doc.** `completeAuth` hard-refuses a bare authorization code
   whenever the AS advertises `authorization_response_iss_parameter_supported`, and refuses a
   mismatched `iss`. The doc says "You can also pass only the `code` query parameter" with no caveat.
5. **The `19876` example is wrong for the default flow.** The doc shows a redirect on port 19876,
   but a dynamically-registered client always binds port `0` — as the doc's own troubleshooting
   section says. 19876 is reached only when `oauth.clientId` is set without `oauth.redirectUri`.
6. **"`oauth.redirectUri` is ignored for `client_credentials`" is half true.** The redirect snapshot
   is `undefined` for that grant and the loopback parser never runs — but `extractOAuthConfig` still
   applies its string and non-empty checks and will still throw for a `client_credentials` server.
7. **The loopback allowlist is understated.** The doc names three literals; the code accepts
   **four**, including the unbracketed `::1`.
8. **The reserved-parameter list is short by one.** The doc enumerates seven; the code's reserved set
   has **eight**, adding `code_challenge_method`. The doc hedges with "like", but an incomplete
   enumeration of a security-relevant deny-list reads as exhaustive.

Two doc statements are **correct** and worth pinning because they are easy to lose: a fresh browser
authorization re-registers a cached dynamic client whose stored redirect URIs are missing or stale,
but **token refresh performs no such check**; and the 5-minute pending timeout is really **two
independent timers** (`CALLBACK_TIMEOUT_MS` on the waiter, `MANUAL_AUTH_TIMEOUT_MS` on the abandoned
flow).

---

### Port units

**MCP-300 — The OAuth subsystem as one shippable unit** · n/a · S · `hand-written`
**upstream** — `mcp-auth-flow.ts`, `mcp-oauth-provider.ts`, `mcp-callback-server.ts`, `OAUTH.md`,
plus eight test files that are the executable spec.
**behavior** — nothing here is independently useful: a flow with no listener cannot complete, and a
listener with no flow state cannot route a `state`. The unit of shipping is MCP-301 through MCP-349
together.
**cyrup** — a `cyrup_mcp::auth::{flow, session, callback, store}` module tree, one `McpOAuthRuntime`
per extension state, sharing one process-wide listener.
**verify** — MCP-347's suite green end to end against a stub authorization server.

**MCP-301 — Flow ownership: runtime, generation counter, four maps** · high · M · `hand-written`
**upstream** — `mcp-auth-flow.ts` `createOAuthRuntime`, `getRuntime`, `setPendingAuth`,
`clearPendingAuth`, `shutdownOAuth`, `initializeOAuth`, `hasPendingAuth`.
**behavior** — §4. A session restart aborts every in-flight login for that session and disturbs no
other's; an abandoned flow does not resurrect after its owner is gone; the shared listener outlives
any single runtime; `setPendingAuth` refuses to publish across a generation bump.
**cyrup** — `Arc<McpOAuthRuntime>` holding a `cyrup_core::CancelToken` plus a `Mutex<RuntimeState>`;
the weak map becomes ownership by the `Arc`. The generation stays an explicit `AtomicU64` — it is
compared against a value captured *before* an await, which a cancel token cannot express.
**verify** — two runtimes, shut one down: the listener is still bound and the other's pending state
survives; shut down the second: the listener is gone. Bump the generation mid-publish and assert
`OAuth runtime stopped`.

**MCP-302 — `extractOAuthConfig` and its twelve validation messages** · medium · M · `hand-written`
**upstream** — `mcp-auth-flow.ts` `extractOAuthConfig`; input shape `OAuthConfig` in `types.ts`.
**behavior** — §5. A malformed `oauth` block fails at config time with a message naming the exact
field, before any network traffic; a `logoUri` a consent screen could not fetch is rejected rather
than rendering nothing.
**cyrup** — a hand-written validator, **not** `serde` — the messages are the contract and serde's are
not reproducible. `url::Url` for the `logoUri` parse; both the parse failure and the scheme failure
emit the identical string. `!`-prefixed `clientSecret` values pass through un-interpolated.
**verify** — one case per row of §5's table.

**MCP-303 — `parseOAuthRedirectUri`'s loopback-only validation** · medium · S · `hand-written`
**upstream** — `mcp-auth-flow.ts` `parseOAuthRedirectUri`.
**behavior** — §6: six ordered checks, five distinct messages, the four-literal allowlist, and the
`[::1]`→`::1` unbracketing.
**cyrup** — `url::Url`, checks in upstream's order (the order is observable).
`cyrup_provider::auth::oauth::callback::bracket_host` is the inverse transform for the bind call.
**verify** — the seven upstream cases plus a `127.0.0.2` rejection, which upstream does not test but
which the literal comparison pins.

**MCP-304 — Callback endpoint configuration and `MCP_OAUTH_CALLBACK_PORT`** · high · S · `hand-written`
**upstream** — `mcp-oauth-provider.ts`'s `DEFAULT_OAUTH_CALLBACK_PORT` / `DEFAULT_OAUTH_CALLBACK_PATH`
and the five live accessors.
**behavior** — §1. The redirect URI advertised to the authorization server and the address the
browser is sent to must agree; the env override is read once, accepted only as an integer in
`1..=65535`, and otherwise ignored silently.
**cyrup** — a `OnceLock<u16>` for the configured port (load-time-once without a static initializer)
plus a `Mutex<(u16, String)>` for the live pair. Per the workspace rename convention already applied
in `cyrup_provider::auth::oauth::callback::callback_host`, read `CYRUP_MCP_OAUTH_CALLBACK_PORT` first
and keep `MCP_OAUTH_CALLBACK_PORT` as a lower-precedence fallback — flagged rather than assumed,
since `OAUTH.md` names the variable in user-facing text.
**cyrup** *(blocking-ness, not severity)* — a wrong value here breaks the pre-registered-client flow
outright. That is "a feature does not work", not data loss, silent wrong output, a permission bypass
or a crash; the destructive consequence lives in MCP-328 where it is observable.
**verify** — env set to `0` / `65536` / `"abc"` leaves the default; set to a valid port, it is
reported. Mutate the live values after building a request and assert neither the redirect URL nor the
registered `redirect_uris` moves.

**MCP-305 — The bind / rebind / strict-port state machine** · high · M · `hand-written`
**upstream** — `mcp-callback-server.ts` `ensureCallbackServer` and `ensureCallbackServerLocked`.
**behavior** — §1. Concurrent `startAuth` calls must not race into two listeners; a path or endpoint
change is refused, not silently applied, while any authorization is pending or reserved; a
strict-port collision tells the user exactly what to do; a failed strict rebind leaves the existing
listener serving; reservation is atomic with the bind.
**cyrup** — `cyrup_provider::auth::oauth::callback::{CallbackServer, CallbackServerConfig}`, with
`fixed(port, path)` under `strictPort` and the default (`port: 0`) otherwise. The serializing wrapper
is a `tokio::sync::Mutex` held across the bind plus an `AtomicU64` generation compared before and
after acquiring it. `EADDRINUSE` is `OAuthError::Listen`'s `source.kind() == AddrInUse`. Reproduce
both "cannot be switched while authorizations are pending" strings verbatim, including the
`host:port` and path interpolations.
**verify** — idempotent ensure; a reservation blocks a host switch; a reservation blocks a path
switch; a failed strict bind leaves **no** reservation (proved by two successive path switches
succeeding afterwards); explicit strict host+port+path; an occupied strict port yields the "already
in use" message; an occupied *configured* port under the default flow still completes on an
OS-assigned port; a failed strict rebind keeps the old listener.

**MCP-306 — The callback request handler's eight branches** · critical · M · `hand-written`
**upstream** — `mcp-callback-server.ts` `handleRequest`.
**behavior** — §2. **Severity `critical` under the permission-bypass clause**: an attacker who can
drive the user's browser to `http://localhost:<port>/callback` must not be able to inject an
authorization code into a flow (no `state` ⇒ 400; unknown `state` ⇒ 400), and must not be able to
reflect arbitrary text into the page for an unknown state (branch 3 deliberately suppresses
`error_description`). Branch 4's removals are synchronous and conditional on the state being
*pending*; a reserved-but-not-awaited state **keeps its reservation** through a provider `error`
callback and can still be completed afterwards. Branch 6 leaves the waiter pending. There is **no
HTTP method check** — do not add one.
**cyrup** — a `CallbackHandler` that never `claim`s and always returns `CallbackOutcome::Continue`,
holding its own `Mutex<(HashMap<String, Pending>, HashSet<String>)>`. Write the response and shut
down the write half **before** settling any channel.
**verify** — drive a real loopback request for each of the eight rows and assert status,
content-type and body substring; plus the two reservation cases (a reserved state serves the manual
page; an `error` callback against a reserved state leaves the reservation intact and a later `code`
against the same state succeeds).

**MCP-307 — The three callback pages, including host branding** · medium · M · `hand-written`
**upstream** — `mcp-callback-server.ts` `PAGE_STYLE`, the two inline SVG icons, `page`, `escapeHtml`,
`htmlSuccess` / `htmlManualSuccess` / `htmlError`, built per request from `getAppName()`.
**behavior** — §3. Fully self-contained (no webfont, no external asset, **zero absolute URLs**),
names the running host rather than "Pi", escapes provider-controlled error text in the page while the
rejection message keeps the raw original, and the success page auto-closes after 2000 ms.
**cyrup** — a `cyrup_mcp::auth::callback::page` module with `format!` templates. Reuse
`cyrup_provider::auth::oauth::page::escape_html` verbatim; reuse nothing else from that module — the
two page families fail each other's tests. Keep the em dash U+2014. `getAppName()` comes from the
config section; this module consumes it.
**verify** — golden-file the three pages; assert zero `https?://` matches; assert the escaping case
(served HTML escaped, rejection message raw); assert the branding cases under a fabricated app name.

**MCP-308 — Listener lifetime: reserve, wait, cancel, stop, restart, process exit** · high · M · `hand-written`
**upstream** — `mcp-callback-server.ts` `reserveCallbackServer`, `releaseCallbackServer`,
`waitForCallback`, `cancelPendingCallback`, `stopCallbackServer`, `isCallbackServerRunning`,
`getPendingAuthCount`, and the `unref` call.
**behavior** — §1. An abandoned flow never holds the process open, but a flow the user is actively
completing is not cut short; stopping rejects every waiter with a distinguishable message; a second
in-flight stop joins the first rather than double-closing; **after a completed stop a later ensure
binds again** rather than throwing forever.
**cyrup** — the reused listener's detached accept thread is the `unref` equivalent; per-state
deadlines are `tokio::time::sleep` races. **The stop gate must not be a `OnceCell`** — use a
`Mutex<Option<Shared<…>>>` taken and cleared, with an `AtomicU64` generation for the
queued-before-shutdown rejections. The deferred rejection becomes settling the channels after the
reset is visible.
**verify** — cancel rejects with `Authorization cancelled`; stop rejects two waiters with
`OAuth callback server stopped`; the pending count tracks 0→3→0; stop waits for an in-progress bind
and **permits later reuse**; a startup queued before shutdown is rejected; a startup issued while
shutdown is closing is rejected; and an integration test asserting the process exits with a pending,
never-completed callback registered.

**MCP-309 — The discovery trigger: proactive probe or reactive challenge** · medium · S · `hand-written`
**upstream** — `mcp-auth-flow.ts` `probeAuthDiscovery`: a 5000 ms-budgeted `POST <serverUrl>` carrying
a JSON-RPC `initialize` with `protocolVersion` and `clientInfo`, with headers filtered so `!command`
expressions are neither executed nor sent (`!!literal` is kept, minus one `!`), `accept:
application/json, text/event-stream`, whose **only** purpose is to read `WWW-Authenticate`. Every
failure returns nothing; an aborted caller signal re-throws. `applyOAuthConfig` then overrides the
scope with `config.scope` when set, and sets `skipIssuerMetadataValidation` only on a literal `true`.
**behavior** — discovery must work against servers that advertise `resource_metadata` only on a 401
and expose no `.well-known` path, and it must never execute a config command or leak its source text
to an unauthenticated endpoint.
**cyrup** — two shapes, both viable. **(a) Reactive** — attempt the real connect, and on failure take
`ClientInitializeError::auth_challenge()` and feed it to `AuthorizationRequest::with_challenge`; this
is what rmcp's own client example does and it costs no extra round trip. **(b) Proactive** — a
`reqwest` POST with an explicit 5-second budget selected against the caller's `CancelToken`, then
`WWWAuthenticateParams::parse` on the header. **Recommendation: (a) for the connect path, (b) only
where the adapter probes without connecting** (`/mcp-auth` on a server that is not currently
connected, and the setup panel's endpoint probe). Either way the header-filtering rule — drop `!cmd`,
keep `!!literal` with one `!` removed — must be reproduced, and rmcp's `WWWAuthenticateParams::parse`
replaces the hand-written scanner the previous edition specified.
**verify** — a 401 with a quoted `resource_metadata` yields the URL; unquoted likewise; a bare
`Bearer` with no parameters yields nothing; a non-Bearer scheme yields nothing; a 5-second hang
yields nothing; a `!cmd` header is absent from the request and a `!!lit` header is present as `!lit`.

**MCP-310 — RFC 9728 protected-resource metadata discovery** · n/a · S · `rmcp`
**upstream** — the SDK's `discoverOAuthProtectedResourceMetadata` / `discoverMetadataWithFallback`.
**behavior** — a server whose MCP endpoint sits at a sub-path must still be discoverable, with the
`.well-known` path-insertion and root fallbacks.
**cyrup** — `AuthorizationManager::resolve_metadata` (and `resolve_metadata_from_challenge` when a
challenge is in hand), whose `well_known_paths` produces `/.well-known/oauth-protected-resource` and
the `/{path}/.well-known/…` insertion forms. **Verified in the checkout.** One named delta: rmcp
does **not** send the `MCP-Protocol-Version` header on discovery GETs, which the TypeScript SDK
does. No server behaviour is known to depend on it; record it rather than patch it.
**verify** — conformance against a stub: sub-path discovery, root fallback, and no fallback when an
explicit metadata URL was supplied.

**MCP-311 — RFC 8414 + OIDC discovery and the issuer echo check** · n/a · S · `rmcp`
**upstream** — the SDK's `buildDiscoveryUrls` / `discoverAuthorizationServerMetadata`.
**behavior** — the issuer echo check is the defence against authorization-server mix-up.
**cyrup** — rmcp walks the same three URL shapes for a non-root AS path
(`/.well-known/oauth-authorization-server{P}`, `/.well-known/openid-configuration{P}`,
`{P}/.well-known/openid-configuration`) and the two root shapes, deriving the expected issuer from
the discovery URL (`expected_issuer_for_authorization_metadata_url`) and comparing with one trailing
slash tolerated (`issuer_identifiers_match`). A mismatch is
`AuthError::AuthorizationServerMismatch`; a missing issuer is `AuthorizationServerMissingIssuer`
unless `set_allow_missing_issuer(true)`. **Verified in the checkout.** See "What does not fit
cleanly" for the partial `skipIssuerMetadataValidation` mapping.
**verify** — conformance: each of the three URL shapes served in turn; a mismatched issuer fails; a
500 on the first URL aborts rather than falling through.

**MCP-312 — RFC 7591 dynamic client registration** · medium · S · `rmcp`
**upstream** — the SDK's `registerClient` with the body from `McpOAuthProvider.clientMetadata`.
**behavior** — a server with no configured `clientId` registers itself on first use and stores the
result; the registered `redirect_uris` must be the exact URI the listener is on, or every later
authorization is rejected.
**cyrup** — `AuthorizationManager::register_client`, driven through `AuthorizationSession::new`'s
priority order. rmcp defaults `application_type` to `"native"`, matching what the SDK injects for a
loopback redirect, and treats an empty-string `client_secret` in the response as no secret at all.
Two deltas: rmcp errors when the AS publishes no `registration_endpoint` rather than falling back to
`/register` on the AS origin (though its own legacy metadata fallback synthesizes `/register` when
no metadata was discovered at all); and the registration body is fixed — see MCP-313 and OA-13.
**verify** — conformance: registration body golden-compared for the public-client, rebranded-host and
`client_credentials` shapes; a missing `registration_endpoint` produces a named error.

**MCP-313 — Client metadata and the host-branding defaults** · medium · S · `hand-written`
**upstream** — `mcp-oauth-provider.ts` `defaultClientName` (`"Pi Coding Agent"` **only** on stock pi,
else the host's own name), `defaultClientUri` (the host's declared client URI, else the adapter
repository URL **only** on stock pi, else **omit the field entirely**), and the two client-metadata
shapes: `token_endpoint_auth_method` is `client_secret_post` iff a secret is configured, else `none`;
the `client_credentials` shape carries `redirect_uris: []` and `grant_types: ["client_credentials"]`.
**behavior** — a rebranded distribution must not ask users to authorize an app they have never run,
and must not advertise this adapter's repository as its homepage. Absent fields must be **absent**,
not `null`.
**cyrup** — `AuthorizationRequest::with_client_name` carries the name. `client_uri`, `logo_uri` and a
confidential `token_endpoint_auth_method` have **no** rmcp equivalent — see OA-13 in "What does not
fit cleanly". The name resolver is the config section's `getAppName()` port; this unit consumes it.
**verify** — the branding cases: stock name, rebranded name with no client URI declared (field
absent), rebranded with a host-declared client URI, and a config-supplied `clientUri` overriding
both.

**MCP-314 — Restore the full client configuration after `initialize_from_store`** · high · S · `hand-written`
**upstream** — `mcp-oauth-provider.ts` `clientInformation()` and `saveClientInformation`: the
configured-`clientId` branch that resolves a `!command` secret at read time and **never persists
it**; the config-stub suppression (explicit `configPreRegistered` marker **or** the legacy
`{clientId, issuer}`-only shape) that returns nothing so a refresh built without the matching config
does not send a `client_id` with no secret; the expired-secret and issuer-mismatch suppressions; and
the issuer back-stamp.
**behavior** — upstream's stub suppression exists to stop a secretless refresh drawing `invalid_client`
and triggering the SDK's retry into `invalidateCredentials("client")` **and** `("tokens")` — silently
destroying a working refresh token. **The port must reproduce the outcome, not the mechanism.** rmcp
has no equivalent destructive retry: an `invalid_client` surfaces as `TokenRefreshFailed` and nothing
is cleared. What rmcp *does* do is restore only `client_id` from `StoredCredentials` and set the
redirect URI to the manager's base URL, so a confidential or explicitly-redirected client is
mis-configured after a restart.
**cyrup** — persist the registration fields (`client_secret`, redirect URI, and the issuer that
minted them) as a second keychain record alongside the token record, and after
`initialize_from_store()` call
`configure_client(OAuthClientConfig::new(client_id, redirect_uri).with_client_secret(secret))`.
Resolve a `!command` secret at that point, once (MCP-349), and **never write it to the store**. Keep
upstream's expired-secret rule: a stored `client_secret_expires_at` in the past means re-register
rather than reuse.
**verify** — a confidential pre-registered client survives a restart and refreshes; a public client
restores with no secret; a stored secret past its expiry forces re-registration; a `!command` secret
is resolved but never appears in the keychain record.

**MCP-315 — The keychain-backed `CredentialStore`, and the expiry arithmetic** · high · M · `hand-written`
**upstream** — `mcp-oauth-provider.ts` `tokens()` / `saveTokens()`; `mcp-auth.ts`'s `StoredTokens`.
**behavior** — §11. `expires_in` on read is `max(0, floor(expiresAt - now_secs))`; `expiresAt` on
write is `now_secs + expires_in` **even when `expires_in` is 0**, so an already-expired token stays
expired instead of becoming never-expiring; `token_type` is hardcoded `"Bearer"`.
**cyrup** — implement `rmcp::transport::auth::CredentialStore` over `keyring` 4.1.6, one instance per
server bound to that server's account key. `cyrup-mcp` owns `StoredTokens { access_token,
refresh_token, expires_at: Option<f64> /* unix seconds */, scope, issuer }`. Do **not** reuse
`cyrup_provider::auth::types::Credential::Oauth` (per-`ProviderId`, millisecond expiry, no client
registration, no URL binding) and do not silently convert units. The keyring chunking manifest and
the Linux keyring-revoked recovery belong to the storage section.
**verify** — round-trip through a fake keyring; `expires_in: 0` round-trips as expired; a
server-URL change makes the entry unreadable.

**MCP-316 — `authorizationParams`' reserved-key guard and the no-browser-mid-turn fence** · high · S · `hand-written`
**upstream** — `mcp-oauth-provider.ts` `addAuthorizationParams` and `redirectToAuthorization`.
**behavior** — §7. `oauth.authorizationParams` may add provider extensions but may never override a
flow-owned parameter — **eight** reserved keys, plus a rejection for any key already present on the
URL; and a 401 during a tool call must surface as `needs-auth` rather than opening a browser from
inside the turn.
**cyrup** — the guard runs on the URL rmcp returned from `AuthorizationSession::get_authorization_url`,
so the "already present" half does the work. The fence is structural: the connect path never calls
`start_authorization`; it classifies from `ClientInitializeError`.
**verify** — each of the eight reserved keys is rejected with its exact message; a non-reserved key
already present on the URL is also rejected; a fresh key is set; and a 401 mid-tool-call produces
`needs-auth` with no launcher invocation.

**MCP-317 — PKCE and the authorization URL** · n/a · S · `rmcp`
**upstream** — the SDK's `startAuthorization` plus the transitive `pkce-challenge` package.
**behavior** — PKCE S256 is mandatory under OAuth 2.1 and is what stops an authorization-code
interception attack on a loopback redirect.
**cyrup** — `AuthorizationManager::get_authorization_url` uses
`oauth2::PkceCodeChallenge::new_random_sha256`; there is no `plain` path anywhere in rmcp.
`validate_server_metadata` refuses an AS advertising challenge methods without `S256`
(`AuthError::PkceUnsupported`) and one whose `response_types_supported` omits `code`. **The
verifier alphabet differs from upstream's** — upstream draws 43 characters by rejection sampling
over a 66-character mask; oauth2's generator differs. Nothing observable depends on it (the AS only
ever sees the challenge), but it is a mechanism change and is recorded rather than slipped in.
One behavioural note: upstream **appends** `prompt=consent` when the resolved scope contains
`offline_access`; rmcp appends the `offline_access` scope (SEP-2207) but emits no `prompt`. For
Google-class providers that is what actually produces a refresh token, so if a server needs it, it
goes in `oauth.authorizationParams` — where `prompt` is **not** reserved.
**verify** — a golden authorization URL for a scope with and without `offline_access`, with and
without a resource; an AS advertising only `plain` is refused.

**MCP-318 — Token endpoint, client authentication, and the retry policy** · high · M · `rmcp` + `hand-written`
**upstream** — the SDK's `executeTokenRequest`, `refreshAuthorization`, `fetchToken`,
`assertSecureTokenEndpoint`, `parseErrorResponse` and the three-row `auth()` retry policy; plus
`mcp-oauth-provider.ts`'s own `addClientAuthentication`.
**behavior** — credentials must never reach a non-TLS non-loopback token endpoint; a refresh that
returns no new refresh token must keep the old one; the adapter's client-auth rule **differs from
the SDK's in two places**: for an empty `token_endpoint_auth_methods_supported` the adapter picks
`client_secret_post` where the SDK picks `client_secret_basic`, and the adapter omits the SDK's
branch that honours a stored `token_endpoint_auth_method`.
**cyrup** — rmcp owns the request shapes, the resource parameter on every leg, the
refresh-token preservation, and the `invalid_grant` → `TokenRefreshRejected` classification.
**Verified: rmcp's rule matches the TypeScript SDK's, not the adapter's** —
`AuthorizationManager::configure_client` selects request-body auth only when the metadata advertises
`client_secret_post` **without** `client_secret_basic`, and otherwise leaves the oauth2 crate's HTTP
Basic default. There is no `addClientAuthentication` hook. The lever that exists is
`AuthorizationManager::set_metadata`: fetch or accept the metadata, inject
`token_endpoint_auth_methods_supported: ["client_secret_post"]` into `additional_fields` when the
server published none and a secret is configured, then `configure_client`. Roughly ten lines.
**verify** — an `http:` non-loopback token endpoint is refused before any request; a refresh response
without a refresh token preserves the old one; an **empty** supported-methods list with a secret
produces a POST body and not Basic; a list containing only `client_secret_basic` produces Basic.

**MCP-319 — RFC 8707 `resource` binding** · n/a · S · `rmcp`
**upstream** — the SDK's `selectResourceURL` / `resourceUrlFromServerUrl` / `checkResourceAllowed`.
**behavior** — resource indicators bind an access token to one MCP server; omitting them yields
tokens a compliant AS refuses, and mis-scoping them is a confused-deputy risk.
**cyrup** — `AuthorizationManager::oauth_resource` returns the discovered resource from
protected-resource metadata, else the base URL, and it is added as an extra parameter on the
authorization URL, the code exchange **and every refresh**. rmcp additionally rejects a
protected-resource `resource` carrying a URL fragment. The adapter never implemented the SDK's
`validateResourceURL` override; do not add one.
**verify** — conformance: exact match, path-prefix match, different origin rejected, fragment
rejected; and that a refresh carries the parameter.

**MCP-320 — Flow-state custody across the browser hop** · n/a · S · `rmcp`
**upstream** — `mcp-oauth-provider.ts` `saveDiscoveryState` / `discoveryState` (a deep clone in both
directions) plus the SDK's callback-leg `AuthorizationServerMismatchError` when no recorded issuer
exists; `saveTokens` clears the discovery state afterwards.
**behavior** — the authorization server that minted the code must be the one the token is redeemed
at.
**cyrup** — structurally built in and stronger. `StateStore` stores `{pkce_verifier, csrf_token,
expected_issuer, require_issuer, created_at, requested_scopes}` keyed by CSRF token at
authorization-URL time, and `exchange_code_for_token_with_issuer` validates the issuer **before**
deleting the state — so a callback with a correct `state` but a forged or missing required `iss`
does not destroy the verifier the legitimate callback needs. Use `InMemoryStateStore`: upstream keeps
this material in memory only, deliberately, and persisting it would be a regression.
**verify** — drop the stored state before completing and assert the exchange fails rather than
silently proceeding; supply a forged `iss` and assert the state survives for a retry.

**MCP-321 — The storage read/write surface this flow consumes** · high · M · `hand-written`
**upstream** — `mcp-auth.ts`'s `StoredTokens` / `StoredClientInfo` / `AuthEntry` /
`AuthStorageOptions`, `getAuthBaseDir` (`MCP_OAUTH_DIR` first), `getAuthForUrl`'s URL binding,
`inspectAuthForUrl`, `isTokenExpired`, `hasStoredTokens`, plus `updateTokens` / `updateClientInfo` /
the `clear*` family / `getOAuthState`. **All synchronous** even though callers await them.
**behavior** — §11. `getAuthForUrl` returns nothing when the stored `serverUrl` is absent or
differs — the invalidation mechanism on a server-URL change. `isTokenExpired` is tri-state.
**cyrup** — an `AuthEntry { tokens, client_info, code_verifier, oauth_state, server_url }` store keyed
by server name, backed by the OS keyring per the storage section. This unit covers only the surface
this flow calls; the keyring, chunking manifest and `keyctl` recovery are that section's. Keep the
functions synchronous where upstream is.
**verify** — `getAuthForUrl` returns nothing for a changed URL and for an entry with no stored URL;
the tri-state expiry; `getAuthStatus` maps a `null` expiry to `authenticated`.

**MCP-322 — Issuer binding of stored credentials** · low · S · `rmcp`
**upstream** — `mcp-oauth-provider.ts` `issuersMatch`, `assertStoredIssuerBindings` and the latched
`flowIssuerMismatch`, whose message is
``OAuth authorization server issuer changed for ${serverName}; clear credentials before authenticating again``.
**behavior** — if a server's authorization server changes underneath stored credentials, a token
minted by one AS must not be sent to another.
**cyrup** — `AuthorizationManager::initialize_from_store` compares the stored issuer against the
freshly discovered one and, when they differ, **clears the stored credentials and returns `false`**
(with a CIMD carve-out that keeps the portable client id but discards the tokens).
**Named delta: rmcp clears where upstream throws.** rmcp's outcome is a silent re-authorization
prompt; upstream's is an explicit "clear credentials before authenticating again". The port should
detect the `false` return with a previously-populated store and surface upstream's message rather
than letting a silent re-login look like a random logout.
**verify** — seed an entry with issuer A, discover issuer B, assert the credential is gone **and**
the user-facing message appears; assert `https://x/` and `https://x` are treated as the same issuer.

**MCP-323 — The RFC 9207 gate in `completeAuth`, including `keepPendingForRetry`** · medium · S · `rmcp` + `hand-written`
**upstream** — `mcp-auth-flow.ts` `completeAuth`'s pre-exchange checks.
**behavior** — §8 steps 3-4. The user-facing text is the whole point:
``The authorization server for <n> requires the RFC 9207 "iss" parameter. Paste the full redirect URL from the browser address bar (not just the authorization code).``
Letting the library's own error surface instead gives an unactionable message.
**cyrup** — rmcp enforces the rule (`AuthError::AuthorizationServerMissingIssuer` /
`AuthorizationServerMismatch`) and preserves the state for a retry by construction, so the port's
gate is a **message-quality wrapper**: check first, emit the friendly text, and set
`keepPendingForRetry` — an explicit flag, not an error-type match, because it is the only path that
skips the `finally` cleanup.
**verify** — an AS advertising `iss` support plus a bare code yields the exact message **and** the
pending flow is retained; a mismatched `iss` yields the mismatch message and the flow is cleared;
the retry with a full URL then succeeds.

**MCP-324 — `getValidToken`'s refresh path and its fall-through** · high · M · `rmcp` + `hand-written`
**upstream** — `mcp-auth-flow.ts` `getValidToken`.
**behavior** — §10. Tri-state expiry; refresh only when expired **and** a refresh token exists; a
context built with an **empty config**; discovery with no server definition so **no configured
headers**; abort errors and credential-store errors rethrown while everything else is logged and
answered `null`; and the deliberate "no expiry info or no refresh token ⇒ assume valid"
fall-through. This is what `oauth.ts`'s public `getMcpOAuthTokensForUrl` is built on, so the exact
null-versus-throw behaviour is a published contract.
**cyrup** — `AuthorizationManager::get_access_token` / `refresh_token` do the network half. The
tri-state, the three log lines (kept verbatim as `tracing` events at the same points), and the
fall-through are hand-written.
**verify** — an entry with no expiry returns its tokens without a network call; a credential-store
failure propagates rather than becoming `None`; a refresh that returns no new refresh token keeps
the old one; a rejected refresh token yields `None` with the log line.

**MCP-325 — The `client_credentials` grant** · medium · S · `rmcp`
**upstream** — `mcp-auth-flow.ts`'s short-circuit plus the provider's grant-specific throws.
**behavior** — §10. No callback listener, no `state`, no browser, and the RFC 9207 check is skipped
entirely on this path.
**cyrup** — `OAuthState::authenticate_client_credentials` with
`ClientCredentialsConfig::ClientSecret { client_id, client_secret, scopes, resource }`;
`AuthorizationManager::validate_client_credentials_metadata` refuses an AS that advertises neither
`client_secret_post` nor `client_secret_basic`. The stale-registration hygiene that runs before it —
clear client info, code verifier and OAuth state when client info exists without tokens and no
`clientId` is configured — is hand-written.
**verify** — a full acquisition that touches neither the listener nor the launcher; stale dynamic
client info cleared first; a `redirectUri` configured on a `client_credentials` server still passes
through `extractOAuthConfig`'s string checks (§14 item 6).

**MCP-326 — The manual/headless leg: parsing and the callback-versus-paste race** · high · M · `hand-written` + `host-verb`
**upstream** — `mcp-auth-flow.ts` `getSearchParamsFromInput`, `parseAuthorizationRedirectInput`,
`parseAuthorizationCodeInput`, `waitForAuthorizationResponse`.
**behavior** — §8. Over SSH the localhost page cannot load, so the user copies the address bar; that
URL must be state-validated before exchange, and the prompt must vanish the moment the loopback
callback wins. A **bare code pasted at the prompt is rejected** even though the parser would accept
one elsewhere. An external abort rejects with the identical reason value.
**cyrup** — `tokio::select!` over the callback receiver and the prompt future, with the prompt's own
`CancelToken` cancelled in a scope guard so it dies on every exit path. The prompt is
`HostServices::input` (or `oauth_prompt`, the same round trip with an OAuth-shaped presentation)
under `HostServices::human_interaction_lock`. rmcp's `AuthorizationCallback::from_redirect_url`
covers only the strict URL case and is **not** a substitute for the parser.
**verify** — the seven upstream race cases, including that the prompt signal is aborted in every
terminating case and that an external abort rejects with the identical reason; plus the parser cases
(URL, hash-merged parameters, bare query string, bare code, `error` with and without a description,
missing state, mismatched state).

**MCP-327 — Browser launch** · low · S · `extension-owned`
**upstream** — `mcp-auth-flow.ts`'s use of the `open` npm package. Note this is **not** the adapter's
own `openUrl` helper, which dispatches per-platform through `pi.exec` and honours a `browser`
override and `$BROWSER`; the OAuth site deliberately uses the simpler one.
**behavior** — the URL is printed **before** the browser is attempted so a headless user is never
stranded; a failed launch logs
``MCP Auth: Failed to open browser for ${serverName}; waiting for manual callback`` and the flow
**continues**; an abort propagates.
**cyrup** — the `opener` crate, called directly. A native crate is not sandboxed, so this is not an
ambient-authority question and does not go through `HostServices::exec`. If the adapter's `openUrl`
is ported elsewhere it must stay a subprocess dispatch, because that one *does* honour `$BROWSER`.
**verify** — with an injected launcher: the URL is emitted before the launch; a launcher error is
logged and swallowed; an abort propagates.

**MCP-328 — `startAuth`'s ordering, stale-registration checks and aggregate cleanup** · high · L · `hand-written`
**upstream** — `mcp-auth-flow.ts` `startAuth`.
**behavior** — §9. **Blocking-ness and the destructive branch, stated plainly:** the tokens-present
branch deliberately **discards the tokens** when the stored `redirectUris` do not include the current
redirect URL, because those tokens belong to a client whose registered redirect URI no longer
matches. A port that inverts or mis-computes that condition destroys a working credential on an
ordinary login. That is a forced re-login, not durable data loss, so the rating is `high` — but it is
the single most defect-prone conditional in the section. Beyond it: the listener must start before
the redirect URI is fixed; a matching in-flight flow returns its stored URL unchanged; and a failure
during startup must leave no reserved callback state, no persisted `oauthState` and no half-published
pending auth.
**cyrup** — direct port. The aggregate-error shape has no Rust equivalent: use an
`McpOAuthError::Aggregate { phase, errors }` whose `Display` renders the phase **and both messages**
(MCP-345).
**verify** — with an occupied strict port, assert no reservation survives and no `oauthState` was
persisted; all five stale-registration variants (no stored client info; client info without tokens;
tokens with a matching redirect URI; tokens with a stale redirect URI; tokens with a non-array
`redirectUris`); the idempotent return for a matching in-flight flow; and an explicit `redirectUri`
driving both the bind and the registered metadata.

**MCP-329 — The 5-minute abandoned-flow timer and its state guard** · medium · S · `hand-written`
**upstream** — `mcp-auth-flow.ts`'s `MANUAL_AUTH_TIMEOUT_MS` and the timer armed in `setPendingAuth`.
**behavior** — a user who starts `/mcp-auth` and walks away leaves no process-lifetime leak, and a
*newer* flow for the same server started six minutes later is **not** destroyed by the old timer.
The timer is unref'd; its failure path logs
``MCP Auth: Timed-out flow cleanup failed: ${formatTerminalError(error)}``.
**cyrup** — a detached `tokio::spawn` with `tokio::time::sleep`; nothing waits on it. The state guard
in `clearPendingAuth` is the important half — dropping it is a use-after-free-class bug.
**verify** — with a paused tokio clock: start flow A, advance five minutes, it is cleared; start flow
A, replace it with A′, advance five minutes, A′ survives. **No upstream test covers either timer's
expiry**, so both of these are net-new obligations.

**MCP-330 — `authenticate`'s in-flight dedup and its cleanup boundary** · high · M · `hand-written`
**upstream** — `mcp-auth-flow.ts` `authenticate`.
**behavior** — §8. Two tool calls that both hit `needs-auth` on the same server produce **one**
browser window; the waiter is registered before the browser opens; the URL is surfaced before the
launch; a manual win cancels the pending callback; cancelling leaves no listener waiting on a dead
`state`; and the dedup entry is removed only if it is still the one this call installed.
**cyrup** — a `Mutex<HashMap<String, Shared<BoxFuture<…>>>>` with the identity check in a scope
guard.
**verify** — two concurrent calls for the same key share one operation and one launcher invocation; a
third for a different `serverUrl` does not.

**MCP-331 — `completeAuth` and `completeAuthFromInput`** · high · M · `hand-written`
**upstream** — `mcp-auth-flow.ts` `completeAuth`, `completeAuthFromInput`.
**behavior** — §8. The pending-flow requirement
(``No pending OAuth flow for server: ${serverName}``), the RFC 9207 gate, the exchange, the
`UnauthorizedError("Failed to authorize")` for a non-`AUTHORIZED` result, and the `finally` that
clears **unless** `keepPendingForRetry`, aggregating cleanup failures under
`OAuth completion cleanup failed` or rethrowing the cleanup error alone when the body succeeded. The
headless `auth-complete` path and the interactive paste path converge here.
**cyrup** — direct port over `AuthorizationSession::handle_callback_with_issuer`.
**verify** — no pending flow yields the exact message; a successful exchange yields `authenticated`
and clears the pending auth; the `iss`-required case retains it; and the reserved callback state is
released after a direct `completeAuth`.

**MCP-332 — `supportsOAuth`, `getAuthStatus`, `removeAuth`** · medium · S · `hand-written`
**upstream** — the three functions in `mcp-auth-flow.ts`.
**behavior** — §10. The truth table's **order is observable**: `auth === "oauth"` beats the
custom-headers row. `removeAuth`'s five steps end in
``MCP Auth: Removed credentials for ${serverName}`` with abort checks interleaved at four points.
**cyrup** — direct port of the branch order; do not reorder for readability.
**verify** — one case per truth-table row; the three status values including `null` expiry mapping to
`authenticated`; `removeAuth` leaves `not_authenticated`.

**MCP-333 — The connect-path 401 classification** · high · M · `rmcp` + `hand-written`
**upstream** — `server-manager.ts` `createAuthProvider`, `HttpAuthProviderState`,
`isUnauthorizedHttpError`, and the one-retry promotion.
**behavior** — §13. Explicit OAuth touches the credential store at connect time; implicit OAuth
defers until the server proves it needs auth, so a non-OAuth HTTP server never triggers a keyring
read. A 401 that survives the single retry becomes `needs-auth`, which the tool layer turns into
either auto-auth or the auth-required message — **never a browser window opened from inside a turn**.
The auth-entry cache is invalidated once per `needs-auth` episode.
**cyrup** — `ClientInitializeError::auth_challenge()` supplies the challenge string and
`is_authorization_required()` the predicate; the four-state union and the retry are hand-written and
sit at the seam with the transport section.
**verify** — a stub HTTP MCP server answering 401 twice yields `needs-auth` with the cache
invalidated once; 401 then 200 connects with no launcher invocation.

**MCP-334 — The `/mcp-auth` command surface and its eleven messages** · medium · M · `host-verb`
**upstream** — `commands.ts` `authenticateServer`.
**behavior** — §13. The guard order is: no interactive UI → unknown server → disabled → not an OAuth
server → no URL. The `mcp-auth` status key is set to ``Authenticating ${serverName}...`` and cleared
in a `finally` **unless the signal aborted**. Every string in the message table is user-visible.
**cyrup** — `InitApi::register_command` plus `NativeExtension::execute_command` at command tier;
`HostServices::set_status` maps 1:1 including its `None`-clears semantics; `HostServices::notify`
carries the outcome; `confirm` then `input` under `human_interaction_lock` is the manual prompt pair.
Terminal hyperlinks (OSC 8) have no helper — emit the escape sequence in the message text, or drop
the hyperlink and record the loss.
**verify** — with a scripted `HostServices`: each guard produces its exact message; the status key is
set then cleared; an aborted signal leaves the status set.

**MCP-335 — `auth-start` / `auth-complete` and auto-auth** · medium · M · `hand-written`
**upstream** — `proxy-modes.ts` `formatManualAuthInstructions`, `getRedirectPort`, `attemptAutoAuth`,
`executeAuthStart`, `executeAuthComplete`; `direct-tools.ts`'s auto-auth.
**behavior** — §13. `auth-start` is the only OAuth entry point that works with no TTY; its output is
a **six-line** block with no interior blank lines, plus a blank line and the port note when the
redirect URI yields a parseable port. `auth-complete` additionally **closes the server connection**
so the next `connect` uses the new token. Auto-auth fires only on `settings.autoAuth === true`, and
with no UI it refuses everything except `client_credentials`. The direct-tools auth-required literal
is genuinely different from the proxy one.
**cyrup** — direct port; the `details` keys (`mode`, `error`, `server`, `authorizationUrl`,
`authenticated`, `status`, `message`) are consumed by the tool-result renderer, so keep the names.
**verify** — golden text for the instruction block with and without a parseable port, asserting six
content lines and no interior blanks in the no-port case; auto-auth skipped when unset, refused with
the auth-required text when there is no UI, attempted for `client_credentials` regardless.

**MCP-336 — Callback-listener ownership: settled** · n/a · S · `extension-owned`
**upstream** — `mcp-callback-server.ts`.
**behavior** — the observable contract is §1-§3 regardless of where the code lives.
**cyrup** — **reuse `cyrup_provider::auth::oauth::callback`.** The `CallbackHandler` /
`CallbackOutcome::Continue` contract already supports a multi-tenant, never-settling handler;
`CallbackServerConfig` already carries fixed-versus-ephemeral port, path, bind host, advertise host,
timeout and cancel; `OAuthError::Listen` already distinguishes `AddrInUse`. `cyrup-mcp` already
depends on `cyrup-provider` for sampling, so there is no layering inversion. Also reuse
`cyrup_provider::auth::oauth::query` (`percent_decode`, `parse_query`, `encode_query`, all public)
and `page::escape_html`. Two named deltas from reuse, both cosmetic: the foreign-path answer is the
HTML 404 page rather than `text/plain` `Not found`, and there is an extra 400 branch for a malformed
request that upstream does not have.
**verify** — n/a; settled. MCP-305 through MCP-308 verify the behaviour.

**MCP-337 — The rmcp split: verified, settled** · n/a · S · `rmcp`
**upstream** — the RFC surface that the adapter never wrote, living in
`@modelcontextprotocol/client`'s `auth()`.
**behavior** — identical either way; the risk profile is not.
**cyrup** — `rmcp = { version = "3.1.2", default-features = false, features = ["client",
"transport-child-process", "transport-streamable-http-client-reqwest", "reqwest", "auth"] }`.
`auth` expands to `["dep:async-trait", "dep:oauth2", "__reqwest", "dep:url"]`, and `oauth2` 5.0.0 is
already in cyrup's lock file at the version rmcp requires, so `auth` adds no new resolution surface.
`auth-client-credentials-jwt` is **not** needed — the adapter has no `private_key_jwt` path.
**The previous edition's three blocking questions are answered against the checkout:** (i) an
`addClientAuthentication`-equivalent hook does **not** exist, and the lever is `set_metadata` before
`configure_client` (MCP-318); (ii) discovery-state custody across the browser hop **does** exist and
is stronger than the SDK's (MCP-320); (iii) a `clientInformation`-suppression hook does not exist and
is **not needed**, because rmcp's refresh path does not destroy credentials on `invalid_client`
(MCP-314).
**verify** — the conformance suites named in MCP-310, MCP-311, MCP-317, MCP-318, MCP-319 and MCP-320,
run against a stub authorization server.

**MCP-338 — Browser-open mechanism: settled** · n/a · S · `extension-owned`
**cyrup** — the `opener` crate for this call site. The previous edition framed this as a new
ambient-authority surface because `HostServices::exec` is denied by default; that gate is the
WASM-guest capability model and does not apply to a native crate. Never-launch (print the URL only)
is a real behaviour loss — the "Pi will complete automatically if the browser can reach its localhost
callback" promise depends on the launch.
**verify** — n/a; MCP-327 verifies the behaviour.

**MCP-339 — Bind `localhost` or `127.0.0.1`** · medium · S · `open-decision`
**upstream** — `mcp-callback-server.ts`'s `DEFAULT_OAUTH_CALLBACK_HOST = "localhost"`, with the
advertised redirect `http://localhost:<port><path>`.
**behavior** — Node's `listen(port, "localhost")` binds whatever the resolver returns first, which on
a dual-stack host is often `::1`; a browser navigating to `http://localhost:<port>` resolves the same
way, so the two agree by construction. Binding `127.0.0.1` while advertising `localhost` breaks
whenever `localhost` resolves to `::1` only.
**cyrup** — `cyrup_provider::auth::oauth::callback` defaults to `127.0.0.1` and already supports an
`advertise_host` split. Options: **(a)** resolve `localhost` and bind every returned address, the
literal mechanism, requiring a multi-listener accept loop; **(b)** bind and advertise `127.0.0.1`,
which diverges from the documentation and **breaks any redirect URI already registered with a
provider as `localhost`**; **(c)** bind `127.0.0.1` and advertise `localhost` via `advertising(…)`,
which the existing primitive supports today and is correct on every host that resolves `localhost` to
IPv4. Recommendation: **(c) now with a named residual, (a) if a report arrives** — (c) is a config
call on an existing primitive, and (a)'s failure mode is confined to IPv6-only-localhost machines.
**verify** — a live test binding on a dual-stack host and fetching the callback over both families.

**MCP-340 — The stale hardcoded client version in the discovery probe** · low · S · `open-decision`
**upstream** — `mcp-auth-flow.ts` `probeAuthDiscovery` sends
`clientInfo: { name: "pi-mcp-adapter", version: "2.11.0" }` at package version 2.25.0. Every other
`initialize` the adapter sends comes from the real client construction path.
**behavior** — servers that log or gate on client version see a stale number on the discovery probe
only. No functional consequence found.
**cyrup** — moot if MCP-309 chooses the reactive path, since there is then no probe. If a probe is
kept: emit the real `cyrup-mcp` client version rather than reproducing an upstream bug, and file the
staleness so it is not later mistaken for a port defect. The `name` field is a separate question — it
identifies the client to third-party servers, so it belongs with MCP-313's branding decision.
**verify** — n/a; settled by a decision.

**MCP-341 — Ship a corrected OAuth document** · medium · S · `hand-written`
**upstream** — `OAUTH.md`, with the eight divergences in §14.
**behavior** — this file is the published contract users configure against; porting it unchanged
ports eight documentation defects.
**cyrup** — port the document with the corrections applied and each correction noted inline, so a
later parity pass does not "restore" the upstream text. Add the port's own deltas: the
`skipIssuerMetadataValidation` narrowing (below), the `prompt=consent` change, and the
`localhost`/`127.0.0.1` bind decision.
**verify** — a review checklist asserting each corrected statement matches the ported code.

**MCP-342 — A reachable, three-form `interpolate_env_vars`** · medium · S · `hand-written`
**upstream** — `utils.ts` `interpolateEnvVars` (three forms) and `interpolateEnvRecord` /
`interpolateSecretExpression`, applied to `clientId`, `clientSecret`, `scope`, every
`authorizationParams` value, `redirectUri`, `clientName`, `clientUri`, `logoUri` and every discovery
header.
**behavior** — an `oauth` block that references environment variables must expand them identically to
the rest of the config, or a `${VAR}`- or `{env:VAR}`-bearing `clientId` goes to the authorization
server literally. `!!X` is sent as `X` with exactly one leading `!` removed.
**cyrup** — two copies exist and **neither implements the third form**:
`cyrup_ext::caps::proc::interpolate_env_vars` (currently `pub(crate)`, and it already carries the
pi-order test) and a private one in `cyrup_ext_subagents::exec::mcp_direct_tools`. **This is a
parity defect in both, not merely a visibility problem.** The fix is one shared implementation with
the `{env:VAR}` form added; promoting the existing one to `pub` is a visibility chore of the same
class as the `npx_resolver` reuse, not a host addition. This helper is also needed by the config
section, so the decision is shared.
**verify** — all three placeholder forms in pi's evaluation order, from every consumer; and that a
`!!literal` header goes out with exactly one `!` removed.

**MCP-343 — Non-unix entropy: dissolved** · n/a · S · `rmcp`
**upstream** — none; `crypto.getRandomValues` is ambient wherever Node runs.
**behavior** — n/a.
**cyrup** — the previous edition filed this because
`cyrup_provider::auth::oauth::random`'s non-unix arm returns an entropy error unconditionally, and
concluded MCP OAuth would be unavailable on Windows. **That module is not in this path.** rmcp
generates the PKCE verifier and the CSRF token itself through `oauth2`, which uses the `rand` crate.
The only remaining CSPRNG use is the adapter's own 64-hex `state` nonce, which is a map key
`cyrup-mcp` can generate however it likes. No workspace RNG dependency is required by this section.
**verify** — n/a; refuted.

**MCP-344 — The process-shared listener refcount** · medium · S · `hand-written`
**upstream** — `mcp-auth-flow.ts`'s live-runtime set: added to on every `getRuntime` and
`initializeOAuth`, removed on `shutdownOAuth`, and the listener stopped **only** when it empties. The
module-level legacy runtime is deliberately not a member at creation.
**behavior** — a session restart must not tear the listener out from under a login another session is
running; conversely the last shutdown must free the port.
**cyrup** — a process-global `Mutex<HashSet<RuntimeKey>>`. The subtlety is that `getRuntime` **adds
on every call**, so the collection must be idempotent — an incremented counter would over-count and
never reach zero.
**verify** — three `getRuntime` calls for one runtime then one shutdown stops the listener; two
runtimes with one shutdown does not.

**MCP-345 — Preserve both errors when cleanup fails** · medium · S · `hand-written`
**upstream** — the four aggregate sites in `mcp-auth-flow.ts` with phase strings
`OAuth startup cleanup failed` (twice), `OAuth completion cleanup failed`,
`OAuth cancellation cleanup failed`; plus `completeAuth`'s rethrow-alone case when the body
succeeded.
**behavior** — a credential-store failure during cleanup must not hide the OAuth error that caused
it, and vice versa. Every one of these paths surfaces through `Failed to authenticate "<n>":
<message>`.
**cyrup** — an `McpOAuthError::Aggregate { phase: &'static str, errors: Vec<McpOAuthError> }` whose
`Display` renders the phase and **both** messages. Do not collapse to the primary — the whole point
of these sites is that the secondary is otherwise invisible.
**verify** — force a store failure inside each of the four cleanup paths and assert both messages
appear, plus the fifth case where the body succeeded.

**MCP-346 — The public token API** · low · S · `extension-owned`
**upstream** — `oauth.ts`: `getMcpOAuthTokensForUrl` (delegates to `getValidToken`, so it may
refresh), `inspectMcpOAuthTokensForUrl` (delegates to `inspectAuthForUrl`, never refreshes, reports
`present` / `absent` / `unavailable`, and exposes **only** tokens — never client info, code verifier
or OAuth state), `updateMcpOAuthTokensForUrl`.
**behavior** — an embedder reuses the adapter's MCP tokens without touching its internals; the store
being unavailable must **throw** from the getter and report `unavailable` from the inspector — never
look like "no tokens". Moving tokens to a new URL clears the old entry's client info, code verifier
and OAuth state.
**cyrup** — three functions on `cyrup-mcp`'s public API. Whether cyrup wants a public embedder
surface at all is the SDK section's call; the behaviour (URL binding, no secret leakage, fail-closed)
is what must survive if it does.
**verify** — the six upstream cases, including the URL-move cleanup.

**MCP-347 — The executable spec as the acceptance suite** · n/a · L · `hand-written`
**upstream** — eight files owned by this section: `mcp-auth-flow.test.ts`,
`mcp-oauth-provider.test.ts`, `mcp-callback-server.test.ts`, `oauth-public-api.test.ts`,
`__tests__/mcp-auth-flow-client-credentials.test.ts`, `__tests__/mcp-oauth-provider.test.ts`,
`__tests__/mcp-callback-server-unref.test.ts`, `__tests__/mcp-callback-server-manual.test.ts` — plus
five harnesses owned by neighbouring sections that exercise this code
(`__tests__/commands-auth.test.ts`, `__tests__/proxy-modes-auto-auth.test.ts`,
`__tests__/proxy-modes-manual-auth.test.ts`, `__tests__/direct-tools-auto-auth.test.ts`,
`__tests__/server-manager-http-auth.test.ts`).
**behavior** — these pin behaviours the prose cannot: the redirect-URL snapshot semantics, the
reservation-versus-rebind interlock, unref-on-success and its absence on failure, stop-then-reuse,
the reservation surviving a provider `error` callback, the escaping-but-not-in-the-rejection rule,
the branding fallbacks, the prompt-dismissal guarantee, the five stale-registration branches, runtime
isolation, and the fail-closed credential-store contract.
**cyrup** — the callback-server files map onto conformance tests over a real loopback socket (they
already drive real HTTP); the unref file mocks the HTTP layer and maps onto unit tests over an
injected listener factory; the rest map onto unit tests. The branding block writes a fabricated
package manifest into a temp dir and sets an env var — port it as a temp-dir fixture over cyrup's
equivalent. **Tests whose subject is the four RFCs now assert against `rmcp` rather than ported
code**, and several collapse into the conformance suites named in MCP-337.
**verify** — self-describing: every upstream `it(...)` in the eight owned files has a named Rust
counterpart or an explicit "covered by rmcp" annotation.

**MCP-349 — `resolveCommandSecret`'s subprocess mechanism** · high · S · `extension-owned`
**upstream** — `utils.ts` `resolveCommandSecret`.
**behavior** — §12: the two constants, the three input shapes, the five failure strings, the
discarded stderr, and the `shell: true` semantics.
**cyrup** — `std::process::Command` with `sh -c <cmd>` / `cmd /C <cmd>` to reproduce `shell: true`,
null stdin and stderr, piped stdout, a 10-second wall-clock kill, and a 1 MiB read cap mapping to the
output-exceeded message. Synchronous. Called **directly**, not through `HostServices::exec` — that
verb is the guest capability gate and does not apply to a native crate. Under rmcp the secret is
applied once at `configure_client` time rather than up to three times per token leg; record the
subprocess-count divergence.
**verify** — `!!literal` yields the literal with one `!` removed and **no** subprocess; a plain value
is interpolated; `!echo hunter2` yields `hunter2` trimmed; an 11-second command yields the timeout
string; a 2 MiB emitter yields the output-exceeded string; `!false` yields
`command exited with code 1`; `!true` yields `command returned empty output`; a stderr-only command
yields the empty-output message with the stderr text nowhere in it.

---

### Out of scope

These are decided, not deferred. Recorded with reasons so a later pass does not re-file them as gaps.

* **The legacy HTTP+SSE transport, and therefore its OAuth interaction.** rmcp 3.1.2 ships no SSE
  client transport at all, so supporting it would mean hand-writing a protocol transport — exactly
  what the dependency decision exists to avoid. Supported transports are **`stdio` and `streamable
  HTTP` only**. **Consequence in this section:** OAuth applies only to `url`-configured streamable-HTTP
  servers, which is already the adapter's rule (`supportsOAuth` returns `false` without a `url`); and
  `ServerEntry.httpTransport: "sse"` must be **rejected at config load with a named diagnostic**, not
  silently ignored, because a server declaring it would otherwise appear to authenticate and never
  connect.
* **The raw unix-socket transport.** rmcp's `UnixSocketHttpClient` is streamable-HTTP-over-UDS, a
  different wire shape from the adapter's raw framed socket. **Consequence in this section:** the
  `createConnection` invariant becomes "exactly one of `command` or `url`", and a config carrying
  `socket` produces a named diagnostic. No OAuth path touched it — a socket server has no `url` and
  therefore no OAuth — so nothing else changes.
* **MCP Apps / the UI extension.** Cut entirely. **Consequence in this section:** the OAuth loopback
  callback listener is a **separate thing and stays** — it is not the Apps host server. Cutting Apps
  removes the only reason to want `axum`, so the callback listener is
  `cyrup_provider::auth::oauth::callback`'s `TcpListener` and accept thread, not an HTTP framework.
  `McpToolApprovalOrigin`'s `"iframe"` variant disappears from the auto-auth call sites.
* **`mcpScript` / the JavaScript worker.** Cut entirely, which removes the only JS-engine question in
  the whole port. **Consequence in this section:** `executeCall`'s `origin?: "proxy" | "script"`
  parameter keeps only its `"proxy"` default, and `McpToolApprovalOrigin`'s `"script"` variant
  disappears from the auto-auth paths. `node` is not a production dependency of `cyrup-mcp`,
  including in the keyring-revoked recovery path, which re-execs the `cyrup` binary in a helper mode
  instead.

---

### What does not fit cleanly

**No host additions survive in this section.** Every capability is either `rmcp`, a direct
dependency a native crate links itself, an existing `HostServices` verb, or hand-written adapter
policy. Five genuine decisions remain.

1. **`skipIssuerMetadataValidation` maps only partially.** The adapter's flag skips the RFC 8414 §3.3
   issuer-echo check outright. rmcp's `set_allow_missing_issuer(true)` tolerates only a **missing**
   `issuer` field; a **mismatched** one still fails with `AuthorizationServerMismatch`. Options:
   **(a)** map the flag to `set_allow_missing_issuer` and document the narrowing — servers that echo a
   *wrong* issuer are no longer usable, which is the safer behaviour and affects a smaller set of
   servers than the ones that simply omit the field; **(b)** fetch the metadata out of band and hand
   it in with `AuthorizationManager::set_metadata`, bypassing the check entirely and reproducing the
   flag exactly, at the cost of duplicating the discovery walk. **Recommendation: (a)**, named in the
   ported documentation as a deliberate narrowing.

2. **The dynamic-registration body loses `client_uri`, `logo_uri` and confidential client auth.**
   rmcp's `ClientRegistrationRequest` is fixed: `client_name`, `redirect_uris`, `grant_types`,
   `token_endpoint_auth_method: "none"`, `response_types`, `scope`, `application_type`. The adapter
   additionally sends `client_uri` and `logo_uri` (both surfaced on the consent screen) and
   `token_endpoint_auth_method: "client_secret_post"` when a secret is configured. Options: **(a)**
   accept the loss — the consent screen shows the app name and no homepage or logo, and a
   *dynamically registered* confidential client is not reachable (a configured `clientSecret` without
   a `clientId` is an unusual configuration and `extractOAuthConfig` does not forbid it); **(b)**
   perform the registration POST in `cyrup-mcp` with the full body and hand the result to
   `AuthorizationManager::configure_client`, keeping rmcp for everything after — about forty lines,
   and it also recovers the `/register` origin fallback rmcp lacks; **(c)** upstream a builder on
   `AuthorizationRequest`. **Recommendation: (b)**, because `logoUri` is a validated, documented
   config field and silently dropping it would make MCP-302's validation pointless.

3. **The client-authentication method on an empty supported-methods list.** rmcp defaults to HTTP
   Basic where the adapter sends a POST body (§MCP-318). Against a server that publishes no
   `token_endpoint_auth_methods_supported`, the port would send Basic where upstream sends a body — a
   silent authentication failure that looks like a bad secret. Options: **(a)** inject
   `token_endpoint_auth_methods_supported: ["client_secret_post"]` via `set_metadata` when the server
   published none and a secret is configured, restoring parity in about ten lines; **(b)** accept
   rmcp's behaviour, which matches the TypeScript SDK's own default and is arguably the more
   standards-typical choice. **Recommendation: (a)** — the adapter's choice is the one users'
   existing server configurations were tested against.

4. **Bind `localhost` or `127.0.0.1`** — MCP-339. Recommendation: bind `127.0.0.1` and advertise
   `localhost` through the existing `advertise_host` split, with the IPv6-only-localhost residual
   named.

5. **The discovery trigger: proactive probe or reactive challenge** — MCP-309. Recommendation:
   reactive on the connect path (rmcp's `auth_challenge()`), proactive only where the adapter probes
   without connecting.

---

### Coverage

**Read**

*Upstream, in full:* `mcp-auth-flow.ts`, `mcp-oauth-provider.ts`, `mcp-callback-server.ts`,
`OAUTH.md`, `oauth.ts`, `oauth-handler.ts`, and the eight OAuth test files listed in MCP-347.

*Upstream, in the regions this section depends on:* `types.ts`'s `OAuthConfig`; `mcp-auth.ts`'s
`StoredTokens` / `StoredClientInfo` / `AuthEntry`, `getAuthBaseDir`, `getAuthForUrl`,
`isTokenExpired`, `hasStoredTokens`; `utils.ts`'s `execOpen` / `openUrl` / `interpolateEnvVars` /
`interpolateSecretExpression` / `interpolateEnvRecord` / `resolveCommandSecret`; `agent-dir.ts`'s
`getAppName` / `getAppClientUri`; `commands.ts`'s `authenticateServer`; `proxy-modes.ts`'s auth
helpers, `attemptAutoAuth`, `executeAuthStart`, `executeAuthComplete`; `server-manager.ts`'s
`createAuthProvider`, `HttpAuthProviderState`, `isUnauthorizedHttpError`; `direct-tools.ts`'s
auto-auth; `init.ts`'s idle-shutdown suppression.

*rmcp, at `rmcp-v3.1.2-7-gf713ebd`:* `crates/rmcp/Cargo.toml` (the real feature graph);
`crates/rmcp/src/transport/auth.rs` — `StoredCredentials`, `CredentialStore`,
`InMemoryCredentialStore`, `StoredAuthorizationState`, `StateStore`, `InMemoryStateStore`,
`AuthError`, `AuthorizationMetadata`, `AuthorizationMetadataSource`, `WWWAuthenticateParams`,
`OAuthClientConfig`, `AuthorizationRequest`, `ClientCredentialsConfig`, `ScopeUpgradeConfig`,
`AuthorizationManager` (`new`, `initialize_from_store`, `resolve_metadata`,
`resolve_metadata_from_challenge`, `configure_client`, `configure_client_id`, `register_client`,
`get_authorization_url`, `select_scopes`, `add_offline_access_if_supported`,
`validate_server_metadata`, `validate_authorization_metadata_issuer`, `issuer_identifiers_match`,
`expected_issuer_for_authorization_metadata_url`, `well_known_paths`,
`validate_authorization_response_issuer`, `exchange_code_for_token_with_issuer`, `get_access_token`,
`refresh_token`, `oauth_resource`, `configure_client_credentials`, `exchange_client_credentials`),
`ClientRegistrationRequest`/`Response`, `AuthorizationCallback`, `AuthorizationSession`,
`AuthorizedHttpClient`, `AuthClient`, `OAuthState`; `crates/rmcp/src/service/client.rs`'s
`auth_challenge` / `is_authorization_required`;
`crates/rmcp/src/transport/streamable_http_client.rs`'s `auth_challenge`; `docs/OAUTH_SUPPORT.md`;
`examples/clients/src/auth/{oauth_client.rs, client_credentials.rs}`.

*cyrup, branch `david/cyrup`, by symbol:* `cyrup_provider::auth::oauth::callback`
(`DEFAULT_CALLBACK_HOST`, `callback_host`, `CallbackServerConfig` and its builders, `CallbackRequest`,
`CallbackReply`, `CallbackOutcome`, `CallbackControl`, `CallbackHandler`, `CallbackServer::{start,
redirect_uri, port, path, close, cancel_wait, wait}`, `serve_connection`, `read_request`,
`bracket_host`); `cyrup_provider::auth::oauth::{page, query, pkce, random, sha256, interaction}`;
`cyrup_provider::auth::{store, types}`; `cyrup_ext::host::services::HostServices` (`confirm`,
`input`, `select`, `oauth_prompt`, `oauth_select`, `open_overlay`, `notify`, `set_status`,
`human_interaction_lock`, `is_run_cancelled`, `exec`); `cyrup_ext::caps::proc`'s env interpolation;
`cyrup_ext_subagents::exec::mcp_direct_tools`' env interpolation; `cyrup_core::CancelToken`.

**Excluded**

* `mcp-auth.ts`'s keyring internals, `mcp-keyring-helper.cjs`, and their three test files — the
  storage subsystem is a different section; duplicating it here would produce two conflicting
  specifications of one file. Read only far enough to specify the surface this flow calls (MCP-321).
* `commands.ts`, `proxy-modes.ts` and `direct-tools.ts` beyond their auth paths, with their four
  auth test harnesses — the command, proxy and direct-tool sections own them; this section specifies
  only the strings and call contracts that originate here.
* `server-manager.ts` beyond the auth-provider seam — transport construction and the connection state
  machine belong to the transport section; MCP-333 names the seam and stops.
* `oauth-handler.ts` — read in full; a compatibility shim with no callers inside this subsystem. Two
  properties are recorded so its absence is deliberate: its expiry check is **stricter** than
  `getValidToken`'s tri-state, and it reads through the by-name accessor, **not** the URL-bound one,
  so it has no server-URL binding at all and bypasses the invalidation mechanism the documentation
  presents as universal. If anything in cyrup ends up needing a "tokens for this server name"
  accessor, that asymmetry is the trap.
* The per-vendor login flows under `cyrup_provider::auth::oauth` (`anthropic`, `openai_codex`,
  `openrouter`, `radius`, `github_copilot`, `xai`, `kimi_coding`) — each builds a fixed authorization
  URL with no discovery, no registration, no resource indicator; none is a landing spot. `device_code`
  is a different grant (RFC 8628).
* `cyrup-sdk` — the public embedding surface MCP-346 would attach to. MCP-346 is rated `low` because
  its scope is that section's to decide.
* rmcp's `server`, `macros`, `schemars`, `transport-streamable-http-server*`, `request-state`,
  `auth-client-credentials-jwt`, `elicitation` and `which-command` features — none is needed by an
  MCP client that never runs a server and has no `private_key_jwt` path.

**Corrections to the first pass**

* **Refuted: "cyrup's callback server is the wrong shape."** Eight of the ten claimed mismatches are
  dissolved by the `CallbackHandler` / `CallbackOutcome::Continue` contract — a handler that never
  claims and never settles makes the listener persistent, multi-tenant and free of the 409 branch.
  The two survivors are cosmetic (HTML 404 versus `text/plain`, and an extra malformed-request 400).
  MCP-336 is now settled as reuse, not a rebuild.
* **Refuted: the layering objection to depending on `cyrup-provider`.** `cyrup-mcp` already depends on
  it for sampling, and `cyrup-ext` depends on it too.
* **Refuted: `rmcp`'s OAuth support was "relayed, not verified".** All of it is now read first-hand in
  the checkout, and MCP-337's three blocking questions are answered.
* **Dissolved: "does rmcp expose `saveDiscoveryState` custody?"** `StateStore` holds the PKCE
  verifier, expected issuer, `require_issuer` flag and requested scopes keyed by CSRF token, and the
  state is deleted only **after** the issuer check — stronger than the SDK's discovery-state blob.
* **Dissolved: "does rmcp expose a `clientInformation` hook that can return nothing?"** Not needed:
  rmcp's refresh path does not clear credentials on `invalid_client`, so the destructive retry the
  config-stub suppression defends against does not exist. MCP-314 becomes "re-apply the full client
  configuration after `initialize_from_store`", and drops from `critical` to `high`.
* **Confirmed and no longer a blind spot: the client-auth-method divergence is real.** rmcp selects
  request-body auth only when the AS advertises `client_secret_post` **without** `client_secret_basic`
  — the TypeScript SDK's rule, not the adapter's. There is no `addClientAuthentication` hook; the
  lever is `set_metadata` before `configure_client`.
* **Dissolved: `HostServices::exec` being denied by default blocks `!command` secrets and browser
  launch.** That gate is the WASM-guest capability model. `crates/cyrup-mcp` is a native built-in
  crate and calls `std::process::Command` and `opener` directly. MCP-338 and MCP-349 are
  `extension-owned`.
* **Dissolved: `interpolate_env_vars` is unreachable from a new crate.** Promoting an existing
  `pub(crate)` item to `pub` is a visibility chore, the same class as the `npx_resolver` reuse — not a
  host addition. The **real** finding survives and is the point of MCP-342: neither existing copy
  implements pi's third placeholder form.
* **Dissolved: `regex` is not a workspace dependency, so the `WWW-Authenticate` scanner must be
  hand-written.** rmcp's `WWWAuthenticateParams::parse` does it, and a native crate could add its own
  dependency regardless.
* **Refuted: MCP-343, "non-unix targets cannot generate a PKCE verifier."** rmcp generates the PKCE
  verifier and CSRF token through `oauth2`; `cyrup_provider::auth::oauth::random` is not in this path.
  No workspace RNG dependency is required by this section.
* **Dissolved: the two competing cyrup PKCE implementations.** Neither is used here; which one
  survives in the workspace is not this section's decision.
* **Demoted from `critical` to `high`:** MCP-304 (endpoint configuration — blocking-ness, not
  severity), MCP-314 (no destructive retry exists under rmcp), MCP-328 (a discarded refresh token is
  a forced re-login, not durable data loss). **Demoted to `n/a` or `low`:** MCP-311, MCP-317, MCP-318's
  protocol half, MCP-320, MCP-322, MCP-337 — all now `rmcp`. **One `critical` remains:** MCP-306, the
  callback handler's CSRF boundary, which is a permission-bypass clause hit.
* **Dropped as dead scaffolding:** the previous MCP-348 (which ADR governs this subsystem). Sequencing
  and document authority live in the methodology, not in a port unit.
* **Dropped:** all `depends` edges, the `Confidence` field, and every revision-provenance paragraph.
* **Named delta added:** rmcp does not send the `MCP-Protocol-Version` header on discovery requests,
  where the TypeScript SDK does.
* **Named delta added:** rmcp appends the `offline_access` scope (SEP-2207) but does **not** append
  `prompt=consent`; a server that needs it must get it through `oauth.authorizationParams`, where
  `prompt` is not a reserved key.
* **Named delta added:** rmcp's `register_client` errors when the AS publishes no
  `registration_endpoint` rather than defaulting to `/register` on the AS origin.
* **Named delta added:** under rmcp a `!command` client secret is resolved once at configuration time
  rather than up to three times per token leg.
