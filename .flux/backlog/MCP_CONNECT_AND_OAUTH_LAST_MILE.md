---
stage: aug
status: done
updated: 2026-08-27 06:00
---

# One Connect Attempt, End To End: The Probe Ladder, Typed Enrichment, And OAuth's Last Production Hop

## Objective

Make **one HTTP attempt against one server** complete, from the request the manager issues to the
diagnosis it produces when that request fails and the credential acquisition that failure is supposed
to trigger. Seven units, five files, one branch:

| unit | what lands | file |
|---|---|---|
| `MCP-132` | `crate::probe` — `mcp-probe.ts`'s three-strategy classifier | new `src/probe.rs` |
| `MCP-133` | `enrichHttpConnectionError` in `connect_inner`'s marked seam | `server_manager.rs` |
| `MCP-123` | the cleanup-class survives that enrichment (typed wrapper + rebuild arms) | `errors.rs`, `server_manager.rs` |
| `MCP-129` | `get_prompt` / `read_resource` on the manager, and the disabled-check split | `server_manager.rs` |
| `MCP-122` | the production `HandlerFactory` and the completion notice | `runtime.rs`, `server_manager.rs` |
| `MCP-309` | the stored-token provider (hop A) and the carried `WWW-Authenticate` (hop B) | `oauth.rs`, `runtime.rs`, `server_manager.rs` |
| `MCP-313` | the RFC 7591 registration POST with the full body | `oauth.rs` |

