---
stage: new
status: done
updated: 2026-08-22 16:18
---

# One Connect Attempt, End To End: The Probe Ladder, Typed Cleanup, And OAuth's Last Production Hop

## Description

Every unit here is **one HTTP or stdio attempt against one server**, from the request the manager
issues to the diagnosis it produces when that request fails and the credential acquisition that
failure is supposed to trigger. They cross five files because the attempt does, not because the
grouping is loose:

* [server_manager.rs:1759-1766](../../crates/cyrup-mcp/src/server_manager.rs) is a **marked seam**
  in `connect_inner` — upstream's `definition.url ? attempt.catch(enrichHttpConnectionError) :
  attempt` — and the comment there says in so many words that it "is not ported: it needs
  `mcp-probe.ts`'s classifier (MCP-132), which no unit in this crate supplies yet". MCP-132 builds
  the classifier; MCP-133 fills the seam. Neither is useful without the other.
* [runtime.rs:2554-2561](../../crates/cyrup-mcp/src/runtime.rs) is the *other* error arm of that
  same ladder: "**neither `McpError::HttpCleanupFailed` nor `McpError::AbortCleanupFailed` has a
  producer on this path**, and arm 2 is therefore written-but-unreachable. That is MCP-123's
  residual verbatim." Once MCP-133 starts rewrapping connect failures, whether the cleanup class
  survives that rewrap becomes a live correctness question rather than a latent one.
* MCP-309 and MCP-313 are the two remaining gaps on the same attempt's credential half: nothing
  attaches a stored token on the way out, and nothing carries the 401's `WWW-Authenticate` back to
  discovery on the way in.
* MCP-347's stub authorization server is the only way to observe MCP-309 and MCP-313 at all, and
  MCP-341 is the contract those two change. One agent building the stub gets the implementation and
  its proof.
* MCP-129 and MCP-122 are the same connection's remaining verbs and its remaining unprompted
  notice.

**Read [MCP_HIGH_SEVERITY_BACKLOG.md](MCP_HIGH_SEVERITY_BACKLOG.md) first.** None of the nine units
below appears in its nine waves, but its **Wave 1 — the request seam** is this task's hard
prerequisite for four of them, and its filed sibling
[MCP_DISCOVERY_PAGINATION.md](MCP_DISCOVERY_PAGINATION.md) already prescribes the exact seam
signatures this task consumes.

---

## Six findings that change the brief before any code is written

### 1. The `Cargo.toml` ask in the brief and in the code is already satisfied

Both the `TODO(MCP-312)` at [oauth.rs:2629-2636](../../crates/cyrup-mcp/src/oauth.rs) ("That needs
an HTTP client this crate does not depend on yet (`reqwest = { workspace = true }`, one line in
`Cargo.toml`)") and `prepare_session`'s doc at
[oauth.rs:2509-2514](../../crates/cyrup-mcp/src/oauth.rs) are **stale**. `reqwest` is a plain
dependency at [Cargo.toml:191](../../crates/cyrup-mcp/Cargo.toml), and `regex` at
[Cargo.toml:107](../../crates/cyrup-mcp/Cargo.toml). MCP-132 and MCP-313 need **no new
dependency**. Delete both stale sentences as part of the work; do not add a dependency line.

### 2. `runtime.rs:204` names the wrong unit, and `server_manager.rs:1146` already says so

[runtime.rs:204](../../crates/cyrup-mcp/src/runtime.rs) reads "`setTraceConfig` has no counterpart
at all — `mcp-trace.ts` is MCP-133, unported."  That is false.
[server_manager.rs:1145-1147](../../crates/cyrup-mcp/src/server_manager.rs) has it right: "MCP-133
is **not** the trace unit: it is `enrichHttpConnectionError`
([13c-mcp-servers.md:1633](../../docs/gap-analysis/13c-mcp-servers.md)), and its seam is named
inside `McpServerManager::connect_inner`." The tracer is `MCP-473`…`MCP-481`, and `setTraceConfig`
specifically is **MCP-480** ([13-cyrup-mcp-STATUS.md:967](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)),
all of which live in 13i and are already triaged by [MCP_13I_SCOPING.md](MCP_13I_SCOPING.md).
**Fix `runtime.rs:204` to cite MCP-480 and build no tracing here.** A porter who reads that comment
and "implements MCP-133" writes a JSONL tracer and leaves the probe unbuilt.

### 3. `readResource`'s "live definition" is wrong in `13c`'s summary line and right in `§3.13`

[13c-mcp-servers.md:1585-1587](../../docs/gap-analysis/13c-mcp-servers.md) says `readResource`
"re-checks `isServerDisabled` on the **live** definition first". `§3.13`
([13c-mcp-servers.md:591-593](../../docs/gap-analysis/13c-mcp-servers.md)) quotes the upstream
expression, and it is `isServerDisabled(this.connections.get(name)?.definition)` — **the
connection's snapshot**. [server_manager.rs:2630-2632](../../crates/cyrup-mcp/src/server_manager.rs)
already documents and implements the snapshot reading. **The snapshot is correct; the summary line
is the defect.** Do not switch `begin_request` to a live-config read.

### 4. …but `begin_request` applies the disabled check to *every* caller, and upstream does not

[server_manager.rs:2634-2646](../../crates/cyrup-mcp/src/server_manager.rs) tests
`definition().is_disabled()` before the not-connected test, unconditionally. Upstream applies that
test in `readResource` **only**; `getPrompt` has just the `status === "connected"` precondition
(`§3.13`, [13c-mcp-servers.md:588-593](../../docs/gap-analysis/13c-mcp-servers.md)). MCP-129's own
verify line makes the difference explicit: "*assert `read_resource` refuses with the disabled
message while `get_prompt` refuses only on not-connected*"
([13c-mcp-servers.md:1588-1590](../../docs/gap-analysis/13c-mcp-servers.md)). As written, a disabled
server's `get_prompt` returns `MCP server "x" is disabled` where upstream returns
`Server "x" is not connected`. This is a real behavioural divergence and MCP-129 is the unit that
must fix it.

### 5. MCP-123's prescribed mechanism cannot be built, and must not be faked

`13c`'s cyrup note ([13c-mcp-servers.md:1495-1498](../../docs/gap-analysis/13c-mcp-servers.md)) asks
for the once-only cleanup handle as
`Option<futures::future::Shared<BoxFuture<'static, Result<(), Arc<Error>>>>>` on the HTTP attempt
handle. **There is nothing for it to hold.** Verified in the vendored crate,
`rmcp-3.1.4/src/service/client.rs:749-828`: `serve_client_with_ct_inner` binds the transport to a
local and every failure path is a bare `return Err(...)`/`?` — `Transport::close()` is **never
called** on any error path, and `serve_client_with_lifecycle_and_ct` (`:724-747`) only races that
future against the token. `connect_client_bounded`'s own comment at
[runtime.rs:1754-1757](../../crates/cyrup-mcp/src/runtime.rs) states the same thing from this side:
"Dropping the `connect_client` future is what tears the half-built connection down". A drop cannot
report a failure, so `McpError::HttpCleanupFailed` and `McpError::AbortCleanupFailed` have no
producer and cannot acquire one without rmcp changing. **Do not synthesise a fake close, and do not
delete the variants** — [errors.rs:376-406](../../crates/cyrup-mcp/src/errors.rs)'s
`is_cleanup_failure` walk must still recognise them if a nested source ever carries one. What MCP-123
*can* close is stated in its own behaviour line — the cleanup-failure-versus-connect-failure
**distinction** — and §5 below prescribes exactly that: making the distinction survive MCP-133's
rewrap. Note also that the third variant is **not** unproduced: `McpError::SetupFailed` is raised at
[runtime.rs:3019-3022](../../crates/cyrup-mcp/src/runtime.rs).

### 6. "Production" in this crate currently means "reachable from `initialize_mcp`", not "from a session"