Two units the previous draft carried are **removed from scope** — see
[What this task must NOT do](#what-this-task-must-not-do).

**Read [MCP_HIGH_SEVERITY_BACKLOG.md](MCP_HIGH_SEVERITY_BACKLOG.md) §"Wave 1" first.** It is the hard
prerequisite for §3 and §6 below, and its filed sibling
[MCP_DISCOVERY_PAGINATION.md:135-203](MCP_DISCOVERY_PAGINATION.md) already prescribes the exact
`ConnectionResource::peer()` signature and `NewConnection::bare` reshape this task consumes.

---

## Augmentation findings — premises this pass overturned or corrected

Everything below was re-derived against the tree and against
[tmp/pi-mcp-adapter](../../tmp/pi-mcp-adapter) at `v2.26.1` (`fafae21`). **Nine of the previous
draft's claims were wrong.** Three of them would have produced code that compiles and is silently
broken.

### F1 · The keychain storage adapter **already exists** — the previous §5 must not be written

The previous draft asserted "`impl McpOAuthStorage for McpAuthStore` … **does not exist**" and
prescribed writing it. **False.** It is at
[credentials.rs:3551-3626](../../crates/cyrup-mcp/src/credentials.rs), with a 32-line doc block at
[credentials.rs:3519-3550](../../crates/cyrup-mcp/src/credentials.rs) carrying the trait→store
delegation table, the "`None` arms take no `server_url`" rule (`mcp-auth.ts:983`/`:994`) and the
`McpError::CredentialStore` error-class note. The earlier grep was
`impl McpOAuthStorage for`; the source reads `impl crate::oauth::McpOAuthStorage for McpAuthStore`.

Two consequences:

* the previous §5's instruction "**Do not** override `get_auth_for_url`" is contradicted by the tree —
  it *is* overridden, at [credentials.rs:3557-3563](../../crates/cyrup-mcp/src/credentials.rs),
  delegating to `auth_for_url_async`, which is 13f's own fail-closed serialized accessor and is
  strictly better than the trait default at
  [oauth.rs:1429-1441](../../crates/cyrup-mcp/src/oauth.rs). Leave it alone;
* what is actually missing is one **construction**: nothing in the crate ever calls
  `McpAuthStore::new` ([credentials.rs:2056](../../crates/cyrup-mcp/src/credentials.rs)).
  `grep -rn 'McpAuthStore::new\|Arc<McpAuthStore>' crates/cyrup-mcp/src` outside `credentials.rs`
  returns nothing. `initialize_mcp` already holds both of its arguments — `dirs`
  ([runtime.rs:127](../../crates/cyrup-mcp/src/runtime.rs)) and `auth_storage_options`
  ([runtime.rs:156-162](../../crates/cyrup-mcp/src/runtime.rs)) — so this is two lines in §5 below.

### F2 · The probe **never returns an empty classification**, and a failed rung does not fall through

The previous draft prescribed `probe_mcp_endpoint(...) -> ProbeOutcome` with
`classification: String::new()` on a timeout, plus an `if outcome.classification.is_empty()` check in
the enricher. Both are inventions. Upstream:

* [mcp-probe.ts:157-163](../../tmp/pi-mcp-adapter/mcp-probe.ts) — `probe()` is
  `const response = await fetch(url, { ...strategy.request, signal: AbortSignal.timeout(PROBE_TIMEOUT_MS) })`
  with **no catch**. A timeout or a transport error rejects out of `probe`, out of `probeMcpEndpoint`,
  and is caught by the one and only `try` in the system;
* [server-manager.ts:637-645](../../tmp/pi-mcp-adapter/server-manager.ts) — `enrichHttpConnectionError`
  is `try { probe } catch { return original }`. That catch is where "swallow-all" lives.

`probeMcpEndpoint` has exactly two exits — an `McpProbeResult` with a **non-empty** classification, or
a throw. So the Rust signature is **fallible**:

```rust
pub async fn probe_mcp_endpoint(client: &reqwest::Client, url: &str) -> McpResult<ProbeOutcome>
```

and MCP-133's verify line — "*a probe that itself times out yields the bare original message*"
([13c-mcp-servers.md:1642-1643](../../docs/gap-analysis/13c-mcp-servers.md)) — is satisfied by the
`Err` arm, not by a sentinel. Likewise the previous draft's "*a timeout or transport error on a rung
falls to the next rung exactly as a non-matching status does*" is **false**: rung 1 throwing means the
whole ladder throws.

### F3 · `aggregate_head` must **not** be extended; `rebuild_manager_error` / `rebuild_mcp_error` **must** be

This is the finding that turns a silently-broken change into a correct one.

The previous draft prescribed `McpError::ProbeEnriched { original, .. } => original.aggregate_head()`.
It is wrong twice over:

1. **Unfaithful.** Upstream's enriched error is `new Error(msg, { cause })` — a plain `Error`, never an
   `AggregateError`. The three `error.message === "MCP connection abort cleanup failed"` comparisons
   ([server-manager.ts:591](../../tmp/pi-mcp-adapter/server-manager.ts), `:918`, `:939`) all execute
   **inside** `createConnection` / `connectHttpClient`, i.e. strictly before `connect`'s
   `.catch(enrichHttpConnectionError)` at
   [server-manager.ts:283-285](../../tmp/pi-mcp-adapter/server-manager.ts) ever runs. Upstream never
   compares an enriched error's head, so exposing one here invents behaviour.
2. **Useless where it was aimed.** Both rebuild functions match on the **pair**:
   [server_manager.rs:329-338](../../crates/cyrup-mcp/src/server_manager.rs) and
   [server_manager.rs:360-370](../../crates/cyrup-mcp/src/server_manager.rs) read
   `(inner.aggregate_head(), inner.aggregate_children())` and only the `(Some, Some)` arm rebuilds.
   `(Some(head), None)` falls straight through to `_ => McpError::Other(inner.to_string())`.

**What actually matters.** `McpServerManager::connect` is
`to_mcp(self.connect_inner(...).await)` ([server_manager.rs:1690-1697](../../crates/cyrup-mcp/src/server_manager.rs)),
and `to_mcp` is `McpError::from(error.as_ref())`
([server_manager.rs:412-414](../../crates/cyrup-mcp/src/server_manager.rs)) — i.e. every public
connect failure goes through `rebuild_manager_error`. Without an explicit `ProbeEnriched` arm in both
functions, the wrapper is flattened to `McpError::Other("<original> — probe: <c>")` at the public
boundary, the `#[source]` edge is destroyed, and `is_cleanup_failure` can no longer see the class —
which is precisely MCP-123's residual and precisely the `From<&ManagerError>` defect class the ledger
already records at
[13-cyrup-mcp-STATUS.md:310-320](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md).

The `close` / `close_all` paths are *not* affected, and it is worth knowing why: they test
`Arc<ManagerError>::is_cleanup_failure`
([server_manager.rs:224-249](../../crates/cyrup-mcp/src/server_manager.rs)), whose `Mcp` arm delegates
to `McpError::is_cleanup_failure`
([errors.rs:376-407](../../crates/cyrup-mcp/src/errors.rs)), which already walks
`std::error::Error::source` at [errors.rs:401-404](../../crates/cyrup-mcp/src/errors.rs). A
`#[source]`-carrying wrapper is transparent to them for free. It is only the `From<&ManagerError>`
one-way door that flattens.

### F4 · MCP-313 has **two** upstream client-metadata shapes, and the second is unreachable in the port

[mcp-oauth-provider.ts:233-260](../../tmp/pi-mcp-adapter/mcp-oauth-provider.ts) branches on
`usesClientCredentials`:

* the `client_credentials` shape carries `redirect_uris: []`, `grant_types: ["client_credentials"]`,
  **no** `response_types` and **no** `scope`;
* the `authorization_code` shape carries `redirect_uris: [redirectUrl]`,
  `grant_types: ["authorization_code","refresh_token"]`, `response_types: ["code"]` and `scope` only
  when `config.scope !== undefined`.

Both shapes carry `client_name`, `client_uri` (spread only when defined), `logo_uri` (likewise) and
`token_endpoint_auth_method`. 13g names both at
[13g-mcp-oauth.md:980-981](../../docs/gap-analysis/13g-mcp-oauth.md).

**The port cannot reach the first one.** `authenticate_client_credentials`
([oauth.rs:2664-2692](../../crates/cyrup-mcp/src/oauth.rs)) is a separate function that never calls
`prepare_session`, and it hard-fails with `… client_credentials OAuth requires oauth.clientId` when
`config.client_id` is absent — so dynamic registration is unreachable for that grant by construction.
Build the `authorization_code` shape only; record the other as a structural non-divergence in the new
type's doc.

### F5 · `application_type` is a port decision, not upstream fidelity

Upstream's `clientMetadata` has **no** `application_type` key
([mcp-oauth-provider.ts:250-259](../../tmp/pi-mcp-adapter/mcp-oauth-provider.ts)); the TypeScript SDK
injects it. rmcp sends `application_type: "native"` today
(`DEFAULT_APPLICATION_TYPE`, `rmcp-3.1.4/src/transport/auth.rs:204`, used at `:684`, `:1318`, `:1688`),
which is exactly what 13g's MCP-312 note records at
[13g-mcp-oauth.md:968-969](../../docs/gap-analysis/13g-mcp-oauth.md). Send `"native"` so nothing else
on the wire changes, and say in the doc comment that it is rmcp's default being preserved, not an
upstream field.

### F6 · Taking rmcp's pre-registered branch **skips `validate_server_metadata`**

`register_client` calls `self.validate_server_metadata("code")?` at
`rmcp-3.1.4/src/transport/auth.rs:1671`. `AuthorizationSession::new`'s branch 1 (`:3360-3367`) builds
an `OAuthClientConfig` and goes straight to `configure_client` (`:3419`), which does **not** validate
(`:1570-1600`). So handing rmcp a pre-registered client — which is the whole mechanism of §7 — removes
two guards: the `response_types_supported` check and the `code_challenge_methods_supported` /
`S256` check (`:1622-1653`). **Reproduce both before the POST.** Skipping them silently would let a
non-PKCE authorization server reach `get_authorization_url` and fail later, further from the cause.

### F7 · `AuthorizationSession::new` rejects a secret without a client id

`rmcp-3.1.4/src/transport/auth.rs:3324-3333`: `request.client_secret.is_some() &&
request.client_id.is_none()` is an immediate `RegistrationFailed`. Pairing
`with_client_secret` with `with_preregistered_client` is therefore mandatory, not stylistic.

### F8 · rmcp type names, and the `arguments` omit rule is free

rmcp 3.1.4 spells the request params **plural**: `GetPromptRequestParams`
(`src/model.rs:2326-2340`) and `ReadResourceRequestParams` (`:1678-1691`). `arguments` is
`Option<JsonObject>` with `#[serde(skip_serializing_if = "Option::is_none")]` (`:2331-2332`), so
upstream's `...(args ? { arguments: args } : {})` — a key **omitted**, not `null` — is what `None`
already produces. No custom serializer.

Neither `Peer::get_prompt_once` (`src/service/client.rs:1409-1427`) nor `read_resource_once`
(`:1431`) accepts `PeerRequestOptions`, and §3.13 requires the per-call timeout
([13c-mcp-servers.md:593-594](../../docs/gap-analysis/13c-mcp-servers.md)), so the two verbs go through
`Peer::send_request_with_option` (`src/service.rs:850-857`) + `RequestHandle::await_response`
(`:544`). rmcp's own `get_prompt_once` body is the template for the request literal and the
`ServerResult` match.

### F9 · MCP-347 and MCP-341 are not source deliverables

MCP-347's remaining content is a stub authorization server and an acceptance suite
([oauth.rs:3970-3975](../../crates/cyrup-mcp/src/oauth.rs)); MCP-341's deliverable is a prose file and
its verify line is literally "*a review checklist*"
([13g-mcp-oauth.md:1391-1399](../../docs/gap-analysis/13g-mcp-oauth.md)). Both are out of scope here.
§14's audit survives as **research** — see [The §14 audit](#the-14-audit) — because two of its eight
items are exactly what §5 and §7 change.

### F10 · Citation drift since the previous draft

Every one of these moved. Use the right-hand column.

| previous draft | actual, today |
|---|---|
| `Cargo.toml:191` reqwest | [Cargo.toml:204](../../crates/cyrup-mcp/Cargo.toml) |
| `Cargo.toml:107` regex | [Cargo.toml:120](../../crates/cyrup-mcp/Cargo.toml) |
| `runtime.rs:2554-2561` MCP-123 residual | [runtime.rs:2558-2568](../../crates/cyrup-mcp/src/runtime.rs) |
| `runtime.rs:2585-2586` arm 2 comment | [runtime.rs:2659-2660](../../crates/cyrup-mcp/src/runtime.rs) |
| `runtime.rs:2666` challenge binding | [runtime.rs:2672](../../crates/cyrup-mcp/src/runtime.rs) |
| `runtime.rs:2678-2696` `NeedsAuth` arm | [runtime.rs:2681-2699](../../crates/cyrup-mcp/src/runtime.rs) |
| `runtime.rs:2637-2643` `Connected` arm | [runtime.rs:2644-2650](../../crates/cyrup-mcp/src/runtime.rs) |
| `runtime.rs:2617-2629` the `Explicit` arm | [runtime.rs:2620-2636](../../crates/cyrup-mcp/src/runtime.rs) |
| `runtime.rs:2344-2351` `HttpConnection` | [runtime.rs:2349-2358](../../crates/cyrup-mcp/src/runtime.rs) |
| `runtime.rs:2287-2292` `with_handler_factory` | [runtime.rs:2294-2299](../../crates/cyrup-mcp/src/runtime.rs) |
| `runtime.rs:2296` `with_auth_provider` | [runtime.rs:2301-2306](../../crates/cyrup-mcp/src/runtime.rs) |
| `runtime.rs:1933-1945` `bare_handler_factory` | [runtime.rs:1939-1952](../../crates/cyrup-mcp/src/runtime.rs) |
| `runtime.rs:1901-1918` `NoStoredCredentials` | [runtime.rs:1908-1926](../../crates/cyrup-mcp/src/runtime.rs) |
| `runtime.rs:3019-3022` `SetupFailed` producer | [runtime.rs:3027-3029](../../crates/cyrup-mcp/src/runtime.rs) |
| `runtime.rs:2965-2973` needs-auth early return | [runtime.rs:2970-2980](../../crates/cyrup-mcp/src/runtime.rs) |
| `runtime.rs:1618-1643` complete dispatch | [runtime.rs:1619-1643](../../crates/cyrup-mcp/src/runtime.rs) |
| `oauth.rs:2955-2970` persist | [oauth.rs:2953-2971](../../crates/cyrup-mcp/src/oauth.rs) |
| `oauth.rs:3794-3797` `TODO(MCP-341)` | [oauth.rs:3795-3797](../../crates/cyrup-mcp/src/oauth.rs) |
| `proxy.rs:1478-1484` `ProxyEnv::read_resource` | [proxy/env.rs:287-293](../../crates/cyrup-mcp/src/proxy/env.rs) |
| `proxy.rs:4882` `FakeEnv` | [proxy/testsupport.rs:41,91](../../crates/cyrup-mcp/src/proxy/testsupport.rs) |
| `proxy.rs:1302` `UrlElicitationAction` | `proxy/` (the file was split; `proxy.rs` no longer exists) |
| `rmcp .../client.rs:749-828` | `rmcp-3.1.4/src/service/client.rs:749-831` |

### F11 · Two claims from the previous draft that **hold**, re-verified

* **Finding 4 (the `get_prompt` divergence) is real.** Upstream `getPrompt`
  ([server-manager.ts:1055-1076](../../tmp/pi-mcp-adapter/server-manager.ts)) has **only** the
  `status === "connected"` precondition; `readResource` (`:1078-1095`) tests
  `isServerDisabled(this.connections.get(name)?.definition)` **first**. The port's `begin_request`
  ([server_manager.rs:2634-2660](../../crates/cyrup-mcp/src/server_manager.rs)) applies the disabled
  test to every caller, so a disabled server's `get_prompt` would answer
  `MCP server "x" is disabled` where upstream answers `Server "x" is not connected`.
* **Finding 5 (no producer for the cleanup variants) is real and measured.**
  `rmcp-3.1.4/src/service/client.rs:749-831`: `serve_client_with_ct_inner` binds
  `let mut transport = transport.into_transport();` and every failure path is a bare `?` /
  `return Err(...)`. `Transport::close()` is never called, and `serve_client_with_lifecycle_and_ct`
  (`:724-747`) only races that future against the token. A drop cannot report a failure, so
  `McpError::HttpCleanupFailed` / `AbortCleanupFailed` have no producer and cannot acquire one without
  rmcp changing. **Do not synthesise a fake close; do not delete the variants** —
  [errors.rs:376-407](../../crates/cyrup-mcp/src/errors.rs)'s walk must still recognise them through a
  nested source. Note the neighbouring claim that `SetupFailed` is unproduced is *already* corrected in
  the tree: [runtime.rs:2947-2955](../../crates/cyrup-mcp/src/runtime.rs) documents its narrow live
  producer and [runtime.rs:3027](../../crates/cyrup-mcp/src/runtime.rs) raises it.

### F12 · Two stale in-code comments, and one wiring trap

* [runtime.rs:204](../../crates/cyrup-mcp/src/runtime.rs) says "`setTraceConfig` has no counterpart at
  all — `mcp-trace.ts` is MCP-133, unported." **Wrong unit.**
  [server_manager.rs:1146-1147](../../crates/cyrup-mcp/src/server_manager.rs) has it right, and the
  trace unit is `MCP-480` ([13-cyrup-mcp-STATUS.md:967](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)),
  triaged by [MCP_13I_SCOPING.md](MCP_13I_SCOPING.md). A porter who reads line 204 and "implements
  MCP-133" writes a JSONL tracer and leaves the probe unbuilt.
* `reqwest` and `regex` are **already dependencies** (F10). The `TODO(MCP-312)` sentence at
  [oauth.rs:2633-2635](../../crates/cyrup-mcp/src/oauth.rs) and `prepare_session`'s doc paragraph at
  [oauth.rs:2509-2514](../../crates/cyrup-mcp/src/oauth.rs) both claim otherwise. Delete both; add no
  dependency line.
* **The wiring trap.** The previous draft's §4 passed `elicitation_mode: None` to the production
  handler factory. `McpClientHandler::new` derives `allow_url` from exactly that field
  ([runtime.rs:1460-1462](../../crates/cyrup-mcp/src/runtime.rs)), and the completion dispatch is gated
  on `aborted || !allow_url` ([runtime.rs:1623](../../crates/cyrup-mcp/src/runtime.rs)) — so `None`
  installs a hook that can never fire and MCP-122 ships inert. Pass the **real step-6 gate**
  ([runtime.rs:118](../../crates/cyrup-mcp/src/runtime.rs)) instead; §4 below does. Note the capability
  advertisement reads `parts.elicitation.is_some()`
  ([runtime.rs:1463-1466](../../crates/cyrup-mcp/src/runtime.rs)), so leaving MCP-118's hook `None`
  keeps `elicitation` off the `initialize` frame — correct, and independent of `allow_url`.

### F13 · Confirmed still absent (the work is real)

`grep -rn 'classify_response\|probe_mcp_endpoint\|MODERN_FALLBACK_STATUSES\|json_rpc_envelope_info\|is_bearer_challenge\|ProbeEnriched\|enrich_http_connection_error\|url_elicitation_completed_message\|last_auth_challenge\|StoredCredentialProvider\|manager_handler_factory\|fn get_prompt\|refuse_if_disabled' crates/cyrup-mcp/src`
returns **only** `proxy/env.rs:287` and `proxy/testsupport.rs:123` (the `ProxyEnv::read_resource`
trait method and its test double). No `probe` module; no `probe` entry in
[lib.rs:132-151](../../crates/cyrup-mcp/src/lib.rs). `ConnectionResource`
([server_manager.rs:510](../../crates/cyrup-mcp/src/server_manager.rs)) still has no `peer()` —
`McpConnection::peer()` is an inherent method at
[runtime.rs:2184-2193](../../crates/cyrup-mcp/src/runtime.rs) whose doc says so in as many words.
`with_handler_factory` ([runtime.rs:2296](../../crates/cyrup-mcp/src/runtime.rs)) has **zero** callers,
tests included; `with_auth_provider` ([runtime.rs:2303](../../crates/cyrup-mcp/src/runtime.rs)) has
eight, all in `runtime.rs`'s own `#[cfg(test)]`.

### F14 · "Production" here means "reachable from `initialize_mcp`"

[runtime.rs:178-192](../../crates/cyrup-mcp/src/runtime.rs) records it: `initialize_mcp` has no
non-test caller, because `McpExtension::on_session_start` is still MCP-008/MCP-011's empty body
([extension.rs:455-470](../../crates/cyrup-mcp/src/extension.rs)). Every "production" obligation below
is therefore discharged at the wiring block
([runtime.rs:193-216](../../crates/cyrup-mcp/src/runtime.rs)) — the one place that builds the live
manager — and the Definition of Done is written against that, not against a running session.

**§3.1 of [MCP_13I_SCOPING.md:221-238](MCP_13I_SCOPING.md) is the reason §4 matters beyond MCP-122**:
it names "the manager supplying a production handler factory" as the single structural blocker under
all of `MCP-450`…`MCP-472`, and assigns it to MCP-118/MCP-120/**MCP-122** in 13c. §4 is that
unblocker.

---

## Implementation

Read [MCP_DISCOVERY_PAGINATION.md:135-203](MCP_DISCOVERY_PAGINATION.md) before touching
`server_manager.rs`. It prescribes `ConnectionResource::peer() -> Option<&Peer<RoleClient>>` and
`NewConnection::bare(resource, status)`; both are Wave 1's, not yours to re-derive.

### 1 · `crate::probe` — the classifier (MCP-132)

New file `crates/cyrup-mcp/src/probe.rs`. Add `pub mod probe;` to
[lib.rs](../../crates/cyrup-mcp/src/lib.rs) between `pub mod owner;` (line 142) and `pub mod proxy;`
(line 143) — alphabetical position, `owner < probe < proxy` — and one row to the Cut-2 module-map
table at [lib.rs:109-115](../../crates/cyrup-mcp/src/lib.rs):
`| [\`probe\`] | \`mcp-probe.ts\` | the three-strategy endpoint probe (13c §3.14) |`.
(The table already omits `server_manager` and `errors`; do not widen the fix here.)
**No `Cargo.toml` change** — F12.

Spec: [mcp-probe.ts](../../tmp/pi-mcp-adapter/mcp-probe.ts) in full (187 lines), transcribed as
[13c-mcp-servers.md:601-673](../../docs/gap-analysis/13c-mcp-servers.md).

```rust
//! `mcp-probe.ts` — the three-strategy endpoint probe (MCP-132, 13c §3.14).
//!
//! Diagnostics only. The probe never selects a transport, never carries credentials, cookies or
//! configured headers, and its sole consumer is
//! [`crate::server_manager::enrich_http_connection_error`] (MCP-133).
//!
//! # Fallible on purpose
//!
//! `probeMcpEndpoint` has exactly two exits: an `McpProbeResult` whose `classification` is always a
//! non-empty string, or a throw — `probe()` (`mcp-probe.ts:157-163`) wraps `fetch` with
//! `AbortSignal.timeout(5_000)` and **no** catch, so a timeout or a transport error on ANY rung
//! rejects the whole ladder rather than falling through to the next one. The single `try` in the
//! system is `enrichHttpConnectionError`'s (`server-manager.ts:639-644`), and that is where the
//! swallow-all rule lives. Returning `McpResult` here keeps that structure instead of smuggling a
//! sentinel classification through the success channel.

use std::time::Duration;

use regex::Regex;
use serde_json::Value;

use crate::errors::{McpError, McpResult};

/// `PROBE_TIMEOUT_MS = 5_000` (`mcp-probe.ts:1`) — **per request**, not per ladder.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// `mcp-probe.ts:2-7`.
const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";
const LEGACY_PROTOCOL_VERSION: &str = "2025-06-18";
const JSON_ACCEPT: &str = "application/json, text/event-stream";
const SSE_ACCEPT: &str = "text/event-stream";
const MODERN_FALLBACK_STATUSES: [u16; 6] = [400, 401, 404, 405, 406, 415];
const POST_ENDPOINT_MISMATCH_STATUSES: [u16; 4] = [404, 405, 406, 415];
/// `clientInfo` of the legacy `initialize` body (`mcp-probe.ts:28`). Kept **byte-exact**, stale
/// version included: it is what a server operator sees in their logs and what MCP-340 filed. Bumping
/// it is MCP-340's decision, not this unit's.
const PROBE_CLIENT_NAME: &str = "pi-mcp-probe";
const PROBE_CLIENT_VERSION: &str = "2.1.2";

/// `isBearerChallenge`: `/(?:^|,)\s*Bearer\b/i` (`mcp-probe.ts:102-104`).
///
/// `Option` rather than `unwrap` because this crate denies `clippy::unwrap_used`; the pattern is a
/// literal with two bounded quantifiers and cannot fail to compile, so `None` is unreachable and
/// degrades to "no Bearer challenge" — the conservative answer.
static BEARER_CHALLENGE: std::sync::LazyLock<Option<Regex>> =
    std::sync::LazyLock::new(|| Regex::new(r"(?i)(?:^|,)\s*Bearer\b").ok());

/// `McpProbeResult` (`mcp-probe.ts:9-12`).
///
/// `classification` is **user-visible** — MCP-133 interpolates it into a connect-failure message —
/// so every string this module produces is byte-exact against `mcp-probe.ts`, including the em-dash
/// in [`not_mcp`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOutcome {
    /// `isMcp`.
    pub is_mcp: bool,
    /// `classification`. Never empty.
    pub classification: String,
}

/// `ProbeStrategy` (`mcp-probe.ts:32-69`). All three hit the **same** URL; `LegacySse` is a GET
/// against the configured endpoint, **not** a `/sse` path (13c:614).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strategy {
    Modern,
    LegacyPost,
    LegacySse,
}

impl Strategy {
    /// `allowJson` — `true` for the two POST arms, `false` for the GET.
    const fn allow_json(self) -> bool {
        !matches!(self, Strategy::LegacySse)
    }
}

/// `JsonRpcEnvelopeInfo` (`mcp-probe.ts:71-73`).
enum Envelope {
    /// `protocol_version` is `result.protocolVersion` **only when `result` is itself an object**;
    /// otherwise `None`, which is upstream's `undefined`.
    Result { protocol_version: Option<Value> },
    Error,
}

/// `ProbeOutcome` (`mcp-probe.ts:75-78`) — the per-rung verdict, not the public result.
enum Rung {
    Mcp(ProbeOutcome),
    UnsupportedModern,
    Unrecognized,
}
```

Then, in order:

* **`fn json_rpc_envelope_info(value: &Value) -> Option<Envelope>`** —
  [mcp-probe.ts:85-100](../../tmp/pi-mcp-adapter/mcp-probe.ts). Object, non-null,
  `jsonrpc == "2.0"`; `value.get("result").is_some()` → `Envelope::Result` (note: **key presence**,
  upstream's `"result" in value`, so an explicit `"result": null` still qualifies — use
  `Value::get`, never `as_object().and_then(...).filter(|v| !v.is_null())`);
  else `value.get("error").is_some()` → `Envelope::Error`; else `None`.
* **`fn is_bearer_challenge(www_authenticate: Option<&str>) -> bool`** — the regex over
  `www_authenticate.unwrap_or_default()` (upstream's `?? ""`).
* **`fn response_kind(content_type: Option<&str>) -> String`** —
  [mcp-probe.ts:106-111](../../tmp/pi-mcp-adapter/mcp-probe.ts). Exactly
  `value.split(';').next().map(str::trim).map(str::to_ascii_lowercase)`, **not** a MIME crate, so a
  malformed header behaves as `responseKind` does. `"text/html"` → `"HTML"`; a non-empty type → that
  type; absent **or empty after the split/trim** → `"an untyped response"` (upstream's `if (contentType)`
  is falsy for `""`, and a header of `"; charset=utf-8"` produces exactly that).
* **`fn not_mcp(content_type: Option<&str>, status: u16) -> ProbeOutcome`** →
  `format!("endpoint returned {kind} ({status}) — this URL does not appear to speak MCP")`, `is_mcp:
  false`. The separator is U+2014 EM DASH with a space either side.
* **`fn is_sse(content_type: Option<&str>) -> bool`** — `content_type.to_ascii_lowercase()
  .starts_with("text/event-stream")` over the **whole** header, deliberately *not* the split-and-trim
  [`response_kind`] performs ([mcp-probe.ts:122](../../tmp/pi-mcp-adapter/mcp-probe.ts) uses
  `.toLowerCase().startsWith(...)`). Note the divergence in a comment or a reader will "unify" them.
* **`fn classify_response(strategy, status, content_type, www_authenticate, body) -> Rung`** —
  [mcp-probe.ts:121-155](../../tmp/pi-mcp-adapter/mcp-probe.ts), five rungs in order. `ok` is
  `(200..300).contains(&status)`, reqwest's `StatusCode::is_success`.

```rust
fn classify_response(
    strategy: Strategy,
    status: u16,
    content_type: Option<&str>,
    www_authenticate: Option<&str>,
    body: &str,
) -> Rung {
    let ok = (200..300).contains(&status);

    // 1. `if (response.ok && isSse)`
    if ok && is_sse(content_type) {
        return Rung::Mcp(ProbeOutcome {
            is_mcp: true,
            classification: "endpoint responded with an MCP event stream".to_string(),
        });
    }

    // 2. `const envelope = (strategy.allowJson || response.status === 401) ? await
    //     getJsonRpcEnvelopeInfo(response) : null;` — a swallow-all `try` around `JSON.parse`.
    let envelope = (strategy.allow_json() || status == 401)
        .then(|| serde_json::from_str::<Value>(body).ok())
        .flatten()
        .as_ref()
        .and_then(json_rpc_envelope_info);

    // 3. `if (response.ok && strategy.allowJson && envelope)`
    if ok && strategy.allow_json()
        && let Some(envelope) = envelope.as_ref()
    {
        if strategy == Strategy::Modern {
            let modern = matches!(
                envelope,
                Envelope::Result { protocol_version: Some(Value::String(version)) }
                    if version == MODERN_PROTOCOL_VERSION
            );
            // `envelope.kind === "error" || envelope.protocolVersion !== MODERN_PROTOCOL_VERSION`
            if !modern {
                return Rung::UnsupportedModern;
            }
        }
        return Rung::Mcp(ProbeOutcome {
            is_mcp: true,
            classification: if strategy == Strategy::Modern {
                format!("endpoint supports stateless MCP {MODERN_PROTOCOL_VERSION} server/discover")
            } else {
                "endpoint responded with a JSON-RPC 2.0 envelope".to_string()
            },
        });
    }

    // 4. `if (response.status === 401 && isBearerChallenge(response) && envelope)`
    if status == 401 && is_bearer_challenge(www_authenticate) && envelope.is_some() {
        return Rung::Mcp(ProbeOutcome {
            is_mcp: true,
            classification: if strategy == Strategy::Modern {
                format!(
                    "endpoint requires Bearer authentication during MCP \
                     {MODERN_PROTOCOL_VERSION} server/discover probing"
                )
            } else {
                "endpoint requires Bearer authentication and responded with a JSON-RPC 2.0 error"
                    .to_string()
            },
        });
    }

    // 5.
    Rung::Unrecognized
}
```

* **`async fn probe(client, url, strategy) -> McpResult<(u16, Rung, Option<String>)>`** — one request.
  Build it from `strategy`, exactly:

  | strategy | method | headers | body |
  |---|---|---|---|
  | `Modern` | POST | `Accept: application/json, text/event-stream`, `Content-Type: application/json`, `MCP-Protocol-Version: 2026-07-28`, `Mcp-Method: server/discover` | `{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{}}` |
  | `LegacyPost` | POST | `Accept: application/json, text/event-stream`, `Content-Type: application/json` | `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"pi-mcp-probe","version":"2.1.2"}}}` |
  | `LegacySse` | GET | `Accept: text/event-stream` | — |

  Wrap the *whole* send-plus-read in one `tokio::time::timeout(PROBE_TIMEOUT, …)` — `AbortSignal`
  covers the body read too — and map both the elapsed case and any `reqwest::Error` to
  `McpError::other(...)`. Read `content-type` and `www-authenticate` off the response **before**
  `text()` consumes it. Return the third element as the content type so the caller can build
  [`not_mcp`] without holding the response.
* **`pub async fn probe_mcp_endpoint(client: &reqwest::Client, url: &str) -> McpResult<ProbeOutcome>`** —
  [mcp-probe.ts:173-187](../../tmp/pi-mcp-adapter/mcp-probe.ts), plus the single Cut-1 arm:

```rust
pub async fn probe_mcp_endpoint(client: &reqwest::Client, url: &str) -> McpResult<ProbeOutcome> {
    let (modern_status, modern, modern_type) = probe(client, url, Strategy::Modern).await?;
    if let Rung::Mcp(result) = modern {
        return Ok(result);
    }
    // `if (modernOutcome.kind !== "unsupported-modern" && !MODERN_FALLBACK_STATUSES.has(status))`
    let unsupported_modern = matches!(modern, Rung::UnsupportedModern);
    if !unsupported_modern && !MODERN_FALLBACK_STATUSES.contains(&modern_status) {
        return Ok(not_mcp(modern_type.as_deref(), modern_status));
    }

    let (post_status, post, post_type) = probe(client, url, Strategy::LegacyPost).await?;
    if let Rung::Mcp(result) = post {
        return Ok(result);
    }
    if !POST_ENDPOINT_MISMATCH_STATUSES.contains(&post_status) {
        return Ok(not_mcp(post_type.as_deref(), post_status));
    }

    let (get_status, get, get_type) = probe(client, url, Strategy::LegacySse).await?;
    let Rung::Mcp(result) = get else {
        return Ok(not_mcp(get_type.as_deref(), get_status));
    };
    // ── the one Cut-1 arm (13c:667-673) ──────────────────────────────────────────────────────────
    //
    // PORT DIVERGENCE, and the only new string in this module. Upstream would answer "endpoint
    // responded with an MCP event stream" here, which is a *success* classification attached to a
    // connect failure — cyrup ships no SSE client transport, so that shape is unreachable, and the
    // whole point of the probe is to make the failure actionable.
    //
    // Gated on BOTH POSTs having been rejected as endpoint mismatches, not just the second: a modern
    // rung that answered 200 with `unsupported-modern` proves the endpoint DOES speak POST, so it is
    // not the legacy HTTP+SSE shape and its own diagnosis must stand.
    if POST_ENDPOINT_MISMATCH_STATUSES.contains(&modern_status)
        && POST_ENDPOINT_MISMATCH_STATUSES.contains(&post_status)
    {
        return Ok(ProbeOutcome {
            is_mcp: false,
            classification:
                "endpoint speaks the legacy HTTP+SSE transport, which cyrup does not support"
                    .to_string(),
        });
    }
    Ok(result)
}
```

**The client is the probe's, not the transport's.** Build it with
`reqwest::Client::builder().build()` and nothing else. Do **not** reuse
[`crate::runtime::build_http_client`](../../crates/cyrup-mcp/src/runtime.rs) (`runtime.rs:2092-2098`):
its `pool_max_idle_per_host(0)` and `redirect(Policy::none())` are the *transport's* contract — the
second one exists so resolved secrets cannot be replayed to a redirect target — while upstream's
`fetch` follows redirects and the probe carries no secret to leak. Carrying no `Authorization`, no
cookie jar and no configured header is the unauthenticated rule at
[13c-mcp-servers.md:615-616](../../docs/gap-analysis/13c-mcp-servers.md); reqwest has no cookie store
unless one is enabled, so "no cookies" is satisfied by construction — say so in the doc.

### 2 · The transparent enrichment wrapper (MCP-133 + MCP-123)

**(a) One variant**, in [errors.rs](../../crates/cyrup-mcp/src/errors.rs) immediately before
`Other` (line 283-286):

```rust
    /// `` new Error(`${originalMessage} — probe: ${probe.classification}`, { cause: error }) `` —
    /// `enrichHttpConnectionError` (`server-manager.ts:641`), MCP-133.
    ///
    /// **Transparent, and that transparency is MCP-123's residual.** Flattening the original into a
    /// string would erase the class [`McpError::is_cleanup_failure`] reads, and that predicate has
    /// two live consumers: `close`'s no-connection rethrow and `close_all`'s child filter. `#[source]`
    /// is upstream's `cause`, so the `source()` edge of the walk at [`McpError::is_cleanup_failure`]
    /// traverses it for free.
    ///
    /// **Not an aggregate.** Upstream's enriched error is a plain `Error`; the three
    /// `error.message ===` comparisons (`server-manager.ts:591`, `:918`, `:939`) all run inside
    /// `createConnection`/`connectHttpClient`, strictly before `connect`'s `.catch` (`:283-285`)
    /// wraps anything. [`McpError::aggregate_head`] therefore stays `None` here — see
    /// `server_manager.rs`'s `rebuild_mcp_error`, which grew an explicit arm instead.
    #[error("{original} — probe: {classification}")]
    ProbeEnriched {
        /// The connect failure this diagnosis is attached to.
        #[source]
        original: Box<McpError>,
        /// [`crate::probe::ProbeOutcome::classification`], never empty.
        classification: String,
    },
```

`is_cleanup_failure` needs **no change** — its `std::error::Error::source(current)` edge at
[errors.rs:401-404](../../crates/cyrup-mcp/src/errors.rs) already carries the class through.
`aggregate_head` / `aggregate_children` need **no change** — F3.

**(b) Two rebuild arms**, and this is the part that must not be skipped (F3). In
[server_manager.rs:322-347](../../crates/cyrup-mcp/src/server_manager.rs), `rebuild_manager_error`,
add above the `ManagerError::Mcp(inner)` catch-all:

```rust
        // MCP-123. Without this arm `to_mcp` flattens the wrapper to `Other(to_string())` and the
        // `#[source]` edge — the only thing that keeps a wrapped `SetupFailed` visible to
        // `is_cleanup_failure` — is destroyed at the public boundary.
        ManagerError::Mcp(McpError::ProbeEnriched {
            original,
            classification,
        }) => McpError::ProbeEnriched {
            original: Box::new(rebuild_mcp_error(original, remaining)),
            classification: classification.clone(),
        },
```

and the identical arm in `rebuild_mcp_error`
([server_manager.rs:353-370](../../crates/cyrup-mcp/src/server_manager.rs)), so a `ProbeEnriched`
nested inside an aggregate's children rebuilds too. Both recurse on `remaining`, so the existing
`AGGREGATE_REBUILD_DEPTH` fuse ([server_manager.rs:316](../../crates/cyrup-mcp/src/server_manager.rs))
covers them.

**(c) The enricher**, in `server_manager.rs` beside `connection_closed_while_connecting`
([server_manager.rs:116-126](../../crates/cyrup-mcp/src/server_manager.rs)):

```rust
/// `enrichHttpConnectionError(definition, error)` (`server-manager.ts:637-645`), MCP-133.
///
/// **Swallow-all is the specification, not defensiveness** (13c:660-665). A probe must never be able
/// to turn a connect failure into a *different* failure, so every arm below returns `original`
/// unchanged: a stdio server (no `url`, filtered by the caller), a URL that only *now* fails to
/// interpolate — upstream's `resolveServerUrl(definition)!` re-resolve, inside the same `try` — a
/// client that will not build, and a probe that times out or cannot reach the host.
///
/// The probe runs only on the failure path, so it costs a successful connect nothing.
async fn enrich_http_connection_error(url: Option<&str>, original: McpError) -> McpError {
    let Some(url) = url else { return original };
    let env = crate::credentials::process_env();
    let Ok(Some(resolved)) = crate::credentials::resolve_server_url(Some(url), &env) else {
        return original;
    };
    let Ok(client) = reqwest::Client::builder().build() else {
        return original;
    };
    let Ok(outcome) = crate::probe::probe_mcp_endpoint(&client, &resolved).await else {
        return original;
    };
    McpError::ProbeEnriched {
        original: Box::new(original),
        classification: outcome.classification,
    }
}
```

`crate::credentials::process_env()` is the production `EnvFn`
([credentials.rs:220-226](../../crates/cyrup-mcp/src/credentials.rs)) and is the same value
`ConnectionBuilder::new` installs at
[runtime.rs:2286](../../crates/cyrup-mcp/src/runtime.rs). The manager holds no env of its own and must
not grow one for this: the swallow-all rule makes an injected env unobservable, so a seam here would
be untestable ceremony.

**(d) Fill the seam.** Replace the comment at
[server_manager.rs:1758-1766](../../crates/cyrup-mcp/src/server_manager.rs) with a one-line
back-reference, and rewrite the `promise` future at
[server_manager.rs:1813-1835](../../crates/cyrup-mcp/src/server_manager.rs):

```rust
                let promise: ConnectFuture = {
                    let factory = Arc::clone(&self.factory);
                    let request = CreateConnection { /* unchanged */ };
                    let definition_for_record = Arc::clone(&definition);
                    // MCP-133 — `definition.url ? attempt.catch(…) : attempt`
                    // (`server-manager.ts:283-285`). URL servers ONLY, so a stdio failure is never
                    // wrapped and its message is byte-identical to before this unit.
                    let probe_url = definition.url.clone();
                    async move {
                        let created = match factory.create(request).await {
                            Ok(created) => created,
                            Err(error) => {
                                return Err(ManagerError::mcp(
                                    enrich_http_connection_error(probe_url.as_deref(), error).await,
                                ));
                            }
                        };
                        Ok(ServerConnection::new(
                            definition_for_record,
                            created.resource,
                            created.status,
                            created.credentials_invalidated,
                        ))
                    }
                    .boxed()
                    .shared()
                };
```

Wave 1 rewrites this same literal (it widens `NewConnection`); land whichever is second on top of the
first rather than in parallel.

**(e) Three comment corrections.**

1. [runtime.rs:204](../../crates/cyrup-mcp/src/runtime.rs) → ``` `setTraceConfig` has no counterpart
   at all — `mcp-trace.ts` is MCP-480, unported ``` (F12).
2. [runtime.rs:2558-2568](../../crates/cyrup-mcp/src/runtime.rs) — keep the finding, cite the
   measurement: `rmcp-3.1.4/src/service/client.rs:749-831` binds the transport to a local and every
   failure path is a bare `?`/`return Err`, so `Transport::close()` is never called and a drop cannot
   report a failure. Change "*That is MCP-123's residual verbatim*" to say the residual is discharged
   by [`McpError::ProbeEnriched`]'s transparency plus the two rebuild arms, and that no fake producer
   was added.
3. [server_manager.rs:1146-1147](../../crates/cyrup-mcp/src/server_manager.rs) — the "MCP-133 is
   **not** the trace unit" note becomes past tense; point it at
   `crate::server_manager::enrich_http_connection_error`.

### 3 · The two verbs (MCP-129) — after Wave 1

Spec: [13c-mcp-servers.md:1580-1590](../../docs/gap-analysis/13c-mcp-servers.md) and §3.13 at
[13c-mcp-servers.md:587-594](../../docs/gap-analysis/13c-mcp-servers.md). Upstream:
[server-manager.ts:1055-1095](../../tmp/pi-mcp-adapter/server-manager.ts).

**(a) Split the disabled check out of `begin_request` (F11).** At
[server_manager.rs:2634-2660](../../crates/cyrup-mcp/src/server_manager.rs), remove the
`definition().is_disabled()` test (lines 2639-2644) so `begin_request` carries the connected
precondition alone, and add beside it:

```rust
    /// `if (isServerDisabled(this.connections.get(name)?.definition)) throw …` —
    /// `readResource`'s **first** statement (`server-manager.ts:1079-1081`), and `readResource`'s
    /// alone.
    ///
    /// `getPrompt` does **not** have it (`server-manager.ts:1061-1064`), which is observable: a
    /// server disabled in config after it connected refuses `read_resource` with
    /// [`server_disabled_message`] and `get_prompt` with [`server_not_connected_message`]. MCP-129's
    /// verify line names exactly that pair (13c:1588-1590).
    ///
    /// The definition read is the connection's **snapshot**, not live config —
    /// `this.connections.get(name)?.definition`. 13c's summary line (`:1584-1585`) says "live"; §3.13
    /// (`:591`) quotes the expression and it is the snapshot. The snapshot is correct.
    ///
    /// # Errors
    ///
    /// [`server_disabled_message`] when the connection exists and its snapshot is disabled.
    fn refuse_if_disabled(&self, name: &str) -> McpResult<()> {
        if self
            .get_connection(name)
            .is_some_and(|connection| connection.definition().is_disabled())
        {
            return Err(McpError::Other(server_disabled_message(name)));
        }
        Ok(())
    }