[runtime.rs:181-189](../../crates/cyrup-mcp/src/runtime.rs) records it: "*Installing the builder
here does not put it on any shipping path… `initialize_mcp` itself has no non-test caller… The
production entry point that would reach it — `McpExtension::on_session_start` — is still
MCP-008/MCP-011's empty body*", and
[extension.rs:455-470](../../crates/cyrup-mcp/src/extension.rs) confirms `on_session_start` ends
after the generation bump and the previous-owner drain. MCP-008 is Wave 2's. So every "production"
obligation below is discharged at `initialize_mcp`'s wiring block
([runtime.rs:193-215](../../crates/cyrup-mcp/src/runtime.rs)) — the one place that builds the live
manager — and the acceptance criteria are written against that, not against a running session.

---

## Per-unit breakdown

### MCP-132 — MCP endpoint probe (three-strategy ladder) · medium · `extension-owned`

**Row.** [13-cyrup-mcp-STATUS.md:643](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) — `missing`.
**Spec.** [13c-mcp-servers.md:1618-1631](../../docs/gap-analysis/13c-mcp-servers.md) plus `§3.14` in
full ([13c-mcp-servers.md:601-673](../../docs/gap-analysis/13c-mcp-servers.md)).

**Confirmed open.** `grep -rn -i probe crates/cyrup-mcp/src/*.rs` returns only: `config.rs:3968`
and `:4576` (binary probing, unrelated), `lifecycle.rs:1078-1084` (`refreshTools` as a liveness
probe), `oauth.rs:2544-2549` (the `TODO(MCP-309)` note about `probeAuthDiscovery`), `owner.rs:934`
(`ProbeUi`, a test double), `runtime.rs:964-1003` (`SessionIdProbe`, a `StreamableHttpClient`
decorator that records `mcp-session-id`), `server_manager.rs:3496-3549` (`TeardownProbe`, a test
double) and `ui.rs:4744`. There is no `probe` module, no `classify_response`, no
`json_rpc_envelope_info`, no `is_bearer_challenge`, no `MODERN_FALLBACK_STATUSES`. `lib.rs`'s module
list ([lib.rs:130-150](../../crates/cyrup-mcp/src/lib.rs)) has no `probe` entry.

**Unmet obligation.** The whole module: the seven constants, the three request shapes with their
exact headers/bodies, `classify_response`'s five rungs, `json_rpc_envelope_info`,
`is_bearer_challenge`, `response_kind`, `not_mcp`, the two status-set gates, the 5 s per-request
budget, the unauthenticated rule, and the one Cut-1 arm.

### MCP-133 — Probe-enriched HTTP connect failures · medium · `hand-written`

**Row.** [13-cyrup-mcp-STATUS.md:644](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) — `missing`.
**Spec.** [13c-mcp-servers.md:1633-1643](../../docs/gap-analysis/13c-mcp-servers.md); the consumer
paragraph of `§3.14` at [13c-mcp-servers.md:664-668](../../docs/gap-analysis/13c-mcp-servers.md).

**Confirmed open.** The seam comment is still a comment:
[server_manager.rs:1759-1766](../../crates/cyrup-mcp/src/server_manager.rs). The future it must wrap
is the `async move { factory.create(request).await … }` block at
[server_manager.rs:1813-1835](../../crates/cyrup-mcp/src/server_manager.rs). No occurrence of
`" — probe: "` exists anywhere in the crate.

**Unmet obligation.** Wrap the attempt for URL servers only; the exact ` — probe: ` separator
(space, em-dash, space); the original preserved as `cause`; and the swallow-all rule — **any** probe
failure, including a `resolve_server_url` throw on the re-resolve, returns the original error
unchanged.

### MCP-123 — Connect-time abort and once-only transport cleanup · medium · `rmcp`

**Row.** [13-cyrup-mcp-STATUS.md:634](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) — `partial`.
**Spec.** [13c-mcp-servers.md:1485-1503](../../docs/gap-analysis/13c-mcp-servers.md).

**Confirmed open, with a corrected premise (finding 5).** `AbortCleanupFailed` and
`HttpCleanupFailed` are declared at [errors.rs:255](../../crates/cyrup-mcp/src/errors.rs) and
[errors.rs:270](../../crates/cyrup-mcp/src/errors.rs) and constructed nowhere outside
`errors.rs`'s own tests (`:605`, `:622-626`, `:674-675`). `SetupFailed` *is* produced, at
[runtime.rs:3019](../../crates/cyrup-mcp/src/runtime.rs). Arm 2 of the ladder is a comment at
[runtime.rs:2585-2586](../../crates/cyrup-mcp/src/runtime.rs).

**Unmet obligation, restated to what is buildable.** The cleanup-failure-versus-connect-failure
distinction must **survive MCP-133's enrichment**. Today's shape of an enrichment — reformatting a
message into `McpError::Other` — would erase the class that
[errors.rs:376](../../crates/cyrup-mcp/src/errors.rs)'s `is_cleanup_failure` reads, and that
predicate has two live consumers: `close`'s no-connection rethrow and `close_all`'s child filter.
That is the same defect class the ledger already records for `From<&ManagerError>`
([13-cyrup-mcp-STATUS.md:310-320](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)). §5 below makes
enrichment a **transparent wrapper** rather than a flattening one.

### MCP-129 — `getPrompt` / `readResource` accounting and disabled re-check · medium · `rmcp`

**Row.** [13-cyrup-mcp-STATUS.md:640](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) — `missing`.
**Spec.** [13c-mcp-servers.md:1580-1590](../../docs/gap-analysis/13c-mcp-servers.md), `§3.13` at
[13c-mcp-servers.md:583-593](../../docs/gap-analysis/13c-mcp-servers.md).

**Confirmed partially open.** The accounting half **exists**:
`McpServerManager::begin_request` at
[server_manager.rs:2634](../../crates/cyrup-mcp/src/server_manager.rs) does both preconditions,
`touch`, `increment_in_flight` and hands back an `InFlightGuard`
([server_manager.rs:2660-2695](../../crates/cyrup-mcp/src/server_manager.rs)) whose `Drop` is the
`finally { decrementInFlight; touch }`. Its own doc says why the call is missing: "*The call itself
is **not** here: it needs the connection's `Peer`, which the `ConnectionFactory` does not yet
produce*" ([server_manager.rs:2625-2628](../../crates/cyrup-mcp/src/server_manager.rs)). `grep -n
"fn get_prompt\|fn read_resource" crates/cyrup-mcp/src/server_manager.rs` returns nothing;
`read_resource` exists only as the `ProxyEnv` trait method at
[proxy.rs:1478-1484](../../crates/cyrup-mcp/src/proxy.rs).

**Unmet obligation.** The two verbs on the manager, on top of `begin_request`, plus the
`get_prompt`-must-not-see-the-disabled-check correction of finding 4, plus `get_prompt` omitting the
`arguments` key entirely when there are no arguments.

### MCP-122 — URL-elicitation acceptance tracking and completion notice · medium · `hand-written`

**Row.** [13-cyrup-mcp-STATUS.md:633](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) — `partial`,
and its evidence ("*no `acceptedUrlElicitations` registry … no `remember_url_elicitation`*") is
**stale**. [MCP_13I_SCOPING.md:156-162](MCP_13I_SCOPING.md) already published that correction.

**Confirmed open, narrowly.** Built: the registry field
([server_manager.rs:1224](../../crates/cyrup-mcp/src/server_manager.rs)),
`remember_url_elicitation` with the runtime-signal no-op
([server_manager.rs:2582-2597](../../crates/cyrup-mcp/src/server_manager.rs)),
`forget_url_elicitation` returning the `Set.delete` boolean
([server_manager.rs:2601-2607](../../crates/cyrup-mcp/src/server_manager.rs)), the per-server clear
on `close` ([server_manager.rs:2265](../../crates/cyrup-mcp/src/server_manager.rs)) and the wholesale
clear on `close_all` ([server_manager.rs:2470](../../crates/cyrup-mcp/src/server_manager.rs)), and
tests at [server_manager.rs:3877-3900](../../crates/cyrup-mcp/src/server_manager.rs). The decode side
is built too: `ELICITATION_COMPLETE_METHOD`
([runtime.rs:1276](../../crates/cyrup-mcp/src/runtime.rs)) dispatched in
`on_custom_notification` under the combined `aborted || !allow_url` gate
([runtime.rs:1618-1643](../../crates/cyrup-mcp/src/runtime.rs)).

**Unmet obligation.** Two things, and only two. (a) The notice text
`MCP browser interaction for <server> completed. You can retry the tool now.` at info level exists
nowhere in the crate. (b) The only production `HandlerFactory` is `bare_handler_factory`
([runtime.rs:1933-1945](../../crates/cyrup-mcp/src/runtime.rs)), which passes
`elicitation_complete: None` at [runtime.rs:1942](../../crates/cyrup-mcp/src/runtime.rs), and
`ConnectionBuilder::with_handler_factory`
([runtime.rs:2287-2292](../../crates/cyrup-mcp/src/runtime.rs)) has no non-test caller — so the hook
type `ElicitationCompleteHook` ([runtime.rs:1383](../../crates/cyrup-mcp/src/runtime.rs)) has no
producer. **Not in scope:** the `-32042` array walker, which is `MCP-470`'s remaining half and is
scheduled by [MCP_13I_SCOPING.md:74](MCP_13I_SCOPING.md).

### MCP-309 — The discovery trigger: proactive probe or reactive challenge · medium · `hand-written`

**Row.** [13-cyrup-mcp-STATUS.md:828](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) — `partial`.
**Spec.** [13g-mcp-oauth.md:913-932](../../docs/gap-analysis/13g-mcp-oauth.md). Recommendation **(a),
the reactive path**, is already chosen and implemented on the receiving side:
[oauth.rs:2541-2555](../../crates/cyrup-mcp/src/oauth.rs) calls
`manager.resolve_metadata_from_challenge(challenge)` and `AuthenticateOptions::challenge` is the
channel ([oauth.rs:2439-2441](../../crates/cyrup-mcp/src/oauth.rs), threaded to `prepare_session` at
[oauth.rs:2932](../../crates/cyrup-mcp/src/oauth.rs)).

**Confirmed open — and the mechanism is two hops, not one.**

*Hop A, the stored-credential hop.* The only `impl HttpAuthProvider` is `NoStoredCredentials`
([runtime.rs:1901-1918](../../crates/cyrup-mcp/src/runtime.rs)), whose `authorize` is
`Box::pin(async { Ok(None) })`. `with_auth_provider`
([runtime.rs:2296](../../crates/cyrup-mcp/src/runtime.rs)) is called only from this file's tests
(`:4244`, `:4300`, `:4493`, `:4536`, `:4559`, `:4574`, `:4596`, `:4621`). `runtime.rs:190-192` names
the consequence: "*an HTTP server whose credential is already in the store still ends at
`needs-auth`… the wrong one for a returning user*".

*Hop B, the challenge hop.* The challenge **is** extracted, at
[runtime.rs:2666](../../crates/cyrup-mcp/src/runtime.rs)
(`let Some(challenge) = unauthorized_challenge(&error)`), and then **thrown away**: the
`UnauthorizedAction::NeedsAuth` arm at
[runtime.rs:2678-2696](../../crates/cyrup-mcp/src/runtime.rs) returns an `HttpConnection`
([runtime.rs:2344-2351](../../crates/cyrup-mcp/src/runtime.rs)) with no field to carry it, and
`NewConnection` ([server_manager.rs:1130-1137](../../crates/cyrup-mcp/src/server_manager.rs)) and
`ServerConnection` ([server_manager.rs:787-825](../../crates/cyrup-mcp/src/server_manager.rs)) have
none either. The only `impl ProxyEnv` is the test `FakeEnv` at
[proxy.rs:4882](../../crates/cyrup-mcp/src/proxy.rs) (the ledger's `proxy.rs:4500` has drifted), and
`AuthenticateOptions` is constructed nowhere in `crates/cyrup-mcp/src` outside `oauth.rs` itself.

*A third blocker the row does not name.* `McpOAuthStorage`
([oauth.rs:1380-1442](../../crates/cyrup-mcp/src/oauth.rs)) documents "*The production implementation
is `crate::credentials::McpAuthStore`*" — **and that impl does not exist**. `grep -rn "impl
McpOAuthStorage for" crates/cyrup-mcp/src` returns one hit, `InMemoryOAuthStorage` at
[oauth.rs:1467](../../crates/cyrup-mcp/src/oauth.rs), whose own doc calls itself "*the interim
default until section 13f's keyring store is wired in*". Hop A cannot be built without it: a
provider that reads tokens needs an `Arc<dyn McpOAuthStorage>` that is the real keychain.

### MCP-313 — Client metadata and the host-branding defaults · medium · `hand-written`

**Row.** [13-cyrup-mcp-STATUS.md:832](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) — `partial`.
**Spec.** [13g-mcp-oauth.md:976-990](../../docs/gap-analysis/13g-mcp-oauth.md); OA-13 at
[13g-mcp-oauth.md:95](../../docs/gap-analysis/13g-mcp-oauth.md).

**Confirmed open.** The config fields are parsed and validated —
`OAuthFlowConfig::{client_uri, logo_uri}` at
[oauth.rs:137-139](../../crates/cyrup-mcp/src/oauth.rs), the emptiness and absolute-URL checks at
[oauth.rs:234-256](../../crates/cyrup-mcp/src/oauth.rs) — and then **dropped**: the
`AuthorizationRequest` built at
[oauth.rs:2612-2634](../../crates/cyrup-mcp/src/oauth.rs) sets only `with_client_name`,
`with_scopes`, `with_preregistered_client` and `with_client_secret`. `default_client_uri()`
([oauth.rs:513-515](../../crates/cyrup-mcp/src/oauth.rs)) has **no caller**. rmcp's
`ClientRegistrationRequest` is `pub(crate)` and fixed at seven fields
(`rmcp-3.1.4/src/transport/auth.rs:1076-1086`) with `token_endpoint_auth_method: "none"` hardcoded
at `:1674-1688`, so the field genuinely cannot be reached through `register_client`.

A second, quieter consequence of the same gap: the registration record persisted at
[oauth.rs:2955-2970](../../crates/cyrup-mcp/src/oauth.rs) hardcodes `client_secret: None`,
`client_id_issued_at: None` and `client_secret_expires_at: None`, because rmcp never hands the port
the registration response. `client_secret_expired`
([oauth.rs:1365](../../crates/cyrup-mcp/src/oauth.rs)), read by `restore_client_configuration` at
[oauth.rs:2348](../../crates/cyrup-mcp/src/oauth.rs), therefore has nothing to test.

**Unmet obligation.** Perform the RFC 7591 POST from this crate with the full body, then hand the
result to rmcp as a pre-registered client so rmcp's own `register_client` never runs.

### MCP-347 — The executable spec as the acceptance suite · n/a · `hand-written`