```

Update `begin_request`'s `# Errors` block ([server_manager.rs:2629-2633](../../crates/cyrup-mcp/src/server_manager.rs))
to name only `server_not_connected_message`, and retarget the test at
[server_manager.rs:3330-3345](../../crates/cyrup-mcp/src/server_manager.rs) — whose name
(`begin_request_refuses_a_disabled_or_unconnected_server`) and doc pin the contract this change moves.

**(b) The two verbs**, on `McpServerManager` immediately after `begin_request`:

```rust
    /// `getPrompt(name, promptName, args, signal)` (`server-manager.ts:1055-1076`; §3.13, MCP-129).
    ///
    /// **No disabled re-check** — see [`Self::refuse_if_disabled`].
    ///
    /// `arguments` is `Option<JsonObject>` and rmcp marks it `skip_serializing_if =
    /// "Option::is_none"` (`rmcp-3.1.4/src/model.rs:2331-2332`), so upstream's
    /// `...(args ? { arguments: args } : {})` — the key **omitted**, never `null` — is what `None`
    /// already produces.
    ///
    /// # Errors
    ///
    /// [`server_not_connected_message`] when the record is missing, not `connected`, or owns no
    /// live `Peer`; otherwise the peer's own failure as [`McpError::Server`].
    pub async fn get_prompt(
        self: &Arc<Self>,
        name: &str,
        prompt: &str,
        arguments: Option<JsonObject>,
        options: Option<PeerRequestOptions>,
    ) -> McpResult<GetPromptResult> {
        let (connection, _guard) = self.begin_request(name)?;
        let peer = connection
            .resource()
            .peer()
            .ok_or_else(|| McpError::Other(server_not_connected_message(name)))?
            .clone();
        let params = GetPromptRequestParams {
            name: prompt.to_string(),
            arguments,
            ..GetPromptRequestParams::new(prompt)
        };
        let result = peer
            .send_request_with_option(
                ClientRequest::GetPromptRequest(GetPromptRequest {
                    method: Default::default(),
                    params,
                    extensions: Default::default(),
                }),
                options.unwrap_or_else(PeerRequestOptions::no_options),
            )
            .await
            .map_err(|error| McpError::Server { server: name.to_string(), message: error.to_string() })?
            .await_response()
            .await
            .map_err(|error| McpError::Server { server: name.to_string(), message: error.to_string() })?;
        match result {
            ServerResult::GetPromptResult(result) => Ok(result),
            other => Err(McpError::Server {
                server: name.to_string(),
                message: format!("unexpected response to prompts/get: {other:?}"),
            }),
        }
    }