**Row.** [13-cyrup-mcp-STATUS.md:866](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) — `partial`.
The in-code `TODO(MCP-347)` is at
[oauth.rs:3970-3975](../../crates/cyrup-mcp/src/oauth.rs) (the ledger's `:4111-4118` has drifted) and
its first sentence is the one that matters: "*all of them need a stub authorization server this
crate does not have yet*".

**Scope here — deliberately narrow.** This task takes MCP-347's **stub authorization server** and
nothing else. It is not a new test suite; it is the fixture without which MCP-309 and MCP-313 are
unobservable, and `crates/cyrup-mcp/` has no `tests/` directory (`ls crates/cyrup-mcp/tests` →
does not exist) and needs none: the template already exists in-crate as `HttpFixture` at
[runtime.rs:3936-4088](../../crates/cyrup-mcp/src/runtime.rs), a raw `tokio::net::TcpListener` loop
with a hand-rolled `read_request`. No `axum`/`hyper`/`wiremock` exists in the workspace
(`grep -n "axum\|hyper\|wiremock" Cargo.toml` → no hits), so the same shape is the only shape.
**Explicitly out of scope:** the rmcp conformance suites (MCP-310/311/317/318/319/320), MCP-328's
five stale-registration variants, MCP-330, MCP-323/331, MCP-324 (Wave 8's).

### MCP-341 — Ship a corrected OAuth document · medium · `hand-written`

**Row.** [13-cyrup-mcp-STATUS.md:860](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md) — `missing`.
In-code `TODO(MCP-341)` at [oauth.rs:3794-3797](../../crates/cyrup-mcp/src/oauth.rs).

**The unit as written cannot be an acceptance criterion, and here is what replaces it.** MCP-341's
deliverable is a prose file (`docs/guide/reference/mcp-oauth.md`, which does not exist — `ls
docs/guide/reference/` → `cli.md environment.md keybindings.md settings.md troubleshooting.md`) and
its own verify line is "*a review checklist*"
([13g-mcp-oauth.md:1400](../../docs/gap-analysis/13g-mcp-oauth.md)). Neither is source-observable.
**What is in scope is the contract half**: the eight `§14` divergences
([13g-mcp-oauth.md:749-775](../../docs/gap-analysis/13g-mcp-oauth.md)) as assertions about the code,
plus the two of them that MCP-309 and MCP-313 change. Audited against the tree today:

| `§14` item | status in the code | evidence |
|---|---|---|
| 1 · `oauth.logoUri` undocumented | declared and validated | [oauth.rs:139](../../crates/cyrup-mcp/src/oauth.rs), `:241-256`, `:317` |
| 2 · rebranding defaults | `app_name()` and `default_client_uri()` exist; the latter has no caller | [oauth.rs:483-515](../../crates/cyrup-mcp/src/oauth.rs), [dirs.rs:88-91](../../crates/cyrup-mcp/src/dirs.rs) |
| 3 · discovery order stated backwards | **the code is right**: challenge first, `.well-known` as the fallback | [oauth.rs:2551-2554](../../crates/cyrup-mcp/src/oauth.rs) — but `challenge` is always `None` (MCP-309) so the primary arm is dead |
| 4 · RFC 9207 absent from the doc | implemented | `expected_issuer` / `requires_issuer` on `PreparedSession`, [oauth.rs:2549-2560](../../crates/cyrup-mcp/src/oauth.rs) |
| 5 · the `19876` example | port-specific; the bind decision is MCP-339(c) | [oauth.rs:701-710](../../crates/cyrup-mcp/src/oauth.rs) |
| 6 · `redirectUri` for `client_credentials` | the string checks still run | [oauth.rs:2665-2690](../../crates/cyrup-mcp/src/oauth.rs) |
| 7 · loopback allowlist is **four** literals | correct, incl. unbracketed `::1` | [oauth.rs:394-405](../../crates/cyrup-mcp/src/oauth.rs) |
| 8 · reserved set is **eight** | correct, incl. `code_challenge_method` | [oauth.rs:2212-2227](../../crates/cyrup-mcp/src/oauth.rs) |

So six of eight already hold. **Items 2 and 3 are exactly what MCP-309 and MCP-313 change**, and
they are this unit's whole remaining source-observable content.

---

## Implementation

Read [MCP_DISCOVERY_PAGINATION.md:136-200](MCP_DISCOVERY_PAGINATION.md) before touching
`server_manager.rs` — it prescribes `ConnectionResource::peer()` and the `NewConnection` reshape
this task builds on top of, and it is Wave 1's, not yours to re-derive.

### 1 · `crate::probe` — the classifier (MCP-132)

New file `crates/cyrup-mcp/src/probe.rs`; add `pub mod probe;` to
[lib.rs:130-150](../../crates/cyrup-mcp/src/lib.rs) in alphabetical position (between `onboarding`
and `proxy`) and one row to the module-map table in the crate doc. No `Cargo.toml` change (finding
1).

```rust
//! `mcp-probe.ts` — the three-strategy endpoint probe (MCP-132, 13c §3.14).
//!
//! Diagnostics only. The probe never selects a transport, never carries credentials, cookies or
//! configured headers, and its sole consumer is
//! [`crate::server_manager::McpServerManager::connect`]'s failure path (MCP-133).

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::Value;

/// `PROBE_TIMEOUT_MS = 5_000`, per request rather than per ladder.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";
const LEGACY_PROTOCOL_VERSION: &str = "2025-06-18";
const JSON_ACCEPT: &str = "application/json, text/event-stream";
const SSE_ACCEPT: &str = "text/event-stream";
const MODERN_FALLBACK_STATUSES: [u16; 6] = [400, 401, 404, 405, 406, 415];
const POST_ENDPOINT_MISMATCH_STATUSES: [u16; 4] = [404, 405, 406, 415];

/// `isBearerChallenge`: `/(?:^|,)\s*Bearer\b/i` over `www-authenticate` (`?? ""`).
static BEARER_CHALLENGE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|,)\s*Bearer\b").ok());

/// What the ladder concluded. `classification` is **user-visible** — it is interpolated into a
/// connect-failure message by MCP-133 — so every string below is byte-exact against
/// `mcp-probe.ts`, including the em-dash in [`not_mcp`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOutcome {
    pub is_mcp: bool,
    pub classification: String,
}
```

Then, in order:

* `enum Strategy { Modern, LegacyPost, LegacySse }` with `allow_json()` returning `true` for the two
  POST arms and `false` for `LegacySse`. All three hit the **same** URL — `LegacySse` is a `GET`
  against the configured endpoint, **not** a `/sse` path.
* `fn response_kind(content_type: Option<&str>, status: u16) -> String`. Content-type parsing is
  `value.split(';').next().map(str::trim).map(str::to_ascii_lowercase)` — **not** a MIME crate, so a
  malformed header behaves exactly as `responseKind` does. `text/html` → `"HTML"`; any other type →
  that type; absent → `"an untyped response"`.
* `fn not_mcp(kind: &str, status: u16) -> String` →
  `format!("endpoint returned {kind} ({status}) — this URL does not appear to speak MCP")`.
* `fn json_rpc_envelope_info(value: &Value) -> Option<Envelope>`: object, non-null,
  `jsonrpc == "2.0"`; `result` present → `Envelope::Result { protocol_version }` where
  `protocol_version` is `result.protocolVersion` **only when `result` is itself an object**;
  `error` present → `Envelope::Error`; else `None`.
* `fn classify_response(...) -> ProbeOutcome` implementing the five rungs verbatim from
  [13c-mcp-servers.md:633-651](../../docs/gap-analysis/13c-mcp-servers.md). Rung 2 is the one that
  is easy to get wrong: the JSON envelope is parsed **only** when
  `strategy.allow_json() || status == 401`, inside a swallow-all `try` — in Rust,
  `serde_json::from_str::<Value>(&body).ok().as_ref().and_then(json_rpc_envelope_info)`.
* `pub async fn probe_mcp_endpoint(client: &reqwest::Client, url: &str) -> ProbeOutcome` — the
  ladder. Each request is `tokio::time::timeout(PROBE_TIMEOUT, …)`; a timeout or transport error on
  a rung falls to the next rung exactly as a non-matching status does, and a timeout on the last
  rung yields `not_mcp` against nothing, i.e. `ProbeOutcome { is_mcp: false, classification:
  "endpoint returned an untyped response (0) — this URL does not appear to speak MCP" }` is **not**
  what to emit — return `ProbeOutcome { is_mcp: false, classification: String::new() }` and let
  MCP-133's empty-classification check swallow the enrichment (§2). Build the client with
  `reqwest::Client::builder().build()`; do **not** reuse `crate::runtime::build_http_client()`,
  whose decompression and header defaults are the transport's contract, not the probe's.
* **The one Cut-1 arm**, and only this one
  ([13c-mcp-servers.md:670-673](../../docs/gap-analysis/13c-mcp-servers.md)): when both POST
  strategies failed with an endpoint-mismatch status and only the GET stream answered, the
  classification is `"endpoint speaks the legacy HTTP+SSE transport, which cyrup does not support"`
  with `is_mcp: false`. Record it in the module doc as a port divergence — it is a new string, not
  an upstream one.

### 2 · The transparent enrichment wrapper (MCP-133 + MCP-123)

Add one variant to `McpError` at [errors.rs:283-287](../../crates/cyrup-mcp/src/errors.rs), just
before `Other`:

```rust
    /// `new Error(`${originalMessage} — probe: ${probe.classification}`, { cause: error })` —
    /// `enrichHttpConnectionError` (13c §3.14's consumer paragraph, MCP-133).
    ///
    /// A **transparent** wrapper, and that is MCP-123's residual: flattening the original into a
    /// string would erase the class [`McpError::is_cleanup_failure`] reads, and `close`'s
    /// no-connection rethrow and `close_all`'s child filter both depend on it. `#[source]` is
    /// upstream's `cause`, so the walk at [`McpError::is_cleanup_failure`] traverses it for free.
    #[error("{original} — probe: {classification}")]
    ProbeEnriched {
        #[source]
        original: Box<McpError>,
        classification: String,
    },
```

`is_cleanup_failure`'s existing `std::error::Error::source(current)` edge at
[errors.rs:401-403](../../crates/cyrup-mcp/src/errors.rs) already carries the class through — no
change to the walk. Extend `aggregate_head`
([errors.rs:319-332](../../crates/cyrup-mcp/src/errors.rs)) with
`McpError::ProbeEnriched { original, .. } => original.aggregate_head()`, so the three upstream
`error.message ===` comparisons still find the head through a wrapped aggregate.

Then fill the seam. Replace the comment at
[server_manager.rs:1759-1766](../../crates/cyrup-mcp/src/server_manager.rs) with the wrapping, built
around the `promise` future at
[server_manager.rs:1813-1835](../../crates/cyrup-mcp/src/server_manager.rs):

```rust
                let promise: ConnectFuture = {
                    let factory = Arc::clone(&self.factory);
                    let request = CreateConnection { /* unchanged */ };
                    let definition_for_record = Arc::clone(&definition);
                    // MCP-133. `definition.url ? attempt.catch(…) : attempt` — URL servers ONLY, so
                    // a stdio failure is never wrapped, and the probe runs only AFTER the failure,
                    // costing nothing on the success path.
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
                        Ok(ServerConnection::new(/* unchanged */))
                    }
                    .boxed()
                    .shared()
                };
```

and, next to `connection_closed_while_connecting`
([server_manager.rs:125-131](../../crates/cyrup-mcp/src/server_manager.rs)):

```rust
/// `enrichHttpConnectionError(definition, serverName, error)` (13c §3.14's consumer paragraph).
///
/// **Swallow-all is the specification, not defensiveness.** A probe must never be able to turn a
/// connect failure into a different failure, so every arm below returns `original` unchanged: a
/// stdio server (no `url`), a URL that only *now* fails to interpolate, a probe that times out, and
/// a probe that answers with an empty classification.
async fn enrich_http_connection_error(url: Option<&str>, original: McpError) -> McpError {
    let Some(url) = url else { return original };
    // `resolveServerUrl(definition)!` — the re-resolve, whose throw is caught by the same catch.
    let env = crate::credentials::process_env();
    let Ok(Some(resolved)) = crate::credentials::resolve_server_url(Some(url), &env) else {
        return original;
    };
    let Ok(client) = reqwest::Client::builder().build() else { return original };
    let outcome = crate::probe::probe_mcp_endpoint(&client, &resolved).await;
    if outcome.classification.is_empty() {
        return original;
    }
    McpError::ProbeEnriched {
        original: Box::new(original),
        classification: outcome.classification,
    }
}
```

`crate::credentials::process_env()` is the production `EnvFn`
([credentials.rs:220](../../crates/cyrup-mcp/src/credentials.rs); the same value
`ConnectionBuilder::new` installs at [runtime.rs:2279](../../crates/cyrup-mcp/src/runtime.rs)). The
manager holds no env of its own and must not grow one for this — the swallow-all rule makes an
injected env unobservable.

Finally, correct the two stale comments: replace the seam comment body with a back-reference to
`enrich_http_connection_error`, and fix [runtime.rs:204](../../crates/cyrup-mcp/src/runtime.rs) to
read `` `setTraceConfig` has no counterpart at all — `mcp-trace.ts` is MCP-480, unported `` (finding
2). Add to `connect_http_client`'s doc at
[runtime.rs:2549-2561](../../crates/cyrup-mcp/src/runtime.rs) the measured reason the once-only
handle is unbuildable (finding 5), citing `rmcp-3.1.4/src/service/client.rs:749-828`, and change
"That is MCP-123's residual verbatim" to say the residual is discharged by `ProbeEnriched`'s
transparency rather than by a producer.

### 3 · The two verbs (MCP-129)

**After Wave 1.** `ConnectionResource::peer()` is Wave 1's addition
([MCP_DISCOVERY_PAGINATION.md:138-160](MCP_DISCOVERY_PAGINATION.md)); this section consumes it.

First fix finding 4. Split the disabled test out of `begin_request`
([server_manager.rs:2634-2646](../../crates/cyrup-mcp/src/server_manager.rs)) into its own
`fn refuse_if_disabled(&self, name: &str) -> McpResult<()>`, leaving `begin_request` with the
connected precondition alone, and call `refuse_if_disabled` from `read_resource` **only**. Update
the doc block at [server_manager.rs:2622-2633](../../crates/cyrup-mcp/src/server_manager.rs) and the
test at [server_manager.rs:3330-3344](../../crates/cyrup-mcp/src/server_manager.rs), which currently
pins the wrong contract for `get_prompt`.

Then, on `McpServerManager`, immediately after `begin_request`:

```rust
    /// `getPrompt(name, promptName, args, signal)` (`server-manager.ts:1057-1075`; §3.13, MCP-129).
    ///
    /// **No disabled re-check** — upstream applies that to `readResource` only, and a `get_prompt`
    /// against a server disabled after it connected fails with `Server "<n>" is not connected`.
    ///
    /// # Errors
    ///
    /// [`server_not_connected_message`], or the peer's own failure.
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
        // `arguments: args ? args : undefined` — the key is OMITTED, not sent as null.
        let params = GetPromptRequestParam { name: prompt.to_string(), arguments };
        peer.send_request_with_option(/* GetPromptRequest, */ params, options)
            .await
            .map_err(|error| McpError::Server { server: name.to_string(), message: error.to_string() })
    }
```

`read_resource` is the same body with `refuse_if_disabled(name)?` as its **first** statement — the
`§3.13` ordering is that the disabled check precedes the connected check
([13c-mcp-servers.md:589-592](../../docs/gap-analysis/13c-mcp-servers.md)) — and
`ReadResourceRequestParam { uri }`. The `_guard` binding is load-bearing: `InFlightGuard`'s `Drop`
([server_manager.rs:2698-2712](../../crates/cyrup-mcp/src/server_manager.rs)) is the second `touch`,
and binding it to `_` instead of `_guard` would drop it immediately and reap the connection
mid-flight. `options` is per call, from `build_request_options`, matching
`this.getRequestOptions(name, signal)`. Leave `reset_timeout_on_progress` and `max_total_timeout` at
their defaults ([13c-mcp-servers.md:576-579](../../docs/gap-analysis/13c-mcp-servers.md)).

### 4 · The production handler factory and the completion notice (MCP-122)

In `server_manager.rs`, next to `forget_url_elicitation`:

```rust
/// `` `MCP browser interaction for ${server} completed. You can retry the tool now.` `` — §3.10.
/// Byte-exact; it reaches the user through `HostServices::notify(.., NotifyKind::Info)`.
#[must_use]
pub fn url_elicitation_completed_message(server: &str) -> String {
    format!("MCP browser interaction for {server} completed. You can retry the tool now.")
}
```

In [runtime.rs](../../crates/cyrup-mcp/src/runtime.rs), beside `bare_handler_factory`:

```rust
/// The manager's own `createClient` — `ConnectionBuilder::with_handler_factory`'s production
/// argument, and the seam MCP-118 (sampling) and MCP-120 (`list_changed`) fill in.
///
/// `Weak` is not a nicety: the manager owns the `Arc<dyn ConnectionFactory>` that owns this
/// closure, so a strong handle here is a reference cycle and the manager is never dropped.
pub fn manager_handler_factory(
    manager: Weak<crate::server_manager::McpServerManager>,
    ui: Option<Arc<crate::owner::OwnedServices>>,
    elicitation_mode: Option<ElicitationMode>,
) -> HandlerFactory {
    Arc::new(move |server: &str, runtime_signal: &CancelToken| {
        let manager = manager.clone();
        let ui = ui.clone();
        McpClientHandler::new(McpClientHandlerParts {
            server: server.to_string(),
            runtime_signal: runtime_signal.clone(),
            elicitation_mode,
            sampling: None,      // MCP-118
            elicitation: None,   // MCP-118
            list_changed: None,  // MCP-120
            elicitation_complete: Some(Arc::new(move |event: ElicitationCompleteEvent| {
                let Some(manager) = manager.upgrade() else { return };
                // "**only if `Set.delete` returned true**" — a duplicate completion is silent.
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

`OwnedServices` is already the stale-generation fence: `notify` is one of its `fenced!` arms
([owner.rs:376](../../crates/cyrup-mcp/src/owner.rs)), so a notice from a dead generation is inert
rather than painted into the replacement session. The `aborted || !allow_url` gate stays where it
is, at dispatch ([runtime.rs:1621-1628](../../crates/cyrup-mcp/src/runtime.rs)); do not duplicate it
here.

Wiring, at [runtime.rs:193-196](../../crates/cyrup-mcp/src/runtime.rs) — the manager and its factory
are built in one expression, so the `Weak` needs a slot the closure can read later:

```rust
    let manager_slot: Arc<OnceLock<Weak<McpServerManager>>> = Arc::new(OnceLock::new());
    let builder = ConnectionBuilder::new(Some(snapshot.cwd.clone()))
        .with_handler_factory(deferred_handler_factory(
            Arc::clone(&manager_slot),
            ui.clone(),
            elicitation_mode,
        ))
        .with_auth_provider(Arc::clone(&auth_provider) as Arc<dyn HttpAuthProvider>);
    let manager = Arc::new(McpServerManager::with_factory(
        Some(snapshot.cwd.clone()),
        Arc::new(builder),
    ));
    let _ = manager_slot.set(Arc::downgrade(&manager));
```

where `deferred_handler_factory` is `manager_handler_factory` reading the slot on each call
(`slot.get().and_then(Weak::upgrade)`), returning a hookless handler until it is set — which cannot
happen in practice, since the slot is filled before any connect. `elicitation_mode` is step 6's gate
([runtime.rs:120-121](../../crates/cyrup-mcp/src/runtime.rs)); pass `None` until MCP-118 lands and
`allow_url` will simply keep the dispatch gate closed.

### 5 · The keychain-backed storage adapter (MCP-309 prerequisite)

In [credentials.rs](../../crates/cyrup-mcp/src/credentials.rs), the impl `McpOAuthStorage`'s doc has
been promising since it was written ([oauth.rs:1377-1379](../../crates/cyrup-mcp/src/oauth.rs)).
Every method has an existing serialized async counterpart, so it is delegation only:

```rust
#[async_trait::async_trait]
impl crate::oauth::McpOAuthStorage for McpAuthStore {
    async fn load(&self, server_name: &str) -> McpResult<Option<AuthEntry>> {
        Ok(self.auth_entry_async(server_name).await?)
    }
    async fn save_credentials(
        &self, server_name: &str, server_url: &str, credentials: Option<StoredCredentials>,
    ) -> McpResult<()> {
        match credentials {
            Some(credentials) => Ok(self
                .update_credentials_async(server_name, credentials, Some(server_url))
                .await?),
            None => Ok(self.clear_credentials_async(server_name).await?),
        }
    }
    // save_client   -> update_client_info_async / clear_client_info_async
    // clear_all     -> remove_auth_entry_async
    // oauth_state   -> auth_entry_async(...).state.map(|state| state.csrf_token)
    // clear_oauth_state -> clear_state_async
    fn base_dir(&self) -> PathBuf { self.auth_base_dir() }
}
```

`AuthStoreError` already has `#[from]` into `McpError::CredentialStore`
([errors.rs:216-222](../../crates/cyrup-mcp/src/errors.rs)), so `?` preserves the class every
consumer of `is_credential_store_failure` depends on. Do **not** override `get_auth_for_url` — the
trait's default at [oauth.rs:1425-1441](../../crates/cyrup-mcp/src/oauth.rs) is the exact-string
URL-binding rule, and `McpAuthStore::auth_for_url` is a second copy of it that would drift.

### 6 · The production `HttpAuthProvider` (MCP-309, hop A)

In [oauth.rs](../../crates/cyrup-mcp/src/oauth.rs) — it belongs here because it is `get_valid_token`
that it wraps, and `runtime.rs` already imports `crate::oauth`:

```rust
/// The production [`crate::runtime::HttpAuthProvider`]: the stored token, refreshed if it can be.
///
/// This is the *only* reason a returning user's HTTP server reaches `connected` instead of
/// `needs-auth` — `NoStoredCredentials` (`runtime.rs:1901-1918`) is upstream-faithful for a first
/// login and wrong for every login after it.
#[derive(Debug)]
pub struct StoredCredentialProvider {
    storage: Arc<dyn McpOAuthStorage>,
    store: Arc<crate::credentials::McpAuthStore>,
    runtime: Arc<McpOAuthRuntime>,
}

impl crate::runtime::HttpAuthProvider for StoredCredentialProvider {
    fn authorize<'a>(
        &'a self, server: &'a str, url: &'a str, _challenge: Option<&'a str>,
    ) -> BoxFuture<'a, McpResult<Option<String>>> {
        Box::pin(async move {
            let mut options = AuthenticateOptions::new(Arc::clone(&self.storage));
            options.runtime = Some(Arc::clone(&self.runtime));
            Ok(get_valid_token(server, url, &options).await?.map(|tokens| tokens.access_token))
        })
    }

    fn invalidate_auth_entry_cache(&self, server: &str) {
        // `invalidateAuthEntryCache(serverName)` — `credentials.rs:2180`. The once-per-episode
        // policy is the ladder's (`runtime.rs:2678-2684`), not this method's.
        self.store.invalidate_cache(server);
    }
}
```

`_challenge` is genuinely unused and the parameter must stay: the trait doc at
[runtime.rs:1882-1888](../../crates/cyrup-mcp/src/runtime.rs) explains that upstream's provider
ignores it too and that it is carried rather than dropped. The challenge reaches discovery by §7's
route, not this one — **do not** turn the provider into a challenge side-channel, because the
`Explicit` arm reaches `needs-auth` without a second `authorize` call
([runtime.rs:2617-2629](../../crates/cyrup-mcp/src/runtime.rs)) and a side-channel would miss it.

Build it in `initialize_mcp` from the values already in scope there —
`AuthStorageOptions` at [runtime.rs:156-162](../../crates/cyrup-mcp/src/runtime.rs), the `dirs`
parameter, and `oauth_runtime` at [runtime.rs:167-170](../../crates/cyrup-mcp/src/runtime.rs) — and
install it with `with_auth_provider` in §4's builder expression. Delete the "Two things the builder
does NOT yet get here" paragraph at
[runtime.rs:188-192](../../crates/cyrup-mcp/src/runtime.rs) once both are installed.

### 7 · The challenge, carried off the failed connect (MCP-309, hop B)

One field, threaded through three records.

* [runtime.rs:2344-2351](../../crates/cyrup-mcp/src/runtime.rs), `HttpConnection`:
  `pub challenge: Option<String>` — "*the `WWW-Authenticate` of the 401 that produced this
  `needs-auth`, kept so `/mcp-auth` seeds discovery with it (MCP-309(a))*". Set it in the
  `UnauthorizedAction::NeedsAuth` arm at
  [runtime.rs:2678-2696](../../crates/cyrup-mcp/src/runtime.rs) from the `challenge` already bound
  at [runtime.rs:2666](../../crates/cyrup-mcp/src/runtime.rs); `None` on the `Connected` arm at
  `:2637-2643`.
* [server_manager.rs:1129-1137](../../crates/cyrup-mcp/src/server_manager.rs), `NewConnection`: the
  same field, defaulted to `None` in Wave 1's `NewConnection::bare`
  ([MCP_DISCOVERY_PAGINATION.md:186-199](MCP_DISCOVERY_PAGINATION.md)). Carry it through
  `create_connection`'s early `needs-auth` return at
  [runtime.rs:2965-2973](../../crates/cyrup-mcp/src/runtime.rs).
* [server_manager.rs:787-825](../../crates/cyrup-mcp/src/server_manager.rs), `ServerConnection`:
  store it and expose `pub fn challenge(&self) -> Option<&str>`. It is immutable for the record's
  life, like `definition` and unlike `status`, so a plain `Option<String>` — no lock.

Then use it. `prepare_session`'s parameter is already there
([oauth.rs:2522](../../crates/cyrup-mcp/src/oauth.rs)) and `start_auth` already forwards
`options.challenge` ([oauth.rs:2932](../../crates/cyrup-mcp/src/oauth.rs)); what is missing is a
producer. Add to `McpServerManager`, beside `auth_storage_options`
([server_manager.rs:1405-1410](../../crates/cyrup-mcp/src/server_manager.rs)):

```rust
    /// MCP-309(a): the challenge the last failed connect for `name` came back with, for
    /// [`crate::oauth::AuthenticateOptions::challenge`]. `None` when the server never 401'd with a
    /// `WWW-Authenticate`, which is exactly when `resolve_metadata_from_challenge(None)` should
    /// fall through to the proactive `.well-known` walk.
    #[must_use]
    pub fn last_auth_challenge(&self, name: &str) -> Option<String> {
        self.get_connection(name)?.challenge().map(str::to_string)
    }
```

That is the seam every future `ProxyEnv`/`McpPanelCallbacks` implementor reads when it builds
`AuthenticateOptions`. Do not build those implementors here — the `ProxyEnv` production impl is
13d's and is not scheduled by any current task; note that in the handoff.

### 8 · The registration POST with the full body (MCP-313)

In `prepare_session` ([oauth.rs:2516-2654](../../crates/cyrup-mcp/src/oauth.rs)), between
`manager.set_metadata(metadata)` at `:2559` and `AuthorizationSession::new` at `:2638`. Keep the
`metadata.registration_endpoint` in a local before `set_metadata` moves the value.

The mechanism is: register **ourselves**, then hand rmcp the result as a *pre-registered* client, so
rmcp's priority order takes branch 1 (`rmcp-3.1.4/src/transport/auth.rs:3359-3367`) and its own
`register_client` (`:1654-1688`) never runs. No double registration, and the fixed body never goes
out.

```rust
/// The RFC 7591 body upstream sends (`mcp-oauth-provider.ts` `clientMetadata`), which rmcp's
/// `pub(crate) ClientRegistrationRequest` (`rmcp-3.1.4/src/transport/auth.rs:1076-1086`) cannot
/// express: it hardcodes `token_endpoint_auth_method: "none"` and has no `client_uri`/`logo_uri`.
///
/// Absent fields are **absent, not null** — `skip_serializing_if` on every optional, per
/// 13g §14 item 2 and MCP-313's behaviour line.
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

Field derivation, each of which is MCP-313's behaviour line:

* `client_name` — `config.client_name.clone().unwrap_or_else(app_name)`, the same expression already
  at [oauth.rs:2611](../../crates/cyrup-mcp/src/oauth.rs).
* `client_uri` — `config.client_uri.clone().or_else(default_client_uri)`. This is
  `default_client_uri()`'s **first caller** ([oauth.rs:513](../../crates/cyrup-mcp/src/oauth.rs));
  its doc already records that cyrup declares `APP_CLIENT_URI`
  ([dirs.rs:90-91](../../crates/cyrup-mcp/src/dirs.rs)) so the omit arm is unreachable here. Correct
  that doc: with a `set_app_name`-style override for the client URI it *is* reachable, and `§14`
  item 2 is precisely about the rebranded case. Leave the omit arm expressible.
* `logo_uri` — `config.logo_uri.clone()`, already validated as absolute http(s) at
  [oauth.rs:241-256](../../crates/cyrup-mcp/src/oauth.rs). This closes `§14` item 1.
* `token_endpoint_auth_method` — `if configured_secret.is_some() { "client_secret_post" } else
  { "none" }`. `configured_secret` is in scope at
  [oauth.rs:2556](../../crates/cyrup-mcp/src/oauth.rs).
* `grant_types` / `response_types` — `["authorization_code", "refresh_token"]` and `["code"]`,
  matching rmcp's own body so nothing else on the wire changes.
* `application_type` — `"native"`, rmcp's `DEFAULT_APPLICATION_TYPE`, so SEP-837 behaviour is
  unchanged.

POST it with `reqwest` (already a dependency, finding 1),
`Content-Type: application/json`, deserialize into a local mirroring rmcp's
`ClientRegistrationResponse` (`auth.rs:1088-1098`) **plus** `client_id_issued_at` and
`client_secret_expires_at`, then:

```rust
    // Hand rmcp a pre-registered client so `AuthorizationSession::new` takes branch 1
    // (`rmcp-3.1.4/src/transport/auth.rs:3359-3367`) and never sends its own fixed body.
    request = request.with_preregistered_client(registered.client_id.clone());
    if let Some(secret) = registered.client_secret.as_deref() {
        request = request.with_client_secret(secret.to_string());
    }
```

and carry the response out on `PreparedSession`
([oauth.rs:2549-2560](../../crates/cyrup-mcp/src/oauth.rs)) as
`registration: Option<StoredClientInfo>`, so `start_auth`'s persist at
[oauth.rs:2952-2971](../../crates/cyrup-mcp/src/oauth.rs) writes the **real** `client_secret`,
`client_id_issued_at` and `client_secret_expires_at` instead of three hardcoded `None`s. That is
what gives `client_secret_expired` ([oauth.rs:1365](../../crates/cyrup-mcp/src/oauth.rs)) and
`restore_client_configuration`'s expiry guard
([oauth.rs:2348-2350](../../crates/cyrup-mcp/src/oauth.rs)) something to read.

**Gating.** Run the POST only when `config.client_id.is_none()` (no pre-registered client), no
usable stored `client` record was restored at
[oauth.rs:2586-2600](../../crates/cyrup-mcp/src/oauth.rs), and
`metadata.registration_endpoint.is_some()`. When the endpoint is absent, leave the request untouched
and let rmcp raise its own `Dynamic client registration not supported`
(`auth.rs:1665-1669`) — MCP-312's named delta stays exactly as it is. A failed POST returns
`McpError::other(...)`; do **not** silently fall back to rmcp's body, because that would send a
public-client registration for a server the user configured a secret for.

Delete the `TODO(MCP-312)` block at
[oauth.rs:2629-2636](../../crates/cyrup-mcp/src/oauth.rs) and the matching paragraph in
`prepare_session`'s doc at [oauth.rs:2509-2514](../../crates/cyrup-mcp/src/oauth.rs).

### 9 · The stub authorization server (MCP-347, scoped)

A `#[cfg(test)] mod` fixture in [oauth.rs](../../crates/cyrup-mcp/src/oauth.rs), modelled on
`HttpFixture` at [runtime.rs:3936-4088](../../crates/cyrup-mcp/src/runtime.rs) — reuse its
`read_request` shape verbatim; it already handles chunk-free `Content-Length` bodies and returns a
`(method, headers, body)` record, which is exactly what the assertions need.

`StubAuthServer::start()` binds `127.0.0.1:0` and answers, on one accept loop, by request target:

| target | response |
|---|---|
| `/.well-known/oauth-protected-resource…` | `200` RFC 9728 metadata pointing at this same origin |
| `/.well-known/oauth-authorization-server…` | `200` RFC 8414 metadata with `issuer` echoing the origin, `authorization_endpoint`, `token_endpoint`, `registration_endpoint`, `code_challenge_methods_supported: ["S256"]` |
| `/register` | `201` with `client_id`, and `client_secret` + `client_secret_expires_at` when the request body's `token_endpoint_auth_method` was `client_secret_post` |
| `/mcp` | `401` with `WWW-Authenticate: Bearer resource_metadata="<origin>/.well-known/oauth-protected-resource"` while no `Authorization: Bearer <issued>` is present, and the ordinary `initialize` result once it is |

`code_challenge_methods_supported` is not optional: rmcp's `validate_server_metadata` warns without
it and hard-fails when it is present and lacks `S256`
(`rmcp-3.1.4/src/transport/auth.rs:1636-1650`). Record every request in an
`Arc<Mutex<Vec<Recorded>>>`; the registration body is read out of that log, not asserted on the
wire twice.

**A knob the stub must have**, because it is the only thing that distinguishes MCP-309's two arms:
`with_well_known(false)`, which 404s both `.well-known` paths. A server that publishes
`resource_metadata` *only* on the 401 is exactly the case MCP-309's behaviour line names, and it is
the case that fails today and passes after §7.

---

## Sequencing

```
Wave 1 (the request seam, MCP_HIGH_SEVERITY_BACKLOG.md §"Wave 1")
   │
   ├─ §3  MCP-129            (needs ConnectionResource::peer)
   ├─ §2  MCP-133 + MCP-123  (writes connect_inner's promise, which Wave 1 also writes)
   └─ §7  MCP-309 hop B      (writes NewConnection, which Wave 1 reshapes)

start now, no prerequisite:
   §1 MCP-132 · §4 MCP-122 · §5 storage adapter · §6 MCP-309 hop A · §8 MCP-313 · §9 MCP-347
```

One agent, one branch. §1 and §2 are the same obligation split across a module boundary; §5, §6, §7
and §8 all rewrite `prepare_session`'s neighbourhood or the record it reads from.

## What this task must NOT do

* **No tracer.** `setTraceConfig`, `wrapTransportWithMcpTrace`, `McpTraceWriter`: MCP-473…MCP-481, in
  13i, triaged by [MCP_13I_SCOPING.md](MCP_13I_SCOPING.md).
* **No `-32042` elicitation-array walker.** `MCP-470`'s remaining half,
  [MCP_13I_SCOPING.md:74](MCP_13I_SCOPING.md).
* **No sampling or `list_changed` hooks.** `MCP-118`/`MCP-120`; §4 leaves their three slots `None`
  and creates the factory they plug into. Both are currently unscheduled by any task file — flag
  that in the handoff.
* **No `probeAuthDiscovery`.** MCP-309 recommendation (a) removes its two call sites
  ([oauth.rs:2544-2550](../../crates/cyrup-mcp/src/oauth.rs)); MCP-340 is then moot
  ([13-cyrup-mcp-STATUS.md:859](../../docs/gap-analysis/13-cyrup-mcp-STATUS.md)).
* **No prose document.** MCP-341's `docs/guide/reference/mcp-oauth.md` is a separate deliverable;
  this task discharges its two source-observable items only.
* **No `tests/` directory and no new dev-dependency.** The in-crate `#[cfg(test)]` loopback pattern
  is the established one and the only one the workspace supports.

---

## Acceptance Criteria

Probe and enrichment

- [ ] `crates/cyrup-mcp/src/probe.rs` exists, is declared in [lib.rs](../../crates/cyrup-mcp/src/lib.rs), and `Cargo.toml` is unchanged.
- [ ] An SSE `200` classifies as `endpoint responded with an MCP event stream` with `is_mcp` true, on the first rung.
- [ ] A `200` whose `result.protocolVersion` is `"2025-06-18"` yields `unsupported-modern` and the ladder **continues** to `legacy-post`.
- [ ] A `200` whose `result.protocolVersion` is `"2026-07-28"` yields `endpoint supports stateless MCP 2026-07-28 server/discover` and stops.
- [ ] A `401` carrying a `Bearer` challenge and a JSON-RPC error classifies as `is_mcp: true` with the modern or legacy Bearer string per rung; a `401` with no `Bearer` token in `www-authenticate` does not.
- [ ] An HTML `404` yields exactly `endpoint returned HTML (404) — this URL does not appear to speak MCP`; a `500` with no content-type yields `an untyped response (500)` in the same template.
- [ ] A modern rung answering a status outside `{400,401,404,405,406,415}` short-circuits to `not_mcp` **without** issuing `legacy-post`, asserted on the fixture's request log.
- [ ] A GET-only SSE endpoint that 405s both POSTs yields the Cut-1 string `endpoint speaks the legacy HTTP+SSE transport, which cyrup does not support`.
- [ ] No probe request carries `Authorization`, a cookie, or any configured header, asserted on the fixture's recorded headers.
- [ ] A URL server's connect failure against an HTML endpoint surfaces `<original> — probe: endpoint returned HTML (404) — this URL does not appear to speak MCP`, with the separator exactly space-em-dash-space.
- [ ] `std::error::Error::source()` of that error downcasts to the original `McpError`.
- [ ] A stdio server's connect failure is byte-identical before and after this change.
- [ ] A probe that times out, and a URL that no longer interpolates, both yield the bare original message.
- [ ] `McpError::SetupFailed(..)` wrapped by the enrichment still answers `true` to `is_cleanup_failure()` and still returns `Some("MCP connection setup failed")` from `aggregate_head()`.
- [ ] `close_all` still surfaces a wrapped teardown failure and still swallows a wrapped ordinary connect failure.

Manager verbs

- [ ] `McpServerManager::{get_prompt, read_resource}` exist and issue through `ConnectionResource::peer()`.
- [ ] A server disabled in config after connecting: `read_resource` fails with `MCP server "<n>" is disabled`, and `get_prompt` fails with `Server "<n>" is not connected`.
- [ ] `get_prompt` with no arguments sends no `arguments` key at all, asserted on the fixture's recorded body.
- [ ] `in_flight` returns to 0 and `last_used_at` advances after both a successful and a failing call on each verb.

URL elicitation

- [ ] Two identical `notifications/elicitation/complete` for one id produce exactly one `notify` call; a completion for an id never remembered produces none.
- [ ] The notice text is `MCP browser interaction for <server> completed. You can retry the tool now.` at `NotifyKind::Info`.
- [ ] The hook is installed by `initialize_mcp`'s builder, not only by a test: `bare_handler_factory` is no longer what `initialize_mcp` reaches.
- [ ] A completion arriving after the runtime signal fires produces no notify.

OAuth acquisition

- [ ] `impl McpOAuthStorage for McpAuthStore` exists and a store failure inside it arrives as `McpError::CredentialStore`, recognised by `is_credential_store_failure`.
- [ ] `initialize_mcp` installs a `HttpAuthProvider` that is not `NoStoredCredentials`; a server with a live stored token reaches `Connected` on the first attempt with exactly one `Authorization` header on the wire.
- [ ] A `401` carrying `WWW-Authenticate` leaves `manager.last_auth_challenge(name)` equal to that header value; a `401` without one leaves it `None`.
- [ ] Against the stub with `with_well_known(false)`, an authorization seeded from that challenge completes; the same flow with `challenge: None` fails discovery. Ablate by passing `None` and confirm the failure returns.
- [ ] The registration body recorded at `/register` contains `client_uri`, contains `logo_uri` when configured, omits each key entirely when it is not, and carries `token_endpoint_auth_method: "client_secret_post"` when a secret is configured and `"none"` when it is not.
- [ ] Exactly one `/register` request is recorded per fresh authorization — rmcp's own registration never fires.
- [ ] The persisted `StoredClientInfo` carries the stub's `client_secret`, `client_id_issued_at` and `client_secret_expires_at`, and a record whose `client_secret_expires_at` is in the past is not re-applied by `restore_client_configuration`.
- [ ] A server with no `registration_endpoint` still produces rmcp's named `Dynamic client registration not supported`, unchanged.

Contract corrections

- [ ] [runtime.rs:204](../../crates/cyrup-mcp/src/runtime.rs) cites MCP-480, not MCP-133.
- [ ] The `TODO(MCP-312)` block and `prepare_session`'s "needs an HTTP client this crate does not depend on yet" sentence are gone.
- [ ] `connect_http_client`'s doc states, with the `rmcp-3.1.4/src/service/client.rs:749-828` citation, that `AbortCleanupFailed`/`HttpCleanupFailed` have no producer by construction, and no fake producer was added.
- [ ] `default_client_uri()` has a caller, and its doc no longer claims the omit arm is unreachable.
- [ ] `§14` items 2 and 3 are true of the code: the rebranded default reaches the registration body, and the challenge-first discovery arm is live rather than dead.