```

`read_resource` is the same body with `self.refuse_if_disabled(name)?;` as its **first** statement —
§3.13's ordering is that the disabled check precedes the connected check
([13c-mcp-servers.md:591-592](../../docs/gap-analysis/13c-mcp-servers.md)) —
`ReadResourceRequestParams { uri: uri.to_string(), ..ReadResourceRequestParams::new(uri) }` and
`ServerResult::ReadResourceResult`. Factor the duplicated
`map_err(|error| McpError::Server { .. })` into one private `fn peer_failure(name: &str) -> impl
Fn(ServiceError) -> McpError` rather than writing it four times.

Three details that are behaviour, not style:

* **`_guard`, never `_`.** `InFlightGuard`'s `Drop`
  ([server_manager.rs:2694-2701](../../crates/cyrup-mcp/src/server_manager.rs)) is the
  `finally { decrementInFlight; touch }`; binding to `_` drops it immediately and lets the idle sweep
  reap the connection mid-flight.
* **`send_request_with_option`, not `Peer::get_prompt_once`.** The `_once` helpers
  (`rmcp-3.1.4/src/service/client.rs:1409-1450`) take no `PeerRequestOptions`, and §3.13 requires the
  per-call timeout. Record the two things that costs — rmcp's SEP-2322 MRTR loop, which upstream's
  SDK also does not run on these verbs, and `read_resource_once`'s response cache — as named deltas in
  the doc comments.
* **`options` is per call**, from `build_request_options`
  ([runtime.rs:1840-1845](../../crates/cyrup-mcp/src/runtime.rs)) via
  `McpServerManager::request_options` ([server_manager.rs:1440-1455](../../crates/cyrup-mcp/src/server_manager.rs)),
  matching `this.getRequestOptions(name, signal)`. Leave `reset_timeout_on_progress` and
  `max_total_timeout` at their defaults
  ([13c-mcp-servers.md:578-580](../../docs/gap-analysis/13c-mcp-servers.md)).

### 4 · The production handler factory and the completion notice (MCP-122)

**(a) The message**, in `server_manager.rs` beside `forget_url_elicitation`
([server_manager.rs:2599-2606](../../crates/cyrup-mcp/src/server_manager.rs)):

```rust
/// `` `MCP browser interaction for ${serverName} completed. You can retry the tool now.` `` —
/// `server-manager.ts:735`. Byte-exact; it reaches the user through
/// `HostServices::notify(.., NotifyKind::Info)`.
#[must_use]
pub fn url_elicitation_completed_message(server: &str) -> String {
    format!("MCP browser interaction for {server} completed. You can retry the tool now.")
}
```

**(b) The factory**, in [runtime.rs](../../crates/cyrup-mcp/src/runtime.rs) beside
`bare_handler_factory` ([runtime.rs:1939-1952](../../crates/cyrup-mcp/src/runtime.rs)). Upstream is
`createClient` ([server-manager.ts:691-742](../../tmp/pi-mcp-adapter/server-manager.ts)); the
completion handler it registers is `:730-738`.

```rust
/// The manager's own `createClient` (`server-manager.ts:691-742`) — `with_handler_factory`'s
/// production argument, and the seam MCP-118 (sampling / elicitation) and MCP-120 (`list_changed`)
/// fill in. §3.1 of the 13i triage names the absence of this function as the single structural
/// blocker under every unit in `MCP-450`…`MCP-472`.
///
/// The manager is reached through an [`std::sync::OnceLock`] slot holding a [`std::sync::Weak`],
/// because it and its factory are built in one expression and because a strong handle here is a
/// reference cycle: the manager owns the `Arc<dyn ConnectionFactory>` that owns this closure, and a
/// cycle means the manager is never dropped and its children never reaped.
#[must_use]
pub fn manager_handler_factory(
    manager: Arc<OnceLock<Weak<crate::server_manager::McpServerManager>>>,
    ui: Option<Arc<crate::owner::OwnedServices>>,
    elicitation_mode: Option<ElicitationMode>,
) -> HandlerFactory {
    Arc::new(move |server: &str, runtime_signal: &CancelToken| {
        let manager = Arc::clone(&manager);
        let ui = ui.clone();
        McpClientHandler::new(McpClientHandlerParts {
            server: server.to_string(),
            runtime_signal: runtime_signal.clone(),
            elicitation_mode,
            sampling: None,     // MCP-118
            elicitation: None,  // MCP-118
            list_changed: None, // MCP-120
            elicitation_complete: Some(Arc::new(move |event: ElicitationCompleteEvent| {
                let Some(manager) = manager.get().and_then(Weak::upgrade) else { return };
                // `if (!accepted?.delete(notification.params.elicitationId)) return;` — a duplicate
                // completion, or one for an id never accepted, is silent.
                if !manager.forget_url_elicitation(&event.server, &event.elicitation_id) {
                    return;
                }
                if let Some(ui) = ui.as_ref() {
                    cyrup_ext::HostServices::notify(
                        ui.as_ref(),
                        &crate::server_manager::url_elicitation_completed_message(&event.server),
                        cyrup_ext::NotifyKind::Info,
                    );
                }
            })),
        })
    })
}
```

`OwnedServices` is already the stale-generation fence — `notify` is one of its `fenced!` arms
([owner.rs:376](../../crates/cyrup-mcp/src/owner.rs)) — so a notice from a dead generation is inert
rather than painted into the replacement session. The `aborted || !allow_url` gate stays where it is,
at dispatch ([runtime.rs:1620-1625](../../crates/cyrup-mcp/src/runtime.rs)); do not duplicate it here.

**(c) The wiring**, replacing [runtime.rs:193-196](../../crates/cyrup-mcp/src/runtime.rs). Note
`manager` is moved into `McpState::new` at [runtime.rs:255](../../crates/cyrup-mcp/src/runtime.rs) and
`ui` at `:261`, so both the slot fill and the `ui.clone()` must happen here.

```rust
    // Step 6's gate — `settings.elicitation !== false && hasUI`, `allowUrl = mode === "tui"`
    // (`config.rs:1237-1241`, `ContextSnapshot::is_tui_mode`). Passing `None` here would derive
    // `allow_url = false` (`McpClientHandler::new`, runtime.rs:1460-1462) and the completion
    // dispatch would be permanently closed — MCP-122 would ship inert.
    let elicitation_mode = config
        .settings
        .as_ref()
        .map_or(snapshot.has_ui, |settings| settings.elicitation(snapshot.has_ui))
        .then(|| ElicitationMode { allow_url: snapshot.is_tui_mode() });

    // The production credential store (13f). `McpAuthStore` already implements
    // `crate::oauth::McpOAuthStorage` (credentials.rs:3551-3626); nothing constructed one.
    let auth_store = Arc::new(crate::credentials::McpAuthStore::new(
        dirs.clone(),
        auth_storage_options.clone(),
    ));
    let auth_provider: Arc<dyn HttpAuthProvider> =
        Arc::new(crate::oauth::StoredCredentialProvider::new(
            Arc::clone(&auth_store),
            Arc::clone(&oauth_runtime),
        ));

    let manager_slot: Arc<OnceLock<Weak<McpServerManager>>> = Arc::new(OnceLock::new());
    let manager = Arc::new(McpServerManager::with_factory(
        Some(snapshot.cwd.clone()),
        Arc::new(
            ConnectionBuilder::new(Some(snapshot.cwd.clone()))
                .with_handler_factory(manager_handler_factory(
                    Arc::clone(&manager_slot),
                    ui.clone(),
                    elicitation_mode,
                ))
                .with_auth_provider(auth_provider),
        ),
    ));
    // Filled before any connect can run, so `Weak::upgrade` inside the hook never sees `None` in
    // practice; the `None` arm is the hookless degradation, not a state anything reaches.
    let _ = manager_slot.set(Arc::downgrade(&manager));
```

Delete the "Two things the builder does NOT yet get here" paragraph at
[runtime.rs:188-192](../../crates/cyrup-mcp/src/runtime.rs) — both are installed by this block — and
replace it with what is still absent (MCP-118's sampling/elicitation hooks and MCP-120's
`list_changed`, both wired as `None` above).

### 5 · The production `HttpAuthProvider` (MCP-309, hop A)

Spec: [13g-mcp-oauth.md:913-934](../../docs/gap-analysis/13g-mcp-oauth.md), recommendation **(a)**,
already chosen and already implemented on the receiving side
([oauth.rs:2541-2554](../../crates/cyrup-mcp/src/oauth.rs)).

The storage adapter exists (F1). What is missing is the provider, and it belongs in
[oauth.rs](../../crates/cyrup-mcp/src/oauth.rs) because `get_valid_token`
([oauth.rs:3541-3593](../../crates/cyrup-mcp/src/oauth.rs)) is what it wraps:

```rust
/// The production [`crate::runtime::HttpAuthProvider`]: the stored token, refreshed if it can be.
///
/// This is the *only* reason a returning user's HTTP server reaches `connected` instead of
/// `needs-auth`. [`crate::runtime::NoStoredCredentials`] (`runtime.rs:1908-1926`) is
/// upstream-faithful for a **first** login — it reproduces `onRedirect: async () => {}`, which ends
/// at `needs-auth` — and wrong for every login after it (`runtime.rs:1876-1886`).
///
/// [`get_valid_token`] never opens a browser: it reads the store, and refreshes only when the token
/// is expired **and** a refresh token and client record exist. [`AuthenticateOptions::launcher`] is
/// therefore never touched on this path, and its `OpenerLauncher` default is inert here.
pub struct StoredCredentialProvider {
    store: Arc<crate::credentials::McpAuthStore>,
    runtime: Arc<McpOAuthRuntime>,
}

// `McpAuthStore` is `#[derive(Clone)]` only (`credentials.rs:2048-2049`), so the `Debug` bound on
// `HttpAuthProvider` has to be satisfied by hand.
impl std::fmt::Debug for StoredCredentialProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredCredentialProvider").finish_non_exhaustive()
    }
}

impl StoredCredentialProvider {
    /// The store is both halves: it *is* the [`McpOAuthStorage`] the flow reads
    /// (`credentials.rs:3551-3626`) and the cache `invalidate_auth_entry_cache` evicts from.
    #[must_use]
    pub fn new(store: Arc<crate::credentials::McpAuthStore>, runtime: Arc<McpOAuthRuntime>) -> Self {
        Self { store, runtime }
    }

    fn options(&self) -> AuthenticateOptions {
        let mut options =
            AuthenticateOptions::new(Arc::clone(&self.store) as Arc<dyn McpOAuthStorage>);
        options.runtime = Some(Arc::clone(&self.runtime));
        options
    }
}

impl crate::runtime::HttpAuthProvider for StoredCredentialProvider {
    fn authorize<'a>(
        &'a self,
        server: &'a str,
        url: &'a str,
        _challenge: Option<&'a str>,
    ) -> BoxFuture<'a, McpResult<Option<String>>> {
        Box::pin(async move {
            Ok(get_valid_token(server, url, &self.options())
                .await?
                .map(|tokens| tokens.access_token))
        })
    }

    fn invalidate_auth_entry_cache(&self, server: &str) {
        // `invalidateAuthEntryCache(serverName)` — `credentials.rs:2180-2182`. The once-per-episode
        // policy belongs to the ladder (`runtime.rs:2685-2688`), not to this method.
        self.store.invalidate_cache(server);
    }
}
```

**`_challenge` is genuinely unused and the parameter must stay.** The trait doc at
[runtime.rs:1888-1893](../../crates/cyrup-mcp/src/runtime.rs) explains that upstream's provider ignores
it too and that it is carried rather than dropped. The challenge reaches discovery by §6's route, not
this one — **do not** turn the provider into a challenge side-channel, because the `Explicit` arm
reaches `needs-auth` without a second `authorize` call
([runtime.rs:2620-2636](../../crates/cyrup-mcp/src/runtime.rs) calls `authorize(name, url, None)` on
the first attempt and the ladder returns from `NeedsAuth` without looping) and a side-channel would
miss it.

Construction is §4(c)'s block. Note `get_valid_token` returns a store failure as
`McpError::CredentialStore` through the adapter's `?`
([credentials.rs:3554](../../crates/cyrup-mcp/src/credentials.rs)), and `authorize`'s `?` propagates it
into `connect_http_client` at [runtime.rs:2627](../../crates/cyrup-mcp/src/runtime.rs) — a broken
keychain fails the connect loudly instead of silently degrading to `needs-auth`, which is
[errors.rs:217-221](../../crates/cyrup-mcp/src/errors.rs)'s stated requirement.

### 6 · The challenge, carried off the failed connect (MCP-309, hop B)

The challenge **is** extracted today, at
[runtime.rs:2672](../../crates/cyrup-mcp/src/runtime.rs), and then thrown away: the
`UnauthorizedAction::NeedsAuth` arm at
[runtime.rs:2681-2699](../../crates/cyrup-mcp/src/runtime.rs) returns an `HttpConnection` with no field
to carry it, and neither `NewConnection`
([server_manager.rs:1130-1137](../../crates/cyrup-mcp/src/server_manager.rs)) nor `ServerConnection`
([server_manager.rs:787-825](../../crates/cyrup-mcp/src/server_manager.rs)) has one either.

One field, threaded through three records:

* **[runtime.rs:2349-2358](../../crates/cyrup-mcp/src/runtime.rs), `HttpConnection`** —
  `pub challenge: Option<String>`, documented as "*the `WWW-Authenticate` of the 401 that produced this
  `needs-auth`, kept so `/mcp-auth` seeds discovery with it (MCP-309(a))*". Set it in the `NeedsAuth`
  arm from the `challenge` already bound at `:2672`; `None` on the `Connected` arm (`:2644-2650`).
* **[server_manager.rs:1130-1137](../../crates/cyrup-mcp/src/server_manager.rs), `NewConnection`** —
  the same field, defaulted to `None` in Wave 1's `NewConnection::bare`
  ([MCP_DISCOVERY_PAGINATION.md:187-203](MCP_DISCOVERY_PAGINATION.md)). Carry it through
  `create_connection`'s early `needs-auth` return at
  [runtime.rs:2970-2980](../../crates/cyrup-mcp/src/runtime.rs).
* **[server_manager.rs:787-825](../../crates/cyrup-mcp/src/server_manager.rs), `ServerConnection`** —
  store it and expose `pub fn challenge(&self) -> Option<&str>`. It is immutable for the record's life,
  like `definition` and unlike `status`, so a plain `Option<String>` — **no lock**. `ServerConnection::new`
  ([server_manager.rs:829-850](../../crates/cyrup-mcp/src/server_manager.rs)) grows a fifth parameter;
  the seam's construction site is §2(d)'s literal.

Then the reader, on `McpServerManager` beside `auth_storage_options`
([server_manager.rs:1440-1455](../../crates/cyrup-mcp/src/server_manager.rs)):

```rust
    /// MCP-309(a): the `WWW-Authenticate` the last failed connect for `name` came back with, for
    /// [`crate::oauth::AuthenticateOptions::challenge`].
    ///
    /// `None` when the server never 401'd with one, which is exactly when
    /// `resolve_metadata_from_challenge(None)` should fall through to the proactive `.well-known`
    /// walk (`oauth.rs:2541-2554`) — the arm `/mcp-auth` on a disconnected server wants.
    #[must_use]
    pub fn last_auth_challenge(&self, name: &str) -> Option<String> {
        self.get_connection(name)?.challenge().map(str::to_string)
    }
```

That is the seam every future `ProxyEnv` / `McpPanelCallbacks` implementor reads when it builds
`AuthenticateOptions`. `prepare_session`'s parameter is already there
([oauth.rs:2522](../../crates/cyrup-mcp/src/oauth.rs)) and `start_auth` already forwards
`options.challenge.as_deref()` ([oauth.rs:2932](../../crates/cyrup-mcp/src/oauth.rs)); the producer is
what was missing. **Do not build the `ProxyEnv` production impl here** — the only implementor today is
the test `FakeEnv` ([proxy/testsupport.rs:41,91](../../crates/cyrup-mcp/src/proxy/testsupport.rs)),
that impl is 13d's, and it is scheduled by no current task file. Note that in the handoff.

### 7 · The registration POST with the full body (MCP-313)

Spec: [13g-mcp-oauth.md:976-990](../../docs/gap-analysis/13g-mcp-oauth.md); upstream
[mcp-oauth-provider.ts:224-260](../../tmp/pi-mcp-adapter/mcp-oauth-provider.ts).

The mechanism: register **ourselves**, then hand rmcp the result as a *pre-registered* client so
`AuthorizationSession::new` takes branch 1 (`rmcp-3.1.4/src/transport/auth.rs:3359-3367`) and its own
`register_client` (`:1655-1689`) never runs. No double registration, and rmcp's fixed body — which
hardcodes `token_endpoint_auth_method: "none"` (`:1681`) and has no `client_uri`/`logo_uri`
(`:1076-1086`) — never goes out.

All of this lives in `prepare_session`
([oauth.rs:2516-2654](../../crates/cyrup-mcp/src/oauth.rs)), between `manager.set_metadata(metadata)`
at [oauth.rs:2560](../../crates/cyrup-mcp/src/oauth.rs) and `AuthorizationSession::new` at
[oauth.rs:2637](../../crates/cyrup-mcp/src/oauth.rs). Keep `metadata.registration_endpoint`,
`metadata.response_types_supported` and `metadata.code_challenge_methods_supported` in locals **before**
`set_metadata` moves the value.

**(a) The body.**

```rust
/// The RFC 7591 body upstream sends (`mcp-oauth-provider.ts:250-259`), which rmcp's
/// `pub(crate) ClientRegistrationRequest` (`rmcp-3.1.4/src/transport/auth.rs:1076-1086`) cannot
/// express.
///
/// Absent fields are **absent, not null** (13g §14 item 2) — `skip_serializing_if` on every optional.
///
/// **Only the `authorization_code` shape.** Upstream has a second, `client_credentials` shape
/// (`mcp-oauth-provider.ts:234-243`: `redirect_uris: []`, `grant_types: ["client_credentials"]`, no
/// `response_types`, no `scope`). It is unreachable here: [`authenticate_client_credentials`]
/// (`oauth.rs:2664-2692`) never calls `prepare_session` and hard-fails without `oauth.clientId`, so
/// that grant never dynamically registers. Structural, not an omission.
///
/// **`application_type` is rmcp's default, not an upstream field.** Upstream's `clientMetadata` has
/// no such key — the TypeScript SDK injects it — and rmcp sends `DEFAULT_APPLICATION_TYPE = "native"`
/// (`auth.rs:204`, `:1688`). Sending it keeps SEP-837 behaviour byte-identical to what a
/// rmcp-driven registration produces today.
#[derive(serde::Serialize)]
struct ClientRegistrationBody {
    client_name: String,
    redirect_uris: Vec<String>,
    grant_types: Vec<String>,
    response_types: Vec<String>,
    token_endpoint_auth_method: &'static str,
    application_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logo_uri: Option<String>,
}
```

Field derivation, each line of which is MCP-313's behaviour line:

| field | value | why |
|---|---|---|
| `client_name` | `config.client_name.clone().unwrap_or_else(app_name)` | the expression already at [oauth.rs:2613](../../crates/cyrup-mcp/src/oauth.rs); `app_name()` is [oauth.rs:483-491](../../crates/cyrup-mcp/src/oauth.rs) |
| `redirect_uris` | `vec![redirect_uri.to_string()]` | the listener's **actual** bound port ([13g-mcp-oauth.md:965-966](../../docs/gap-analysis/13g-mcp-oauth.md)) |
| `grant_types` | `["authorization_code", "refresh_token"]` | [mcp-oauth-provider.ts:255](../../tmp/pi-mcp-adapter/mcp-oauth-provider.ts) |
| `response_types` | `["code"]` | `:256` |
| `token_endpoint_auth_method` | `if configured_secret.is_some() { "client_secret_post" } else { "none" }` | `:257`; `configured_secret` is in scope at [oauth.rs:2556](../../crates/cyrup-mcp/src/oauth.rs) |
| `application_type` | `"native"` | F5 |
| `scope` | `config.scope.clone()` | `:258` — spread only when defined, so `None` omits |
| `client_uri` | `config.client_uri.clone().or_else(default_client_uri)` | `:226`, `:253` |
| `logo_uri` | `config.logo_uri.clone()` | `:254`; already validated absolute http(s) at [oauth.rs:241-256](../../crates/cyrup-mcp/src/oauth.rs) — this closes §14 item 1 |

`client_uri` is `default_client_uri()`'s **first caller**
([oauth.rs:513-515](../../crates/cyrup-mcp/src/oauth.rs)). Correct its doc while you are there: it
claims "*cyrup declares `APP_CLIENT_URI` so the omit arm is unreachable here*", and §14 item 2 is
precisely about the rebranded case — the omit arm becomes reachable the moment a host overrides the
client URI the way [`set_app_name`](../../crates/cyrup-mcp/src/oauth.rs) already overrides the name.
Leave the `Option` return; do not collapse it to `String`.

**(b) The two guards rmcp will no longer run (F6),** before the POST:

```rust
    // `validate_server_metadata("code")` (`rmcp-3.1.4/src/transport/auth.rs:1622-1653`). rmcp runs
    // it inside `register_client`; taking the pre-registered branch below skips it, so it runs here
    // instead. Same two checks, same order, same errors.
    if let Some(supported) = response_types_supported.as_ref()
        && !supported.iter().any(|value| value == "code")
    {
        return Err(McpError::other(format!(
            "MCP server \"{server_name}\" authorization server does not support response_type=code"
        )));
    }
    match code_challenge_methods_supported.as_ref() {
        Some(methods) if !methods.iter().any(|method| method == "S256") => {
            return Err(McpError::other(format!(
                "MCP server \"{server_name}\" authorization server does not support PKCE S256"
            )));
        }
        None => tracing::warn!(
            "MCP Auth: {server_name}: authorization server metadata omits \
             code_challenge_methods_supported; proceeding with an S256 challenge anyway"
        ),
        Some(_) => {}
    }
```

**(c) The POST.** `reqwest`, `Content-Type: application/json`, deserialize into a local mirroring
rmcp's `ClientRegistrationResponse` (`auth.rs:1088-1098`) **plus** `client_id_issued_at` and
`client_secret_expires_at`, which rmcp drops into `additional_fields`. Treat an empty-string
`client_secret` as no secret at all, matching rmcp
([13g-mcp-oauth.md:969](../../docs/gap-analysis/13g-mcp-oauth.md)). A non-2xx or a body that will not
deserialize is `McpError::other(...)` — **do not** fall back to rmcp's own registration, because that
would send a public-client body for a server the user configured a secret for.

**(d) Hand it to rmcp.** Replace the `TODO(MCP-312)` block at
[oauth.rs:2629-2635](../../crates/cyrup-mcp/src/oauth.rs):

```rust
    // Branch 1 of `AuthorizationSession::new`'s priority order
    // (`rmcp-3.1.4/src/transport/auth.rs:3359-3367`), so rmcp's own `register_client` never runs.
    // `with_client_secret` MUST be paired with `with_preregistered_client`: the unpaired form is an
    // immediate `RegistrationFailed` (`auth.rs:3324-3333`).
    request = request.with_preregistered_client(registered.client_id.clone());
    if let Some(secret) = registered.client_secret.as_deref() {
        request = request.with_client_secret(secret.to_string());
    }
```

**(e) Carry the response out.** Add `registration: Option<StoredClientInfo>` to `PreparedSession`
([oauth.rs:2484-2490](../../crates/cyrup-mcp/src/oauth.rs)) so `start_auth`'s persist at
[oauth.rs:2953-2971](../../crates/cyrup-mcp/src/oauth.rs) writes the **real** `client_secret`,
`client_id_issued_at` and `client_secret_expires_at` instead of three hardcoded `None`s. That is what
finally gives `client_secret_expired` ([oauth.rs:1365-1369](../../crates/cyrup-mcp/src/oauth.rs)) and
`restore_client_configuration`'s expiry guard
([oauth.rs:2346-2350](../../crates/cyrup-mcp/src/oauth.rs)) something to read — today
`client_secret_expires_at` is `None` on every record the port writes, so both are dead code.
Keep `redirect_uris`, `issuer` and `config_pre_registered` exactly as the existing literal sets them.

**(f) Gating.** Run the POST only when **all three** hold: `config.client_id.is_none()` (no
pre-registered client — that branch already returns at
[oauth.rs:2620-2625](../../crates/cyrup-mcp/src/oauth.rs)); no usable stored `client` record was
restored at [oauth.rs:2580-2591](../../crates/cyrup-mcp/src/oauth.rs); and
`registration_endpoint.is_some()`. When the endpoint is absent, leave the request untouched and let
rmcp raise its own `Dynamic client registration not supported` (`auth.rs:1666-1670`) — MCP-312's named
delta stays exactly as it is.

**(g) Delete the two stale sentences** (F12): the `TODO(MCP-312)` block body and the
"`client_uri` / `logo_uri` do not reach the registration body" paragraph in `prepare_session`'s doc at
[oauth.rs:2509-2514](../../crates/cyrup-mcp/src/oauth.rs). Replace the latter with what is now true and
what still is not (rmcp's `ClientRegistrationRequest` remains `pub(crate)`; the port simply no longer
uses it).

---

## The §14 audit

Research, not a deliverable. Audited against the tree today; six of the eight items already hold, and
**items 2 and 3 are exactly what §5, §6 and §7 change**.

| [13g §14](../../docs/gap-analysis/13g-mcp-oauth.md) item (`:749-773`) | status in the code | evidence |
|---|---|---|
| 1 · `oauth.logoUri` undocumented | declared and validated; **reaches the wire after §7** | [oauth.rs:138-139](../../crates/cyrup-mcp/src/oauth.rs), `:241-256` |
| 2 · rebranding defaults | `app_name()` exists and is used; `default_client_uri()` had **no caller** — §7 gives it one | [oauth.rs:483-515](../../crates/cyrup-mcp/src/oauth.rs), [dirs.rs:86-91](../../crates/cyrup-mcp/src/dirs.rs) |
| 3 · discovery order stated backwards | **the code is right** — challenge first, `.well-known` the fallback — but `challenge` was always `None`; §6 makes the primary arm live | [oauth.rs:2551-2554](../../crates/cyrup-mcp/src/oauth.rs) |
| 4 · RFC 9207 absent from the doc | implemented | `expected_issuer` / `requires_issuer`, [oauth.rs:2558-2559](../../crates/cyrup-mcp/src/oauth.rs) |
| 5 · the `19876` example | port-specific; the bind decision is MCP-339(c) | [oauth.rs:695-710](../../crates/cyrup-mcp/src/oauth.rs) |
| 6 · `redirectUri` for `client_credentials` | the string checks still run | [oauth.rs:2664-2692](../../crates/cyrup-mcp/src/oauth.rs) |
| 7 · loopback allowlist is **four** literals | correct, incl. unbracketed `::1` | [oauth.rs:394-405](../../crates/cyrup-mcp/src/oauth.rs) |
| 8 · reserved set is **eight** | correct, incl. `code_challenge_method` | [oauth.rs:2212-2226](../../crates/cyrup-mcp/src/oauth.rs) |

---

## Sequencing

```
Wave 1 (the request seam — MCP_HIGH_SEVERITY_BACKLOG.md §"Wave 1")
   │
   ├─ §3  MCP-129            needs ConnectionResource::peer()
   ├─ §2d MCP-133            rewrites connect_inner's `promise`, which Wave 1 also rewrites
   └─ §6  MCP-309 hop B      widens NewConnection, which Wave 1 also widens

no prerequisite — start now:
   §1 MCP-132 · §2a-c MCP-123 · §4 MCP-122 · §5 MCP-309 hop A · §7 MCP-313
```

One agent, one branch. §1 and §2 are the same obligation split across a module boundary; §5, §6 and §7
all rewrite `prepare_session`'s neighbourhood or the record it reads from; §4 constructs what §5
installs.

## What this task must NOT do

* **No stub authorization server and no acceptance suite.** MCP-347's remaining content
  ([oauth.rs:3970-3975](../../crates/cyrup-mcp/src/oauth.rs)) is a test fixture and the suites that
  ride on it. Out of scope, and it is why the Definition of Done below is source-observable.
* **No prose document.** MCP-341's `docs/guide/reference/mcp-oauth.md` is a separate deliverable whose
  own verify line is "a review checklist"
  ([13g-mcp-oauth.md:1399](../../docs/gap-analysis/13g-mcp-oauth.md)). Leave the `TODO(MCP-341)` at
  [oauth.rs:3795-3797](../../crates/cyrup-mcp/src/oauth.rs) in place.
* **No tracer.** `setTraceConfig`, `wrapTransportWithMcpTrace`, `McpTraceWriter`: `MCP-473`…`MCP-481`,
  in 13i, triaged by [MCP_13I_SCOPING.md](MCP_13I_SCOPING.md). Only the *comment* at
  [runtime.rs:204](../../crates/cyrup-mcp/src/runtime.rs) changes.
* **No `-32042` elicitation-array walker.** `MCP-470`'s remaining half,
  [MCP_13I_SCOPING.md:74](MCP_13I_SCOPING.md).
* **No sampling or `list_changed` hooks.** `MCP-118` / `MCP-120`; §4 leaves their three slots `None`
  and creates the factory they plug into. Both are currently unscheduled by any task file — flag that
  in the handoff.
* **No `probeAuthDiscovery`.** MCP-309 recommendation (a) removes its two call sites; the
  `TODO(MCP-309)` note at [oauth.rs:2544-2550](../../crates/cyrup-mcp/src/oauth.rs) becomes a
  divergence note. MCP-340 is thereby moot
  ([13-cyrup-mcp-STATUS.md:859](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)).
* **No `Cargo.toml` change and no new dependency.** `reqwest` is
  [Cargo.toml:204](../../crates/cyrup-mcp/Cargo.toml), `regex`
  [Cargo.toml:120](../../crates/cyrup-mcp/Cargo.toml), `async-trait` `:99`.
* **No production `ProxyEnv` impl.** 13d's, unscheduled (§6).
* **No fake cleanup producer.** F11.

---

## Definition of Done

Every line below is checkable by reading the tree or by one `grep`. No fixture, no suite, no
benchmark.

**Builds clean.** `cargo check --workspace --all-targets` and
`cargo doc --workspace --no-deps --bins` both exit 0 — the latter matters because
`.cargo/config.toml` sets `--document-private-items` and the workspace denies
`rustdoc::broken_intra_doc_links`. No `unwrap`/`expect`/`panic`/indexing was introduced (all four are
`deny`).

**Probe (MCP-132)**

- [ ] `crates/cyrup-mcp/src/probe.rs` exists; `pub mod probe;` sits between `owner` and `proxy` in
      [lib.rs](../../crates/cyrup-mcp/src/lib.rs); `Cargo.toml` is byte-unchanged.
- [ ] `probe_mcp_endpoint` returns `McpResult<ProbeOutcome>`, and no code path in the module produces
      an empty `classification`.
- [ ] The seven constants match [mcp-probe.ts:1-7](../../tmp/pi-mcp-adapter/mcp-probe.ts) exactly, and
      the six classification strings — event-stream, modern `server/discover`, JSON-RPC envelope, the
      two Bearer strings, and `not_mcp`'s template — are byte-identical to
      [mcp-probe.ts:124,137,138,148,149,168](../../tmp/pi-mcp-adapter/mcp-probe.ts), em-dash included.
- [ ] The three request shapes match the table in §1 (grep the file for `Mcp-Method`,
      `MCP-Protocol-Version`, `pi-mcp-probe`); `LegacySse` is a GET against the same URL and carries
      neither header.
- [ ] `is_sse` reads the whole lowercased header while `response_kind` splits on `;` — both present,
      and the divergence is noted in a comment.
- [ ] No `Authorization`, cookie store, or configured header appears anywhere in the module, and the
      client is built with `reqwest::Client::builder().build()` rather than
      `crate::runtime::build_http_client`.
- [ ] The Cut-1 arm is present, gated on **both** POST statuses being in
      `POST_ENDPOINT_MISMATCH_STATUSES`, and recorded as a port divergence in the module doc.

**Enrichment (MCP-133 + MCP-123)**

- [ ] `McpError::ProbeEnriched { #[source] original: Box<McpError>, classification: String }` exists
      with `#[error("{original} — probe: {classification}")]` — separator space-em-dash-space.
- [ ] `aggregate_head` and `aggregate_children` were **not** touched.
- [ ] Both `rebuild_manager_error` and `rebuild_mcp_error`
      ([server_manager.rs:318-371](../../crates/cyrup-mcp/src/server_manager.rs)) have an explicit
      `ProbeEnriched` arm that recurses on `remaining`. (This is F3; a diff without it is incomplete.)
- [ ] `enrich_http_connection_error` exists with four early-return arms — no `url`, re-resolve failure,
      client-build failure, probe failure — each returning `original` unchanged, and is called from
      `connect_inner`'s `promise` **only** when `definition.url` is `Some`.
- [ ] `McpError::HttpCleanupFailed` / `AbortCleanupFailed` still have no producer outside
      `errors.rs`'s tests, and neither variant was deleted.
- [ ] `connect_http_client`'s doc cites `rmcp-3.1.4/src/service/client.rs:749-831` and states the
      residual is discharged by the wrapper's transparency, not by a producer.

**Manager verbs (MCP-129)**

- [ ] `McpServerManager::{get_prompt, read_resource}` exist, go through
      `ConnectionResource::peer()`, and use `send_request_with_option` + `await_response`.
- [ ] `begin_request` no longer tests `definition().is_disabled()`; `refuse_if_disabled` exists and is
      called from `read_resource` **only**, as its first statement.
- [ ] `get_prompt`'s `arguments` is `Option<JsonObject>` passed straight into
      `GetPromptRequestParams` — no manual key-omission code.
- [ ] Both verbs bind the guard as `_guard`, not `_`.
- [ ] The test at [server_manager.rs:3330-3345](../../crates/cyrup-mcp/src/server_manager.rs) no longer
      pins the disabled contract on `begin_request`.

**URL elicitation (MCP-122)**

- [ ] `url_elicitation_completed_message` exists and is byte-exact against
      [server-manager.ts:735](../../tmp/pi-mcp-adapter/server-manager.ts).
- [ ] `manager_handler_factory` exists, holds the manager as `Weak` behind a `OnceLock`, calls
      `forget_url_elicitation` and notifies **only** when it returned `true`, and routes the notice
      through `OwnedServices` at `NotifyKind::Info`.
- [ ] `initialize_mcp` calls `with_handler_factory` — `grep -n 'with_handler_factory' runtime.rs` now
      shows a non-test caller, and `bare_handler_factory` is no longer what `initialize_mcp` reaches.
- [ ] `elicitation_mode` is derived from `settings.elicitation(has_ui)` and
      `snapshot.is_tui_mode()`, **not** hardcoded `None` (F12).

**OAuth acquisition (MCP-309 + MCP-313)**

- [ ] `credentials.rs` was **not** modified — the storage adapter already existed (F1).
- [ ] `initialize_mcp` constructs `McpAuthStore::new(dirs, auth_storage_options)` and installs a
      `StoredCredentialProvider` through `with_auth_provider`; `NoStoredCredentials` remains only as
      `ConnectionBuilder::new`'s default.
- [ ] `StoredCredentialProvider` has a hand-written `Debug`, ignores `_challenge`, and its
      `invalidate_auth_entry_cache` calls `McpAuthStore::invalidate_cache`.
- [ ] `HttpConnection`, `NewConnection` and `ServerConnection` each carry `challenge`;
      `McpServerManager::last_auth_challenge` exists; the `Connected` arm sets `None`.
- [ ] `ClientRegistrationBody` exists with `skip_serializing_if` on `scope`, `client_uri` and
      `logo_uri`, and `token_endpoint_auth_method` derived from `configured_secret.is_some()`.
- [ ] The two `validate_server_metadata` guards are reproduced before the POST (F6).
- [ ] `prepare_session` gates the POST on all three conditions in §7(f), and the no-endpoint arm still
      reaches rmcp's `Dynamic client registration not supported`.
- [ ] `PreparedSession::registration` exists and `start_auth`'s persist at
      [oauth.rs:2953-2971](../../crates/cyrup-mcp/src/oauth.rs) no longer hardcodes three `None`s.
- [ ] `default_client_uri()` has a caller, and its doc no longer claims the omit arm is unreachable.

**Contract corrections**

- [ ] [runtime.rs:204](../../crates/cyrup-mcp/src/runtime.rs) cites MCP-480, not MCP-133.
- [ ] `grep -rn 'reqwest = { workspace = true }, one line in' crates/cyrup-mcp/src` returns nothing —
      the `TODO(MCP-312)` block and `prepare_session`'s matching doc paragraph are gone.
- [ ] The "Two things the builder does NOT yet get here" paragraph at
      [runtime.rs:188-192](../../crates/cyrup-mcp/src/runtime.rs) is replaced by what is still absent
      (MCP-118, MCP-120).
- [ ] `grep -rn 'MCP-133' crates/cyrup-mcp/src` names only `enrichHttpConnectionError`, never the
      tracer.
