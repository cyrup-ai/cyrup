# 13 · cyrup-mcp — implementation status against the port plan

> Part of **[13 — cyrup-mcp](13-cyrup-mcp.md)**, whose canonical table names every port unit. That
> table records what the port must BUILD; it has never recorded what is BUILT. This file is that
> second axis, and nothing else: one row per unit, what state it is actually in, and — where it is
> not done — the specific obligation that is unmet.

## Provenance

**Audited 2026-08-21** against upstream `pi-mcp-adapter` at tag **v2.26.1** (`fafae21`), the tag the
2026-08-20 retarget adopted — not the package's drifted HEAD. Upstream at that tag is 59 production
`.ts` files / 22,312 lines, checked out from `github.com/nicobailon/pi-mcp-adapter` (note: NOT under
the `earendil-works` org that hosts pi itself). cyrup side is `crates/cyrup-mcp`, 19 modules /
45,156 lines — a figure that includes the inline `#[cfg(test)]` modules and so overstates production
code substantially.

**Method, stated plainly so the numbers can be discounted correctly.** Nine independent readers, one
per section file, each holding that section's prose spec, the Rust, and the upstream TypeScript, and
ruling on every unit in its section. Every negative ruling — every `missing` and every `partial` —
was then handed to a second, adversarial reader whose instruction was to REFUTE it by finding the
implementation, on the assumption that the first reader had grepped TypeScript spellings against
Rust that names things differently. Where the two disagreed the skeptic won. That pass overturned
**15 rulings** — the honest measure of the first pass's false-positive rate, and a reason to treat
any single row below as a lead rather than a verdict.

**What this is not.** No row here was verified by building or running anything; every ruling is a
reading of source. `cut` and `open-decision` units are reported `not-applicable` and are NOT work.

## Update — 2026-08-21, wave 1 (MCP-141 / 142 / 146 / 370)

The census below is **as of the audit** and is not rewritten by later work; this section records what
has moved since, so the two are never in conflict.

All four units concerned one file — `crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs`, the
in-tree READER of the metadata cache, which was never upgraded when `cyrup-mcp`'s writer was. Landed:
the identity pre-image is now the writer's key set in the same *resolved* forms, `stable_stringify`
renders an absent field as the bare `undefined` (a three-state `HashValue` mirroring the writer's,
replacing `serde_json::Value`), resource tools are `read_` not `get_`, `ToolPrefix` carries upstream's
four modes, and the server prefix preserves hyphens. `cyrup-mcp` was added to that crate's
**`[dev-dependencies]` only**, so the conformance tests assert against the writer itself rather than
against constants copied out of it — drift is now impossible rather than merely detectable.

| unit | was | now | what remains |
|---|---|---|---|
| `MCP-142` | partial | **implemented** | — |
| `MCP-146` | partial | **implemented** | — |
| `MCP-141` | partial | **partial** | `socket`, the vectors and the `lenient` cluster are CLOSED (below); `ResolvedIdentity::resolve` is the writer's production constructor |
| `MCP-370` | partial | **partial** | `includeTools` and glob `excludeTools` still unported in the reader, so it over-approximates what the adapter registers |

### The four hashing divergences are CLOSED (later wave)

Every cyrup digest now equals the digest a stock `pi-mcp-adapter` @ `v2.26.1` (`fafae21`) computes
for the same definition, measured by running upstream's own `stableStringify` + `computeServerHash`
on node 22 rather than by reasoning about them. This was free to do only because
`cyrup_mcp::dirs::save_metadata_cache` still has **no production call site** — its five callers are
all tests — so no deployed digest had to be invalidated.

1. **`socket` — the missing 15th key. CLOSED.** Upstream's `computeServerHash` builds a **15**-key
   identity whose third key is `socket` (`metadata-cache.ts:89`), and its `stableStringify` walks
   `Object.keys()`, so an absent socket is still emitted as `"socket":undefined`. Neither Rust side
   emitted the key, so *every* cyrup digest differed from pi's by exactly that member. Both now emit
   `"socket": undefined` unconditionally, which is complete as well as correct: `to_server_entries`
   rejects any entry that configures a socket (MCP-054), so the value can only ever be
   `resolveConfigPath(undefined)`. Upstream's digest for the stdio fixture is
   `2190558e470a75c0f992989bd1799b374e669deecb8093e4118a1a9419068cf4`; cyrup produced `4dd46c1f…`
   and now produces upstream's, pinned by
   `the_socket_key_is_no_longer_a_divergence_from_upstream`. 13c-mcp-servers.md:1753 ("Keep `socket`
   … in the pre-image despite Cut 3") was right and is now satisfied.

2. **The `lenient` cluster — three divergences, one root cause. CLOSED.** `config.rs` read `auth`,
   `protocolVersion`, `env` and `headers` behind `deserialize_with = "lenient"`, which silently drops
   any value the Rust type rejects. Upstream hashes all four verbatim and validates none of them at
   load. `AuthMode::Other` and `ProtocolVersionSetting::Other` now carry the raw value into the
   pre-image, and `cyrup_mcp::config::StringRecord` carries `env`/`headers` (and
   `requestHeadersCommand.env`) with their **throw** — upstream's `interpolateEnvRecord` raises a
   TypeError on a non-string member and `isServerCacheValid` catches it to `false`, which the reader
   always reproduced and the writer did not: it dropped the map, hashed `"env":undefined`, and called
   the entry VALID. That one was the two crates giving *opposite* answers, not merely different
   digests. **Connect-time validation is unchanged in kind and now actually happens**:
   `Invalid MCP protocolVersion` is raised by `runtime::version_negotiation` — which is where
   `resolveVersionNegotiation` raises it — and never by the deserialiser.

3. **A fifth divergence, found by measuring the four.** `mcp_direct_tools`'s `ServerEntry` holds
   `auth` and `protocolVersion` as `Option<Value>`, and serde's derived `Option<T>` reads a JSON
   `null` as `None` — so `auth: null` hashed as `"auth":undefined` where upstream hashes
   `"auth":null` (`d5e9d0fe71ad5cc5d6a82b93d537f69ee59809f7f10e1f5c1f26c1d0a97e28e4`, node 22).
   Here the **writer** was the correct side. A `present_or_absent` deserialiser closes it. It
   surfaced only because the new differential table asserts both implementations against
   *upstream's* digest rather than against each other.

4. **The golden vectors are upstream-faithful.** Every constant in `cyrup_mcp::dirs` and in
   `mcp_direct_tools` was regenerated by running upstream's own functions on node 22 and now
   **includes `socket`**, so the word "golden" no longer carries a caveat. Each reconstruction of the
   identity literal was proved faithful by asserting
   `sha256(preImage) === computeServerHash(definition)` against upstream's exported function on the
   same run.

**What wave 1 left open and is still open.**

* **The reader resolves; so does the writer now** (`ResolvedIdentity::resolve` replaced
  `::verbatim` at every production call site), so this one is closed too. `::verbatim` survives only
  as the fixture constructor, and its doc comment says what it cannot express.
* **MCP-370's filtering half** — `includeTools` and glob `excludeTools` are hashed by both sides but
  still not *applied* by the reader, so it over-approximates what the adapter registers.

Verified: `cargo nextest run --workspace` 7697/7698 (the one failure is the pre-existing
`cyrup-modes rpc_cycle_model_spans_the_full_auth_filtered_registry`, unrelated and documented
elsewhere), `cargo clippy --workspace --all-targets` exit 0 with no new warnings in the changed file.

## Update — 2026-08-22, wave 5 (the transport and connection units)

`createConnection` has a body. `McpServerManager`'s [`ConnectionFactory`] seam — the one wave 4 left
filled with `UnbuiltConnectionFactory` — is now `cyrup_mcp::runtime::ConnectionBuilder`, and
`initialize_mcp` is where it is installed (`runtime.rs:170-186`), so a configured server connects
for real against a real child and a real HTTP server for the first time. Landed in `runtime.rs`,
`errors.rs`, `request_headers_command.rs` and (for MCP-105) `cyrup-ext`'s `npx_resolver.rs`.

**How far that reaches, stated exactly.** `initialize_mcp` has no non-test caller:
`grep -rn 'initialize_mcp(' --include=*.rs` over the repo returns its definition
(`runtime.rs:125`) and one call, `runtime.rs:403`, which is inside the `#[cfg(test)]` module opening
at `runtime.rs:325`. The production entry point that would reach it,
`McpExtension::on_session_start` (`extension.rs:279-300`), is still MCP-008/MCP-011's empty body and
calls nothing. So this wave fills the seam and makes it reachable — it does **not** yet make a
configured server connect from a live session. An earlier draft of this section said "in production
for the first time"; that was wrong and is corrected here rather than quietly dropped, because the
next reader would otherwise have taken it as evidence that MCP-008/MCP-011 were no longer on the
critical path.

| unit | was | now | what remains |
|---|---|---|---|
| `MCP-101` | partial | **implemented** | — |
| `MCP-105` | missing | **implemented** | — |
| `MCP-109` | partial | **implemented** | its own verify line's `@modelcontextprotocol/conformance` client baseline was **not run** — the transport is proven against a hand-rolled loopback fixture instead |
| `MCP-113` | implemented | **implemented** | `select_transport` now has a caller — `ConnectionBuilder::create_connection` — but that caller is only installed at `initialize_mcp`, which is itself still test-only pending MCP-008/MCP-011 in `extension.rs::on_session_start`. Its census-row note ("no production caller yet") therefore still stands |
| `MCP-114` | partial | **implemented** | four of its five verify bullets are asserted on the wire here; the `bearerTokenEnv` fallback and the `HTTP bearer token` context string were already asserted in `secrets.rs`'s own tests and are not re-asserted |
| `MCP-115` | partial | **partial** | the ladder, its arm order and the 401 predicate are done — the predicate covering a **bare** 401 as well, which it did not until review-pass item 5; the *provider* is a seam (`HttpAuthProvider`) whose production binding is section 05's, and `skipIssuerMetadataValidation` is read and consumed by nothing because rmcp's streamable-HTTP config has no such field |
| `MCP-115a` | missing | **implemented** | — |
| `MCP-128` | partial | **partial** | its connect half now lands — `request_options.timeout` bounds the handshake on both arms (review-pass item 4), where before it was built by the manager and read by nothing. The manager-side `setDefaultRequestTimeoutMs` / `getRequestOptions` half is unchanged from its census note |
| `MCP-124` | partial | **implemented** | — (this row said `implemented \| —` before the `ManagerError` half landed, and was wrong: see review-pass item 2) |

**What each one actually did.**

* **MCP-101.** `ConnectionBuilder::connect_stdio` in upstream's order: `createClient` (so an invalid
  `protocolVersion` throws before a child exists), `args.map(interpolateEnvVars)`, the abort check,
  `mkdirSync(pluginDataDir)`, `resolveConfigPath(cwd) ?? defaultCwd`, `resolveEnv`, the stderr
  drain. Six tests spawn real children and read the environment back out of them.
* **MCP-105.** `parse_package_spec` + `EXACT_PACKAGE_VERSION_RE` replace `extract_package_name`,
  threaded into both the cache-hit predicate and `find_cached_package_dir`'s version filter. The
  29-row parse table was produced by running upstream's own `parsePackageSpec` on node 22.
* **MCP-109/114.** The transport is constructed for real — `reqwest` is now a declared dependency of
  `cyrup-mcp` (no new resolution surface; rmcp already resolves onto the workspace's `0.13.4`)
  because wrapping the client is what the header-command decorator needs. `resolve_server_url` and
  `resolve_http_secrets` were already written and are now wired.
* **MCP-115.** The four-arm ladder, in upstream's order, with `crate::oauth::on_unauthorized` as
  arms 4-5. **The 401 predicate is hand-written and the plan said it should not be**: rmcp's
  `ClientInitializeError::auth_challenge()` matches `AuthRequiredError` (401) *and*
  `InsufficientScopeError` (403), while upstream's `isUnauthorizedHttpError` is 401 only — using it
  as-is would turn every scope-denied 403 into a `needs-auth`.
* **MCP-115a.** `RequestHeadersCommandClient` is built once per connect (matching
  `server-manager.ts:868-870`, which is *outside* `attempt`) and used by every attempt. **Divergence
  3 of that module is closed**: `apply_derived` now clears rmcp's `auth_header` when the derived set
  carries an `authorization`, so a bearer-configured server with a signing command sends one
  `Authorization`, the derived one. Measured before the fix as
  `["Bearer static-bearer", "Signature derived"]`.
* **MCP-124.** The five aggregate variants, their byte-exact heads, and a structural
  `is_cleanup_failure` — **and, since the review pass below, the `ManagerError` half**: an
  `Aggregate` raised by `disposeConnection`/`closeAll` is now mapped onto the matching `McpError`
  variant by `From<&ManagerError>` instead of flattened to `Other`, and `ManagerError::Display`
  routes through `errors::render_aggregate_texts`. Without those two, the type that actually reaches
  a user through `closeAll` kept neither the class nor the rendering the unit was filed to fix.

**What this wave leaves open, stated so it is not mistaken for closed.**

* **`McpError::SetupFailed`'s only producer today is a narrow race.** The arm that raises it is
  `createConnection`'s catch after a *post-handshake* step fails and the cleanup after it also
  fails. `ConnectionBuilder::post_handshake`'s own abort check is such a step, so a `close` that
  races a settled handshake **and** whose `resource.close()` then fails does raise it against a real
  server — pinned by `an_abort_whose_own_cleanup_fails_is_a_setup_failure`. What is missing is
  *upstream's* producer, discovery (MCP-119), which cannot land through this seam at all:
  `NewConnection` has no field for tools/resources/prompts and `ServerConnection::new` hardcodes
  them empty. Widening that seam is `server_manager.rs`'s change. (An earlier draft of this bullet
  said the variant had "no producer" and "cannot fire against a real server". That was too strong;
  the reachable case is narrow, not empty.)
* **`McpError::AbortCleanupFailed` and `McpError::HttpCleanupFailed` have no producer either**, for
  a different reason: `serve_client_with_lifecycle_and_ct` closes the transport on every failure
  path and reports one error, so this port has no separate cleanup outcome to observe. That is
  MCP-123's residual verbatim.
* **`McpConnection`'s `Peer` is unreachable through `ConnectionResource`.** The trait exposes
  `close`/`has_session_id`/`child_pid`/`stderr_detail` and nothing else, so nothing outside
  `runtime.rs` can issue a request on a connection the builder made. Same seam, same owner.
* **MCP-103 is still unported**, so an `npx` server's tracked child is still the npm launcher. The
  call site is marked in `connect_stdio`.
* **A failed handshake SIGKILLs its child with no graceful window.** Nothing on that path calls
  `close()`; `serve_client_with_ct_inner` drops the transport and `ChildWithCleanup::drop` spawns a
  fire-and-forget `kill()`. Upstream's catch runs `client.close()`, and the TS SDK escalates
  close-stdin → 2 s → SIGTERM → 2 s → SIGKILL. Bounded to the failed-connect path — a successful
  connection tears down through `graceful_shutdown` — and pinned by
  `a_failed_handshake_leaves_no_child_behind`, which also guards against the child leaking outright.
* **`secrets::resolve_command_secret` takes no `EnvFn`**, so `ConnectionBuilder::with_environment`'s
  environment reaches `args`, `cwd`, the URL and the bearer ladder but not `env`/`headers` values.
  In production both are `process.env` and nothing diverges; in a test they are two seams.

Verified: `cargo check --workspace --all-targets` clean; `cargo nextest run --workspace` 7850/7851,
the one failure being the pre-existing `cyrup-modes
rpc_cycle_model_spans_the_full_auth_filtered_registry`. Clippy on `cyrup-mcp`/`cyrup-ext`: 2 warning
sites, both pre-existing and both measured against the tree with this wave's block sliced out.
rustdoc warnings 34 → 34 (`cyrup-mcp`) and 39 → 39 (`cyrup-ext`).

### Wave 5 review pass — six defects found in the wave's own output, and what changed

Every item below was **measured before the fix and re-measured after**, and each carries a test that
fails on the pre-fix tree. Two of them were false statements already written into the source and the
first draft of this section; those are corrected in place rather than appended to, because a wrong
comment outlives a wrong line of code — the next reader trusts it instead of checking.

| # | defect | where | now |
|---|---|---|---|
| 1 | "`initialize_mcp` installs it, so a configured server actually connects in production" — it has no non-test caller | this section's preamble, `runtime.rs:170-186`, the `MCP-113` row | corrected above; the builder is installed **at** `initialize_mcp`, which is test-only pending MCP-008/MCP-011 |
| 2 | `MCP-124` marked `implemented \| —` while the aggregates the manager actually raises had no typed variant, rendered head-prefixed, and lost their class at the public boundary | `server_manager.rs` `ManagerError` | `From<&ManagerError>` maps `Aggregate` (and a carried `Mcp(<aggregate>)`) onto the `McpError` variants; `Display` routes through `errors::render_aggregate_texts`; the five head constants are now **re-exported** from `errors.rs` instead of redefined, so the dispatch cannot drift |
| 3 | an OAuth token and a config-supplied `Authorization` both went on the wire | `runtime.rs::http_attempt` | the configured header wins and the token is dropped, matching `_commonHeaders`' spread order. Measured before: `["Bearer from-store", "Static abc123"]` |
| 4 | `requestTimeoutMs` never reached the handshake — a server that accepts and never answers `initialize` hung `connect` forever | `runtime.rs`, both arms | `connect_client_bounded` applies `request_options.timeout`; a lapse raises upstream's byte-exact `Request timed out`. Ablation: with the budget ignored, the two `wedged` tests do not terminate |
| 5 | a bare 401 (no `WWW-Authenticate`) was a hard error instead of `needs-auth`, and the doc at the site asserted the opposite | `runtime.rs::unauthorized_challenge` | widened with `bare_unauthorized`; the 403/`InsufficientScope` exclusion is unchanged and separately pinned |
| 6 | `has_session_id` was a hardcoded `true` under a comment claiming it was a live read | `runtime.rs::http_attempt` | a real read: `SessionIdProbe` wraps the HTTP client and records the `Mcp-Session-Id` the handshake response carried. A stateless server now reads `false` and stops tripping the session-recovery gate |

**Why 3 keeps recurring, written down because it is the transferable part.** rmcp carries the bearer
in a separate `auth_header` channel from the custom-header map, and *both* channels append —
`RequestBuilder::bearer_auth` and `builder.header(name, value)`. Upstream has one `Headers` object
with `set` semantics. So parity is not the default: **every path that can produce an `Authorization`
has to clear the other channel explicitly.** Wave 2 fixed instance one in
`secrets::resolve_http_secrets` (a resolved `bearerToken` strips a configured `Authorization`),
MCP-115a fixed instance two in `request_headers_command::apply_derived`, and this pass fixed the
third. There is no reason to think a fourth producer would be born correct.

**Also landed in this pass**, from the review's minor findings:

* **`StdioTransportSpec::resolve` now runs under `spawn_blocking`.** It reaches
  `secrets::resolve_command_secret`, a `std::process::Command` spawn polled with
  `std::thread::sleep` and bounded by a 10-second timeout. Run inline it held a tokio worker for up
  to ten seconds inside the manager's single-flight connect future, where `close`/`close_all`'s
  abort could not preempt it — a guarantee wave 4 measured against `UnbuiltConnectionFactory`, which
  returned instantly and so could not have caught this. Upstream's `spawnSync` blocks node's whole
  event loop, so leaving it inline was arguable parity; it was still the one way this
  `createConnection` body could weaken a guarantee the rest of the crate relies on. Measured on a
  one-worker runtime: a 200 ms timer over a connect carrying a 1-second `!command` env value fired
  at 1.019 s inline and at 0.2 s under `spawn_blocking`.
* **`SetupFailed`'s residual restated** (see the bullet above).
* **`close_inner`'s "Blocker, stated plainly" note** at `server_manager.rs` was stale in its first
  half (`is_cleanup_failure` does match all seven now) and true in its second; both halves are
  rewritten to what the code does.

### Still open — a 401 rmcp never turns into an error at all (found by the confirming pass)

**MCP-115 / F5 is incomplete, and the gap is invisible from this crate's own code.** The bare-401
fix works for a 401 with no body. It does NOT work for a 401 carrying
`Content-Type: application/json` and a parseable JSON-RPC error, because rmcp applies its
JSON-RPC-error shortcut to **every** non-success status, not just 400:
`rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:278-293` returns
`Ok(StreamableHttpPostResponse::Json(..))` for that case, so the
`Err(UnexpectedServerResponse("HTTP {status}: {body}"))` at `:296` — which `runtime.rs:2063`
prefix-matches — is never constructed. `bare_unauthorized` cannot fix this: it is never called.

MEASURED through the real `ConnectionBuilder::connect_http_client` against a loopback fixture
answering `initialize` with `401` + a JSON-RPC error body: the connect ends as a hard failure and
the OAuth ladder is never reached. A server that answers this way — which is legal, and which the
MCP spec's own error shape encourages — can never authenticate.

Fix shape: catch the status before rmcp collapses it, in the client-decorator seam this crate
already occupies (`SessionIdProbe` / `RequestHeadersCommandClient`), raising the unauthorized shape
whenever the response was HTTP 401 regardless of body; or carry the status out of the decorator into
the ladder. The ladder tests need a `json_rpc_body` mode on `FixtureOptions` alongside
`challenge: false` — the fixture's inability to produce this shape is exactly why it went unseen.

Not fixed here because it is a second, distinct mechanism from the one F5 addressed and wants its
own measured pass. It fails SAFE (a hard connect error, never a wrongly-authenticated request).

### Still open after the review pass — items outside this unit's files

Each was measured and is recorded here so it is not lost; none is fixed, because each lives in a
file this unit does not own.

* **`config.rs:618-621`'s cross-reference is dangling.** `StringRecord`'s `Deserialize` doc defers a
  residual to "`13c-mcp-servers.md`'s MCP-144 notes"; that block records only that
  `interpolate_env_record` drops non-string values, and says nothing about the non-object-`env`
  case. Measured on node 22 @ v2.26.1: `computeServerHash({command:"x",env:"abc"})` =
  `01ed7340…`, the writer produces `f0211144…` (upstream's digest for the same definition with `env`
  **absent**). Same family, also unrecorded: `env: []`, `env: 5`, `env: true` all hash as `{}`
  upstream (`1d224401…`) and as absent here. The same doc calls this "a fifth" divergence while this
  file and the wave report call `auth: null` the fifth and this the sixth. Fix: record it in 13c's
  MCP-144 block, or repoint `config.rs:621`.
* **That residual is described as writer-only and is not.** The **writer** degrades to `None`; the
  **reader** drops the whole server from the direct-tool surface —
  `mcp_direct_tools.rs::extract_server_map` skips any entry `serde_json::from_value::<ServerEntry>`
  rejects, and `env: Option<BTreeMap<String, Value>>` rejects a string, array, number or bool.
  Measured over six definitions, the reader keeps three where upstream keeps six. `args: [1,"b"]`
  and `command: 5` behave identically — one root cause (typed reader fields with no `lenient`
  equivalent), not three items.
* **`StringRecord` opened an unnamed connect-path divergence.** `secrets.rs:386` passes
  `entry.env.as_deref()`, which `Deref`s to the string members only, so
  `env: {"GOOD":"1","BAD":5}` now spawns the child with `GOOD=1`; before the retype `lenient`
  dropped the whole block and it spawned with none. Upstream does neither — measured,
  `resolveCommandSecretsRecord({GOOD:"1",BAD:5}, …)` throws `value.startsWith is not a function`
  and refuses the connect. The hash side is correct on both crates. Fix: route
  `resolve_stdio_env`/`resolve_http_secrets` through `StringRecord::unhashable()`, or name the
  divergence in `StringRecord`'s doc.
* **`registration.rs:792` and `:865-866` are stale.** Both say the hasher's `None` "has exactly one
  source" (`resolve_server_url`); `ResolvedIdentity::resolve` has had a second `Err` arm since the
  hashing wave, and `dirs.rs:1082-1085` already says "**two**". Behaviour is correct — `Option::ok()`
  swallows both — so this is docs only, in a crate whose house style is that comments carry the
  specification. `registration.rs` is untouched by wave 5.
* **The same class loss still applies to `McpError::CredentialStore` across `ManagerError`.**
  `From<&ManagerError>` now rebuilds the aggregates and keeps `Aborted`/`Config`/`Server`, but a
  credential-store failure raised inside the factory (`ConnectionBuilder::connect_http_client`'s
  `self.auth.authorize(..)?`) still arrives as `McpError::Other` and
  `is_credential_store_failure()` answers `false` for it. It cannot be fixed the same way:
  `AuthStoreError` is `#[non_exhaustive]` and not `Clone`, so the one-way `&ManagerError ->
  McpError` door cannot reconstruct it, and the type lives in `credentials.rs`. The class matters
  for the same reason the doc on the variant gives — section 07's refresh driver rethrows a store
  failure and swallows everything else — but no consumer of `is_credential_store_failure` currently
  sits downstream of this conversion, so it is a latent hazard rather than a live bug. Fix shape:
  make `AuthStoreError` `Clone`, or give `ManagerError` a `CredentialStore` arm.
* **`13c-mcp-servers.md:1110-1113`'s MCP-100 attribution is wrong.** It says
  `MCP connection for <name> was closed while connecting` is reachable "when the generation advanced
  **without** the attempt being aborted (what `reconnect`/`closeAll` can produce)". Upstream writes
  `closeGenerations` at exactly two places, `server-manager.ts:1098` and `:1146`, and **both** abort
  the attempt controller on the next line; `reconnect` never touches it (`doReconnect` delegates to
  `this.close(name)`, which aborts). The only window is between `connect`'s generation read at
  `:279` and its `connectAttempts.set` at `:286`. The measured half of that bullet is sound.

Verified after the review pass: `cargo check --workspace --all-targets` clean;
`cargo nextest run --workspace --no-fail-fast` 7858/7859, the one failure still the pre-existing
`cyrup-modes rpc_cycle_model_spans_the_full_auth_filtered_registry`; `cyrup-mcp` alone 612/612.
Clippy on `cyrup-mcp`: 2 diagnostics, both pre-existing (`dirs.rs:1863`'s empty line after a doc
comment, and `result_large_err` on `connect_client`, whose `ClientInitializeError` is returned
unflattened on purpose — see its doc). rustdoc warnings for `cyrup-mcp` 34 → 33.

Every fix in this pass is pinned by a test that fails on the pre-fix tree. The ablations, run one at
a time:

| fix | ablation | result |
|---|---|---|
| duplicate `Authorization` | restore `if config.auth_header.is_none()` | `an_oauth_token_never_joins_a_configured_authorization_header` fails with `left: ["Bearer from-store", "Static abc123"]` |
| handshake timeout | ignore the budget in `connect_client_bounded` | both `wedged` tests never terminate (`timeout 90` kills the run, exit 124) |
| bare 401 | drop the `bare_unauthorized` arm | `a_bare_401_with_no_challenge_still_reaches_the_oauth_ladder` fails |
| `has_session_id` | restore the hardcoded `true` | `a_stateless_http_server_reports_no_session_id` fails |
| MCP-124 rendering | restore head-prefixed `ManagerError::Display` | `close_all_aggregates_only_cleanup_failures` fails with `"MCP manager cleanup failed: MCP connection cleanup failed: client close failed"` |
| MCP-124 class | drop the `Aggregate` / `Mcp(<aggregate>)` arms of `From<&ManagerError>` | `close_rethrows_a_pending_connects_setup_failure_and_swallows_everything_else` fails with `Other("connect ECONNREFUSED: transport close failed")` |
| `spawn_blocking` | inline `StdioTransportSpec::resolve` | `a_slow_env_command_does_not_hold_the_worker_carrying_the_connect` fails: the 200 ms timer fires at 1.019 s |

## Census

| status | units | meaning |
|---|---:|---|
| `implemented` | 212 | the unit's obligations are met in the Rust |
| `partial` | 100 | lands, but a named obligation is unmet |
| `missing` | 98 | no implementation found |
| `not-applicable` | 27 | `cut` or `open-decision` — not work |
| **total** | **437** | |

**198 units carry open work** (98 missing + 100 partial). By the plan's own severity:

| severity | open | of total | 
|---|---:|---:|
| critical | 8 | 22 |
| high | 73 | 147 |
| medium | 91 | 172 |
| low | 24 | 60 |
| n/a | 2 | 35 |

### By section

| § | missing | partial | open | units | critical+high open |
|---|---:|---:|---:|---:|---:|
| [`13a-mcp-activation.md`](13a-mcp-activation.md) | 17 | 22 | 39 | 51 | 10 |
| [`13b-mcp-config.md`](13b-mcp-config.md) | 6 | 13 | 19 | 51 | 9 |
| [`13c-mcp-servers.md`](13c-mcp-servers.md) | 20 | 20 | 40 | 51 | 23 |
| [`13d-mcp-proxy-modes.md`](13d-mcp-proxy-modes.md) | 1 | 4 | 5 | 36 | 3 |
| [`13e-mcp-tools.md`](13e-mcp-tools.md) | 7 | 8 | 15 | 53 | 7 |
| [`13f-mcp-credentials.md`](13f-mcp-credentials.md) | 0 | 5 | 5 | 41 | 1 |
| [`13g-mcp-oauth.md`](13g-mcp-oauth.md) | 1 | 8 | 9 | 49 | 2 |
| [`13h-mcp-tui.md`](13h-mcp-tui.md) | 15 | 9 | 24 | 55 | 10 |
| [`13i-mcp-protocol-and-verification.md`](13i-mcp-protocol-and-verification.md) | 31 | 11 | 42 | 50 | 16 |

The shape of that table is the finding. **`13i` (protocol and verification) is the weakest surface** —
31 of its 50 units have no implementation at all — and **`13c` (servers, transports, metadata cache)
carries the most critical-or-high open work (23)**. `13f` (credentials) is the strongest: nothing
missing, five partials, one of them high.

## Critical-severity open work

Eight of the plan's 22 `critical` units are open. None is a clean greenfield gap; every one is a
divergence inside something that already exists, which is why they read as `partial`.

| id | status | § | the unmet obligation |
|---|---|---|---|
| `MCP-083` | partial | 13b | Two obligations unmet. (1) `resolveCommandSecretsRecord` — the per-record form applied to `env` and `headers` — does not exist; grep over crates/cyrup-mcp/src for `resolve_command_secrets_record` / `interpolate_env_record` returns nothing, and … |
| `MCP-141` | partial | 13c | Three gaps, each independently fatal to the contract. (1) **The reader was not upgraded.** `cyrup_ext_subagents::exec::mcp_direct_tools::compute_mcp_server_hash` (/home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:531-584) still hashes … |
| `MCP-142` | partial | 13c | The **reader still emits `null`**: `mcp_direct_tools::stable_stringify` (/home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:796-825) maps `Value::Null => "null"`, and every absent field is materialised as `Value::Null` by `opt_str_value` … |
| `MCP-146` | partial | 13c | The reader was **not** changed: `cyrup_ext_subagents::exec::mcp_direct_tools::resolve_direct_tool_names` still builds `format!("get_{}", resource_name_to_tool_name(name))` at /home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:466, and … |
| `MCP-232` | partial | 13e | The gate itself is unimplemented — `ensure_tool_call_approved` exists only as a `ProxyEnv` trait method (proxy.rs:1488) with a test-only implementor. Missing: the cache lookup/insert against `approved_tool_calls`, the headless check performed **before** … |
| `MCP-370` | partial | 13h | The other half of option (a) — upgrading the in-tree consumer in the same change — has NOT been done. /home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs still has a 3-variant `enum ToolPrefix { Server, None, Short }` (line 45-49), … |
| `MCP-394` | partial | 13h | The orchestration is explicitly outstanding — `TODO(MCP-394)` at ui.rs:4781-4786. Absent: the `programmaticConfig` branch (notify `MCP status is shown from the in-memory SDK config; configuration discovery is unavailable.` + `showStatus`), the … |
| `MCP-455` | missing | 13i | Missing: `confirmSampling`'s three-branch gate (auto-approve short-circuit; explicit `has_ui: bool` sourced from the host config producing the distinct "MCP sampling requires interactive approval. Set settings.samplingAutoApprove to true to allow it without … |

## High-severity open work

73 of 147 `high` units are open.

| id | status | § | the unmet obligation |
|---|---|---|---|
| `MCP-008` | partial | 13a | Three obligations unmet in the handler itself. (1) It never calls `shutdown_previous_generation`/`shutdown_state` — `_previous_state` is bound to `_` and dropped, so the previous generation's status … |
| `MCP-009` | partial | 13a | The snapshotted state is discarded (`let _state = …`) instead of being passed to `lifecycle::shutdown_state`, so the shutdown-time status snapshot, the metadata flush and the flush-error-wins rule … |
| `MCP-010` | partial | 13a | Not wired: `shutdown_state` has no production caller (grep across `crates/cyrup-mcp/src` finds it only in lifecycle.rs's own definition, `shutdown_previous_generation`, and tests). And the only … |
| `MCP-011` | missing | 13a | Everything in this unit: the triple staleness check (`owner.is_active()` && `generation == my_gen` && `Arc::ptr_eq(init_task, promise)`), the stale-state teardown with `MCP: failed to clean stale … |
| `MCP-014` | partial | 13a | Two halves unproven/unbuilt. (1) The `SessionStart` rebuild half is empty (see MCP-008/MCP-011), so nothing that a replacement is supposed to preserve across generations is exercised. (2) The unit's … |
| `MCP-023` | missing | 13a | §12 in full: pass one building `startupKnownMetadata: Map<server, ToolMetadata[]>` over every successful connection (tools plus, when `exposeResources !== false`, `read_<resource>` entries with their … |
| `MCP-025` | partial | 13a | The notification half is entirely absent. Grep for `servers connected`, `tools skipped`, `Failed to connect to {name}` (the startup form) across `crates/cyrup-mcp/src` returns nothing: the … |
| `MCP-029` | partial | 13a | `updateMetadataCache(state, serverName, {preserveEmptyResources})` itself does not exist — grep for `preserve_empty`, `preserveEmptyResources`, `prompt_discovery_failed`, `serialize_tools`, … |
| `MCP-037` | missing | 13a | Build one of the two shapes: (i) a defaulted `NativeExtension::set_ext_host(&self, host: Weak<ExtensionHost>)` called from `ExtensionHost::load_native_with_services` beside `set_host_services`, plus … |
| `MCP-043` | partial | 13a | The two are disjoint types and the model reaches only the inert one. `proxy::McpTool::new` is constructed **only in tests** (all hits proxy.rs:5497-5584 are inside `#[cfg(test)]`); the tool actually … |
| `MCP-068` | partial | 13b | Three obligations unmet. (1) `MCP_UI_DEBUG` has no reader — the string does not appear anywhere in crates/cyrup-mcp/src (grep over src/*.rs: zero hits), so the logger level bootstrap does not exist. … |
| `MCP-070` | partial | 13b | Three things stop this from being a working contract. (1) Every production caller hashes UNRESOLVED values: src/ui.rs:1758 uses `ResolvedIdentity::verbatim(definition)`, whose own doc at … |
| `MCP-073` | missing | 13b | A `pub fn resolve_server_from_tool_name(tool_name, server_names, prefix) -> Option<String>` on `cyrup-mcp`: `None` for `ToolPrefix::None`; collect every configured server whose non-empty … |
| `MCP-075` | partial | 13b | The second copy is wrong. src/proxy.rs:486 `format_legacy_tool_name` derives the legacy prefix by taking `get_server_prefix(server_name, prefix)` — which has ALREADY applied the … |
| `MCP-076` | partial | 13b | src/registration.rs:334 compiles with bare `Regex::new(&out).ok()`, without the `RegexBuilder::size_limit` / `dfa_size_limit` ceilings the unit explicitly requires. The proxy copy does set them … |
| `MCP-084` | partial | 13b | `resolveServerUrl` is not implemented anywhere. Grep over crates/cyrup-mcp/src for each of its three exact strings — `MCP server URL must be a string`, `Missing environment variable`, `Invalid MCP … |
| `MCP-092` | missing | 13b | The whole dialect gate: `schemaDialect(schema)` (no string `$schema` ⇒ unstamped; else strip ONE trailing `#`), routing unstamped and `https://json-schema.org/draft/2020-12/schema` to a 2020-12 … |
| `MCP-094` | missing | 13b | One scheduled change to crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs covering all nine indexed divergences above, plus the shared conformance suite the unit names: one golden `mcp.json` + … |
| `MCP-100` | missing | 13c | The entire manager is unbuilt. Absent: `ServerConnection` (client/transport/definition/tools/resources/prompts/promptDiscoveryFailed/instructions/lastUsedAt/inFlight/status/credentialsInvalidated); … |
| `MCP-101` | partial | 13c | No `resolveEnv` and no connection builder. Specifically absent: (a) `args = (definition.args ?? []).map(interpolateEnvVars)` — grep for any arg-interpolation call site returns nothing; (b) … |
| `MCP-105` | missing | 13c | Add `ParsedPackageSpec { package_name, exact_version: Option<String> }` and `parse_package_spec` (the `@scope/name` `rfind('@') > find('/')` rule, `^=` then case-insensitive `^v` strip, validated … |
| `MCP-109` | partial | 13c | Nothing constructs the transport in production: `build_http_transport_config` and `http_transport_with_client` have only test callers (runtime.rs:1733/1756) and a doc reference from … |
| `MCP-114` | partial | 13c | The `connectHttpClient` pre-flight (§3.4 steps 1-7) does not exist. Specifically absent, verified by grepping all 19 files: (a) `resolveServerUrl` — no `resolve_server_url` symbol, and neither of its … |
| `MCP-115` | partial | 13c | The attempt loop itself is absent — there is no `connect_http_client` anywhere in the crate. Missing: the per-attempt fresh client; the transport-options assembly including … |
| `MCP-115a` | missing | 13c | Two concrete work items. (1) The wiring: build `RequestHeadersCommandClient::new(client, cfg, ct)?` **inside** MCP-115's retry closure and pass it to `http_transport_with_client(..., config)`, so it … |
| `MCP-116` | missing | 13c | No connection record carries `credentials_invalidated`, so `connect`'s step-7 carry-forward (`existing?.status == "needs-auth" && existing.credentialsInvalidated === true`) does not exist, nor does … |
| `MCP-119` | missing | 13c | No discovery at all. Needed: the unconditional `list_all_tools` with errors propagating; the `resources`/`prompts` capability gate read from `RunningService::peer_info() -> … |
| `MCP-124` | partial | 13c | Add the five variants (`AbortCleanupFailed`, `SetupFailed`, `HttpCleanupFailed`, `ConnectionCleanupFailed`, `ManagerCleanupFailed`) with `Display` rendering the byte-exact heads `MCP connection abort … |
| `MCP-125` | missing | 13c | Missing entirely: the disabled and stopped guards firing **before** the single-flight map is consulted (with `MCP server "<n>" is disabled` / `MCP server manager is closed` — neither string exists in … |
| `MCP-126` | missing | 13c | Nothing of §3.12 exists: the generation bump, the `connect_attempts[name].cancel()` with the `MCP connection <n> was closed` reason, removal from the map **before** awaiting cleanup, the … |
| `MCP-131` | partial | 13c | Nothing in production ever owns or closes a `TokioChildProcess`: `spawn_stdio_transport` has only test callers, `ManagerSupervisor::close`/`close_all` are no-ops (lifecycle.rs:307-322), and grep … |
| `MCP-134` | missing | 13c | The predicate does not exist. Needed: the absolute `hadSessionId` gate captured **before** the call; the 404 arm (which `StreamableHttpError::SessionExpired` supplies once an HTTP transport exists); … |
| `MCP-135` | missing | 13c | The whole wrapper is absent: the disabled/not-connected preconditions, `hadSessionId` captured before the call, the **live** config re-read after the failure, the 401 credential-cache invalidation … |
| `MCP-139` | partial | 13c | Three real gaps. (1) **The agent-dir consolidation did not happen.** `npx_resolver::agent_dir` (/home/user/cyrup/crates/cyrup-ext/src/caps/proc/npx_resolver.rs, anchored on … |
| `MCP-140` | partial | 13c | The **serialisers are absent**. Nothing converts a live MCP tool/resource/prompt list into `CachedTool`/`CachedResource`/`CachedPrompt`: grepping all 19 files for a `ServerCacheEntry {` construction … |
| `MCP-143` | partial | 13c | Both in-tree copies the unit names are unchanged. (1) `cyrup_ext::caps::proc::interpolate_env_vars_with` (/home/user/cyrup/crates/cyrup-ext/src/caps/proc.rs:148-156) is still `interpolate_braces` … |
| `MCP-144` | partial | 13c | Two call sites still bypass it. (1) `mcp_direct_tools::interpolate_env_record` (/home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:696-712) calls plain `interpolate_env_vars` … |
| `MCP-145` | partial | 13c | The throw arm is unreachable in practice: no fallible hasher is ever installed (`install_server_hasher` at registration.rs:754 has no production caller, and registration.rs:746-752 documents that … |
| `MCP-164` | partial | 13d | The rmcp invocation itself does not exist. `ProxyEnv::call_tool` (proxy.rs:1397) and `ProxyEnv::read_resource` (:1410) are declared with doc comments naming `Peer::send_request_with_option(...)` → … |
| `MCP-191` | partial | 13d | Neither of the unit's deliverables exists. (1) The hazard is undocumented: a grep for `MCP-191` across crates/cyrup-mcp/src returns zero hits — the only unit id in the section with no reference in … |
| `MCP-196` | partial | 13d | Not the named 46-ported + 1-re-specified target, and the expensive third of the suite is absent. Concretely missing, by upstream case name: proxy-modes-auto-auth's "runs URL elicitations returned by … |
| `MCP-207` | missing | 13e | The whole live `tools`+`resources` → `Vec<ToolMetadata>` pipeline is absent: no `failedTools` accumulation for unnamed tools, no post-visibility `seenNames` reservation, no `description ?? ""`, no … |
| `MCP-214` | missing | 13e | None of §7's ordered state machine exists: no owned-signal composition, no `lazyConnect`, no auto-auth-on-`needs-auth`, no connection assertion, no approval call, no request options, no `tools/call` … |
| `MCP-214a` | partial | 13e | Nothing wires either into a direct-tool call, because the executor does not exist (MCP-214). `ProxyEnv::call_tool` / `read_resource` (proxy.rs:1397, 1410) take a recovery callback but have no … |
| `MCP-217` | missing | 13e | No fingerprint-diff `syncDirectTools`, no `deactivateTools` fallback pass, no `syncProxyTool` description refresh at runtime and no `syncToolSurface` entry point. The state slots exist unused on … |
| `MCP-231` | missing | 13e | The whole predicate is unwritten: server-level `approveTools` overriding the global on presence, `true` → always required, non-array/empty → not required, the legacy-alias disambiguation reusing … |
| `MCP-249` | partial | 13e | Two gaps. (1) `server_unavailable` — emitted by upstream direct-tools.ts step 7 (`details: { error: "server_unavailable", server }`, verified at … |
| `MCP-260` | partial | 13f | Two items. (1) `crates/cyrup/src/mcp_keyring_helper_cmd.rs` does not exist: no `SUBCOMMAND`/`is_selected(argv)`/`dispatch()` triple, no `pub mod` in crates/cyrup/src/lib.rs, and no pre-dispatch in … |
| `MCP-324` | partial | 13g | The credential-store rethrow is a STRING-PREFIX test, not a structural one: oauth.rs:3706 does `error.to_string().starts_with(CREDENTIAL_STORE_PREFIX)` where `CREDENTIAL_STORE_PREFIX = "credential … |
| `MCP-326` | partial | 13g | An external abort does NOT reject with the identical reason value. `AuthenticateOptions::combined_signal` (oauth.rs:2618) builds a bare `CancelToken` via `crate::abort::combine`, which carries no … |
| `MCP-381` | missing | 13h | Everything in §4.1–§4.2 is absent: the owner-fenced prologue (capture `currentOwner` + a bound reload, build the synthetic `commandCtx` before the first await, await `initPromise` with the two … |
| `MCP-386` | missing | 13h | All ten steps of §4.7 are absent: the unknown-target guard, the sequential (`for … await`, not a join) all-servers loop, `manager.close` → `connect` with the two `throwIfInactive` checks, the … |
| `MCP-387` | missing | 13h | Absent: the `programmaticConfig` refusal (`MCP setup is unavailable when config is supplied by createMcpAdapter().`), the once-only computation of `discovery = getMcpDiscoverySummary(...)` and … |
| `MCP-388` | missing | 13h | All four steps of §4.9 are absent, including the load-bearing string `OAuth credentials were cleared for "{name}", but its connection could not be closed: {msg}` that distinguishes "credentials gone, … |
| `MCP-390` | partial | 13h | The command-level flow is explicitly outstanding — `TODO(MCP-334)` at oauth.rs:3922-3929 says so. Absent: the `/mcp-auth` handler itself (no `execute_command`, MCP-381); `terminalHyperlink`'s OSC-8 … |
| `MCP-392` | missing | 13h | The whole of §4.11's `buildMcpPanelCallbacks` is absent: the per-open `authStatusFailures: Map<String,String>` (deliberately NOT session state), the eight-rung `getConnectionStatus` ladder … |
| `MCP-395` | partial | 13h | The live half has nothing to land on and none of the three additions have been made: grep across /home/user/cyrup/crates/cyrup-ext/src, /home/user/cyrup/crates/cyrup-session-svc/src and … |
| `MCP-398` | missing | 13h | All nine steps of §5.5 are absent: the `MCP not initialized` guard, the `promptMetadataLive`-guarded staleness check BEFORE `lazyConnect` (the guard that stops a cache-only command being refused … |
| `MCP-450` | missing | 13i | The whole 12-step `handleSamplingRequest` free function is absent: no `SamplingOptions` bag, no `handle_sampling_request(&SamplingOptions, CreateMessageRequestParams) -> Result<CreateMessageResult, … |
| `MCP-452` | missing | 13i | Missing entirely: `fn sampling_candidates(available, hints, current) -> Vec<Model>` with hint-order-major / registry-order-minor appending, lowercase substring `.contains()` matching over … |
| `MCP-453` | missing | 13i | Missing: the direct `cyrup_provider` completion call with `{systemPrompt?, messages}` and `{apiKey?, headers?, maxTokens, temperature?, metadata?, cancel}`; `max_tokens` passed through … |
| `MCP-458` | missing | 13i | Missing: a sampling options bag holding two live closures over the stashed `Arc<dyn HostServices>` — `current_model()` read live, and a cancellation source composed as a child `CancellationToken` — … |
| `MCP-461` | missing | 13i | Missing in full: the `MCP Input Request\nServer: …` gate dialog with `["Continue","Decline"]` and `None`→cancel; the `properties.len() == 0` → `{action:"accept", content:{}}` short-circuit before any … |
| `MCP-464` | missing | 13i | Missing: the whole coercion core — 13 distinct message templates across 15 throw sites over `PrimitiveSchemaDefinition`'s typed limit fields (`StringSchema::{min_length,max_length}`, … |
| `MCP-465` | missing | 13i | Missing: compiling the original `requested_schema` with `jsonschema` + `.should_validate_formats(true)`, running it over the coerced `output`, and throwing `Invalid elicitation response: {err}`. Also … |
| `MCP-467` | missing | 13i | Missing the whole handler: the `!allow_url` gate, `url::Url::parse` failure and the http/https scheme allowlist — all three as `ErrorData::invalid_params` (-32602); the exact 9-line confirmation … |
| `MCP-471` | missing | 13i | Missing: taking a `#[must_use]` `HostCtx::begin_human_wait()` guard and the session-scoped `HostServices::human_interaction_lock` across every `select`/`input`/`confirm` in `cyrup-mcp` — the two … |
| `MCP-474` | missing | 13i | Missing: the keyword guard `\b(?:token\|secret\|password\|passwd\|api[_-]?key\|authorization\|cookie)\b` (case-insensitive) returning `"[REDACTED]"`; the three replacements (URL scheme, … |
| `MCP-483` | missing | 13i | Missing: adopting `@modelcontextprotocol/conformance` (pinned to rmcp's `0.2.0-alpha.10`, per the docket, not upstream's 0.1.16) as the port's protocol gate, run for both `--spec-version` values, … |
| `MCP-484` | missing | 13i | Missing the whole driver: scenario allowlist with non-zero exit on an unknown scenario; the scripted elicitation UI with preference order `["Use default","Submit","Continue"]` then `options[0]`; … |
| `MCP-490` | partial | 13i | Section 13i's own share is entirely absent — zero tests for sampling, elicitation or tracing, because none of that code exists (MCP-450..MCP-481). Also missing: the case-count parity metric the unit … |
| `MCP-492` | partial | 13i | REFUTED on its central assertion. The claim says 'three of the four surviving upstream files have no port'; all three do have substantial ports, they are just not named after the .ts files. (a) … |
| `MCP-496` | missing | 13i | Missing: pty infrastructure (none in the workspace) plus a driven run of the full elicitation sequence — gate → one dialog per widget kind → 20-option multi-select with `✓ ` toggle state → review → … |

## Every unit, by section

`implemented` rows carry the evidence that settled them; open rows carry the unmet obligation.

### 13a · Activation, lifecycle and the host seam

[`13a-mcp-activation.md`](13a-mcp-activation.md) — 51 units, 17 missing, 22 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-001` | n/a | `hand-written` | **implemented** | Stand up `crates/cyrup-mcp` and attach it at the session-build arms | None for this unit. Noted only: three of `McpState`'s collaborator types are still forward declarations owned by other sections — … |
| `MCP-002` | low | `host-verb` | **implemented** | Read `--mcp-config` from argv directly, and register the flag for `--help` | `config::config_path_from_argv` (config.rs:1506) scans for the exact token `--mcp-config` and returns the *next* element only; … |
| `MCP-003` | critical | `host-verb` | **implemented** | Register the entire tool/command surface from disk caches inside `init()`, and … | `registration::register_surface(api, dirs, config) -> RegisteredSurface` (registration.rs:1858) is `pub fn`, has no `?` and no `Err` path: it … |
| `MCP-004` | high | `hand-written` | **implemented** | Port `McpRuntimeOwner` | `owner::McpRuntimeOwner` (owner.rs:68) over a `CancelToken` with `token()`, `is_active()`, `add_cleanup`, `begin_stop`/`stop` (memoised through a … |
| `MCP-005` | medium | `hand-written` | **implemented** | Reverse-order cleanup, the aggregate error, and the late-cleanup path | `McpRuntimeOwner::begin_stop` drains `cleanups.lock().drain(..).rev()`, invokes every closure at call time, joins with `futures::future::join_all`, … |
| `MCP-006` | medium | `extension-owned` | **partial** | Port `createOwnedUi` as a fenced services handle | Two obligations unmet. (1) Coverage: the `fenced!` list carries 31 methods (notify, set_status, set_widget, set_working_message, confirm, input, select, oauth_prompt, oauth_select, editor, custom, open_overlay, theme, session_id, session_file, current_model, models, context_usage, is_idle, … |
| `MCP-007` | medium | `hand-written` | **implemented** | Port the abort helpers (combineAbortSignals, isAbortError, throwIfAborted, … | `abort::combine(owner, other)` returns `owner.clone()` for `None` and otherwise spawns one `tokio::select!` joiner over a fresh child token … |
| `MCP-008` | high | `hand-written` | **partial** | The `session_start` generation protocol, abort-before-await | Three obligations unmet in the handler itself. (1) It never calls `shutdown_previous_generation`/`shutdown_state` — `_previous_state` is bound to `_` and dropped, so the previous generation's status snapshot and metadata flush never run. (2) It never builds the new owner or the new OAuth runtime … |
| `MCP-009` | high | `hand-written` | **partial** | The `session_shutdown` handler | The snapshotted state is discarded (`let _state = …`) instead of being passed to `lifecycle::shutdown_state`, so the shutdown-time status snapshot, the metadata flush and the flush-error-wins rule never execute; and the OAuth runtime is not explicitly shut down (it is only reached indirectly if … |
| `MCP-010` | high | `hand-written` | **partial** | `shutdownState`, preserving the metadata-flush error | Not wired: `shutdown_state` has no production caller (grep across `crates/cyrup-mcp/src` finds it only in lifecycle.rs's own definition, `shutdown_previous_generation`, and tests). And the only `MetadataFlush` implementation in the tree is `lifecycle::no_metadata_flush()` (lifecycle.rs:386), which … |
| `MCP-011` | high | `hand-written` | **missing** | `startInitialization`'s triple staleness check and metadata-update hook install | Everything in this unit: the triple staleness check (`owner.is_active()` && `generation == my_gen` && `Arc::ptr_eq(init_task, promise)`), the stale-state teardown with `MCP: failed to clean stale initialization state: …`, the commit into the `state` slot, the `onToolMetadataUpdated` hook install … |
| `MCP-012` | medium | `extension-owned` | **partial** | `startLoadTimeInitialization` — the eager/keep-alive pre-warm | The pre-warm itself is absent. `extension::init` calls the gate and then only emits `tracing::debug!("MCP: eager/keep-alive servers configured — pre-warm pending")` (extension.rs:527-530): there is no `tokio::spawn`, no generation re-check at the top of the task, no synthetic … |
| `MCP-013` | low | `hand-written` | **partial** | The `MCP_DIRECT_TOOLS` blocking wait at session start | The blocking wait is not implemented. `extension::McpExtension::on_session_start` (extension.rs:282-305) contains no `MCP_DIRECT_TOOLS` read and no `await initialization` — there is no initialization to await (MCP-008/MCP-011). A `MCP_DIRECT_TOOLS`-pinned run therefore does not delay `SessionStart`. |
| `MCP-014` | high | `hand-written` | **partial** | Re-`init` per session, and the build-before-dispose inversion | Two halves unproven/unbuilt. (1) The `SessionStart` rebuild half is empty (see MCP-008/MCP-011), so nothing that a replacement is supposed to preserve across generations is exercised. (2) The unit's four cyrup-it assertions are absent — `crates/cyrup-it/tests/mcp/activation.rs` contains only the … |
| `MCP-015` | medium | `extension-owned` | **partial** | Snapshot every context value before the first await in `initialize` | The two live-closure replacements are not built: nothing constructs a sampling config, so `getCurrentModel -> HostServices::current_model()` and `getSignal -> combine(owner, ctx)` have no call site, and `runtime.rs:139` binds the combined token to `_runtime_signal` (unused). No production code … |
| `MCP-016` | medium | `hand-written` | **partial** | The sampling and elicitation wiring gates | No call site applies the gates. `runtime::initialize_mcp` (runtime.rs:125-247) never reads `settings.sampling(...)`/`settings.elicitation(...)` and never wires a sampling or elicitation hook; `McpClientHandlerParts` (runtime.rs:1076) derives capabilities from whether hooks are present, but nothing … |
| `MCP-017` | medium | `hand-written` | **partial** | Register owner cleanups in the exact LIFO order, plus the list-changed listener | Two of the unit's three parts are missing. (1) `cleanupMaterializedBinaryResources` is never registered as the first cleanup (so it would not run last): `renderers::MaterializedResources::cleanup` exists (renderers.rs, see the doc at :79) but has no `owner.add_cleanup` call site — grep for … |
| `MCP-018` | low | `hand-written` | **partial** | The zero-enabled-servers early return | The user-facing half is absent: the `MCP: All {n} server(s) are disabled` info notification, gated on `allServerEntries.length > 0 && hasUI`, is not emitted — grep for `are disabled` across `crates/cyrup-mcp/src` returns only an unrelated proxy.rs comment at :4977. There is also no unit test … |
| `MCP-019` | medium | `hand-written` | **missing** | Metadata-cache bootstrap: file-absent means connect everything once | The whole §9 block: the `existsSync(cachePath)` probe distinguished from `loadMetadataCache() == None`; `!cacheFileExists ⇒ bootstrapAll = true` plus `saveMetadataCache({version:1, servers:{}})`; `file present but unparseable ⇒ rewrite empty WITHOUT bootstrapping`; and the `bootstrap_all` flag … |
| `MCP-020` | medium | `hand-written` | **partial** | Per-server lifecycle registration and idle-override derivation | The per-server registration loop in `initializeMcp` is absent — `runtime::initialize_mcp` never iterates `serverEntries` and never calls `register_server`/`mark_keep_alive`. Critically, the `idleOverride = definition.idleTimeout ?? (persistsAfterFirstSpawn ? 0 : undefined)` derivation exists … |
| `MCP-021` | medium | `hand-written` | **missing** | Rehydrate tool/resource/prompt/instruction metadata from a hash-valid cache … | §10 step 6 in full: for each hash-valid `cachedEntry`, populate `toolMetadata` via `reconstructToolMetadata(name, entry, prefix, definition, config.mcpServers, cache)`, `resourceCounts` from `entry.resources.length`, `promptMetadata` via `reconstructPromptMetadata` **without** adding to … |
| `MCP-022` | medium | `hand-written` | **missing** | The bounded startup connect pass | The whole §11: the `bootstrapAll ? all : keep-alive\|eager` selection, the `connecting to {n} servers...` status write through `formatMcpStatus`, an index-preserving `parallel_limit` worker pool at concurrency 10, the `needs-auth ⇒ "OAuth authentication required. Run /mcp-auth {name}."` byte-exact … |
| `MCP-023` | high | `hand-written` | **missing** | The two-pass startup metadata build | §12 in full: pass one building `startupKnownMetadata: Map<server, ToolMetadata[]>` over every successful connection (tools plus, when `exposeResources !== false`, `read_<resource>` entries with their `resourceUri` and `Read resource: {uri}` default description) before any per-server build; pass two … |
| `MCP-024` | medium | `hand-written` | **missing** | Failure tracking with a 60-second backoff | All of §13: the `FAILURE_BACKOFF_MS = 60_000` and `MAX_FAILURE_MESSAGE_CHARS = 8*1024` constants, `clearFailure`, `recordFailure` (clear-first, `failedAt` stamp, message truncation, the expiry task holding a `Weak<McpState>` and selecting on the owner token, and the `== failedAt` generation check … |
| `MCP-025` | high | `hand-written` | **partial** | Startup connect notifications, terminal sanitising, and skipped-tool warnings | The notification half is entirely absent. Grep for `servers connected`, `tools skipped`, `Failed to connect to {name}` (the startup form) across `crates/cyrup-mcp/src` returns nothing: the per-failure `MCP: Failed to connect to {name}: {sanitized}` double report (`ui.notify(Error)` **and** always … |
| `MCP-026` | low | `hand-written` | **missing** | The `MCP_DIRECT_TOOLS` cache-bootstrap pass inside `initialize` | All of §14's second half: the `__none__` skip, the deliberate re-read of `process.env.MCP_DIRECT_TOOLS` and of the cache from inside `initializeMcp` (not the factory's closure value), the exclusion of servers already connected in the startup pass, the concurrency-10 pass, the `MCP server "{name}" … |
| `MCP-027` | medium | `hand-written` | **partial** | Lifecycle callbacks (reconnect, reconnect-failure, idle shutdown) | No callback is ever installed. `runtime::initialize_mcp` calls none of the three setters (grep for `set_reconnect_callback`/`set_idle_shutdown_callback` outside lifecycle.rs and its tests returns nothing), so the owner-guarded bodies the unit specifies — `updateServerMetadata → updateMetadataCache … |
| `MCP-027a` | medium | `hand-written` | **missing** | `sendMessage`'s `triggerTurn` pre-turn convergence gate **(v2.26.1 retarget, … | Grow `SendMessage` to carry the `triggerTurn` flag (`Fn(String, bool)` or a small options struct), update both `state.rs` structs that hold it and the builder at runtime.rs:188, then implement the gate: with the flag unset deliver synchronously; with it set await … |
| `MCP-028` | medium | `hand-written` | **missing** | `updateServerMetadata` | The whole function: the connection-exists-and-connected guard, the definition-exists guard, the **disabled ⇒ delete from all five maps and return** arm, and otherwise `buildToolMetadata(..., state.toolMetadata)` (the *current* map as collision universe, not the startup snapshot), setting … |
| `MCP-029` | high | `hand-written` | **partial** | `updateMetadataCache` write rules | `updateMetadataCache(state, serverName, {preserveEmptyResources})` itself does not exist — grep for `preserve_empty`, `preserveEmptyResources`, `prompt_discovery_failed`, `serialize_tools`, `serialize_resources`, `serialize_prompts` across `crates/cyrup-mcp/src` returns nothing outside the … |
| `MCP-030` | low | `hand-written` | **partial** | `notifyToolMetadataUpdated` must never let a hook break a connect | Two obligations unmet. (1) No panic containment: the unit requires `std::panic::catch_unwind(AssertUnwindSafe(...))` around the call as the closest analogue of a thrown JS exception — grep for `catch_unwind` across `crates/cyrup-mcp/src` returns nothing, so a panicking hook propagates out of … |
| `MCP-031` | medium | `hand-written` | **missing** | `flushMetadataCache` on shutdown | The real flush: iterate `manager.getAllConnections()` and, for every connection whose status is `connected`, call `updateMetadataCache(state, name)` synchronously (or awaited), then wire it as the `MetadataFlush` passed to `shutdown_state` from `on_session_shutdown`. Depends on MCP-029 and on 13c's … |
| `MCP-032` | low | `host-verb` | **partial** | `updateStatusBar` — the three footer verbosities | The stateful wrapper `updateStatusBar(state)` does not exist. Missing: step 1's unconditional `publishMcpStatusSnapshot(state)` **before** the `!ui` early return, step 2's `if (!state.ui) return`, step 5's `connectedCount` derivation (connections `connected` **and** whose definition exists and is … |
| `MCP-033` | medium | `hand-written` | **missing** | `lazyConnect` | The eight-step algorithm in full: the combined `ownedSignal` + `throwIfAborted`; the four `false` guards in order (`needs-auth`; already-`connected` returning `true` after `updateServerMetadata` + `markKeepAliveAfterConnect`; inside the 60 s failure backoff via `getFailureAgeSeconds`; … |
| `MCP-034` | medium | `hand-written` | **implemented** | `McpLifecycleManager` — the health-check state machine | None for the state machine. Two downstream notes, owned by other units: `start()` has no production caller (`initializeMcp`'s … |
| `MCP-035` | high | `hand-written` | **implemented** | `gracefulShutdown` — memoised, and it waits for the in-flight check | None for this unit. `ManagerSupervisor::close_all` is still the deliberate `Ok(())` no-op pending 13c/MCP-126 (lifecycle.rs:320-327), matching … |
| `MCP-036` | medium | `hand-written` | **partial** | `syncDirectTools`: the fingerprint diff, the re-activation path, and the … | `syncDirectTools`/`syncToolSurface` as a *within-session* operation does not exist. `register_surface` registers everything unconditionally (correct for `init` per MCP-014) but there is no diff: no `added`/`updated`/`deactivated` computation against `registered_direct_tools`, no re-activation path … |
| `MCP-037` | high | `host-addition` | **missing** | HA-1: a native extension has no handle to `ExtensionHost::register_late_tool` | Build one of the two shapes: (i) a defaulted `NativeExtension::set_ext_host(&self, host: Weak<ExtensionHost>)` called from `ExtensionHost::load_native_with_services` beside `set_host_services`, plus a new `ExtensionHost::register_late_command`; or (ii) defaulted `HostServices::{register_late_tool, … |
| `MCP-037a` | critical | `host-addition` | **implemented** | HA-1b: `refresh_tools` drops the native tier's dirty flag in the `wasm-host` … | Minor residual against the verify paragraph: the tests assert `refresh_tools()` reports the change, not the full chain through … |
| `MCP-038` | medium | `host-verb` | **missing** | `deactivateTools`: the optional `unregisterTool` primary path and the … | Implement `deactivate_tools(names)`: empty ⇒ `[]`; cyrup lands on upstream's `unregisterTool === undefined` branch, so go straight to the fallback — `remove = set(names)`, `active = services.active_tools()`; when `None` or empty, add every name to `fallback_deactivated_tools` and return; otherwise … |
| `MCP-039` | medium | `host-addition` | **partial** | MCP prompts as slash commands registered after `init` | The after-`init` half is missing because its host seam is missing (same seam as HA-1): `ExtensionHost` has no `register_late_command` sibling to `register_late_tool` (grep over `crates/cyrup-ext/src` finds only `register_late_tool` at facade.rs:645), and `InitApi` is `&mut` only during `init`. … |
| `MCP-040` | medium | `host-verb` | **missing** | The `/mcp` command handler | The whole handler: the fenced `commandCtx` (owner-fenced services, `commandReload`, `commandHasUI`, the owner's token as signal), the un-timed `await initPromise` preamble with `MCP initialization failed: {message}` / `MCP not initialized`, the arg split (`parts[0]` / `parts[1]` / `rest`), and the … |
| `MCP-041` | medium | `host-addition` | **missing** | HA-2: `/mcp`'s dynamic argument completions have no native path and no TUI … | (a) A defaulted `NativeExtension::argument_completions(&self, command, prefix) -> Vec<(String, String)>` plus a non-`wasm-host` arm on `ExtensionHost::command_completions` routing through the native map the way `execute_native_command` already does, preserving the value/label pair; (b) … |
| `MCP-042` | medium | `host-verb` | **missing** | The `/mcp-auth` command handler | The handler: the same fenced `commandCtx`; **`if (!serverName && !commandCtx.hasUI) return;` silently, before the init-await** (the ordering detail the unit says must survive); the shared init-await/`MCP not initialized` preamble; no-name-with-UI ⇒ the `programmaticConfig` notice or the auth … |
| `MCP-043` | high | `hand-written` | **partial** | The `mcp` gateway tool: registration, the init wait, and the dispatch order | The two are disjoint types and the model reaches only the inert one. `proxy::McpTool::new` is constructed **only in tests** (all hits proxy.rs:5497-5584 are inside `#[cfg(test)]`); the tool actually handed to `api.register_tool` is `registration::ProxyTool`, whose `execute` … |
| `MCP-044` | n/a | `cut` | **not-applicable** | The `mcpScript` tool | Cut 4, and honoured. `mcpScript` is never registered: `registration::register_surface` registers only the prompt commands, the flag, `/mcp`, … |
| `MCP-045` | medium | `host-verb` | **partial** | The `tool_result` `isError` override | The handler arm is not written. `McpExtension::on_event` (extension.rs:535-559) matches `SessionStart`, `Input`, `SessionShutdown` and falls through everything else to `HookOutcome::Noop`, with the comment `// MCP-045 fills the isError override.` at extension.rs:557. Missing: a … |
| `MCP-046` | medium | `hand-written` | **partial** | The abort call-site discipline inside the runtime | The audit this unit *is* cannot pass, because the guarded sites do not exist yet. None of the four `owner.throwIfInactive()` checkpoints inside `initializeMcp` are present — after the startup connect pass, at the top of every pass-two iteration, after the `MCP_DIRECT_TOOLS` bootstrap, and before … |
| `MCP-047` | critical | `hand-written` | **implemented** | Port `agent-plugin-loader.ts` | `crates/cyrup-mcp/src/agent_plugin.rs` (2115 lines) ports the ruleset. Entry points … |
| `MCP-048` | high | `open-decision` | **implemented** | Agent-directory resolution, and whether `~/.pi/agent` is a migration source | Residual, small: the unit's cyrup-it assertion that `cyrup-mcp` and `cyrup-permission-system` resolve the **same** `mcp.json` path for a given … |
| `MCP-049` | medium | `hand-written` | **missing** | Port `cli.js init` as a `cyrup mcp init` subcommand | Add `"mcp"` to the visible `SUBCOMMANDS` table and an `mcp init [--dry-run] [--discover-host-configs]` arm to `dispatch`. Port `runInit`: `findAvailableImports` over the seven families (first existing candidate per family); `loadPiConfig` via `cyrup_permission_system::jsonc` accepting `mcpServers` … |

### 13b · The six-source config ladder

[`13b-mcp-config.md`](13b-mcp-config.md) — 51 units, 6 missing, 13 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-050` | n/a | `extension-owned` | **implemented** | Create `cyrup-mcp` and its config module skeleton | crates/cyrup-mcp/Cargo.toml exists as a workspace member; src/lib.rs carries `#![forbid(unsafe_code)]` and `#![deny(clippy::{unwrap_used, … |
| `MCP-051` | high | `extension-owned` | **implemented** | Read `mcp.json` as JSONC, not JSON | src/config.rs:1527 `parse_json_config(raw, path)` = `cyrup_permission_system::jsonc::parse_config_into::<RawJson>(raw, path, "MCP config")`; … |
| `MCP-052` | high | `hand-written` | **implemented** | Port the six-source precedence ladder | src/config.rs:2578 `ConfigContext::sources()` emits all six rungs with the exact dedup guards (`generic != user_path`; each `.agents` path `!= … |
| `MCP-053` | critical | `hand-written` | **implemented** | Port `mergeServerMaps`, including URL-bound credential stripping | src/config.rs:1702 `URL_BOUND_AUTH_FIELDS: [&str; 4] = ["headers","bearerToken","bearerTokenEnv","requestHeadersCommand"]`; src/config.rs:1738 … |
| `MCP-054` | n/a | `cut` | **not-applicable** | socket ⇄ command/url transport-swap stripping | Cut. `ServerEntry` (src/config.rs:630-751) has no `socket` field, so neither transport-swap rule has an input. The Cut-3 propagation the plan … |
| `MCP-055` | medium | `hand-written` | **implemented** | Port `expandImports` / `mergeImports` | src/config.rs:1841 `merge_imports(left,right)` is concat + first-seen dedup; src/config.rs:2322 `expand_imports(config, home, cwd, diagnostics)` … |
| `MCP-056` | medium | `hand-written` | **implemented** | Port the 7 host-config import families | src/config.rs:157 `ImportKind` has all 7 variants in `IMPORT_PATHS` declaration order with `ALL` (src/config.rs:177) fixing iteration order; … |
| `MCP-057` | medium | `hand-written` | **implemented** | Port the `opencode` multi-file merge and entry translation | src/config.rs:1957 `resolve_opencode_project_candidate` does the two-phase walk (up for `.git` to find gitRoot; then down-to-up from cwd for the … |
| `MCP-058` | medium | `hand-written` | **implemented** | Port `hostConfigDiscovery` and `loadDiscoveredHostConfigs` | src/config.rs:2694 `ConfigContext::merged_settings` walks the ladder and one-level-merges each source's `settings` only; src/config.rs:2713 … |
| `MCP-059` | medium | `hand-written` | **implemented** | Port `getMcpDiscoverySummary`, conflicts and the fingerprint | src/config.rs:3866 `config_source_summaries`, src/config.rs:3891 `mcp_standard_config_summary` (its own narrower `{sources}` fingerprint), … |
| `MCP-060` | low | `hand-written` | **implemented** | Port RepoPrompt detection and `KNOWN_SERVER_PRESETS` | src/config.rs:4196 `known_server_presets()` returns all five with `id`/`name`/`summary`/`entry` byte-matching §15's table (including … |
| `MCP-061` | high | `extension-owned` | **implemented** | Port the atomic raw-config writer | src/config.rs:2891 `write_raw_config_object` does `create_dir_all(parent)`, writes `<path>.<std::process::id()>.tmp`, then `std::fs::rename` — no … |
| `MCP-062` | low | `hand-written` | **implemented** | Port `buildUnifiedDiff` (LCS) and `ConfigWritePreview` | src/config.rs:2975 `build_unified_diff` is the literal bottom-up `(rows+1)×(cols+1)` LCS DP with the addition-preferring tie-break spelled `right >= … |
| `MCP-063` | high | `hand-written` | **implemented** | Port `writeProjectServerDisabledOverride` | src/config.rs:3226 `ConfigContext::write_project_server_disabled_override` writes only the `disabled` key into `<cwd>/.cyrup/mcp.json`, preserves … |
| `MCP-064` | medium | `hand-written` | **implemented** | Port `getServerProvenance` and `writeDirectToolsConfig` | src/config.rs:3402 `ConfigContext::server_provenance` runs the three passes: host families (only when discovery is `On`, in `ImportKind::ALL` order, … |
| `MCP-065` | low | `hand-written` | **implemented** | Port `ensureCompatibilityImports`, starter config and shared-entry writers | src/config.rs:3510 `ensure_compatibility_imports` returns `added: []` and does not write when nothing is added; src/config.rs:3487 … |
| `MCP-066` | high | `hand-written` | **implemented** | Port `McpSettings` as a permissive struct with per-site defaults | src/config.rs:804 `McpSettings` carries 23 `Option<T>` fields (§5's 22 live keys after the `scriptMode` cut, plus v2.26's `warnOnLargeDirectTools`), … |
| `MCP-067` | medium | `hand-written` | **implemented** | Port the settings merge as a one-level key merge | Nothing behavioural. (Consequence of rule 4, already documented: a wrong-typed `settings.trace` in a higher-precedence file becomes `None` and … |
| `MCP-068` | high | `hand-written` | **partial** | Port env-var overrides, including the `__none__` sentinel | Three obligations unmet. (1) `MCP_UI_DEBUG` has no reader — the string does not appear anywhere in crates/cyrup-mcp/src (grep over src/*.rs: zero hits), so the logger level bootstrap does not exist. (2) `BROWSER` has no reader — it appears only in doc comments at src/oauth.rs:2534 and … |
| `MCP-069` | high | `hand-written` | **implemented** | Port `ServerEntry` as a typed struct | REFUTED. MCP-069's only obligation about this message is 'The exactly-one-transport message loses `, or socket`'. Upstream is `Server ${name} must … |
| `MCP-069a` | critical | `hand-written` + `open-decision` | **not-applicable** | Fail **closed** on a malformed `requestHeadersCommand` **(v2.26.1 retarget, … | NOT-APPLICABLE by verdict class. The canonical table gives MCP-069a the verdict `hand-written` + `open-decision`, and the plan text itself says … |
| `MCP-070` | high | `hand-written` | **partial** | Enforce the absent-vs-null hash pre-image contract | Three things stop this from being a working contract. (1) Every production caller hashes UNRESOLVED values: src/ui.rs:1758 uses `ResolvedIdentity::verbatim(definition)`, whose own doc at src/dirs.rs:687-694 says it is a placeholder "until MCP-082 and MCP-084 land" and is "wrong, silently" for any … |
| `MCP-071` | high | `hand-written` | **implemented** | Port `ToolPrefix` with all four modes and `sanitizeServerPrefix` | src/registration.rs:184 `sanitize_server_prefix(server_name, preserve_provider_valid)` iterates `chars()` (code points), keeps `[A-Za-z0-9_-]` when … |
| `MCP-072` | high | `hand-written` | **implemented** | Port `formatToolName` / `resolveToolPrefix` | src/registration.rs:235 `format_tool_name` does `tool_name.replace('.', "_")` only — hyphens survive — and joins with `{prefix}_{sanitized}` or … |
| `MCP-073` | high | `hand-written` | **missing** | Port `resolveServerFromToolName` with its ambiguity fail-safe | A `pub fn resolve_server_from_tool_name(tool_name, server_names, prefix) -> Option<String>` on `cyrup-mcp`: `None` for `ToolPrefix::None`; collect every configured server whose non-empty `server_prefix` satisfies `tool_name.starts_with(prefix + "_")`; sort by prefix length descending; return `None` … |
| `MCP-074` | medium | `hand-written` | **implemented** | Port `sanitizePromptName` / `formatPromptCommandName` | src/registration.rs:517 `sanitize_prompt_name` replaces each `[^A-Za-z0-9_-]+` run with one `_`, trims leading/trailing `[_-]`, returns `"prompt"` … |
| `MCP-075` | high | `hand-written` | **partial** | Port `getToolNameCandidates` (the legacy candidate set) | The second copy is wrong. src/proxy.rs:486 `format_legacy_tool_name` derives the legacy prefix by taking `get_server_prefix(server_name, prefix)` — which has ALREADY applied the preserve-provider-valid grammar — and then re-escaping every non-alphanumeric character of that result, instead of … |
| `MCP-076` | high | `hand-written` | **partial** | Port glob matching and `isToolIncluded`/`isToolExcluded`/`isToolAllowed` | src/registration.rs:334 compiles with bare `Regex::new(&out).ok()`, without the `RegexBuilder::size_limit` / `dfa_size_limit` ceilings the unit explicitly requires. The proxy copy does set them (src/proxy.rs:583-587 via `REGEX_SIZE_LIMIT`/`REGEX_DFA_SIZE_LIMIT`), so the two glob compilers differ in … |
| `MCP-077` | high | `hand-written` | **implemented** | Port the metadata/cache type model | src/dirs.rs:385-537 carries the writer-side model: `CACHE_VERSION = 1`, `CACHE_MAX_AGE_MS = 7 days`, `CachedTool` (with `ui_resource_uri` / … |
| `MCP-078` | medium | `extension-owned` | **partial** | Port the status-snapshot types | The snapshot's SHAPE was not ported. Present: four `Vec<String>` fields (`connected`, `idle`, `failed`, `pending_auth`). Absent: `MCP_STATUS_SNAPSHOT_VERSION = 1` as a constant (grep for `SNAPSHOT_VERSION` over src/*.rs: zero hits); the closed 6-variant `McpServerRuntimeStatus` … |
| `MCP-079` | medium | `hand-written` | **partial** | Port the tool-approval decision and origin types | Two obligations unmet. (1) `McpToolApprovalDecision` — the four-arm `allow_once \| allow_for_session \| deny \| abstain` enum — does not exist; grep for those spellings over crates/cyrup-mcp/src returns nothing. The nearest type, `ApprovalOutcome { Approved, Denied, NoInteractiveSession }` … |
| `MCP-080` | n/a | `cut` | **not-applicable** | MCP-UI type surface in `types.ts` | Cut 2. No bridge-protocol or `ui://`-envelope type is exported from the crate: grep over crates/cyrup-mcp/src for `UiResourceMeta`, … |
| `MCP-081` | medium | `hand-written` | **implemented** | Port `McpAdapterOptions` / programmatic config mode | src/extension.rs:144 `McpExtension::with_config(dirs, programmatic_config)`; src/extension.rs:453 `init` short-circuits the whole ladder when a … |
| `MCP-082` | high | `hand-written` | **implemented** | Port `interpolateEnvVars` including the `{env:VAR}` form | src/credentials.rs:3322 `interpolate_env_vars_with(value, lookup)` runs three CHAINED `replace_all` passes in upstream's order over `\$\{(\w+)\}`, … |
| `MCP-083` | critical | `extension-owned` | **partial** | Port `!` / `!!` command-secret resolution | Two obligations unmet. (1) `resolveCommandSecretsRecord` — the per-record form applied to `env` and `headers` — does not exist; grep over crates/cyrup-mcp/src for `resolve_command_secrets_record` / `interpolate_env_record` returns nothing, and src/runtime.rs:608-612 and :701-703 explicitly defer it … |
| `MCP-084` | high | `hand-written` | **partial** | Port `resolveServerUrl` / `resolveConfigPath` / `resolveBearerToken` | `resolveServerUrl` is not implemented anywhere. Grep over crates/cyrup-mcp/src for each of its three exact strings — `MCP server URL must be a string`, `Missing environment variable`, `Invalid MCP server URL after` — returns zero hits, and `getMissingEnvVars`'s combined-alternation scan has no … |
| `MCP-085` | medium | `hand-written` | **partial** | Port terminal sanitisation and error flattening | `formatTerminalError` is not ported. No function walks an error's children/`source()` chain with a cycle guard, falls back to the aggregate's own message when the nested walk yielded nothing, de-duplicates and joins with `": "`, then sanitises. `CleanupErrors`'s `Display` (src/errors.rs:250-262) … |
| `MCP-086` | medium | `extension-owned` | **partial** | Port the browser/path open dispatch | `openUrl`/`execOpen`'s browser arm is missing. `BROWSER` is never read — grep over crates/cyrup-mcp/src finds it only in prose at src/oauth.rs:2534 and src/ui.rs:3062, both of which state the dispatch belongs elsewhere. So the macOS `.app`-vs-executable distinction (`exec(browser,[target])` when … |
| `MCP-087` | medium | `hand-written` | **partial** | Port `parallelLimit`, argv scan, `toStringRecord`, … | `parallelLimit` is not ported. No order-preserving bounded-concurrency helper exists: grep over crates/cyrup-mcp/src for `parallel_limit`, `buffered(`, `buffer_unordered` and `JoinSet` returns nothing. The only bounded fan-out is src/lifecycle.rs:947 … |
| `MCP-088` | medium | `host-verb` | **implemented** | Port `formatMcpStatus` and `formatAuthRequiredMessage` | src/ui.rs:4636 `format_mcp_status(config, message)` returns `None` when `mcp_footer_status() == FooterStatus::Off` and otherwise prefixes `"\u{1f50c} … |
| `MCP-089` | medium | `hand-written` | **partial** | Port the error taxonomy | The unit's named obligations are all unmet. There is no `fn code(&self) -> &'static str` and no `fn recovery_hint(&self) -> &'static str` on `McpError` (grep over src/*.rs for `fn code(` / `recovery_hint`: only the errors.rs doc comment). There is no `ConsentError` arm — neither the … |
| `MCP-090` | low | `extension-owned` | **partial** | Port the logger as a `tracing` adapter | The user-facing contract the unit says to keep is absent. `MCP_UI_DEBUG` is never read — grep over crates/cyrup-mcp/src returns zero hits — so the level bootstrap (`"1"`/`"true"` ⇒ debug) does not exist. There is no `[MCP-UI…]` prefix or stable `tracing` target (grep `[MCP-UI`: zero hits), no … |
| `MCP-091` | medium | `hand-written` | **missing** | Port `renderTsShape` | The whole of ts-shape.ts: the `try/catch → None` envelope, `UNSUPPORTED_KEYWORDS` with the `additionalProperties: false` exemption re-tested at every node, the `$defs`/`definitions` collection with `~1`/`~0` pointer-token decoding, `aliasFor`'s bare-name reuse and `Definition{n}` fallback, the … |
| `MCP-092` | high | `hand-written` | **missing** | Port the dual-dialect JSON Schema validator | The whole dialect gate: `schemaDialect(schema)` (no string `$schema` ⇒ unstamped; else strip ONE trailing `#`), routing unstamped and `https://json-schema.org/draft/2020-12/schema` to a 2020-12 validator and the two draft-07 URIs to a draft-07 validator, the exact `Unsupported JSON Schema dialect: … |
| `MCP-093` | medium | `hand-written` | **missing** | Register the `ajv-formats` formats `jsonschema` does not ship | `ValidationOptions::with_format(name, fn)` for each format `ajv-formats` supplies beyond `jsonschema`'s built-ins (`url, int32, int64, float, double, byte, binary, password, iso-time, iso-date-time, json-pointer-uri-fragment`), registered on BOTH builders, plus the test that enumerates … |
| `MCP-094` | high | `hand-written` | **missing** | Reconcile `mcp_direct_tools` with this section's contract | One scheduled change to crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs covering all nine indexed divergences above, plus the shared conformance suite the unit names: one golden `mcp.json` + `mcp-cache.json` pair asserted identically by `cyrup-mcp` and by `mcp_direct_tools`'s resolver. … |
| `MCP-095` | n/a | `extension-owned` | **implemented** | JSONC parser home | Settled as the plan directs. crates/cyrup-mcp/Cargo.toml declares `cyrup-permission-system = { workspace = true }` with the reasoning verbatim; … |
| `MCP-096` | high | `open-decision` | **not-applicable** | Project trust and the two project-scoped config sources | REFUTED / NOT-APPLICABLE. The canonical table gives MCP-096 the verdict `open-decision`, and the plan's own words are 'This is the only genuine open … |
| `MCP-097` | low | `hand-written` | **implemented** | Port `getConfigDiscoveryPaths` and `findAvailableImportConfigs` | src/config.rs:3835 `ConfigContext::config_discovery_paths` maps `sources()` to `{label, path, exists}` using only `read_path.exists()` — it never … |
| `MCP-098` | medium | `hand-written` | **missing** | Preserve `renderTsShape`'s re-entrant alias emission | When MCP-091 is written, its output loop must be an index-based `while i < aliases.len() { … i += 1 }` over a growing `Vec<(String,String)>` (or `IndexMap`) so a `$ref` registered by `render(definition)` inside the loop is itself visited and emitted, and insertion order is preserved. The golden … |
| `MCP-099` | low | `hand-written` | **implemented** | Reproduce `buildConfigWritePreview`'s reserialised "before" text | src/config.rs:3056 `build_config_write_preview` computes `before_text` as `serialize_raw_object(&read_raw_config_object(path))` when the file exists … |

### 13c · Server manager, transports, metadata cache

[`13c-mcp-servers.md`](13c-mcp-servers.md) — 51 units, 20 missing, 20 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-100` | high | `hand-written` | **missing** | McpServerManager: the five race guards and the full public API | The entire manager is unbuilt. Absent: `ServerConnection` (client/transport/definition/tools/resources/prompts/promptDiscoveryFailed/instructions/lastUsedAt/inFlight/status/credentialsInvalidated); the seven manager-owned maps (`connections`, `connectPromises`, `reconnectPromises`, `closePromises`, … |
| `MCP-101` | high | `rmcp` | **partial** | stdio transport: spawn, env resolution, cwd, plugin data dir | No `resolveEnv` and no connection builder. Specifically absent: (a) `args = (definition.args ?? []).map(interpolateEnvVars)` — grep for any arg-interpolation call site returns nothing; (b) `resolveEnv(env, serverName, literalEnv)` — the full-process-env copy, the `literalEnv === true` verbatim arm, … |
| `MCP-102` | medium | `rmcp` | **partial** | stderr tail capture and failure-message enrichment | The pure functions are done; the live wiring is not. No production task reads the returned `ChildStderr` into a tail — the only readers are the unit tests (runtime.rs:1670-1700). And the enrichment call site (`createConnection`'s catch appending `(detail)` to the connect failure) does not exist … |
| `MCP-103` | medium | `extension-owned` | **missing** | Wire npx/npm resolution into the connection builder | `resolve_npx_binary` is never called from cyrup-mcp. Needed: the `pub` promotion + re-export in `cyrup_ext::caps::proc`, and the call in the (not-yet-existing) connection builder applying `command = resolved.is_js ? "node" : resolved.bin_path` / `args = is_js ? [bin_path, ...extra_args] : … |
| `MCP-104` | medium | `hand-written` | **missing** | npx cache: bump to CACHE_VERSION = 2 and port clearLegacyCache | `CACHE_VERSION` must become 2, and `clear_legacy_cache() -> bool` must be added (unlink, falling back to `write("")`), invoked once at module load via `std::sync::Once` inside `load_cache` **and** on every `load_cache()`, returning `None` when it evicted. |
| `MCP-105` | high | `hand-written` | **missing** | npx resolver: exact package-version pinning is missing | Add `ParsedPackageSpec { package_name, exact_version: Option<String> }` and `parse_package_spec` (the `@scope/name` `rfind('@') > find('/')` rule, `^=` then case-insensitive `^v` strip, validated against upstream's exact-semver regex); thread `exact_version` into the cache-hit predicate (`!exact … |
| `MCP-106` | low | `hand-written` | **missing** | npx resolver: cache key must be [command, packageSpec, binName] | Move the computation after the parse and serialise `[command, &parsed.package_spec, parsed.bin_name.as_deref().unwrap_or("")]`. Today two invocations differing only in trailing args, and `npx pkg bin` vs `npx --package pkg bin`, occupy different cache slots. |
| `MCP-107` | medium | `hand-written` | **missing** | npx resolver: no cancellation path | Add a `cancel: &cyrup_core::CancelToken` parameter with `throw_if_aborted`-equivalent checks on entry, on `force_npx_cache` entry and on its exit, and a `cancel.is_cancelled()` check in the 50 ms poll loop that kills and reaps the child. A session shutdown currently cannot interrupt up to 30 s of … |
| `MCP-108` | low | `hand-written` | **missing** | npx resolver: entry-level cache validation and Windows npm resolution | Deserialise `entries` as `HashMap<String, serde_json::Value>` and convert per entry (dropping failures, including a non-finite/absent `resolvedAt` and a non-string `packageVersion`). For Windows, resolve `npm.cmd`/`npm.exe` via a PATH+PATHEXT walk or invoke through `cmd /c npm` at both call sites. |
| `MCP-109` | high | `rmcp` | **partial** | Streamable HTTP client transport | Nothing constructs the transport in production: `build_http_transport_config` and `http_transport_with_client` have only test callers (runtime.rs:1733/1756) and a doc reference from request_headers_command.rs:51. No `HttpTransportSpec` is ever populated because `resolveServerUrl`/header resolution … |
| `MCP-110` | n/a | `cut` | **not-applicable** | Legacy HTTP+SSE transport and the shouldFallbackToSse ladder | The load-time diagnostic string in config.rs:1615-1618 is a *paraphrase* ("...requests `httpTransport: \"sse\"`; rmcp ships no SSE client transport … |
| `MCP-111` | n/a | `cut` | **not-applicable** | Unix-domain-socket transport | Cut 3 by owner decision. `ServerEntry` (/home/user/cyrup/crates/cyrup-mcp/src/config.rs:630+) has no `socket` field — grepping config.rs for `pub … |
| `MCP-112` | n/a | `rmcp` | **implemented** | MCP NDJSON framing | Nothing to write, per the plan. /home/user/cyrup/crates/cyrup-mcp/src/runtime.rs:410-411 imports `rmcp::transport::TokioChildProcess` and … |
| `MCP-113` | medium | `hand-written` | **implemented** | Transport selection and mutual exclusion | `select_transport` has no production caller yet (only runtime.rs tests) because the connection builder (MCP-100) does not exist. The selection logic … |
| `MCP-114` | high | `extension-owned` | **partial** | HTTP header, bearer and command-secret resolution | The `connectHttpClient` pre-flight (§3.4 steps 1-7) does not exist. Specifically absent, verified by grepping all 19 files: (a) `resolveServerUrl` — no `resolve_server_url` symbol, and neither of its two user-visible throws (`Missing environment variable{s} in MCP server URL: ...` with … |
| `MCP-115` | high | `hand-written` | **partial** | Implicit-vs-explicit OAuth provider state machine and the attempt loop | The attempt loop itself is absent — there is no `connect_http_client` anywhere in the crate. Missing: the per-attempt fresh client; the transport-options assembly including `skipIssuerMetadataValidation` (the config field exists at config.rs:1311 but has no consumer); the once-only per-attempt … |
| `MCP-115a` | high | `hand-written` | **missing** | Wire the per-request header command into `connectHttpClient` **(v2.26.1 … | Two concrete work items. (1) The wiring: build `RequestHeadersCommandClient::new(client, cfg, ct)?` **inside** MCP-115's retry closure and pass it to `http_transport_with_client(..., config)`, so it survives the implicit-OAuth retry the way upstream's `requestFetch` does. Blocked by … |
| `MCP-116` | high | `hand-written` | **missing** | needs-auth connection state and one-shot credential invalidation | No connection record carries `credentials_invalidated`, so `connect`'s step-7 carry-forward (`existing?.status == "needs-auth" && existing.credentialsInvalidated === true`) does not exist, nor does either needs-auth exit (the HTTP ladder's own and `createConnection`'s catch-path downgrade), nor the … |
| `MCP-117` | medium | `rmcp` | **implemented** | Protocol-revision negotiation | The open decision was resolved as option **(a)**: rmcp's `ClientLifecycleMode` is adopted as-is and the disposable-sibling mechanism is *not* … |
| `MCP-118` | medium | `rmcp` | **implemented** | Client capability advertisement (sampling / elicitation form+url) | One recorded, unavoidable divergence documented at runtime.rs:885-891: rmcp's `InitializeRequestParams::capabilities` is not an `Option`, so the port … |
| `MCP-119` | high | `rmcp` | **missing** | Paginated discovery with capability gating and per-list failure policy | No discovery at all. Needed: the unconditional `list_all_tools` with errors propagating; the `resources`/`prompts` capability gate read from `RunningService::peer_info() -> InitializeResult.capabilities`; the per-list failure policy (abort and 401 re-throw; resources → silent `[]`; prompts → … |
| `MCP-120` | medium | `rmcp` | **partial** | list_changed refresh with identity guards | Only the notification plumbing exists. The `ListChangedHook` (runtime.rs:1051) is a type alias with no production implementation, so the actual §3.10 body is absent: the identity check against the live connection map, the `status == connected` check, the re-call of `list_all_*`, the wholesale field … |
| `MCP-121` | n/a | `cut` | **not-applicable** | Adapter-private UI stream-patch notification handler | Cut 2 by owner decision, and the cut is honoured with the exact behaviour the unit's verify line asks for: … |
| `MCP-122` | medium | `hand-written` | **partial** | URL-elicitation acceptance tracking and completion notice | Everything the manager owns is missing. Verified by grep across all 19 files: no `acceptedUrlElicitations` registry (`Mutex<HashMap<String, HashSet<String>>>`), no `remember_url_elicitation` (and therefore no `runtimeSignal.aborted` no-op rule), no `Set.delete`-returned-true gate, and the notice … |
| `MCP-123` | medium | `rmcp` | **partial** | Connect-time abort and once-only transport cleanup | The residual adapter policy is not written. Absent: the once-only cleanup handle for the HTTP retry ladder — grepped for `futures::future::Shared`, `BoxFuture<'static, Result<(), Arc<`, `abort_cleanup` across all 19 files, no hits — and the cleanup-failure-vs-connect-failure *distinction*, since … |
| `MCP-124` | high | `hand-written` | **partial** | Error taxonomy and containsCleanupFailure | Add the five variants (`AbortCleanupFailed`, `SetupFailed`, `HttpCleanupFailed`, `ConnectionCleanupFailed`, `ManagerCleanupFailed`) with `Display` rendering the byte-exact heads `MCP connection abort cleanup failed`, `MCP connection setup failed`, `MCP HTTP connection cleanup failed`, `MCP … |
| `MCP-125` | high | `hand-written` | **missing** | reconnect: guards, single-flight, identity, in-flight preservation | Missing entirely: the disabled and stopped guards firing **before** the single-flight map is consulted (with `MCP server "<n>" is disabled` / `MCP server manager is closed` — neither string exists in the crate); `reconnect_promises: Mutex<HashMap<String, Shared<BoxFuture<..>>>>` with … |
| `MCP-126` | high | `hand-written` | **missing** | close / closeAll: generations, attempt aborts, late-name sweep | Nothing of §3.12 exists: the generation bump, the `connect_attempts[name].cancel()` with the `MCP connection <n> was closed` reason, removal from the map **before** awaiting cleanup, the no-connection path that awaits a pending close or re-throws only cleanup failures from a pending connect, … |
| `MCP-127` | medium | `hand-written` | **missing** | Idle and in-flight accounting | No connection carries `last_used_at` or `in_flight`, so none of `touch` / `increment_in_flight` / `decrement_in_flight` (floor at zero) / `is_idle` (connected AND zero in-flight AND strict `now - last_used_at > timeout`) exists, and the RAII guard the plan specifies is unwritten. `lifecycle.rs`'s … |
| `MCP-128` | medium | `rmcp` | **partial** | Request options: timeout normalisation and owned signal | The manager-side half is missing: `setDefaultRequestTimeoutMs` (which normalises on the way in) and the public `getRequestOptions(name, signal?)` that resolves the definition **by name from the live connection map** — neither exists, because the manager does not. The cancellation half is also … |
| `MCP-129` | medium | `rmcp` | **missing** | getPrompt / readResource accounting and disabled re-check | The manager's `getPrompt` and `readResource` do not exist. Missing: the `status == connected` precondition with the exact `Server "<n>" is not connected` message; `touch → incrementInFlight → … → finally { decrementInFlight; touch }` (touch **twice**); `readResource`'s live-definition … |
| `MCP-130` | medium | `hand-written` | **missing** | Startup connect concurrency limit | There is no startup connect path at all, so the concurrency bound has nothing to bound. When the startup connect lands it must be `futures::stream::iter(..).map(..).buffered(10).collect()` (limit 10, output in config order). |
| `MCP-131` | high | `rmcp` | **partial** | Child-process cleanup and orphan avoidance | Nothing in production ever owns or closes a `TokioChildProcess`: `spawn_stdio_transport` has only test callers, `ManagerSupervisor::close`/`close_all` are no-ops (lifecycle.rs:307-322), and grep finds no `graceful_shutdown()` call on a transport anywhere in cyrup-mcp. The non-orphan property … |
| `MCP-132` | medium | `extension-owned` | **missing** | MCP endpoint probe (three-strategy ladder) | The entire three-strategy ladder is unwritten: the seven constants, the `modern` / `legacy-post` / `legacy-sse` request shapes (exact headers, bodies, 5 s timeout, unauthenticated, all three against the same URL), `classifyResponse`'s five rungs, `jsonRpcEnvelopeInfo`, `isBearerChallenge` … |
| `MCP-133` | medium | `hand-written` | **missing** | Probe-enriched HTTP connect failures | Missing: the URL-only wrapping of the connect future, the exact ` — probe: ` separator (space, em-dash, space), preserving the original error as `cause`, and the swallow-all rule (any probe failure — including a `resolveServerUrl` throw on the re-resolve — returns the original error unchanged). … |
| `MCP-134` | high | `rmcp` | **missing** | isTerminatedSession predicate | The predicate does not exist. Needed: the absolute `hadSessionId` gate captured **before** the call; the 404 arm (which `StreamableHttpError::SessionExpired` supplies once an HTTP transport exists); the 400 arm requiring both `"code"\s*:\s*-32000` and `"message"\s*:\s*"Bad Request: Server not … |
| `MCP-135` | high | `hand-written` | **missing** | withSessionRecovery retry wrapper | The whole wrapper is absent: the disabled/not-connected preconditions, `hadSessionId` captured before the call, the **live** config re-read after the failure, the 401 credential-cache invalidation running **before** the `isTerminatedSession` gate, the exactly-one retry, the `onNeedsAuth` hook, … |
| `MCP-136` | n/a | `hand-written` | **not-applicable** | Tracker: what survives a restart | The one live issue this tracker names — agent-dir resolution — is still open and is owned by MCP-139: `cyrup-mcp` takes an already-resolved … |
| `MCP-137` | medium | `hand-written` | **missing** | Status snapshot construction | `createMcpStatusSnapshot` does not exist. Needed: `MCP_STATUS_SNAPSHOT_VERSION = 1`, `FAILURE_BACKOFF_MS = 60_000`, `getActiveFailureAgeSeconds` (falsy `failedAt` → absent; `ageMs > 60_000` → absent; else `round(ageMs/1000)`), the per-server six-key object with `resourceCount`/`failedAgoSeconds` … |
| `MCP-138` | low | `extension-owned` | **partial** | Publish the status snapshot | The channel only ever carries a default value: the sole production publishers are `runtime.rs:234` and `lifecycle.rs:1559`, both `McpStatusSnapshot::default()`. The payload type is also the wrong shape (see MCP-137), so a consumer reading it gets four empty vectors. No shutdown snapshot equivalent … |
| `MCP-139` | high | `hand-written` | **partial** | Metadata cache: path, schema, version, load and merge-save | Three real gaps. (1) **The agent-dir consolidation did not happen.** `npx_resolver::agent_dir` (/home/user/cyrup/crates/cyrup-ext/src/caps/proc/npx_resolver.rs, anchored on `caps::proc::host_home_dir`) and `mcp_direct_tools::resolve_agent_dir`/`home_dir` … |
| `MCP-140` | high | `hand-written` | **partial** | Metadata cache: serialisers and reconstructors | The **serialisers are absent**. Nothing converts a live MCP tool/resource/prompt list into `CachedTool`/`CachedResource`/`CachedPrompt`: grepping all 19 files for a `ServerCacheEntry {` construction outside tests returns nothing (only dirs.rs:1254/1342, registration.rs:1981, ui.rs:5049 — all … |
| `MCP-141` | critical | `hand-written` | **partial** | computeServerHash must hash all 14 fields; the in-tree reader hashes 11 | Three gaps, each independently fatal to the contract. (1) **The reader was not upgraded.** `cyrup_ext_subagents::exec::mcp_direct_tools::compute_mcp_server_hash` (/home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:531-584) still hashes **11** keys — `protocolVersion`, … |
| `MCP-142` | critical | `hand-written` | **implemented** (wave 1) | stableStringify emits the bare token `undefined`, not `null` | The **reader still emits `null`**: `mcp_direct_tools::stable_stringify` (/home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:796-825) maps `Value::Null => "null"`, and every absent field is materialised as `Value::Null` by `opt_str_value` (:587-589), by … |
| `MCP-143` | high | `hand-written` | **partial** | interpolateEnvVars is missing its third pattern {env:NAME} | Both in-tree copies the unit names are unchanged. (1) `cyrup_ext::caps::proc::interpolate_env_vars_with` (/home/user/cyrup/crates/cyrup-ext/src/caps/proc.rs:148-156) is still `interpolate_braces` (:157) + `interpolate_dollar_env` (:180) — two patterns; grepping proc.rs for `{env:` returns nothing. … |
| `MCP-144` | high | `hand-written` | **partial** | !/!! secret-expression semantics in hashed values | Two call sites still bypass it. (1) `mcp_direct_tools::interpolate_env_record` (/home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:696-712) calls plain `interpolate_env_vars` and additionally **drops non-string values** (`if let Some(text) = value.as_str()` at :706) where … |
| `MCP-145` | high | `hand-written` | **partial** | isServerCacheValid including the throw-to-false rule | The throw arm is unreachable in practice: no fallible hasher is ever installed (`install_server_hasher` at registration.rs:754 has no production caller, and registration.rs:746-752 documents that without one the hash comparison is **skipped entirely**), and `compute_server_hash` cannot fail because … |
| `MCP-146` | critical | `hand-written` | **implemented** (wave 1) | Resource tool naming: read_ upstream vs get_ in the in-tree reader | The reader was **not** changed: `cyrup_ext_subagents::exec::mcp_direct_tools::resolve_direct_tool_names` still builds `format!("get_{}", resource_name_to_tool_name(name))` at /home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs:466, and the test at :1044 still asserts … |
| `MCP-147` | medium | `hand-written` | **implemented** | Direct-tool selector parsing and the missing-server gate | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs:835-853 `parse_direct_tool_selectors` strips trailing `/`, then for a slash-bearing selector … |
| `MCP-148` | n/a | `rmcp` | **implemented** | The protocol layer is rmcp, client-only | /home/user/cyrup/crates/cyrup-mcp/Cargo.toml declares exactly the settled set: `rmcp = { version = "3.1.2", default-features = false, features = … |
| `MCP-149` | n/a | `hand-written` | **not-applicable** | Tracker: section 03 index and cross-section edges | No work item of its own; the three out-of-crate changes it indexes are all still outstanding and are filed under MCP-103, MCP-139 and … |

### 13d · Proxy modes

[`13d-mcp-proxy-modes.md`](13d-mcp-proxy-modes.md) — 36 units, 1 missing, 4 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-151` | high | `host-verb` | **implemented** | Register the `mcp` tool with the exact JSON Schema | None for the unit's obligation. Code-health note only: the schema literal exists twice (`proxy::mcp_tool_schema` and … |
| `MCP-152` | high | `hand-written` | **implemented** | Port `buildProxyDescription` and re-register on change | The hand-written half of MCP-152 is complete and tested. The description is built with `write!`-style assembly in TWO byte-identical copies … |
| `MCP-153` | high | `hand-written` | **implemented** | Port mode dispatch: precedence, args coercion, init gate | Port complete, but the ported dispatcher is not reachable in production: the tool actually registered is `registration::ProxyTool`, whose `execute` … |
| `MCP-154` | medium | `hand-written` | **implemented** | Port `executeStatus` | proxy.rs:1811 `execute_status`: the six-rung ladder (disabled → connected → needs-auth → failed → cached → not connected) over … |
| `MCP-155` | medium | `hand-written` | **implemented** | Port `executeList` | proxy.rs:1912 `execute_list`: `not_found` with empty `tools`/`count`, `disabled_result("list", …)`, the 300-char preview via … |
| `MCP-156` | low | `hand-written` | **implemented** | Port `executeInstructions` | proxy.rs:2013 `execute_instructions` checks `not_found` → `server_disabled` → cached instructions (`<s> instructions:\n\n<text>` with … |
| `MCP-157` | medium | `hand-written` | **implemented** | Port `executeDescribe` | proxy.rs:2060 `execute_describe`. Verified line-by-line against upstream /home/user/cyrup/tmp/pi-mcp-adapter/proxy-modes.ts `executeDescribe`: … |
| `MCP-158` | high | `hand-written` | **implemented** | Port `executeSearch` match selection | proxy.rs:2171 `execute_search` selection half, diffed against upstream proxy-modes.ts `executeSearch`. Disabled-`server` short-circuit first (:2192); … |
| `MCP-159` | medium | `hand-written` | **implemented** | Port the regex search path onto a linear-time engine | Every item in MCP-159's **verify** list exists. (1) The re-specified catastrophic-backtracking case: proxy.rs:5471-5481 compiles `(a+)+$`, runs it, … |
| `MCP-160` | medium | `hand-written` | **implemented** | Port `executeSearch` rendering, pagination footer and connecting hint | proxy.rs:2276-2420 rendering half, diffed byte-for-byte against upstream proxy-modes.ts. Zero results: `connecting` computed as `[server]` iff … |
| `MCP-161` | high | `hand-written` | **implemented** | Port `executeConnect` | Runs only against `ProxyEnv`'s `connect`/`reconnect`; the trait has no production implementor (see MCP-164), so this mode is exercised solely by … |
| `MCP-162` | high | `hand-written` | **implemented** | Port `attemptAutoAuth` and the single-shot latch | proxy.rs:2611 `attempt_auto_auth` reproduces the ladder: `settings.auto_auth()` opt-in, missing/disabled/non-OAuth ⇒ Skipped, `resolve_server_url` … |
| `MCP-163` | critical | `hand-written` | **implemented** | Port `executeCall`'s resolution state machine (phases 1-5) | proxy.rs:2949-3283 `execute_call` phases 1-5. Fail-closed resolution is present and is the sentinel form, not first-match: `SingleMatch` … |
| `MCP-164` | high | `hand-written` | **partial** | Port `executeCall`'s invocation paths and result shaping | The rmcp invocation itself does not exist. `ProxyEnv::call_tool` (proxy.rs:1397) and `ProxyEnv::read_resource` (:1410) are declared with doc comments naming `Peer::send_request_with_option(...)` → `RequestHandle::cancel(reason)` and `Peer::read_resource`, but a workspace grep for … |
| `MCP-165` | medium | `hand-written` | **implemented** | Port `executeCall`'s error taxonomy | No behavioural test exercises any of the three arms. `grep -n 'url_elicitation\|SessionRecoveryAuthRequired\|ProxyCallError'` over the test module … |
| `MCP-167` | medium | `hand-written` | **implemented** | Port `executeAuthStart` and `formatManualAuthInstructions` | proxy.rs:2443 `format_manual_auth_instructions` builds the literal vector and applies `.filter(Boolean)` semantics via `lines.retain(\|l\| … |
| `MCP-168` | medium | `hand-written` | **implemented** | Port `executeAuthComplete` | The unit's two named unit tests are absent: nothing asserts all three input keys are accepted, and nothing asserts a non-`"authenticated"` status … |
| `MCP-169` | high | `hand-written` | **implemented** | Freeze the `details.error` vocabulary as a conformance table | proxy.rs:180 `#[non_exhaustive] #[serde(rename_all = "snake_case")] enum McpErrorCode` with all 32 variants, `McpErrorCode::ALL` (:249) as a … |
| `MCP-170` | high | `extension-owned` | **implemented** | Use insertion-ordered maps for servers and metadata | Integration note (not this unit's gap): `ProxyCtx` holds its own `tool_metadata` separate from `McpState::tool_metadata`, documented at … |
| `MCP-171` | low | `open-decision` | **implemented** | Decide the `localeCompare` tie-break | None functionally. Performance note worth a follow-up: `locale_compare` constructs a fresh `feruca::Collator` on every comparison (config.rs:3791 … |
| `MCP-172` | high | `hand-written` | **implemented** | Port `normalizeSearchText` and `tokenize` | proxy.rs:767 `normalize_search_text` is a hand-written char scanner doing the three steps in order — camelCase split before lowercasing, advancing … |
| `MCP-173` | high | `hand-written` | **implemented** | Port `scoreToolMatch` field scoring | proxy.rs:902 `score_tool_match` with `WEIGHT_NAME/ORIGINAL_NAME/SERVER/DESCRIPTION = 12/10/8/5` (proxy.rs:118-124) and `MIN_STEM_LENGTH = 4` (:115). … |
| `MCP-174` | medium | `hand-written` | **partial** | Port keyword scoring and `resolveSearchKeywords` | One malformed-config divergence remains. Upstream's `resolveSearchKeywords` skips only the offending key (`if (!Array.isArray(values)) continue;`), but cyrup deserialises `search_keywords` with `#[serde(deserialize_with = "lenient")]` (config.rs:715), and `lenient` (config.rs:479) drops the … |
| `MCP-175` | high | `hand-written` | **implemented** | Port the coverage gate and final bonuses | proxy.rs:962-987. `full_coverage` is the integer comparison `matched == total`, never a float equality; the gate is `if !phrase_matched && (if total … |
| `MCP-176` | high | `hand-written` | **implemented** | Port `rankToolMatches` and `paginate` | proxy.rs:1073 `rank_tool_matches` walks the IndexMap in insertion order, skips disabled and non-matching servers, computes `has_keywords = … |
| `MCP-177` | low | `hand-written` | **implemented** | Port keyword resolution inside the regex search path | No test asserts a keyword-only regex match returns with `score: 0` (upstream's "matches keywords in regex search mode"). Folded into MCP-196. |
| `MCP-178` | high | `open-decision` | **implemented** | Port `rankSuggestions`, and settle the `getServerPrefix` conflict | MCP-178 is verdict **open-decision**, and the Rust has already picked a side — option (a): cyrup-mcp implements the adapter's FOUR-mode, … |
| `MCP-191` | high | `open-decision` | **partial** | `auth-start` / `auth-complete` derive no distinct permission targets | Neither of the unit's deliverables exists. (1) The hazard is undocumented: a grep for `MCP-191` across crates/cyrup-mcp/src returns zero hits — the only unit id in the section with no reference in the code — and nothing in cyrup-permission-system records that `auth-start`/`auth-complete` fall … |
| `MCP-192` | medium | `host-verb` | **implemented** | Satisfy the permission system's contracts on the `mcp` tool | The cyrup-it half of "verify" (with `mcp` denied, assert the guideline disappears from the system prompt — the assertion that actually catches a … |
| `MCP-193` | medium | `host-addition` | **missing** | Reach `register_late_tool` from a native extension | HA-1 is unbuilt. Needed: either `NativeExtension::set_ext_host(Weak<ExtensionHost>)` called beside the existing `set_host_services`, or a defaulted `HostServices::register_late_tool` backed by a late-attached sink (the `set_overlay_sink` / `attach_dynamic_tools` precedent in … |
| `MCP-194` | low | `open-decision` | **implemented** | Tool-schema property order is alphabetised by `serde_json` | Decision (c) accepted and made visible. proxy.rs:3965-3971 documents that the workspace builds `serde_json` without `preserve_order` so … |
| `MCP-195` | medium | `hand-written` | **implemented** | Port the ranking conformance suite (11 cases) | 11/11. Upstream __tests__/search-ranking.test.ts has exactly 11 `it(` cases (8 in `describe("search ranking")` including the two paginate assertions, … |
| `MCP-196` | high | `hand-written` | **partial** | Port the proxy-mode conformance suites (47 cases) | Not the named 46-ported + 1-re-specified target, and the expensive third of the suite is absent. Concretely missing, by upstream case name: proxy-modes-auto-auth's "runs URL elicitations returned by proxy tool calls", "rethrows proxy auto-auth cancellation", "surfaces aborted proxy tool calls via … |
| `MCP-197` | medium | `host-verb` | **implemented** | Port the render binding, including the `toolResultRendering` fork | `tool_render_kind(settings)` (registration.rs:1472) returns `ToolRenderKind::Default` only for `Some(ToolResultRendering::Boxed)` and `SelfRendered` … |
| `MCP-198` | medium | `hand-written` | **implemented** | Port the cross-server candidate-collision set behind the description's counts | proxy.rs:3737 `collision_candidates` builds the cross-server candidate set once per `build_proxy_description` call: it iterates every configured, … |
| `MCP-199` | low | `host-verb` | **implemented** | Wire native-tool detection to `all_tool_names` | REFUTED — the claim rests on a misreading. owner.rs:408 is NOT 'a stale-generation no-op that always returns None': `OwnedServices` is … |

### 13e · Tool registration, naming, approval, output guard

[`13e-mcp-tools.md`](13e-mcp-tools.md) — 53 units, 7 missing, 8 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-200` | high | `hand-written` | **implemented** | The four-mode server-prefix / tool-name formatter | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs: `sanitize_server_prefix` (code-point walk, `_{:x}_` escape, `preserve_provider_valid` flag), … |
| `MCP-201` | high | `hand-written` | **implemented** | getToolNameCandidates, including the legacy arm | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `tool_name_candidates(tool, server, prefix, include_legacy) -> HashSet<String>` plus … |
| `MCP-202` | high | `hand-written` | **implemented** | matchesToolPattern / matchesToolSelector / isToolAllowed | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs: `glob_to_regex` (escape set `[.+^${}()\|[]\\]`, `*`→`.*`, `?`→`.`, anchored), `is_glob`, … |
| `MCP-203` | medium | `hand-written` | **implemented** | resourceNameToToolName and the read_ resource base name | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `resource_name_to_tool_name` (non-alphanumeric → `_`, run-collapse, trim both edges, lowercase, … |
| `MCP-204` | medium | `hand-written` | **missing** | resolveServerFromToolName with its ambiguity fail-safe | No pure `resolve_server_from_tool_name(tool_name, server_names, prefix)` exists: no `None` for `ToolPrefix::None`, no longest-prefix winner, and no ambiguity fail-safe returning `None` when two different servers produce the same winning prefix. The two existing prefix scans pick a first match … |
| `MCP-205` | high | `open-decision` | **not-applicable** | Reconcile mcp_direct_tools.rs with pi-mcp-adapter naming | Verdict is **open-decision**, and no ruling has been recorded: registration.rs:179 says verbatim "MCP-205, unresolved" and proxy.rs:419 says … |
| `MCP-206` | low | `hand-written` | **implemented** | sanitizePromptName / formatPromptCommandName | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `sanitize_prompt_name` (`[^A-Za-z0-9_-]+` run → one `_`, trim `[_-]`, `"prompt"` when empty, … |
| `MCP-207` | high | `hand-written` | **missing** | buildToolMetadata | The whole live `tools`+`resources` → `Vec<ToolMetadata>` pipeline is absent: no `failedTools` accumulation for unnamed tools, no post-visibility `seenNames` reservation, no `description ?? ""`, no resource arm with `read_` + `Read resource: <uri>` + `resourceUri`, and none of the v2.26.1 shape the … |
| `MCP-208` | medium | `hand-written` | **partial** | extractUiToolVisibility / isUiToolVisibleToModel (kept half) | `extractUiToolVisibility(tool._meta)` is absent — nothing walks a live `_meta.ui.visibility` (grep for `_meta` / `"visibility"` / `extract_ui` across crates/cyrup-mcp/src finds no reader). The fail-closed extraction cases (`_meta.ui` non-object or an array → visible; `visibility` present but … |
| `MCP-209` | n/a | `cut` | **not-applicable** | getToolUiResourceUri / extractToolUiStreamMode and the UI spec fields | Cut 2, correctly honoured: /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `DirectToolSpec` has no `uiResourceUri`/`uiStreamMode` and … |
| `MCP-210` | medium | `hand-written` | **implemented** | findToolByName, getToolNames, totalToolCount | /home/user/cyrup/crates/cyrup-mcp/src/proxy.rs `find_tool_by_name` (exact `name` match first, then `-`→`_` normalised on both sides) at proxy.rs:735, … |
| `MCP-211` | medium | `hand-written` | **missing** | formatSchema and its four helpers | The whole pretty-printer is unwritten: `formatSchema`'s five-way dispatch, `formatProperty` (non-object early return, parts joined by one space, recursion at `indent + " "`), `formatType`'s six ordered rules including `Object.hasOwn(schema,"const")` (must be `Map::contains_key`), … |
| `MCP-212` | critical | `hand-written` | **implemented** | resolveDirectTools, including the builtin-collision drop | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `resolve_direct_tools`: `cache == None` → empty; iterates `config.mcp_servers` in file order … |
| `MCP-213` | high | `hand-written` | **implemented** | buildProxyDescription | One text delta against the plan's stated port literal: the header reads `Non-MCP cyrup tools should be called directly` where 13e §6 specifies … |
| `MCP-214` | high | `hand-written` | **missing** | The direct-tool execute state machine | None of §7's ordered state machine exists: no owned-signal composition, no `lazyConnect`, no auto-auth-on-`needs-auth`, no connection assertion, no approval call, no request options, no `tools/call` / `resources/read`, no content transform → guard hand-off, no error/abort mapping and no in-flight … |
| `MCP-214a` | high | `hand-written` | **partial** | recoverAuthConnection and the per-server request options | Nothing wires either into a direct-tool call, because the executor does not exist (MCP-214). `ProxyEnv::call_tool` / `read_resource` (proxy.rs:1397, 1410) take a recovery callback but have no production implementor, and `build_request_options` has no production call site (grep shows only … |
| `MCP-215` | medium | `hand-written` | **partial** | attemptDirectAutoAuth and the auth message templates | The direct-tool flavour has no caller. /home/user/cyrup/crates/cyrup-mcp/src/oauth.rs defines `msg_auth_required_direct_tools` (the `MCP server "x" requires OAuth…` literal, distinct from the proxy's `Server "x" …`) and `msg_auto_auth_failed`, and a grep shows both are referenced only from a doc … |
| `MCP-216` | medium | `host-verb` | **implemented** | The direct-tool registration shape | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `DirectTool::new` computes and owns `label = "MCP: {original}"`, `description` falling back to … |
| `MCP-217` | high | `host-addition` | **missing** | Post-init dynamic tool (and command) registration | No fingerprint-diff `syncDirectTools`, no `deactivateTools` fallback pass, no `syncProxyTool` description refresh at runtime and no `syncToolSurface` entry point. The state slots exist unused on `McpExtension` (`registered_direct_tools`, `fallback_deactivated_tools`, `proxy_tool_description` in … |
| `MCP-217a` | medium | `hand-written` | **partial** | freezeDirectTools and the frozen-surface escape hatches | Neither accessor has a production caller (grep for `freeze_direct_tools()` / `direct_tools_frozen()` across crates/cyrup-mcp/src returns only the definitions). Missing: setting the latch immediately after the initial post-init sync; the freeze log line `MCP: direct tools frozen after initial sync — … |
| `MCP-217b` | low | `host-verb` | **missing** | The tool-surface refresh notification | No `MCP: direct tools refreshed (+{added}, ~{updated}, -{deactivated})` toast, no added/updated/deactivated counting, and no `ctx.hasUI`-equivalent guard (i.e. emit only when an interactive surface is present). Depends on MCP-217 producing the counts. |
| `MCP-218` | medium | `hand-written` | **implemented** | syncProxyTool's registration/deactivation predicate | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `should_register_proxy_tool` implements the three-way OR (`disableProxyTool !== true` \|\| no … |
| `MCP-219` | medium | `hand-written` | **implemented** | MCP_DIRECT_TOOLS, __none__ and parseDirectToolSelectors | /home/user/cyrup/crates/cyrup-mcp/src/runtime.rs `DIRECT_TOOLS_NONE_SENTINEL = "__none__"` and `direct_tools_override` (comma split, trim, … |
| `MCP-220` | high | `hand-written` | **implemented** | transformMcpContent for every standard MCP content type | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `transform_mcp_content`: text, image (`image/png` default — the only non-text output), `resource` … |
| `MCP-221` | medium | `hand-written` | **implemented** | transformMcpResourceContents | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `transform_mcp_resource_contents`: string `text` wins, then string `blob` materialises, else … |
| `MCP-222` | high | `hand-written` | **implemented** | resolveMcpResultContent and the structured-content fallback | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `resolve_mcp_result_content`: transforms `result.content` when it is an array, and only when the … |
| `MCP-223` | high | `hand-written` | **implemented** | Binary-resource materialization with its four limits | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `MaterializedResources::materialize`: cancelled scope → `runtime stopped`; `base64_decoded_len` … |
| `MCP-224` | medium | `hand-written` | **partial** | The materialized-resource cleanup drain and retry | No pending-cleanup set, no per-directory attempt counters capped at `MAX_CLEANUP_RETRY_ATTEMPTS = 3`, no `CLEANUP_RETRY_DELAY_MS = 30_000` timer guarded by "already pending or nothing retryable", no timer-clear when the set empties, and no aggregate error carrying `Vec<io::Error>` with the message … |
| `MCP-225` | medium | `hand-written` | **partial** | resolveMcpOutputGuardOptions and the MCP_OUTPUT_GUARD kill switch | The env variable is never actually read in production: grep for `MCP_OUTPUT_GUARD` across crates/cyrup-mcp/src finds only doc comments, and grep for `output_guard(` finds no production call site at all (the guard is reached through the `ProxyEnv::guard_mcp_output` seam, which has no production … |
| `MCP-226` | high | `hand-written` | **implemented** | guardMcpOutput's normalize / affix / passthrough path | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `guard_mcp_output` steps 1–2 plus `sanitize_content` (image mime trimmed then `take_utf16(.., … |
| `MCP-227` | high | `hand-written` | **implemented** | The truncation arithmetic and notice format | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs: `text_stats` (0 lines for empty), `reserve_budget` (charges `"\n\n" + notice` against both caps, … |
| `MCP-228` | high | `hand-written` | **implemented** | saveArtifact's private-directory spill | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `save_artifact(kind, text)`: `make_private_temp_dir("pi-mcp-output-")` (`DirBuilder::mode(0o700)`, … |
| `MCP-229` | medium | `hand-written` | **implemented** | boundMcpResult and the result-summary schema | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `bound_mcp_result` (raw kept under the threshold), `summarize_mcp_result` (spill via … |
| `MCP-230` | medium | `hand-written` | **implemented** | Record the output guard's actual security contract | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs module docs, section "The output guard's security contract, stated exactly (MCP-230)": states it … |
| `MCP-231` | high | `hand-written` | **missing** | isToolCallApprovalRequired | The whole predicate is unwritten: server-level `approveTools` overriding the global on presence, `true` → always required, non-array/empty → not required, the legacy-alias disambiguation reusing MCP-201/MCP-202, the explicit injection of the first non-bare current candidate with `-`→`_` into the … |
| `MCP-232` | critical | `host-verb` | **partial** | ensureToolCallApproved and the approval dialog | The gate itself is unimplemented — `ensure_tool_call_approved` exists only as a `ProxyEnv` trait method (proxy.rs:1488) with a test-only implementor. Missing: the cache lookup/insert against `approved_tool_calls`, the headless check performed **before** calling `select` (so a cancelled dialog and a … |
| `MCP-233` | medium | `host-verb` | **implemented** | Drop the approval broker; before_tool_call is the broker | The broker is deliberately absent and the decision is recorded in code: /home/user/cyrup/crates/cyrup-mcp/src/lib.rs:80 ("The fifth cut field is … |
| `MCP-234` | high | `open-decision` | **not-applicable** | Direct MCP tools do not reach the mcp permission category | Verdict is **open-decision** with no ruling recorded, so by the class rule this is not-applicable rather than outstanding work. Nothing behavioural … |
| `MCP-235` | high | `hand-written` | **implemented** | sanitizeTerminalText / stripOscSequences | /home/user/cyrup/crates/cyrup-mcp/src/ui.rs `strip_osc_sequences` (hand-written scanner over both `ESC ]` and C1 `U+009D` introducers, terminated by … |
| `MCP-236` | medium | `hand-written` | **implemented** | Give the mcp tool its prompt guideline | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `PROXY_TOOL_PROMPT_GUIDELINE` holds the lowercase literal and `impl Tool for ProxyTool { fn … |
| `MCP-237` | medium | `hand-written` | **implemented** | The call-row formatters | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `format_mcp_proxy_tool_call_lines` (all seven surviving branches in order, with `@ server`, … |
| `MCP-238` | low | `host-verb` | **implemented** | resolveMcpToolRenderOptions and the renderShell selection | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `resolve_mcp_tool_render_options` over /home/user/cyrup/crates/cyrup-mcp/src/config.rs … |
| `MCP-239` | medium | `hand-written` | **implemented** | collectCollapsedResultLines / formatMcpToolResultLines / blockToLines | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `collect_collapsed_result_lines` (UTF-16 `utf16_len` budget, `indexOf`-style split without … |
| `MCP-240` | low | `hand-written` | **implemented** | formatMcpToolResultIdentity | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `format_mcp_tool_result_identity`: `None` unless `details.mode == "call"`, server from `server` … |
| `MCP-241` | low | `hand-written` | **implemented** | The compact result row without a render width | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `compact_result_widget` — drops the trailing `"…"` when truncated, prefixes `"{title} → "` on the … |
| `MCP-242` | low | `host-verb` | **implemented** | Expanded rendering without a per-row expansion flag | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `render_mcp_tool_result` computes `expanded = tools_expanded \|\| is_truthy(details.error)`; … |
| `MCP-243` | low | `hand-written` | **implemented** | The compact call-row suppression has no cyrup equivalent | The recommended "drop the stash entirely" was taken: /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs `render_call` always returns a drawn call row … |
| `MCP-244` | low | `hand-written` | **implemented** | The renderer contract carries no theme | /home/user/cyrup/crates/cyrup-mcp/src/renderers.rs emits only `text` / `truncated-text` widget nodes (`text_widget`, `truncated_text_widget`); … |
| `MCP-245` | low | `extension-owned` | **not-applicable** | Width-aware truncation is not needed | Dissolved by the plan ("nothing to build"), and the tree matches: no width crosses `NativeExtension::render_result`, … |
| `MCP-246` | low | `extension-owned` | **implemented** | Route the five collision/advisory warnings | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `resolve_direct_tools` emits all five as `tracing::warn!`: `MCP: skipping direct tool "…" … |
| `MCP-247` | high | `hand-written` | **implemented** | The mcp proxy tool's parameter schema | /home/user/cyrup/crates/cyrup-mcp/src/registration.rs `proxy_tool_parameters()` — twelve optional properties, no `required` array; `args` is the … |
| `MCP-248` | n/a | `hand-written` | **not-applicable** | Tracker: registration, approval, guard and rendering | Tracker unit. State of the critical path it indexes, from this audit: MCP-200 ✓, MCP-201 ✓, MCP-202 ✓, MCP-203 ✓, MCP-207 ✗, MCP-212 ✓, MCP-216 ✓, … |
| `MCP-249` | high | `hand-written` | **partial** | Freeze the details schema this subsystem emits | Two gaps. (1) `server_unavailable` — emitted by upstream direct-tools.ts step 7 (`details: { error: "server_unavailable", server }`, verified at /home/user/cyrup/tmp/pi-mcp-adapter/direct-tools.ts:420) — has no enum variant and no producer; a grep for `server_unavailable` across … |

### 13f · Credentials and keychain storage

[`13f-mcp-credentials.md`](13f-mcp-credentials.md) — 41 units, 0 missing, 5 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-250` | high | `hand-written` | **implemented** | The `AuthEntry` record and its strict normalization | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs — `AuthEntry` (line 596) and `StoredClientInfo` (line 448) are … |
| `MCP-251` | high | `hand-written` | **implemented** | Derive the keychain account and legacy directory from `sha256-<hex>` of the … | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `hex_sha256` (732), `auth_entry_account` (761) = `format!("sha256-{}", … |
| `MCP-252` | high | `extension-owned` | **implemented** | Add the OS keyring backend and map its error taxonomy | Two secondary points, neither behaviour-changing today: (a) `keyring::Entry::store_status()` is named in a doc comment (line 963) but never called — … |
| `MCP-253` | high | `hand-written` | **implemented** | The chunking manifest write path | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `AuthEntryChunkManifest` (784, declaration order … |
| `MCP-254` | high | `hand-written` | **implemented** | The chunked read path and the `AuthStoreError` taxonomy | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `StoreOp` (254) with `verb()`/`preposition()`, `AuthStoreError` (336) with … |
| `MCP-255` | medium | `hand-written` | **implemented** | Stale-chunk cleanup ordering and its error-swallowing | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `existing_chunk_manifest` (2258, swallow-all `.ok()??`), `try_remove_chunk_payloads` (2282, `let … |
| `MCP-256` | high | `hand-written` | **implemented** | The legacy plaintext import-and-delete path (and the record translator) | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `AuthStorageOptions::from_settings` (1802), `McpAuthStore::auth_base_dir` (2126) delegating to … |
| `MCP-257` | high | `hand-written` | **implemented** | The process-lifetime auth-entry cache and its three external invalidation points | None for this unit's own obligation (the eviction primitive). Informational: `invalidate_cache` has no production caller anywhere in … |
| `MCP-258` | medium | `extension-owned` | **implemented** | Fault-injection backends behind an explicit selector | Mechanism divergence from the plan text, behaviourally equivalent: the four backends are a hand-rolled `MemorySecretStore` + injected … |
| `MCP-259` | low | `hand-written` | **implemented** | Honour the auth-cache disable switch | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `AUTH_CACHE_DISABLED_ENV` (176) dual-read, `env_is_one` (236, strict `== "1"`), … |
| `MCP-260` | high | `hand-written` | **partial** | Re-exec under `keyctl session -` via a hidden `__mcp-keyring-helper` subcommand | Two items. (1) `crates/cyrup/src/mcp_keyring_helper_cmd.rs` does not exist: no `SUBCOMMAND`/`is_selected(argv)`/`dispatch()` triple, no `pub mod` in crates/cyrup/src/lib.rs, and no pre-dispatch in main.rs before clap parsing. Consequence: the default `current_exe() __mcp-keyring-helper` path … |
| `MCP-261` | medium | `hand-written` | **implemented** | The helper's one-shot JSON stdio protocol | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `KeyringHelperRequest` (1320) with `#[serde(skip_serializing_if = "Option::is_none")]` on … |
| `MCP-262` | medium | `hand-written` | **implemented** | The revoked-keyring cause-chain predicate | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `KEY_REVOKED_PATTERN` (1261, `(?i)key\s*(?:has been\s*)?revoked\|keyrevoked` in a … |
| `MCP-263` | low | `hand-written` | **implemented** | Emit the two credential-store-unavailable messages verbatim | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `format_oauth_credential_store_unavailable` (1981): Linux + predicate ⇒ `OAuth credential store … |
| `MCP-264` | critical | `hand-written` | **implemented** | URL binding and the mutators' sibling-purge rule | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `AuthEntry::matches_url` (640, exact string equality, empty stored URL treated as absent), … |
| `MCP-265` | high | `hand-written` | **implemented** | `inspectAuthForUrl`'s three-state status and its fail-open/fail-closed split | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `OAuthCredentialStatus` (1963: `Present(AuthEntry)`/`Absent`/`Unavailable{message}`) and … |
| `MCP-266` | medium | `hand-written` | **implemented** | The accessor surface section 07 consumes | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `McpAuthStore` (2048) exposes inherent methods rather than free functions over globals: … |
| `MCP-267` | medium | `rmcp` | **implemented** | Expiry arithmetic | Live path is rmcp's: /home/user/cyrup/crates/cyrup-mcp/src/oauth.rs:2740 calls `manager.get_access_token()`. The one surviving hand-written predicate … |
| `MCP-268` | high | `hand-written` | **implemented** | Serialize read-modify-write per server | Narrow residual: the synchronous inherent methods (`update_credentials` 2890, `update_client_info` 2902, `update_state` 2916, `save_auth_entry` 2765, … |
| `MCP-269` | medium | `hand-written` | **partial** | MCP credentials never reach `auth.json` | The standing guard the unit asks for does not exist: there is no repo-level test asserting that no MCP credential material can reach `cyrup_config::env`'s auth path and no `Serialize` route that could send an `AuthEntry` there. The crate flags this itself as `TODO(MCP-269)` at … |
| `MCP-270` | low | `extension-owned` | **implemented** | The embedder facade (`oauth.ts`) | /home/user/cyrup/crates/cyrup-mcp/src/oauth.rs `get_mcp_oauth_tokens_for_url` (3876, delegates to `get_valid_token`), … |
| `MCP-271` | n/a | `rmcp` | **implemented** | The MCP-SDK `OAuthTokens` conversion | Dissolved as the plan requires: /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `AuthEntry::credentials` is … |
| `MCP-272` | n/a | `cut` | **not-applicable** | `ConsentManager` | Cut 2, correctly not ported. `grep -rni "consent" /home/user/cyrup/crates/cyrup-mcp/src/*.rs` finds only OAuth-consent-screen prose (config.rs:1306, … |
| `MCP-273` | n/a | `cut` | **not-applicable** | `ConsentError` | Cut with MCP-272. `grep -rn "ConsentError\|CONSENT_DENIED\|CONSENT_REQUIRED" /home/user/cyrup/crates/cyrup-mcp/src/` returns nothing; … |
| `MCP-274` | n/a | `cut` | **not-applicable** | Consent state is process-scoped and must not be persisted | Cut with MCP-272. No consent state of any kind exists in /home/user/cyrup/crates/cyrup-mcp/src/state.rs (`McpState`, 451 lines) — the module doc at … |
| `MCP-275` | medium | `hand-written` | **implemented** | Compact JSON serialization | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `write_secure_auth_entry_to_store` (2313) uses `serde_json::to_string` (never … |
| `MCP-276` | n/a | `extension-owned` | **not-applicable** | The non-string server-name guards do not port | Correctly not ported, and filed rather than dropped: /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs:757-759 records the reasoning on … |
| `MCP-277` | critical | `hand-written` | **implemented** | Prove the absence of secret leakage through `Debug`, logs and errors | One verify item unported: there is no grep-based standing test asserting that no error string anywhere in the crate interpolates a payload or token, … |
| `MCP-278` | medium | `hand-written` | **partial** | The storage acceptance suite (17 tests) | Definitively missing: the two subprocess cases — `routes revoked Linux keyring operations through the recovery helper` and `does not use the recovery helper for generic secure-store failures` (the fake `keyctl` exiting 99 and the assertion that the fake store file was never created). … |
| `MCP-280` | high | `hand-written` | **implemented** | The keychain service name, and what happens to a co-installed pi-mcp-adapter | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `AUTH_SECRET_SERVICE = "cyrup.mcp.oauth"` (125) and `LEGACY_AUTH_SECRET_SERVICE = … |
| `MCP-281` | medium | `hand-written` | **implemented** | Adopt the keychain-mandatory posture | The unit's verify is only half covered: `the_store_unavailable_sentence_is_verbatim` asserts the sentence on the error type, but there is no … |
| `MCP-282` | low | `hand-written` | **implemented** | Env-var namespace for the surviving switches | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs declares all six switches as `[&str; 2]` pairs with `CYRUP_MCP_*` first: `TEST_AUTH_STORE_ENV` … |
| `MCP-283` | medium | `hand-written` | **partial** | The cache acceptance suite (13 tests) | Not ported, each a distinct upstream case: (a) `normalizes publication exactly as a later store reload does` — no test asserting an unknown key is dropped identically on the publish (hit) path and the store-reload (miss) path; only the generic `unknown_keys_are_dropped_not_rejected` exists; (b) the … |
| `MCP-284` | medium | `hand-written` | **implemented** | The parse-error wrapping asymmetry between read and remove | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `read_auth_entry_from_store` (2549) wraps **only** the backend `store.read` in … |
| `MCP-285` | medium | `hand-written` | **implemented** | Remove-path chunk cleanup is fatal, not best-effort | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `remove_chunk_payloads` (2269, `?` on every chunk removal) is used only by … |
| `MCP-286` | low | `hand-written` | **implemented** | Bound `chunkCount` on read | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `AUTH_CHUNK_COUNT_LIMIT: usize = 64` (154) with the cyrup-addition rationale in the doc comment, … |
| `MCP-287` | medium | `hand-written` | **partial** | The subprocess timeout path and the unreachable ladder rung | The three fixtures the unit names are absent: a helper that sleeps 30 s (⇒ rung-1 message within ~10 s and no zombie), a helper printing `{"ok":false,"error":"boom"}` and exiting 1 (⇒ rung-2 message), and the same helper exiting 0 (⇒ rung-5 message `boom`). No such test exists in credentials.rs's … |
| `MCP-288` | low | `rmcp` | **implemented** | The three `expiresAt` predicates | Two of the three sites are gone as specified: no SDK-shape conversion exists (MCP-271) and the live predicate is rmcp's `get_access_token` … |
| `MCP-289` | n/a | `extension-owned` | **implemented** | Create the `cyrup-mcp` crate | Layout divergence only, no behavioural consequence: this section landed as one 4796-line `src/credentials.rs` rather than the planned … |
| `MCP-290` | medium | `hand-written` | **implemented** | Persist the DCR client record rmcp's `StoredCredentials` drops | /home/user/cyrup/crates/cyrup-mcp/src/credentials.rs `StoredClientInfo` (448) persists `client_id`, `client_secret`, `client_id_issued_at`, … |
| `MCP-291` | high | `hand-written` | **implemented** | Implement `rmcp::transport::auth::{CredentialStore, StateStore}` over the … | REFUTED on its central point. (1) Both traits ARE implemented over the keychain — `McpCredentialStore` (credentials.rs:3103/3128) and `McpStateStore` … |

### 13g · OAuth 2.1 acquisition

[`13g-mcp-oauth.md`](13g-mcp-oauth.md) — 49 units, 1 missing, 8 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-300` | n/a | `hand-written` | **partial** | The OAuth subsystem as one shippable unit | The unit's `verify` is MCP-347's suite green end to end against a stub authorization server; that stub does not exist and `crates/cyrup-mcp/` has no `tests/` directory. Separately, the subsystem is not reachable from a running session: `McpExtension` (extension.rs:419) does not override … |
| `MCP-301` | high | `hand-written` | **implemented** | Flow ownership: runtime, generation counter, four maps | oauth.rs: `McpOAuthRuntime` (line 1967) holds `token`/`controller` `CancelToken`s, an explicit `generation: AtomicU64`, a `stop_reason: … |
| `MCP-302` | medium | `hand-written` | **implemented** | extractOAuthConfig and its twelve validation messages | oauth.rs `extract_oauth_config` (line 322) applies the value-shaped rules in source order — `clientSecret` `!`-prefixed values preserved … |
| `MCP-303` | medium | `hand-written` | **implemented** | parseOAuthRedirectUri's loopback-only validation | oauth.rs `parse_oauth_redirect_uri` (line 542) with `RedirectEndpoint { port, callback_host, callback_path }` (line 521). Checks run in upstream's … |
| `MCP-304` | high | `hand-written` | **implemented** | Callback endpoint configuration and MCP_OAUTH_CALLBACK_PORT | oauth.rs: `DEFAULT_OAUTH_CALLBACK_PORT = 19876` (838), `DEFAULT_OAUTH_CALLBACK_PATH = "/callback"` (840), `DEFAULT_OAUTH_CALLBACK_HOST = "localhost"` … |
| `MCP-305` | high | `hand-written` | **implemented** | The bind / rebind / strict-port state machine | oauth.rs `ensure_callback_server` (1171) is the serializing wrapper: refuses with `OAuth callback server stopped` while a stop future is present, … |
| `MCP-306` | critical | `hand-written` | **implemented** | The callback request handler's eight branches | oauth.rs `CallbackMultiplexer::handle` (line 981) implements `cyrup_provider::auth::oauth::callback::CallbackHandler`, never calls … |
| `MCP-307` | medium | `hand-written` | **implemented** | The three callback pages, including host branding | oauth.rs `PAGE_STYLE` (676), `CHECK_ICON`/`CROSS_ICON` (735/738, both `xmlns`-free), `page()` (741) reproducing the template byte for byte including … |
| `MCP-308` | high | `hand-written` | **implemented** | Listener lifetime: reserve, wait, cancel, stop, restart, process exit | oauth.rs `reserve_callback_server` (1335), `release_callback_server` (1340), `wait_for_callback` (1355, deletes the reservation first, inserts the … |
| `MCP-309` | medium | `hand-written` | **partial** | The discovery trigger: proactive probe or reactive challenge | Nothing in production ever supplies `AuthenticateOptions::challenge`: grepping `challenge` across crates/cyrup-mcp/src finds only oauth.rs's own declaration and use, and the only `ProxyEnv` implementor is the test `FakeEnv` (proxy.rs:4500), so no connect failure is wired into it. With no challenge … |
| `MCP-310` | n/a | `rmcp` | **implemented** | RFC 9728 protected-resource metadata discovery | Supplied by rmcp and actually called: oauth.rs:2694 `manager.resolve_metadata_from_challenge(challenge)` and oauth.rs:3774 … |
| `MCP-311` | n/a | `rmcp` | **implemented** | RFC 8414 + OIDC discovery and the issuer echo check | rmcp-owned and reached through the same `resolve_metadata_from_challenge`/`resolve_metadata` calls; the port's only lever is … |
| `MCP-312` | medium | `rmcp` | **implemented** | RFC 7591 dynamic client registration | rmcp-owned: `AuthorizationSession::new(manager, request)` at oauth.rs:2782 drives the MCP client-registration priority order. The port supplies the … |
| `MCP-313` | medium | `hand-written` | **partial** | Client metadata and the host-branding defaults | Neither `client_uri` nor `logo_uri` nor a confidential `token_endpoint_auth_method` reaches the registration body — rmcp's `ClientRegistrationRequest` is fixed and the port does not perform its own registration POST; recorded as `TODO(MCP-312)` at oauth.rs:2771-2779. `default_client_uri()` … |
| `MCP-314` | high | `hand-written` | **implemented** | Restore the full client configuration after initialize_from_store | REFUTED — the claim inverts the requirement. MCP-314's cyrup column asks for exactly three things and all three are present: persist the registration … |
| `MCP-315` | high | `hand-written` | **implemented** | The keychain-backed CredentialStore, and the expiry arithmetic | Two `rmcp::transport::auth::CredentialStore` impls, both per-server and URL-bound: `oauth::ServerCredentialStore` (oauth.rs:1696, generic over the … |
| `MCP-316` | high | `hand-written` | **implemented** | authorizationParams' reserved-key guard and the no-browser-mid-turn fence | oauth.rs `RESERVED_AUTHORIZATION_PARAMS` (2359) has all eight members including `code_challenge_method`, and `add_authorization_params` (2379) … |
| `MCP-317` | n/a | `rmcp` | **implemented** | PKCE and the authorization URL | rmcp-owned and reached: the authorization URL comes from `AuthorizationSession::get_authorization_url()` (oauth.rs:2932 in `start_auth_inner`), which … |
| `MCP-318` | high | `rmcp` | **implemented** | Token endpoint, client authentication, and the retry policy | REFUTED as port work. The canonical table's verdict is `rmcp`, and the ONLY hand-written obligation the plan names is the client-auth lever — 'the … |
| `MCP-319` | n/a | `rmcp` | **implemented** | RFC 8707 resource binding | rmcp-owned; the port adds nothing and correctly adds no `validateResourceURL` override — grepping crates/cyrup-mcp/src for `resource`-parameter … |
| `MCP-320` | n/a | `rmcp` | **implemented** | Flow-state custody across the browser hop | oauth.rs:2680 `manager.set_state_store(InMemoryStateStore::new())` in `prepare_session`, and again at oauth.rs:3770 in `refresh_tokens` — the … |
| `MCP-321` | high | `hand-written` | **implemented** | The storage read/write surface this flow consumes | The seam is `oauth::McpOAuthStorage` (oauth.rs:1523) with `load`, `save_credentials`, `save_client`, `clear_all`, `oauth_state`, `clear_oauth_state`, … |
| `MCP-322` | low | `rmcp` | **implemented** | Issuer binding of stored credentials | oauth.rs `issuers_match` (2467) is equality with exactly one trailing slash tolerated on either side. `prepare_session` (2704-2717) computes … |
| `MCP-323` | medium | `rmcp` | **implemented** | The RFC 9207 gate in completeAuth, including keepPendingForRetry | oauth.rs `complete_auth` (3373): step 3 fires when `expected_issuer.is_some() && iss.is_none() && requires_issuer`, sets the explicit … |
| `MCP-324` | high | `rmcp` | **partial** | getValidToken's refresh path and its fall-through | The credential-store rethrow is a STRING-PREFIX test, not a structural one: oauth.rs:3706 does `error.to_string().starts_with(CREDENTIAL_STORE_PREFIX)` where `CREDENTIAL_STORE_PREFIX = "credential store"` (3742) and only `map_auth_error` (3745) applies that prefix, to `AuthError::InternalError`. … |
| `MCP-325` | medium | `rmcp` | **implemented** | The client_credentials grant | oauth.rs `authenticate_client_credentials` (2806), short-circuited from `start_auth` at step 3 (before the redirect parse, the state generation and … |
| `MCP-326` | high | `hand-written` | **partial** | The manual/headless leg: parsing and the callback-versus-paste race | An external abort does NOT reject with the identical reason value. `AuthenticateOptions::combined_signal` (oauth.rs:2618) builds a bare `CancelToken` via `crate::abort::combine`, which carries no payload, and both the abort arm of `wait_for_authorization_response`'s `tokio::select!` and the … |
| `MCP-327` | low | `extension-owned` | **implemented** | Browser launch | oauth.rs `BrowserLauncher` trait (2524) with `OpenerLauncher` calling `opener::open` directly (2538; `opener = "0.8.5"` at Cargo.toml:159) and … |
| `MCP-328` | high | `hand-written` | **implemented** | startAuth's ordering, stale-registration checks and aggregate cleanup | oauth.rs `start_auth` (2899) + `start_auth_inner` (3018) implement all fourteen steps in order: disabled check (`MCP server "{s}" is disabled`); … |
| `MCP-329` | medium | `hand-written` | **implemented** | The 5-minute abandoned-flow timer and its state guard | oauth.rs `MANUAL_AUTH_TIMEOUT` = 5 min (1869); `set_pending_auth` (2208) arms a detached `tokio::spawn` racing `timer.cancelled()` against … |
| `MCP-330` | high | `hand-written` | **implemented** | authenticate's in-flight dedup and its cleanup boundary | oauth.rs `authenticate` (3475) builds the dedup key `{server}\|{url}\|{base_dir}`, and the lookup-or-insert of the … |
| `MCP-331` | high | `hand-written` | **implemented** | completeAuth and completeAuthFromInput | oauth.rs `complete_auth` (3373) requires a pending flow (`No pending OAuth flow for server: {s}`), reads the stored session/issuer/base_dir with the … |
| `MCP-332` | medium | `hand-written` | **implemented** | supportsOAuth, getAuthStatus, removeAuth | oauth.rs `supports_oauth` (491) reproduces the truth table branch for branch in the observable order, with `auth === "oauth"` beating the … |
| `MCP-333` | high | `rmcp` | **implemented** | The connect-path 401 classification | REFUTED — the auditor judged MCP-333 against a scope the plan explicitly excludes. 13g's Coverage/Excluded section says: '`server-manager.ts` beyond … |
| `MCP-334` | medium | `host-verb` | **partial** | The /mcp-auth command surface and its eleven messages | There is no command handler: `McpExtension` (extension.rs:419) implements `id`, `is_ambient`, `init`, `render_call`, `render_result` and `set_host_services` but does NOT override `NativeExtension::execute_command`, so invoking `/mcp-auth <server>` hits cyrup-ext's default body at … |
| `MCP-335` | medium | `hand-written` | **implemented** | auth-start / auth-complete and auto-auth | proxy.rs `format_manual_auth_instructions` (2443) — re-exported from oauth.rs so there is one copy — builds the ten-element array, drops every empty … |
| `MCP-336` | n/a | `extension-owned` | **implemented** | Callback-listener ownership: settled as reuse | The reuse is real, not a rebuild. oauth.rs imports `cyrup_provider::auth::oauth::callback::{CallbackServer, CallbackServerConfig, CallbackHandler, … |
| `MCP-337` | n/a | `rmcp` | **implemented** | The rmcp split: verified, settled | crates/cyrup-mcp/Cargo.toml:70-76 declares exactly the settled set: `rmcp = { version = "3.1.2", default-features = false, features = ["client", … |
| `MCP-338` | n/a | `extension-owned` | **implemented** | Browser-open mechanism: settled on opener | `opener = "0.8.5"` at crates/cyrup-mcp/Cargo.toml:159, and `OpenerLauncher::open` (oauth.rs:2538) calls `opener::open(url)` directly — no … |
| `MCP-339` | medium | `open-decision` | **not-applicable** | Bind localhost or 127.0.0.1 | Open decision, and the Rust has picked option (c): bind `127.0.0.1`, advertise `localhost`. oauth.rs:850 declares `DEFAULT_OAUTH_CALLBACK_HOST = … |
| `MCP-340` | low | `open-decision` | **not-applicable** | The stale hardcoded client version in the discovery probe | Open decision, rendered moot by the side the Rust picked for MCP-309: recommendation (a), the reactive path, means there is no `probeAuthDiscovery` … |
| `MCP-341` | medium | `hand-written` | **missing** | Ship a corrected OAuth document | The whole unit: ship a ported OAuth document with §14's eight divergences corrected inline (undocumented `oauth.logoUri`; the rebranding defaults for `clientName`/`client_uri`; discovery order stated backwards — `WWW-Authenticate` is primary and `.well-known` the fallback; the absent RFC 9207 … |
| `MCP-342` | medium | `hand-written` | **partial** | A reachable, three-form interpolate_env_vars | The consolidation the unit asks for did not happen — a THIRD implementation was added instead of one shared implementation, and the two pre-existing copies still carry the two-form parity defect. `cyrup_ext::caps::proc::interpolate_env_vars` (crates/cyrup-ext/src/caps/proc.rs:139) is still … |
| `MCP-343` | n/a | `rmcp` | **not-applicable** | Non-unix entropy: dissolved | Dissolved, and the refutation holds against the current tree. `cyrup_provider::auth::oauth::random` (crates/cyrup-provider/src/auth/oauth/random.rs) … |
| `MCP-344` | medium | `hand-written` | **implemented** | The process-shared listener refcount | oauth.rs `live_runtimes()` (2029) is a process-global `OnceLock<StdMutex<HashSet<u64>>>` keyed by runtime id — a set, not a counter, so the repeated … |
| `MCP-345` | medium | `hand-written` | **implemented** | Preserve both errors when cleanup fails | `McpError::OAuthAggregate { phase: &'static str, errors: CleanupErrors }` (errors.rs:105-113) renders as `#[error("{phase}: {errors}")]` and … |
| `MCP-346` | low | `extension-owned` | **implemented** | The public token API | All three functions exist in oauth.rs: `get_mcp_oauth_tokens_for_url` (3876) delegating to `get_valid_token` so it may refresh and propagating a … |
| `MCP-347` | n/a | `hand-written` | **partial** | The executable spec as the acceptance suite | The file's own `TODO(MCP-347)` at oauth.rs:4111-4118 enumerates what is outstanding, and it is accurate: the rmcp conformance suites named in MCP-337 (MCP-310, MCP-311, MCP-317, MCP-318, MCP-319, MCP-320), `start_auth`'s five stale-registration variants (MCP-328), `authenticate`'s dedup and browser … |
| `MCP-349` | high | `extension-owned` | **implemented** | resolveCommandSecret's subprocess mechanism | oauth.rs `resolve_command_secret` (170) with `COMMAND_SECRET_TIMEOUT = 10 s` (142) and `COMMAND_SECRET_MAX_OUTPUT_BYTES = 1 MiB` (144). All three … |

### 13h · The two panels and the slash commands

[`13h-mcp-tui.md`](13h-mcp-tui.md) — 55 units, 15 missing, 9 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-350` | — | `tracker` | **not-applicable** | Section-08 tracker: poll-repaint replaces push-repaint; the overlay pair is the … | Tracker row; the plan (docs/gap-analysis/13h-mcp-tui.md, "MCP-350 — Tracker") says it proposes no schedulable work and is excluded from every count. … |
| `MCP-350a` | high | `extension-owned` | **implemented** | Stash the `HostServices` handle so panels and commands can reach the host — … | Only the stash is proven. The plan's verify ("drive a panel action that notifies, assert it reached the injected double") cannot run because no … |
| `MCP-351` | high | `hand-written` | **implemented** | `McpPanel`'s construction from config plus validated cache | `McpPanelModel::new` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:1596-1751) walks `config.mcp_servers` as an `IndexMap`, skips non-authenticatable … |
| `MCP-352` | high | `hand-written` | **implemented** | `getOtherCurrentCandidates` and the include/exclude engine it feeds | `McpPanelModel::other_current_candidates` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:1773-1812) iterates every enabled server INCLUDING the current … |
| `MCP-353` | high | `hand-written` | **implemented** | `rebuildVisibleItems`: the flattened list plus the filter state machine | `McpPanelModel::rebuild_visible_items` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:1822-1870) reproduces all three behaviours: a non-empty query … |
| `MCP-354` | medium | `hand-written` | **implemented** | `fuzzyScore` | `fuzzy_score` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:483-511) is the literal formula: substring → `100.0 + (lq.len()/lt.len())*50.0`, otherwise … |
| `MCP-355` | critical | `hand-written` | **implemented** | The panel's top-level key dispatch, in order | Only unit-level evidence. The plan's "not done until the same sequence has been typed into a real terminal" cannot be confirmed from source. |
| `MCP-356` | medium | `hand-written` | **implemented** | The description-search modal | `McpPanelModel::handle_desc_search_key` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:2186-2226) handles only escape/confirm (exit + clear … |
| `MCP-357` | high | `hand-written` | **implemented** | The discard-confirmation modal | `McpPanelModel::handle_discard_key` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:2228-2268) with `discard_selected` initialised to 1 in `new` … |
| `MCP-358` | critical | `hand-written` | **implemented** | Toggling, dirty tracking and the tri-state `buildResult` | `toggle_item` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:1982-2017) with the `!tools.iter().all(is_direct)` rule and the import-notice at both … |
| `MCP-359` | high | `hand-written` | **implemented** | In-panel OAuth (`authenticateServer`) on the sync overlay seam | The panel-side half only. No production `McpPanelCallbacks::authenticate` exists (all impls are `#[cfg(test)]`, ui.rs:5023/5374/5514) — that is … |
| `MCP-360` | high | `hand-written` | **implemented** | In-panel reconnect and `rebuildServerTools` | `start_reconnect` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:2296-2306) and the `Reconnected` arm of `finish_job` (ui.rs:2385-2432): status … |
| `MCP-361` | medium | `extension-owned` | **implemented** | `ctrl+y` copies a server's failure message | `copy_to_clipboard` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:3019-3057) over `CLIPBOARD_COMMANDS` (ui.rs:2995-3012): `pbcopy`/`wl-copy`/`xclip … |
| `MCP-362` | medium | `host-verb` | **partial** | The 60 s inactivity auto-cancel | The panel does not actually close itself. `InteractiveOverlay::tick` returns `bool` (/home/user/cyrup/crates/cyrup-ext/src/host/overlay.rs:289) with no way to request a close, so the code (ui.rs:3258-3266, an explicit `TODO(MCP-362)`) only sets `expired`, publishes the cancelled result, and closes … |
| `MCP-363` | high | `extension-owned` | **implemented** | `panel-keys.ts`: resolve the three canonical ids and `mcp.panel.save` | `PanelKeys` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:980-1117): `from_user_bindings` (ui.rs:1029) reproduces the three-way `mcp.panel.save` … |
| `MCP-363a` | medium | `open-decision` | **implemented** | Where the canonical select-key defaults live | The guard test is not actually cross-crate: it asserts the literals `"up"/"down"/"return"` against themselves rather than against `cyrup-tui`'s … |
| `MCP-364` | critical | `hand-written` | **implemented** | The terminal-injection sanitizers | `strip_osc_sequences` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:309-338) is a hand-written scanner that consumes to BEL/ST/`ESC \` or to end of … |
| `MCP-365` | low | `hand-written` | **implemented** | `estimateTokens` and the footer statistics | `estimate_tokens` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:534-545) is `ceil((utf16_len(name)+utf16_len(desc)+utf16_len(stringify(schema ?? … |
| `MCP-366` | medium | `hand-written` | **implemented** | The panel frame layout | Two residues, both stated in the plan and neither closeable from source: (a) the plan's `visibleWidth` \t→three-spaces normalisation is not … |
| `MCP-367` | medium | `hand-written` | **implemented** | The row renderers, status labels and word wrap | `render_server_row` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:2865-2928) with the `(not cached)` branch (no toggle icon) and the two distinct … |
| `MCP-368` | low | `host-addition` | **partial** | Overlay geometry: the requested column counts, and the silent height clip (HA-3) | Two open pieces. (1) HA-3 itself has not landed: no `OverlayOptions { anchor, width, min_width, max_height, margin }` on `open_overlay`, no `OverlayRequest` plumbing, so the 82-column browser panel and the 92-column setup panel are painted at 95% of the terminal. (2) The height half for the SETUP … |
| `MCP-369` | critical | `host-verb` | **implemented** | `McpPanelResult` escaping an `open_overlay` that returns only `bool` | No test drives it: there is no stub `HostServices` exercising the Close path or the `false` branch, and no production caller of `open_mcp_panel` … |
| `MCP-370` | critical | `open-decision` | **partial** | Tool/resource/prompt name formatting versus the in-tree consumer | The other half of option (a) — upgrading the in-tree consumer in the same change — has NOT been done. /home/user/cyrup/crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs still has a 3-variant `enum ToolPrefix { Server, None, Short }` (line 45-49), `get_tool_prefix` folds every unknown value … |
| `MCP-371` | medium | `hand-written` | **implemented** | `McpSetupPanel`'s screen model and dynamic action list | `McpSetupPanelModel` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:3449-3467) with `SetupScreen` (ui.rs:3283) initialised from the caller's mode in … |
| `MCP-372` | medium | `hand-written` | **implemented** | The imports and paths sub-screens | `handle_imports_key` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:3710-3735) with clamped cursor, `space` toggling membership in `selected_imports`, … |
| `MCP-374` | medium | `hand-written` | **implemented** | `runAction`, the busy latch and the notice model | `run_action` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:3785-3822) is the eight-way dispatch including the muted `Review the details below. Press … |
| `MCP-375` | medium | `hand-written` | **implemented** | The per-action preview builders | `McpSetupPanelModel::action_preview` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:4312-4455) covers all nine bodies: the `run-setup` sentence, the … |
| `MCP-376` | medium | `hand-written` | **implemented** | `formatWritePreview` and `formatPreview` | `format_preview` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:4256-4258) and `format_write_preview` (ui.rs:4267-4305): intro lines, a blank line when … |
| `MCP-377` | low | `hand-written` | **partial** | The compact-width action window | The unit also owns the setup panel's half of MCP-368's height problem and that half is untouched, marked `TODO(MCP-368, MCP-377)` at ui.rs:4023-4028: above `inner_w >= 60` the action list is not windowed at all and `action_preview`'s output is appended unbounded (ui.rs:4120-4123), so a long action … |
| `MCP-378` | low | `hand-written` | **implemented** | The two summary lines | `discovery_summary_line` (/home/user/cyrup/crates/cyrup-mcp/src/ui.rs:4167-4209) has all three branches with the first varying on … |
| `MCP-379` | medium | `hand-written` | **implemented** | `KNOWN_SERVER_PRESETS` | `known_server_presets()` (/home/user/cyrup/crates/cyrup-mcp/src/config.rs:4196-4238) returns the five presets in order — deepwiki, context7, notion, … |
| `MCP-380` | low | `hand-written` | **implemented** | The onboarding-state file | /home/user/cyrup/crates/cyrup-mcp/src/onboarding.rs in full: `OnboardingState` (line 32-43), `load_onboarding_state` (line 62-79) hand-normalises … |
| `MCP-381` | high | `hand-written` | **missing** | `/mcp`: registration, the owner-fenced prologue and the eight-way switch | Everything in §4.1–§4.2 is absent: the owner-fenced prologue (capture `currentOwner` + a bound reload, build the synthetic `commandCtx` before the first await, await `initPromise` with the two failure notices), the `split(/\s+/)` argument split with `targetServer = parts[1]` for `reconnect` versus … |
| `MCP-382` | medium | `host-addition` | **partial** | HA-2: `/mcp`'s dynamic argument completions have no native path, no label and … | HA-2 itself has not landed anywhere. `NativeExtension` still has no `argument_completions` method (trait method list, /home/user/cyrup/crates/cyrup-ext/src/native.rs:458-683). `ExtensionHost::command_completions` (facade.rs:1737-1743) still delegates to `LiveExtension::argument_completions` … |
| `MCP-383` | medium | `hand-written` | **missing** | Port `showStatus` | The whole of §4.4 is absent: the `["MCP Server Status:", ""]` header, the per-server rows in `Object.keys` order, the disabled row `⊘ {name}: disabled (run /mcp enable {name}, then /reload)` with `continue`, the five-way first-match ladder (`connected` ✓ / `needs auth` ⚠ / `failed {N}s ago — … |
| `MCP-384` | low | `hand-written` | **missing** | Port `showTools` | §4.5 is absent: flat-mapping `toolMetadata` over non-disabled servers to the PREFIXED registered names in map-iteration order, the `No MCP tools available` empty case, and the `MCP Tools:` / blank / two-space-indented names / blank / `Total: {N} tools` block (never singularised) as one Info notify. |
| `MCP-385` | medium | `hand-written` | **missing** | Port `showPrompts` | §4.6 is absent in full: the grouped-by-server listing with servers sorted by `localeCompare` and prompts sorted in place by `commandName`, the `<required>`/`[optional]` usage rendering, the two-space `/{commandName}` row, the six-space description row, the per-group blank line, `Total: {N} … |
| `MCP-385a` | low | `hand-written` | **missing** | `/mcp prompts` opens each group with a `{serverName}:` header row | The per-group `{serverName}:` header row — unindented, no icon, plain colon, unsanitized — is absent along with its parent function. Without it the eventual listing would be one flat run of `/mcp__a__x` rows separated by unexplained blank lines. |
| `MCP-386` | high | `hand-written` | **missing** | Port `reconnectServer` / `reconnectServers` | All ten steps of §4.7 are absent: the unknown-target guard, the sequential (`for … await`, not a join) all-servers loop, `manager.close` → `connect` with the two `throwIfInactive` checks, the `needs-auth` early return with its exact warning, `buildToolMetadata` + `state.toolMetadata.set`, the … |
| `MCP-387` | high | `hand-written` | **missing** | Port `/mcp setup` and the reload-after-write flow | Absent: the `programmaticConfig` refusal (`MCP setup is unavailable when config is supplied by createMcpAdapter().`), the once-only computation of `discovery = getMcpDiscoverySummary(...)` and `loadOnboardingState()` at OPEN time with `markSetupCompleted` persisting that pre-write fingerprint, the … |
| `MCP-388` | high | `hand-written` | **missing** | Port `logoutServer` | All four steps of §4.9 are absent, including the load-bearing string `OAuth credentials were cleared for "{name}", but its connection could not be closed: {msg}` that distinguishes "credentials gone, connection alive" from a total failure, plus the usage error (`Usage: /mcp logout <server>`) and … |
| `MCP-389` | medium | `hand-written` | **missing** | Port `/mcp disable` and `/mcp enable` | The shared `disable`/`enable` arm is absent: the `programmaticConfig` refusal `"/mcp {sub} is unavailable when config is supplied by createMcpAdapter()."`, the `Usage: /mcp {sub} <server>` error, the `Server "{name}" not found in effective config` error, and the two result notices … |
| `MCP-390` | high | `host-verb` | **partial** | Port `authenticateServer` and `/mcp-auth` | The command-level flow is explicitly outstanding — `TODO(MCP-334)` at oauth.rs:3922-3929 says so. Absent: the `/mcp-auth` handler itself (no `execute_command`, MCP-381); `terminalHyperlink`'s OSC-8 emission (grep for `terminal_hyperlink` and for a `\u{1b}]8` producer across the crate finds only the … |
| `MCP-391` | medium | `host-verb` | **partial** | Port `openMcpAuthPanel` | The entry point `openMcpAuthPanel` itself does not exist. Absent: the `!hasUI` guard, the `programmaticConfig` refusal `Use /mcp-auth <server> to authenticate a server from the in-memory SDK config.`, the zero-OAuth-capable-servers warning `No OAuth-capable MCP servers are configured.` (grep finds … |
| `MCP-392` | high | `hand-written` | **missing** | Port `buildMcpPanelCallbacks`'s connection-status derivation | The whole of §4.11's `buildMcpPanelCallbacks` is absent: the per-open `authStatusFailures: Map<String,String>` (deliberately NOT session state), the eight-rung `getConnectionStatus` ladder (delete-from-map → disabled → `resolveServerUrl` throws ⇒ `failed` → the four-condition OAuth guard calling … |
| `MCP-393` | low | `hand-written` | **implemented** | Port the shared-config notice and its one-shot state | REFUTED. This unit names two things — `buildSharedConfigNoticeLines` and the one-shot state — and both are ported, byte-for-byte on the strings: … |
| `MCP-394` | critical | `hand-written` | **partial** | Port `openMcpPanel`'s orchestration and the direct-tools write-back | The orchestration is explicitly outstanding — `TODO(MCP-394)` at ui.rs:4781-4786. Absent: the `programmaticConfig` branch (notify `MCP status is shown from the in-memory SDK config; configuration discovery is unavailable.` + `showStatus`), the ZERO-SERVERS-delegates-to-`openMcpSetup(…, "empty", … |
| `MCP-394a` | medium | `hand-written` | **implemented** | A change for a server with no provenance entry is silently dropped | Two soft residues rather than behavioural gaps: the skip is an unannotated `let-else continue` with no comment naming it as upstream's deliberate … |
| `MCP-395` | high | `host-addition` | **partial** | HA-1's command leg: MCP prompts are slash commands, and there is no late … | The live half has nothing to land on and none of the three additions have been made: grep across /home/user/cyrup/crates/cyrup-ext/src, /home/user/cyrup/crates/cyrup-session-svc/src and /home/user/cyrup/crates/cyrup-tui/src for `register_late_command`, `mark_commands_dirty`, `take_commands_dirty`, … |
| `MCP-395a` | medium | `hand-written` | **implemented** | Cache-time prompt resolution and command naming | `resolve_cached_prompts` (/home/user/cyrup/crates/cyrup-mcp/src/registration.rs:1720-1765) walks the cache's server order, skips servers absent from … |
| `MCP-396` | medium | `hand-written` | **missing** | Port `parsePromptArgs`'s bash-style tokenizer | §5.3 is absent in full: the character-by-character tokenizer with `escaped` carried across iterations (so a trailing lone backslash is dropped), the backslash-is-literal-inside-single-quotes rule, quote characters RETAINED in the token, unterminated quotes running to end of input, … |
| `MCP-397` | medium | `hand-written` | **missing** | Port `resolvePromptArgs` and the usage message | §5.4 is absent: loop 1 over the declared arguments in declaration order with the named-lookup-first / positional-cursor-advances-only-on-a-miss rule, loop 2 forwarding undeclared named arguments unfiltered, the `missing` filter over required-and-empty, and `buildUsageMessage`'s `Missing required … |
| `MCP-397a` | low | `hand-written` | **missing** | An explicit empty named value for a declared optional argument is still sent | The two-loop ordering that makes an explicit empty named value survive for a declared OPTIONAL argument (`args["topic"] = ""` on the wire) while a declared REQUIRED one still fails the `missing` filter must be written in upstream's order with no `is_empty()` guard on loop 2. |
| `MCP-398` | high | `host-verb` | **missing** | Port the prompt command handler | All nine steps of §5.5 are absent: the `MCP not initialized` guard, the `promptMetadataLive`-guarded staleness check BEFORE `lazyConnect` (the guard that stops a cache-only command being refused before its server has been contacted), argument parse/resolve, the un-configured-server check, the two … |
| `MCP-399` | medium | `hand-written` | **missing** | Port `formatPromptResult` and `extractMessageText` | §5.6 is absent: the `lines.join("\n\n").trim()` flattening, the single-`user`-message-emitted-bare special case with `[{role}] ` prefixes otherwise (including a lone ASSISTANT message keeping its prefix), the skip of empty extractions, and the five content-kind placeholders `[resource {uri}]` … |

### 13i · Protocol tracer, conformance, verification

[`13i-mcp-protocol-and-verification.md`](13i-mcp-protocol-and-verification.md) — 50 units, 31 missing, 11 partial.

| id | sev | verdict | status | title | detail |
|---|---|---|---|---|---|
| `MCP-450` | high | `hand-written` | **missing** | handleSamplingRequest as a pure function of an options bag | The whole 12-step `handleSamplingRequest` free function is absent: no `SamplingOptions` bag, no `handle_sampling_request(&SamplingOptions, CreateMessageRequestParams) -> Result<CreateMessageResult, ErrorData>`, no producer of `SamplingHook`, and none of the 11 mirrored unit cases from … |
| `MCP-451` | medium | `hand-written` | **missing** | The six unsupported-sampling-feature rejections, in order (task becomes … | Missing: the ordered `match` over `CreateMessageRequestParams::{include_context, tools, tool_choice, stop_sequences}` producing the four byte-exact messages; the per-content-block guard (`MCP sampling ${type} content is not supported` / `MCP sampling assistant ${type} content is not supported`) … |
| `MCP-452` | high | `extension-owned` | **missing** | resolveSamplingModel candidate ordering and the sequential auth probe | Missing entirely: `fn sampling_candidates(available, hints, current) -> Vec<Model>` with hint-order-major / registry-order-minor appending, lowercase substring `.contains()` matching over `provider/id` \| `id` \| `name`, first-wins dedupe on `(provider, id)`, then current model, then the whole … |
| `MCP-453` | high | `extension-owned` | **missing** | Run the nested completion via cyrup-provider directly | Missing: the direct `cyrup_provider` completion call with `{systemPrompt?, messages}` and `{apiKey?, headers?, maxTokens, temperature?, metadata?, cancel}`; `max_tokens` passed through unmodified/unclamped; the composed child `tokio_util::sync::CancellationToken` cancelled by either the run … |
| `MCP-454` | medium | `extension-owned` | **missing** | Source the candidate set from the whole configured catalogue | Missing: reading `cyrup_provider::catalog::{builtin_catalog, load_catalog}` directly to build the candidate set, plus the `HostServices::{models, scoped_models, current_model}` session view. Nothing consumes the fenced `models()`/`current_model()` delegations that already exist in `owner.rs`. |
| `MCP-455` | critical | `host-verb` | **missing** | The two sampling approval gates and their formatters | Missing: `confirmSampling`'s three-branch gate (auto-approve short-circuit; explicit `has_ui: bool` sourced from the host config producing the distinct "MCP sampling requires interactive approval. Set settings.samplingAutoApprove to true to allow it without UI." message; `HostServices::confirm` … |
| `MCP-456` | medium | `hand-written` | **missing** | convertSamplingMessage, convertAssistantResult, mapStopReason | Missing: the `SamplingContent::{Single, Multiple}` normalisation; the synthetic assistant record with the literal sentinels `api: "mcp-sampling"`, `provider: "mcp"`, `model: "sampling-request"`, all-zero usage and `stopReason: "stop"`; `convertAssistantResult`'s error/aborted rethrows, … |
| `MCP-457` | low | `rmcp` | **implemented** | Sampling capability advertisement and handler-before-connect | None for this unit's obligations. Note for context only: because no `SamplingHook` producer exists (MCP-450), the sampling capability is never … |
| `MCP-458` | high | `host-verb` | **missing** | Bind sampling's model and cancellation to the live runtime owner | Missing: a sampling options bag holding two live closures over the stashed `Arc<dyn HostServices>` — `current_model()` read live, and a cancellation source composed as a child `CancellationToken` — plus the required *two independent* signal-accessor reads (once at handler entry, once inside the … |
| `MCP-459` | low | `hand-written` | **implemented** | truncateAtWord with UTF-16 length semantics | `crates/cyrup-mcp/src/registration.rs:571 pub fn truncate_at_word(text: &str, target: usize) -> String` implements the exact five-branch algorithm … |
| `MCP-460` | low | `rmcp` | **partial** | Elicitation dispatch; absent/unknown mode falls to form | There is no form-vs-url dispatch and no form handler, so the "absent/unknown mode → form" behaviour cannot be exercised or verified. Work item: implement the `match ElicitRequestParams { FormElicitationParams => handle_form_elicitation, UrlElicitationParams => handle_url_elicitation }` split plus … |
| `MCP-461` | high | `hand-written` | **missing** | handleFormElicitation's gate, review loop and edit picker | Missing in full: the `MCP Input Request\nServer: …` gate dialog with `["Continue","Decline"]` and `None`→cancel; the `properties.len() == 0` → `{action:"accept", content:{}}` short-circuit before any review screen; the per-field collect pass; the `while(true)` review loop with its deliberately … |
| `MCP-462` | low | `rmcp` | **missing** | Iterate requestedSchema.properties in document order | No iteration site exists yet. When the form handler lands it must zip `ElicitationSchema::property_order` against `ElicitationSchema::properties`; iterating the `BTreeMap` directly is the silent bug the plan names. Verify with a `z`/`a`/`m` key-order test. |
| `MCP-463` | medium | `hand-written` | **missing** | collectValidField's per-field re-prompt loop | Missing: the unbounded per-field re-prompt loop; the single-property synthetic schema built by copying `params` and replacing only `requested_schema` (so sibling fields like `message` survive), carrying `required` only when the field is required; `HostServices::notify(msg, NotifyKind::Error)` on … |
| `MCP-464` | high | `hand-written` | **missing** | coerceAndValidateFormValues, including JS Number() semantics | Missing: the whole coercion core — 13 distinct message templates across 15 throw sites over `PrimitiveSchemaDefinition`'s typed limit fields (`StringSchema::{min_length,max_length}`, `NumberSchema`/`IntegerSchema::{minimum,maximum}`, `*MultiSelectEnumSchema::{min_items,max_items}`); an explicit JS … |
| `MCP-465` | high | `hand-written` | **missing** | Final schema assertion with format as an assertion, not an annotation | Missing: compiling the original `requested_schema` with `jsonschema` + `.should_validate_formats(true)`, running it over the coerced `output`, and throwing `Invalid elicitation response: {err}`. Also missing the compiled-schema cache (the validator runs twice per field), and the decision to … |
| `MCP-466` | medium | `hand-written` | **missing** | The label-uniquifying and humanising helpers | Missing: `formatChoice(value, title)`; `uniqueLabels`/`uniqueAction` appending `…` in a `while` loop against an accumulating `used` set (required because `HostServices::select` returns the chosen *string*); `extractMultiSelectOptions` as a `match` on `MultiSelectEnumSchema::{Untitled(items.enum_), … |
| `MCP-467` | high | `hand-written` | **missing** | handleUrlElicitation, including the three -32602 rejections | Missing the whole handler: the `!allow_url` gate, `url::Url::parse` failure and the http/https scheme allowlist — all three as `ErrorData::invalid_params` (-32602); the exact 9-line confirmation dialog with `Host:` = host+port (`Url::host_str` + port) and `Full URL:` = the **raw** input string (no … |
| `MCP-468` | medium | `rmcp` | **partial** | Advertise elicitation {form, url?} with allowUrl == (mode == tui) | Nothing wires them in production: grep shows `.elicitation(` (config.rs:1090) has no caller, `ElicitationMode` is constructed only in tests, and `McpClientHandler::new` is called only at runtime.rs:1615 (a test). `runtime.rs:118` still lists "the **elicitation gate**" as work inside … |
| `MCP-469` | medium | `rmcp` | **partial** | The notifications/elicitation/complete dedupe and its notice | The hand-written half is absent. `crates/cyrup-mcp/src/state.rs` (read in full, fields listed at l.77-128) has no accepted-elicitation registry; grep of the crate for `HashSet<String>` keyed by server, `remember_url_elicitation`, and the notice text `MCP browser interaction for … completed. You can … |
| `MCP-470` | medium | `hand-written` | **partial** | handleUrlElicitationRequired for the -32042 elicitation array | Missing: decoding `ErrorData { code: ErrorCode(-32042), data }` into the elicitation array (rmcp models neither), and the sequential loop — cancel immediately if the runtime is aborted or `allow_url` is false; otherwise iterate `error.elicitations` in order, short-circuit on the first non-`accept`, … |
| `MCP-471` | high | `host-verb` | **missing** | Hold the dispatcher budget and the interaction lock across every dialog | Missing: taking a `#[must_use]` `HostCtx::begin_human_wait()` guard and the session-scoped `HostServices::human_interaction_lock` across every `select`/`input`/`confirm` in `cyrup-mcp` — the two sampling approval dialogs, the elicitation gate/field/review/edit dialogs (with the guard wrapping the … |
| `MCP-472` | low | `rmcp` | **missing** | The three URL rejections carry JSON-RPC -32602 | Missing: the three `ErrorData::invalid_params(msg, None)` returns for "URL elicitation is not supported", "URL elicitation supplied an invalid URL" and "URL elicitation only supports HTTP and HTTPS URLs", plus the discipline that every *other* throw in the two handlers stays `-32603` … |
| `MCP-473` | medium | `hand-written` | **missing** | The McpTraceEvent schema v1, exact key set and insertion order | Missing: the `#[derive(Serialize)]` event struct with the 13 fields in `createMcpTraceEvent`'s **insertion** order (`version, timestamp, direction, server, transport, kind, status, bytes, method, id, relatedRequestId, errorCode, durationMs`) — explicitly NOT the interface declaration order where … |
| `MCP-474` | high | `hand-written` | **missing** | redactTraceText, dead third branch and all | Missing: the keyword guard `\b(?:token\|secret\|password\|passwd\|api[_-]?key\|authorization\|cookie)\b` (case-insensitive) returning `"[REDACTED]"`; the three replacements (URL scheme, `bearer\|basic`, and the third — port it verbatim including the literal `"$1=[REDACTED]"` replacement string … |
| `MCP-475` | low | `hand-written` | **missing** | traceId, messageKind, messageBytes | Missing: `messageKind` (`"method" in msg` → `"id" in msg ? request : notification`, else `response`); `traceId` as a two-arm match on rmcp's `NumberOrString::{Number(i64), String(Arc<str>)}` producing the number or the literal `"[REDACTED_ID]"`; and `messageBytes` as the serialised UTF-8 length … |
| `MCP-476` | medium | `hand-written` | **missing** | McpTraceWriter: latching caps, injectable fs, serialized append queue | Missing the writer itself: injectable `append_file`/`write_file`/`mkdir` (the seam without which the `["reset","append"]` ordering test and the `maxBytes: 20` latch test cannot be unit tests); truncate-on-open with the `mkdir → writeFile("")` init chain latching `disabled` on failure; a sync, … |
| `MCP-477` | low | `open-decision` | **partial** | Trace file path derivation, and .pi to .cyrup | The side picked is (a) `.cyrup/mcp-traces/`. Still missing the rest of `createMcpTraceWriter`'s path derivation: `settings.file` used verbatim when absolute and resolved against the session cwd when relative, else `mcp-<ISO timestamp with `:`/`.` → `-`>-<≤8 base36 chars>.jsonl` inside … |
| `MCP-478` | low | `hand-written` | **partial** | isMcpTraceEnabled and the reduced transport-kind enum | Missing: the combining function `is_mcp_trace_enabled(entry, settings) = entry.trace.unwrap_or(settings.trace_enabled())` — the `??` semantics where a per-server `false` beats a global `true`; `\|\|` would be wrong. Missing the five-case test (including `{debug:true}` → false and `{trace:false}` + … |
| `MCP-479` | medium | `hand-written` | **missing** | TracingTransport<T> over rmcp::transport::Transport | Missing: a `TracingTransport<T>` newtype implementing `rmcp::transport::Transport<RoleClient>` — `send` timing the inner send and emitting one event per JSON-RPC batch member *after* it resolves (status `sent`, or `error` + rethrow); `receive` emitting an `inbound` event before returning; `close` … |
| `MCP-480` | medium | `hand-written` | **missing** | Wire the trace writer lifecycle into the server manager | Missing: a lazily-created `OnceCell<Arc<TraceWriter>>` on the manager shared across all traced servers (session-global byte/event budgets); instrumenting the transport at construction with the kind carried as an enum (post-Cut-1/Cut-3 there is one instrumentation point per kind, so … |
| `MCP-481` | low | `hand-written` | **partial** | The trace settings surface (settings.trace object, per-server trace bool) | Nothing consumes it — grep shows `TraceSettings` has no reference outside its declaration and `McpSettings::trace`, and `trace_enabled()` has no caller. The unit's own verify ("assert per-server `false` beats global `true`") cannot be written until MCP-478's combining function exists; `config.rs` … |
| `MCP-482` | n/a | `hand-written` | **implemented** | Tracker: the upstream verification surface, with the cut census | None as an index. It remains a document-only unit; nothing in `crates/` tracks the case-count parity metric it defines (see MCP-490). |
| `MCP-483` | high | `hand-written` | **missing** | Adopt the MCP conformance harness as the port's protocol gate | Missing: adopting `@modelcontextprotocol/conformance` (pinned to rmcp's `0.2.0-alpha.10`, per the docket, not upstream's 0.1.16) as the port's protocol gate, run for both `--spec-version` values, with results archived. The client contract to implement is `argv[1]` = server URL, … |
| `MCP-484` | high | `hand-written` | **missing** | A hidden cyrup mcp conformance-driver subcommand | Missing the whole driver: scenario allowlist with non-zero exit on an unknown scenario; the scripted elicitation UI with preference order `["Use default","Submit","Continue"]` then `options[0]`; `CONFORMANCE_DRIVER_DEBUG`; the definition builder; the headless OAuth round trip; `connectWithAuth` … |
| `MCP-485` | medium | `hand-written` | **missing** | A sequential runner with post-hoc log assertions | Missing: the sequential-or-parallel runner with an env-overridable results dir and timeout, the `is_baselined` literal `grep -Fqx " - $1"` check on the YAML text, the `allows_client_error` cases, live scenario discovery, the three-way per-scenario outcome, the closing summary line, and — most … |
| `MCP-486` | medium | `hand-written` | **missing** | Re-derive the expected-failures baseline; do not copy it | Missing: an empty `expected-failures` file, one observed run, and a file written from the observed failures with a mechanism-level rationale per entry, preserving the exact two-space ` - scenario` indentation the runner greps for. Copying upstream's five entries would be actively unsafe (a listed … |
| `MCP-487` | low | `hand-written` | **missing** | Allocate the ephemeral callback port in Rust | Missing: `std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port()` then drop, per driver process, plus the per-scenario tempdir OAuth store. Note this unit may be **dissolved rather than implemented**: `docs/gap-analysis/MCP-PORT-METHODOLOGY.md` ADR-0022 recommends "runner (b) — binding … |
| `MCP-488` | n/a | `hand-written` | **implemented** | Record what conformance does not cover | Three of the plan's named non-coverage bullets are not enumerated explicitly and are only implied by "the entire adapter layer": **sampling** (no … |
| `MCP-489` | medium | `open-decision` | **not-applicable** | The fate of the eight surviving fixture MCP servers | Unresolved. Ratifying it needs two sub-rulings the docket names: whether `node` may appear in the test environment at all, and whether `rmcp/server` … |
| `MCP-490` | high | `hand-written` | **partial** | Port the unit-testable share of the vitest suite | Section 13i's own share is entirely absent — zero tests for sampling, elicitation or tracing, because none of that code exists (MCP-450..MCP-481). Also missing: the case-count parity metric the unit names as its tracking measure is recorded nowhere in the tree, so "how much of the 84 in-scope … |
| `MCP-491` | medium | `open-decision` | **partial** | A home for the MCP seam tests without breaking the 7-target cap | Three concrete work items: (1) reconcile the conflict — either fold `tests/mcp/` into `bin`/`session_svc` per ADR-0021(b), or write the justification into `docs/TEST-ARCHITECTURE.md` §9.1 and raise G2's threshold; (2) the G2 count is in fact already `11`, not `8` — `crates/cyrup-tui/tests/` holds … |
| `MCP-492` | high | `hand-written` | **partial** | Port the node:test OAuth suite as a serialised group | REFUTED on its central assertion. The claim says 'three of the four surviving upstream files have no port'; all three do have substantial ports, they are just not named after the .ts files. (a) mcp-callback-server.test.ts → `the_callback_listener_end_to_end` (oauth.rs:4797, ~170 lines over a real … |
| `MCP-493` | low | `hand-written` | **missing** | A Cargo/manifest policy test pinning the rmcp feature set | Missing: a `#[test]` in `cyrup-mcp` that parses its own `Cargo.toml` (the `toml` crate is already a dependency) and asserts the rmcp feature set is exactly the five named features with `default-features = false`, that `server` and `elicitation` are absent, that the pinned version matches, and that … |
| `MCP-494` | medium | `open-decision` | **not-applicable** | The CI gate's shape, including the conformance step | Unresolved, and there is nothing to build on: no workflow file exists, so the clippy/typecheck step, the chosen gate, the `cyrup-it --features it` … |
| `MCP-495` | medium | `hand-written` | **partial** | Reconcile the test-time environment contract with cyrup's isolation rules | Two obligations unmet. (1) The doc reconciliation the unit explicitly asks for has not happened: `docs/TEST-ARCHITECTURE.md:613-614` still tells readers to "use `cyrup_test_support::env::scoped`" and l.650-657 references `cyrup_test_support::env::PROVIDER_KEYS`, but `crates/cyrup-test-support/src/` … |
| `MCP-496` | high | `hand-written` | **missing** | Live-pty verification for the elicitation dialogs and sampling gates | Missing: pty infrastructure (none in the workspace) plus a driven run of the full elicitation sequence — gate → one dialog per widget kind → 20-option multi-select with `✓ ` toggle state → review → edit → submit — and both sampling approval dialogs, screenshotting each step, checking scrollability … |
| `MCP-497` | n/a | `cut` | **not-applicable** | Coverage tracking | Verdict is `cut` — coverage tracking is out of scope by owner decision (the vitest v8 coverage config gates nothing upstream and is not run in … |
| `MCP-498` | medium | `hand-written` | **missing** | The two child-process host harnesses | Missing both: (1) a child-process startup harness asserting a direct MCP tool is registered **before** `agent_start` from a cold cache — and the unit's own note applies, that on a cold cache cyrup exposes only the `mcp` proxy tool unless HA-1 is built, so the test must state whether it is testing … |
| `MCP-499` | medium | `open-decision` | **not-applicable** | A trace-JSONL differential harness against the TS adapter | Unresolved, and doubly blocked: it needs the ADR-0027 ruling *and* the tracer (MCP-473..MCP-480) before the oracle can be bootstrapped on … |

## Rulings the skeptic overturned

Recorded because they are the measure of how much to trust the rest, and because each is a place
where the Rust implements the contract under a name the plan does not use.

| id | first pass | corrected to | why |
|---|---|---|---|
| `MCP-069` | partial | **implemented** | REFUTED. MCP-069's only obligation about this message is 'The exactly-one-transport message loses `, or socket`'. Upstream is `Server ${name} must configure exactly one of command, url, or socket` … |
| `MCP-069a` | missing | **not-applicable** | NOT-APPLICABLE by verdict class. The canonical table gives MCP-069a the verdict `hand-written` + `open-decision`, and the plan text itself says '*Filed 2026-08-20 by the v2.25.0 → v2.26.1 retarget. NOT implemented.*' … |
| `MCP-096` | partial | **not-applicable** | REFUTED / NOT-APPLICABLE. The canonical table gives MCP-096 the verdict `open-decision`, and the plan's own words are 'This is the only genuine open decision in the section, and it is a policy choice, not a missing … |
| `MCP-152` | partial | **implemented** | The hand-written half of MCP-152 is complete and tested. The description is built with `write!`-style assembly in TWO byte-identical copies (registration.rs:1196 from the cold cache, proxy.rs:3797 from live metadata) … |
| `MCP-159` | partial | **implemented** | Every item in MCP-159's **verify** list exists. (1) The re-specified catastrophic-backtracking case: proxy.rs:5471-5481 compiles `(a+)+$`, runs it, and asserts completion under a 250 ms wall-clock bound, with the … |
| `MCP-178` | partial | **implemented** | MCP-178 is verdict **open-decision**, and the Rust has already picked a side — option (a): cyrup-mcp implements the adapter's FOUR-mode, hyphen-preserving grammar (`ToolPrefix::{Server, None, Short, Mcp}`, … |
| `MCP-199` | partial | **implemented** | REFUTED — the claim rests on a misreading. owner.rs:408 is NOT 'a stale-generation no-op that always returns None': `OwnedServices` is `createOwnedUi`'s fence (owner.rs:279-340), and the `fenced!` macro (:317-337) … |
| `MCP-205` | partial | **not-applicable** | Verdict is **open-decision**, and no ruling has been recorded: registration.rs:179 says verbatim "MCP-205, unresolved" and proxy.rs:419 says "MCP-178/MCP-205 open decision". Per the verdict class this is not-applicable, … |
| `MCP-234` | partial | **not-applicable** | Verdict is **open-decision** with no ruling recorded, so by the class rule this is not-applicable rather than outstanding work. Nothing behavioural is missing: the plan's recommended (c) is 'document the split and match … |
| `MCP-291` | partial | **implemented** | REFUTED on its central point. (1) Both traits ARE implemented over the keychain — `McpCredentialStore` (credentials.rs:3103/3128) and `McpStateStore` (3173/3196) — which is what the unit's **cyrup** paragraph asks for. … |
| `MCP-314` | partial | **implemented** | REFUTED — the claim inverts the requirement. MCP-314's cyrup column asks for exactly three things and all three are present: persist the registration fields as a second keychain record (oauth.rs:3095-3111 writes … |
| `MCP-318` | partial | **implemented** | REFUTED as port work. The canonical table's verdict is `rmcp`, and the ONLY hand-written obligation the plan names is the client-auth lever — 'the lever that exists is AuthorizationManager::set_metadata: fetch or accept … |
| `MCP-333` | partial | **implemented** | REFUTED — the auditor judged MCP-333 against a scope the plan explicitly excludes. 13g's Coverage/Excluded section says: '`server-manager.ts` beyond the auth-provider seam — transport construction and the connection … |
| `MCP-393` | partial | **implemented** | REFUTED. This unit names two things — `buildSharedConfigNoticeLines` and the one-shot state — and both are ported, byte-for-byte on the strings: `shared_config_notice_lines` (ui.rs:4694-4716) reproduces the … |
| `MCP-492` | partial | **partial** | REFUTED on its central assertion. The claim says 'three of the four surviving upstream files have no port'; all three do have substantial ports, they are just not named after the .ts files. (a) … |


<!-- BEGIN 13i PLAN — owned by MCP_13I_SCOPING; a non-13i plan is appended after the END marker -->

## Plan — §13i (protocol tracer, conformance, verification), scoped 2026-08-22

> **Scope fence.** Everything between this heading and the `END 13i PLAN` marker below is §13i and
> nothing else. A second plan covering the non-13i sections is appended *after* that marker; the two
> do not overlap and neither rewrites the census above.

This is the output of a scoping task, not of an implementation wave: **no production code was
changed**. `13i` was singled out because it is the only section where the work is *building absent
surfaces* rather than realigning existing ones (31 of 50 units `missing`), and that is a different
job — one that goes wrong if units are picked off a list in id order.

### Method, and what "measured" means in this section

Every one of the 42 open units was re-read **against the Rust as it stands on 2026-08-22**, not
against the audit. That distinction is the whole value of this pass: the census is explicitly "as of
the audit and is not rewritten by later work", and at least one wave has landed in 13i's territory
since. Four kinds of evidence were used, and they are labelled below so a reader can discount them
correctly:

* **grep/read** — the symbol is present or absent in `crates/cyrup-mcp/src` (and, where the unit
  names another crate, there). This is the same class of evidence the census carries.
* **manifest** — read out of `Cargo.toml` / `Cargo.lock`, or out of the vendored `rmcp-3.1.4`
  sources under `~/.cargo/registry`.
* **executed** — a command was actually run and its output is quoted. Four facts in this plan are of
  this kind, and they are the only ones: `cargo metadata`'s test-target count, the availability of
  `node`/`npx`, the conformance CLI's own `--help`, and its client scenario list.
* **not verified** — stated as such. No `cargo build`, `cargo test` or `cargo clippy` was run for
  this triage; it changes no code, so there was nothing of its own to gate.

### The count: 42 is correct

Verified rather than inherited, by counting the §13i census table above (rows `MCP-450`..`MCP-499`,
50 rows): 31 `missing` + 11 `partial` = **42 open**, plus 4 `implemented` (`MCP-457`, `MCP-459`,
`MCP-482`, `MCP-488`) and 4 `not-applicable` (`MCP-489`, `MCP-494`, `MCP-497`, `MCP-499`). Severity
of the 42: 1 critical, 15 high, 17 medium, 9 low — i.e. the "16 critical-or-high" figure in
[§By section](#by-section) is also correct.

**But the *composition* of the 42 has changed since the audit, and three of its headline facts have
not survived contact with the tree.** The critical unit is no longer missing; the highest-severity
`host-addition` blocker the scoping brief names has already landed; and the test-architecture unit is
chasing a guardrail the workspace broke twenty ways over.

### Triage — all 42, in one table

`triage` is one of **confirmed-missing** (the census row holds), **actually-present** (the Rust
implements it, under a name or in a file the audit did not look in), or **blocked-on-X** (the unit's
own body may be writable, but the obligation it is filed to meet cannot be met until X lands).
`blocked-on` beats `confirmed-missing` where both apply: the point of the column is scheduling.

| id | sev | census | triage | evidence / what actually blocks it |
|---|---|---|---|---|
| `MCP-450` | high | missing | confirmed-missing | grep: no `handle_sampling_request`, no `SamplingOptions`. The **seam is present** — `runtime::SamplingHook` (runtime.rs:1388), `ClientHandlerConfig::sampling` (:1413), `ServerManager::set_sampling_config` (server_manager.rs:1338) — so the pure function is writable today; only its *installation* is blocked (see `MCP-458`) |
| `MCP-451` | medium | missing | confirmed-missing | grep for `include_context`, `tool_choice`, `stop_sequences`, `is not supported`: zero hits in the crate |
| `MCP-452` | high | missing | confirmed-missing | grep: no `resolve_sampling_model` / `sampling_candidates`. `HostServices::{models, scoped_models, current_model}` exist and are already fenced in `owner.rs`, so this is unblocked |
| `MCP-453` | high | missing | confirmed-missing | manifest: `cyrup-provider` **is already a dependency** of `cyrup-mcp` (Cargo.toml:37, with the layering argument written at :35-36). Unblocked |
| `MCP-454` | medium | missing | confirmed-missing | grep: nothing reads `cyrup_provider::catalog`. Unblocked |
| `MCP-455` | critical | missing | **actually-present** | owner.rs:600-770 — the four literal strings, `SamplingApproval{auto_approve, has_ui, dialog}`, the three-branch `confirm_sampling`, `format_request_approval`, `format_response_approval`, `sampling_block_type`, with tests at owner.rs:1079-1160 covering all three branches, both formatters and the `has_ui && dialog.is_none()` wiring-bug arm. Residual: **no production constructor of `SamplingApproval`** — that is `MCP-450`'s job. See the correction below |
| `MCP-456` | medium | missing | confirmed-missing | grep: no `convert_sampling_message` / `map_stop_reason`; the sentinels `api: "mcp-sampling"` and `provider: "mcp"` appear nowhere (`"sampling-request"` appears once, in an owner.rs *test* fixture) |
| `MCP-458` | high | missing | **blocked-on 13a + 13c** | The bag itself is writable; the two live closures it must bind cannot be *reached* until a session runs the runtime. `initialize_mcp` still has no non-test caller (`MCP-008`/`MCP-011`), and `ConnectionBuilder::with_handler_factory` (runtime.rs:2289) has **no caller at all** — the manager never supplies a factory, which is `MCP-118`/`MCP-120`/`MCP-122` in 13c |
| `MCP-460` | low | partial | confirmed-missing | grep: no `handle_form_elicitation` / `handle_url_elicitation`, so there is no dispatch site for absent/unknown-mode to fall through. Folds into `MCP-461`/`MCP-467` |
| `MCP-461` | high | missing | confirmed-missing | grep: zero. Needs an `input` arm on `McpDialog` (see `MCP-471`) |
| `MCP-462` | low | missing | confirmed-missing | manifest: rmcp 3.1.4 **does** expose `ElicitationSchema::property_order: Option<Vec<String>>` (model/elicitation_schema.rs:1142), so this is unblocked and is one loop inside `MCP-461`, not a schedulable unit |
| `MCP-463` | medium | missing | confirmed-missing | grep: zero. Blocked only by `McpDialog` having no `input` arm today |
| `MCP-464` | high | missing | confirmed-missing | grep: no `coerce_and_validate`. The largest single body in the elicitation cluster (13 message templates / 15 throw sites) |
| `MCP-465` | high | missing | confirmed-missing | manifest: `jsonschema` **is already a dependency** of `cyrup-mcp` (Cargo.toml:118, workspace 0.46.9). Cheaper than the census implies — no new crate decision |
| `MCP-466` | medium | missing | confirmed-missing | grep: no `format_choice` / `unique_labels` / `extract_multi_select_options` |
| `MCP-467` | high | missing | confirmed-missing | grep: zero; `url` parsing and the three `-32602` returns absent (`invalid_params`: zero hits crate-wide) |
| `MCP-468` | medium | partial | **blocked-on 13a + 13c** | `build_client_capabilities` (runtime.rs:1220) and `ElicitationMode` (:1244) are built and correct; the same two missing callers as `MCP-458` are why nothing wires them |
| `MCP-469` | medium | partial | **blocked-on 13c `MCP-122`** | The rmcp half exists (`ElicitationCompleteHook` runtime.rs:1383, `ClientHandlerConfig::elicitation_complete` :1421). The hand-written half **is the same accepted-elicitation registry `MCP-122` is filed for** — grep of `state.rs` finds no registry on either unit's behalf. Build once, credit both |
| `MCP-470` | medium | partial | confirmed-missing | The *consumer* seam exists — `ProxyEnv::handle_url_elicitation_required` (proxy.rs:1486), `UrlElicitationAction` (:1302), the three action strings rendered at :3730-3738 — with a test-only implementor. The producer (decoding `ErrorCode(-32042)`'s elicitation array, and the sequential loop) is absent |
| `MCP-471` | high | missing | **actually-present** | owner.rs:478-596. `McpDialog` takes `human_interaction_lock().acquire()` **and** `HostCtx::begin_human_wait()` in one `enter()`, exposes `confirm` and `select`; `state.rs:210-232` builds it from the **fenced** `OwnedServices` and carries the recorded dispatch ctx; `extension.rs:730,1058` is the producer half. Residual: no `input` arm, and the dialogs it must cover (elicitation) do not exist yet. See the correction below |
| `MCP-472` | low | missing | confirmed-missing | Three `ErrorData::invalid_params` returns; a tail of `MCP-467`, not a unit |
| `MCP-473` | medium | missing | confirmed-missing | grep: no `McpTraceEvent` anywhere in `crates/` |
| `MCP-474` | high | missing | confirmed-missing | grep: no `redact_trace`. Prior art that is **not** a substitute: `cyrup-ext-subagents`' `model::redact_verify_env` is a different algorithm for a different input |
| `MCP-475` | low | missing | confirmed-missing | grep: no `trace_id` / `message_kind` / `message_bytes`. Tail of `MCP-473` |
| `MCP-476` | medium | missing | confirmed-missing | grep: no writer type. The injectable-fs seam is the unit's whole testability argument and has no counterpart today |
| `MCP-477` | low | partial | confirmed-missing (remainder) | The `.cyrup` side is **taken and built**: `dirs::TRACE_DIR` (dirs.rs:116), `Dirs::trace_dir()` (:201), pinned by a test at :1422. Missing: the `settings.file` absolute/relative rule and the `mcp-<ISO>-<base36>.jsonl` name. Folds into `MCP-476` |
| `MCP-478` | low | partial | confirmed-missing (remainder) | Both inputs exist — `McpSettings::trace_enabled()` (config.rs:1267) and `ServerEntry::trace` (:876) — and the `??`-semantics combiner does not. One function |
| `MCP-479` | medium | missing | confirmed-missing | grep: no `TracingTransport`. Unblocked — `select_transport` and both transport arms exist in `runtime.rs` |
| `MCP-480` | medium | missing | confirmed-missing | The three flush sites are already **named** in `server_manager.rs` (:2337, :2479, :1432) as awaiting T-10/MCP-477. Touches a hot shared file — schedule it with, not against, other `server_manager.rs` work |
| `MCP-481` | low | partial | **actually-present** | `TraceSettings{enabled,file,max_bytes,max_events}` (config.rs:1620-1633), `ServerEntry::trace` (:876), `trace_enabled`/`trace_max_bytes`/`trace_max_events` accessors (:1267+). Its entire residual is "nothing consumes it", which is by definition `MCP-476`/`MCP-478`/`MCP-480`. **Do not schedule it**; it closes when they land |
| `MCP-483` | high | missing | confirmed-missing (feasibility **executed**) | No conformance artefact of any kind exists (`find` over the repo for `*conformance*` and `*expected-failures*`: zero). Feasibility measured here: `node v22.22.2` and `npx` are on PATH, and `npx -y @modelcontextprotocol/conformance@0.2.0-alpha.10 --help` runs. Pin drift to note when adopting: `npm view` lists `0.2.0-alpha.11` as published and `0.1.16` as the newest stable |
| `MCP-484` | high | missing | **blocked-on a reference checkout** | The hidden-subcommand *precedent* is real and reusable (`__subagent-runner`, `__intercom-broker` pre-dispatch in `crates/cyrup/src/main.rs:115,131`). Two real constraints: (i) rmcp's `conformance/src/bin/client.rs` — the plan's "working reference" — is **not in the published crate** (`~/.cargo/registry/.../rmcp-3.1.4` contains no `conformance` path), so it needs a git checkout of the rmcp repo; (ii) `MCP-260`'s keyring-helper host half is also unbuilt (credentials.rs:65-66), and 13i says design the two subcommands together |
| `MCP-485` | medium | missing | confirmed-missing | Runner home exists (`xtask` is a real member with a real binary). Blocked only by ADR-0022's sequential-vs-parallel ruling, which also decides `MCP-487` |
| `MCP-486` | medium | missing | **blocked-on 483+484+485** | It is by construction an *observed* artefact — an empty baseline, one run, then the file. Copying is what the unit forbids. **Executed evidence that copying would now be worse than the unit says**: `conformance@0.2.0-alpha.10 list --client` returns **43** client scenarios, **33** of them `auth/*` — against upstream's five-entry baseline written for `0.1.16`, and against `MCP-PORT-METHODOLOGY.md` §5.4's "thirty `auth/*` scenarios", which has itself drifted |
| `MCP-487` | low | missing | confirmed-missing (may dissolve) | `TcpListener::bind("127.0.0.1:0")` per driver process. ADR-0022 choosing the rmcp shape deletes this unit rather than implementing it — the section's own text says so |
| `MCP-490` | high | partial | confirmed-missing (remainder, **smaller than filed**) | The row's claim "zero tests for sampling, elicitation or tracing" is now **half wrong**: `owner.rs`'s `#[cfg(test)]` module tests the sampling consent gate and both formatters. True for elicitation and tracing. The case-count parity metric is still recorded nowhere |
| `MCP-491` | medium | partial | **blocked-on a workspace ruling, and out of 13i's scope** | See the correction below. Measured with `cargo metadata`: the workspace has **20** `[[test]]` targets, not the 8 G2 permits or the 11 the census row cites, and **five** non-`cyrup-it` crates still carry `tests/` in violation of G1. `cyrup-it/tests/mcp/` already exists as a declared target (Cargo.toml:214-220). No `.github/workflows/` exists, so neither guardrail is enforced |
| `MCP-492` | high | partial | confirmed-missing (remainder) | Already refuted once by the skeptic pass; the ports exist (`the_callback_listener_end_to_end`, oauth.rs:4655). Residual is the serialised-group arrangement and the named gaps, not a port |
| `MCP-493` | low | missing | confirmed-missing | grep: no `CARGO_MANIFEST_DIR` reader in `cyrup-mcp`. manifest: `toml = "1.1.2"` is already a dependency (Cargo.toml:111) and the feature-set rationale is already written as prose at Cargo.toml:45-70 — this unit turns that comment into an assertion. Cheapest unit in the section |
| `MCP-495` | medium | partial | confirmed-missing (remainder, **much smaller than filed**) | See the correction below. `PROVIDER_KEYS`, `FEATURE_GATE_KEYS`, `SCRUBBED_KEYS`, `assert_no_ambient_provider_credentials()` and a `hermetic()` `env_clear`+allowlist child builder **all exist**, at `crates/cyrup-it/tests/support/env.rs` — not at `cyrup_test_support::env`, which is what the audit grepped for |
| `MCP-496` | high | missing | **blocked-on new infrastructure + the E-waves** | Two independent blocks. (i) `Cargo.lock` contains **no pty crate** (`portable-pty`, `expectrl`, `rexpect`: absent), so the harness is a workspace-level dependency decision, not MCP work. (ii) There is nothing to drive: every dialog it must screenshot is `MCP-461`..`MCP-467` |
| `MCP-498` | medium | missing | **blocked-on 13a + 13c — no longer on HA-1** | See the correction below. HA-1 has landed. What still gates the "registered before `agent_start`" assertion is discovery (`MCP-119`, missing) and `on_session_start` (`MCP-008` partial / `MCP-011` missing). Target and conventions are ready: `cyrup-it`'s `mcp`/`bin` targets and the `CYRUP_IT_BIN_*` variables |

Distribution of the 42: **31 confirmed-missing**, **3 actually-present**, **8 blocked-on-X**. Six of
the 31 were filed `partial` and are confirmed-missing only in their *remainder* — `MCP-460`,
`MCP-477`, `MCP-478`, `MCP-490`, `MCP-492`, `MCP-495` — which is why the table says so rather than
re-grading them to `missing`. In two of those six the remainder is materially smaller than the census
row implies, and that is stated on the row.

### Corrections the tree forces on the census

Recorded here rather than by editing the rows above, because the census is the audit's record and
this is a later reading of a moved tree.

**1. `MCP-455` — the section's only `critical` unit is not missing.** It landed in `owner.rs`
alongside `MCP-471`, deliberately placed there rather than in the `sampling.rs` the module map
assigns it, with the placement delta argued in that file's module doc (owner.rs:53-61). The gate, its
three branches in upstream's order, the fail-closed polarity, the four literal strings and both
formatters are present and tested. **The honest grade is `partial`, and the residual is not this
unit's**: nothing constructs a `SamplingApproval` outside tests because `handleSamplingRequest`
(`MCP-450`) does not exist. Consequence for scheduling: 13i's critical count is **0 open**, and the
sampling wave gets cheaper by an `M`.

**2. `MCP-471` — present as a primitive, incomplete as a rule.** `McpDialog` is exactly the "one type
both gates go through" the unit asks for, and the `HumanInteractionLock` + `begin_human_wait` pair is
taken in one place. What is unfinished is the part that cannot be finished yet: the unit's obligation
is *coverage* — "across **every** dialog" — and the elicitation dialogs do not exist. It also has no
`input` arm, which `MCP-463` requires. **Treat it as a standing coverage rule attached to waves E1-E3
plus a one-method extension, not as a unit to schedule on its own.**

**3. `MCP-498` is no longer blocked on HA-1 — HA-1 has landed.** The scoping brief inherited
`13i-mcp-protocol-and-verification.md:1648`'s note that the child-process harness "cannot assert the
cold-cache case until HA-1 exists". That is now stale: HA-1's handle is in the tree as
`cyrup_ext::LateRegistrar` — `HostLateRegistrar` at facade.rs:120-150, `register_late_tool`/
`register_late_command` at facade.rs:707-745, the defaulted `NativeExtension::set_late_registrar` at
native.rs:697, and `cyrup-mcp`'s consumer half at extension.rs:118/783 and registration.rs:2022. Note
the shape chosen is **narrower than either option the census row names**: a capability object with
`owner` bound at construction, not a `Weak<ExtensionHost>` — which is why a grep for `set_ext_host`
still returns nothing and a reader could conclude the opposite. The 13a census row for `MCP-037` is
therefore also stale and should be re-graded by whoever owns 13a — flagged, not edited here. What is
*not* stale is `MCP-395`: the command leg's downstream refresh does not exist (see the HA
recommendation below), and no 13i unit needs it.

**4. `MCP-495`'s first obligation is a documentation repoint, not a build.** The audit grepped
`crates/cyrup-test-support/src/` for an `env` module, correctly found none, and concluded the
mechanism was absent. It exists one crate over, at `crates/cyrup-it/tests/support/env.rs`, including
the R5 guard function verbatim from `docs/TEST-ARCHITECTURE.md`'s own snippet and a `hermetic()`
child builder implementing the R5 layer-1 allowlist. So the fix is: repoint
`docs/TEST-ARCHITECTURE.md:613-614,650-657` at the module that exists, **or** promote it into
`cyrup-test-support`. Both are cheap; the doc is currently telling contributors to call something
that does not exist.

**5. `MCP-491` is chasing a guardrail that is already broken workspace-wide, and 13i should not own
the fix.** Measured, not read: `cargo metadata --no-deps` reports **20** test targets against G2's
`<= 7`, and `crates/{cyrup,cyrup-provider,cyrup-tools,cyrup-tui,cyrup-permission-system}/tests/` all
exist against G1's "matches nothing". Meanwhile `cyrup-it/tests/mcp/` is a declared target with a
real assembled-session test in it, so MCP has *de facto* taken the 8th slot the methodology said it
could not have. Nothing about that is MCP-specific any more. **Recommendation: strike `MCP-491` from
the 13i backlog and re-file it as a test-architecture item**, leaving 13i with the one sentence it
actually owns — that `cyrup-it/tests/mcp/` is where MCP seam tests go.

### Dependency order

Four kinds of edge, strongest first. An arrow means "the tail must land before the head can meet its
obligation" — not merely before it compiles.

**Edge type 1 — gated on another section (hard).** These are the only 13i units that cannot be
*finished* inside 13i:

```
13a MCP-008 (partial) + MCP-011 (missing)   ──▶  MCP-458, MCP-468, MCP-498
   (on_session_start has an empty body, so initialize_mcp has no production caller)

13c MCP-118/120/122  ──▶  MCP-458, MCP-468, MCP-469
   (ConnectionBuilder::with_handler_factory, runtime.rs:2289, has no caller: the manager
    never installs a handler factory, so no sampling/elicitation hook can reach a connection)

13c MCP-119 (discovery, missing)  ──▶  MCP-498 (the "tool registered before agent_start" premise)

13f MCP-260 (partial: `crates/cyrup/src/mcp_keyring_helper_cmd.rs` does not exist)
                     ──▶  MCP-484 (13i asks for the two hidden subcommands to be designed together)
```

`MCP-469` is a special case of the second edge and should be **merged with `MCP-122`, not sequenced
after it**: they are one accepted-elicitation registry filed twice, once from the manager's side and
once from the handler's.

**Edge type 2 — gated inside 13i.**

```
MCP-450 ──▶ MCP-451, MCP-452, MCP-453, MCP-454, MCP-456   (all are its steps)
MCP-455 ──▶ (nothing; it is already present and waiting for MCP-450 to call it)
MCP-461 ──▶ MCP-462, MCP-463, MCP-464, MCP-465, MCP-466   (all live inside its loops)
MCP-467 ──▶ MCP-472                                        (its three rejection sites)
MCP-473 ──▶ MCP-474, MCP-475                               (the event's own fields)
MCP-476 ──▶ MCP-477(remainder), MCP-479, MCP-480           (nothing to write without a writer)
MCP-478 ──▶ MCP-480                                        (the gate the wiring consults)
MCP-483 ──▶ MCP-484 ──▶ MCP-485 ──▶ MCP-486                (strictly serial; 486 needs an observed run)
MCP-461..467 ──▶ MCP-496                                   (nothing to drive in a pty before then)
MCP-450..481 ──▶ MCP-490's 13i share                       (tests for code that must exist first)
```

**Edge type 3 — gated on a ruling, not on code.** `MCP-485`/`MCP-487` on **ADR-0022** (sequential
fixed-port vs parallel `:0`; choosing `:0` per rule R4 *deletes* `MCP-487`). `MCP-489` on **ADR-0023**
(fixture strategy) — and note the environment can support either arm: `node v22.22.2` is present.
`MCP-491`/`MCP-494` on **ADR-0021**. `MCP-499` on **ADR-0027**, and it is doubly blocked: it also
needs the tracer (`MCP-473`..`MCP-480`) before an oracle can exist, and `pi-mcp-adapter` is **not
checked out** in this environment.

**Edge type 4 — none.** `MCP-493` depends on nothing at all.

### The `host-addition` recommendation (HA-1 / HA-2 / HA-3)

**Recommendation: 13i should schedule none of them, and should stop citing HA-1 as a blocker.**

The section's own analysis already reached this conclusion —
`13i-mcp-protocol-and-verification.md:1718` says the three neighbours "are owned elsewhere and none of
them gates sampling, elicitation or tracing" — and the tree now agrees on each one:

* **HA-1 (late tool registration) — the handle has LANDED**, in both legs: `LateRegistrar::register_tool`
  and `::register_command`, the latter firing an `on_commands_changed` callback. That is `MCP-037`;
  `MCP-037a`, HA-1's other half, was already graded `implemented` in the census. It is **not** all of
  HA-1's unit family: `MCP-395`'s downstream half is still absent — grep across `cyrup-ext`,
  `cyrup-session-svc` and `cyrup-tui` for `mark_commands_dirty` / `take_commands_dirty` /
  `commands_dirty` returns nothing — so a late-registered *command* reaches the registry but nothing
  rebuilds the `/` surface from it. **No 13i unit depends on that half.** The one 13i unit that ever
  mentioned HA-1, `MCP-498`, is unblocked on that axis; what still gates it is 13a/13c, which is a
  different conversation and should be recorded as such rather than left attached to HA-1's name.
* **HA-2 (argument completions, `MCP-041`/`MCP-382`) — NOT landed, and irrelevant here.**
  `ExtensionHost::command_completions` still routes only to `LiveExtension::argument_completions`
  (facade.rs:1867 → host/live.rs:1593); there is no native arm. Nothing in 13i touches slash-command
  completion. It gates 13h's `/mcp <TAB>` and nothing else.
* **HA-3 (overlay geometry, `MCP-368`) — NOT landed, and irrelevant here.**
  `ExtensionOverlay::box_rect` (cyrup-tui/src/overlay.rs:112) is still one hardcoded rect and there is
  no `OverlayOptions`. 13i renders no overlay: its dialogs are `confirm`/`select`/`input`. The one
  place a reader might expect a link — `MCP-496`'s pty run — screenshots *dialogs*, not panels.

The corollary is the useful part: **13i is not waiting on the host.** Every remaining external edge
points at 13a and 13c, i.e. at `cyrup-mcp` itself.

### Waves

Sized in the shape that worked in PR #30 — **grouped by shared obligation, never by file**. The rule
that produced this grouping is the one the brief names: if two units must agree on a byte-exact
string, an ordering, or a state machine, they go in one agent's set even when that means one agent
touches three files; and the file that carries an obligation travels with the obligation, not with
its directory. `owner.rs` is the live example — `MCP-455` and `MCP-471` landed together because they
share `McpDialog`, and splitting them by file would have put the gate in one set and the primitive it
calls in another.

Waves are ordered by dependency, not by severity. Sizes are the section's own S/M/L, summed.

| wave | units | obligation held in common | size | may start |
|---|---|---|---|---|
| **T1 — the trace record** | `MCP-473`, `MCP-474`, `MCP-475` | one serialised event: the 13-key **insertion** order, the redactor (dead third branch and all), and the three field derivations. All three are byte-exactness against one schema; they cannot be split without splitting a golden vector | S+S+S | **now** |
| **T2 — the writer and its wiring** | `MCP-476`, `MCP-477`(rem.), `MCP-478`, `MCP-479`, `MCP-480`, closes `MCP-481` | one budget and one lifecycle: latching caps, the injectable fs seam, path derivation, the `??` enable-gate, the transport newtype, and the three flush sites already named in `server_manager.rs` | M+S+S+M+S | after T1 |
| **S1 — the sampling handler** | `MCP-450`, `MCP-451`, `MCP-452`, `MCP-453`, `MCP-454`, `MCP-456`, closes `MCP-455` | one 12-step function and its steps. The rejections, the candidate ordering, the nested completion, the catalogue read and the conversions are all *inside* `handleSamplingRequest`; the consent gate is already written and this wave is what calls it | M+S+M+M+S+M | **now** |
| **S2 — sampling's live bindings** | `MCP-458` | the two live closures and the two independent signal reads | M | after 13a `MCP-008`/`MCP-011` **and** 13c's handler-factory install |
| **E1 — the form handler core** | `MCP-461`, `MCP-462`, `MCP-463`, `MCP-466`, + `McpDialog::input` | one review loop: the gate, document-order iteration, the per-field re-prompt, and the label uniquifier the `select`-returns-a-string design forces. Every one of them is a call *inside* the loop | M+S+S+S | **now** |
| **E2 — coercion and validation** | `MCP-464`, `MCP-465` | one value pipeline: JS `Number()` semantics and 13 message templates feeding a `jsonschema` assertion with `should_validate_formats(true)`. Split them and the messages drift from the assertion | M+M | with or after E1 |
| **E3 — the URL leg** | `MCP-467`, `MCP-472`, `MCP-460`, `MCP-470` | one refusal policy: the `allow_url` gate, the parse and scheme checks, the three `-32602`s (and the discipline that every *other* throw stays `-32603`), the dispatch fallthrough, and the `-32042` array loop that consumes the same verdict | M+S+S+S | after E1 |
| **E4 — the elicitation registry** | `MCP-469` **merged with 13c `MCP-122`**, `MCP-468` | one accepted-elicitation registry and one capability wiring. Filed twice across two sections; build once | S+S | after 13c's handler-factory install |
| **C1 — the conformance driver** | `MCP-483`, `MCP-484`, co-designed with 13f `MCP-260` | one process contract: `argv[1]`, the four `MCP_CONFORMANCE_*` variables, the scenario allowlist, the scripted UI preference order, `MAX_AUTH_ROUND_TRIPS = 3`. Needs an rmcp **repo** checkout for the reference client | S+M | **now**, once the checkout is available |
| **C2 — the runner and its baseline** | `MCP-485`, `MCP-486`, `MCP-487`(or its dissolution) | one green/red verdict: the runner, the post-hoc log greps, the port strategy, and a baseline **derived from an observed run** | S+S+S | after C1 **and** ADR-0022 |
| **V1 — the cheap standing gates** | `MCP-493`, `MCP-495`(rem.) | one class of assertion: pin a policy that is currently only a comment. Independent of everything | S+S | **now** |
| **V2 — the test-corpus reckoning** | `MCP-490`(rem.), `MCP-492`(rem.) | one measure: the case-count parity metric and the serialised OAuth group. Both are about the *shape* of the corpus, not new behaviour | L+M | after S1/E1-E3/T1-T2 |
| **V3 — the process and terminal harnesses** | `MCP-498`, `MCP-496` | one thing a unit test cannot see: a real child process and a real terminal. Grouped because both are infrastructure-first and both fail the same way if faked | M+M | 498 after 13a/13c `MCP-119`; 496 after E1-E3 **and** a pty dependency ruling |
| **not scheduled** | `MCP-481`, `MCP-471`, `MCP-491` | closed by T2 / a standing rule across E1-E3 / re-filed as workspace test-architecture work | — | — |

**Startable today with no external dependency: T1, S1, E1, V1** — and C1 as soon as the rmcp checkout
is available. That is four independent agents' worth of work landing in four different places: a new
trace module, a new sampling module, a new elicitation module, and two small assertions.

**The one file three of them collide on is `lib.rs`.** T1, S1 and E1 each create a module and each
must declare it, which is the classic one-line merge conflict between agents who otherwise never meet
— and it is also what pushed `MCP-455` into `owner.rs` in the first place (owner.rs:53-57 says so).
Decide it once, before the waves start: either declare `trace`, `sampling` and `elicitation` in
`lib.rs` as empty modules up front, or nominate one wave to own the declarations. Do not let three
agents each discover it.

**One coverage rule binds E1-E3 and S1 across their agents**, and it is the thing most likely to be
lost by splitting: **every** `confirm`/`select`/`input` this crate performs goes through `McpDialog`
(`MCP-471`), which is the only place the interaction lock and the P-3 guard are taken. An agent that
writes a dialog against `HostServices` directly will compile, pass, and silently break the rule.

### Rulings that should be taken before the waves they gate

None of these is 13i's to take; each is named with what it unblocks so it can be asked for once.

1. **ADR-0022 — the conformance runner's port strategy.** Gates **C2**. Choosing rmcp's shape (`:0`,
   parallel) also *deletes* `MCP-487`. `docs/TEST-ARCHITECTURE.md`'s R4 already points that way.
2. **ADR-0027 — does `pi-mcp-adapter` stay checked out?** Gates `MCP-499` entirely. It is not checked
   out in this environment. If the answer is no, the adapter layer loses its only oracle and that
   should be *recorded* as a confidence reduction rather than absorbed.
3. **ADR-0023 — fixture strategy** (`MCP-489`). Not on any wave's critical path, but it decides what
   E1-E3 and T1-T2 are tested *against*. `node` is available here, so both arms are live.
4. **A pty dependency for the workspace.** Gates `MCP-496`. Not an MCP decision — no pty crate is in
   `Cargo.lock` at all.
5. **The `metadata` pass-through for sampling** (13i's open decision 1). One line, inside **S1**.
   The section recommends dropping it and recording the divergence.

### What this plan does not claim

* No unit was verified by building or running anything, with the four exceptions labelled
  **executed** above. Everything else is a reading of source, and carries the same false-positive
  risk the census's own provenance note describes.
* The three `actually-present` rulings are the ones most worth re-checking before scheduling, because
  a wrong one costs a wave. Each cites file and line so the check is cheap.
* Sizes are the plan's own S/M/L, carried forward unmodified. They were not re-estimated against the
  Rust, and at least two look generous in the light of what already exists (`MCP-465`'s crate is
  already a dependency; `MCP-493` is a comment turned into an assertion).
* The 13a `MCP-037` row and the 13c `MCP-118`..`MCP-122` rows are cited as they stand in the census.
  `MCP-037` is known stale (correction 3); the 13c rows were **not** re-verified against the tree by
  this pass and may have moved the same way.

<!-- END 13i PLAN -->

## Plan — the `high` backlog outside §13i, scoped 2026-08-22

> **Scope fence.** This plan covers the open `high` units in **13a…13h**. §13i is planned above and
> is not touched here; neither plan rewrites the census. Where this pass found a census row that no
> longer describes the tree it says so here rather than editing the row, for the same reason the 13i
> plan gives: the census is the audit's record of 2026-08-21.

Like the 13i pass, this is a scoping task: **no production code was changed.**

### Scope, and the arithmetic behind "73"

[§High-severity open work](#high-severity-open-work) lists 73 rows, and **15 of them are 13i**
(`MCP-450`, `452`, `453`, `458`, `461`, `464`, `465`, `467`, `471`, `474`, `483`, `484`, `490`,
`492`, `496`). The set this plan owns is therefore **58**, not 73:

| § | open `high` | ids |
|---|---:|---|
| `13a` | 10 | `008` `009` `010` `011` `014` `023` `025` `029` `037` `043` |
| `13b` | 8 | `068` `070` `073` `075` `076` `084` `092` `094` |
| `13c` | 20 | `100` `101` `105` `109` `114` `115` `115a` `116` `119` `124` `125` `126` `131` `134` `135` `139` `140` `143` `144` `145` |
| `13d` | 3 | `164` `191` `196` |
| `13e` | 6 | `207` `214` `214a` `217` `231` `249` |
| `13f` | 1 | `260` |
| `13g` | 2 | `324` `326` |
| `13h` | 8 | `381` `386` `387` `388` `390` `392` `395` `398` |

(The by-section counts in [§By section](#by-section) are *critical+high*; the per-section numbers
above are `high` alone, which is why they differ by exactly that section's open criticals.)

### Method, and what "measured" means in this section

* **grep/read** — the symbol is present or absent in the tree as it stands on 2026-08-22. Every
  file:line below was re-read at that date, not copied from the census.
* **task-file** — read out of `.flux/`, which records what has already been queued or completed.
* **not verified** — stated as such. **Nothing in this plan was executed.** No `cargo build`, no
  `cargo test`, no `cargo clippy`, and — the one that matters most — **no upstream measurement**:
  `tmp/pi-mcp-adapter` is **not checked out in this container** (`tmp/pi-mcp-adapter/package.json`
  does not exist), so the PR #30 rule "measure, do not read" cannot be applied here at all. Step 0
  of every wave below is re-cloning it (`github.com/nicobailon/pi-mcp-adapter`, tag `v2.26.1` =
  `fafae21`); an agent that skips that step is reasoning about upstream, which is the failure mode
  the methodology exists to prevent.

### The ledger's error rate, measured before anything was scheduled

The brief asked for a spot-check on the assumption that the audit's false-positive rate was non-zero
by construction. It is much larger than a sampling error, and most of it is not the audit's fault:
**at least four bodies of work have landed since 2026-08-21 — waves 1, 4 and 5 and the HA-1 task —
and only two of them wrote a re-grade back into this file.** Wave 4, which appears to have built
`server_manager.rs` outright, is named only in passing inside wave 5's prose and has no update
table of its own; that single omission accounts for six of the wrong rows below.

**43 of the 58 rows were re-checked** — all 25 `missing` rows, plus 18 of the 33 `partial` rows
chosen for having a cheap decisive grep. Three verdicts are used:

* **holds** — the row still describes the tree.
* **superseded** — the implementation is present and the row's obligations appear met.
* **overstated** — the row's *leading* claim is false; a smaller residual may survive, and was not
  sized by this pass.

| verdict | rows | share of the 43 |
|---|---:|---:|
| holds | 23 | 53% |
| superseded | 16 | 37% |
| overstated | 4 | 9% |

**20 of 43 re-checked rows (47%) do not describe the tree as written. Six of those 20 are already
corrected by this file's own [wave 5 table](#update--2026-08-22-wave-5-the-transport-and-connection-units)
(`MCP-101`, `105`, `109`, `114`, `115a`, `124`); the other 14 — 33% of the sample — are corrected
nowhere in this file.** For the `missing` rows alone, which is what the brief asked about: **10 of
25 are wrong, 8 of them uncorrected anywhere.** A maintainer should read the ledger's remaining rows
as *at best two-in-three reliable*, and no wave below should begin without re-verifying its own
units first — the cost of not doing so is an agent building something that exists.

Stated the other way, because it is the number a maintainer needs: of the 10 wrong `missing` rows,
only `MCP-105` and `MCP-115a` are corrected in this file. The other **eight** — `MCP-037`, `094`,
`100`, `116`, `125`, `126`, `134`, `231` — read as greenfield work and are not.

**The rows that are wrong, with the evidence:**

| id | census | verdict | evidence as of 2026-08-22 |
|---|---|---|---|
| `MCP-037` | missing | superseded | HA-1 landed in a **third** shape, which is why a grep for the census row's two spellings still returns nothing: `cyrup_ext::LateRegistrar` (native.rs:768), `HostLateRegistrar` (facade.rs:131), `register_late_tool`/`register_late_command` (facade.rs:707/724), defaulted `set_late_registrar` (native.rs:697), consumer half at cyrup-mcp extension.rs:118/783 and registration.rs:2022. `.flux/done/2026-08-22-14-00/HOST_LATE_TOOL_REGISTRATION.md` records it **completed**. (The 13i plan reached this independently — correction 3.) |
| `MCP-070` | partial | overstated | Gap (1) — "every production caller hashes UNRESOLVED values: ui.rs:1758 uses `ResolvedIdentity::verbatim`" — is closed: ui.rs:1763 calls `registration::default_server_hasher` (:807), which hashes through `ResolvedIdentity::resolve` (dirs.rs:1091). `verbatim` now has **only** test callers. Gaps (2)/(3) not re-checked |
| `MCP-084` | partial | superseded | All three resolvers exist: `resolve_server_url` (credentials.rs:3478, with its own upstream-parity test at :4862), `resolve_bearer_token` (:3386), `resolve_config_path` (dirs.rs:1073). Wave 5's own note says they "were already written and are now wired" |
| `MCP-094` | missing | overstated | The reciprocal change is claimed landed at dirs.rs:61 and the shared conformance suite the row asks for **exists**: the cross-crate golden vector (mcp_direct_tools.rs:2213-2226, asserted against `cyrup_mcp::dirs`'s own) and the end-to-end writer→reader case at :2498-2544. Residual is the filter half, which is `MCP-370`'s queued task |
| `MCP-100` | missing | superseded | "The entire manager is unbuilt" — `pub struct McpServerManager` at server_manager.rs:1247 with all seven maps (`connect_promises` :1210, `reconnect_promises` :1212, `close_promises` :1218, `close_generations` :1220 …), the `ServerConnection` record at :744, the race-guard tests at :2986. state.rs:338 says "Landed by 13c (MCP-100 / 116 / 125 / 126 / 131 / 134)" |
| `MCP-101` | partial | superseded | wave 5 table |
| `MCP-105` | missing | superseded | wave 5 table; `ParsedPackageSpec`/`parse_package_spec` at npx_resolver.rs:481/499 with a 29-row measured parity table |
| `MCP-109` | partial | superseded | wave 5 table |
| `MCP-114` | partial | superseded | wave 5 table |
| `MCP-115a` | missing | superseded | wave 5 table |
| `MCP-116` | missing | superseded | `credentials_invalidated` on the record (server_manager.rs:877) **and** step 7's carry-forward at :1809, comment-anchored `MCP-116 step 7` and measured against a permanent-401 fixture |
| `MCP-124` | partial | superseded | wave 5 table |
| `MCP-125` | missing | superseded | Both guard strings exist (`disabled_error` :96-101, `MANAGER_CLOSED` :104), `reconnect_promises` exists, the reconnect section is at :3347 and `raise_in_flight_to` (:918) carries the `Math.max(inFlight)` rule the row says is absent |
| `MCP-126` | missing | superseded | `MCP connection <n> was closed` at :107-113, the close/closeAll section at :3460, `close_generations` at :1220, and `ManagerSupervisor::close`/`close_all` now delegate (lifecycle.rs:335-341) rather than being the no-ops the `MCP-131` row cites |
| `MCP-131` | partial | superseded | Same evidence: lifecycle.rs:288-345 is six one-line delegations onto the real manager; only `refresh_tools` (:324) is still unbound, and for `MCP-120`'s reason, not this one |
| `MCP-134` | missing | superseded | `is_terminated_session` at server_manager.rs:2769 with all fifteen upstream cases measured (:4739+) |
| `MCP-140` | partial | overstated | "The serialisers are absent" is false: `serialize_tools` (dirs.rs:761), `serialize_resources` (:786), `serialize_prompts` (:808), each with its own test. The **reconstructors** are genuinely absent — no `reconstruct_tool_metadata`/`reconstruct_prompt_metadata` anywhere — so a real residual survives, and it is the half `MCP-021`/`MCP-023` consume |
| `MCP-144` | partial | superseded | Both call sites the row names now go through the `!`/`!!` grammar: `interpolate_env_record` → `interpolate_secret_expression` (mcp_direct_tools.rs:1239/1258/1277) and the bearer arm at :1414-1424, each carrying the MCP-144 rationale inline |
| `MCP-145` | partial | overstated | "No fallible hasher is ever installed" is false: `default_server_hasher` (registration.rs:807) **is** fallible — `Option<String>`, the throw expressed as `None` — and is the production default the panel takes at ui.rs:1763. What holds is the narrower claim that the *injectable* `install_server_hasher` (:785) has no production caller |
| `MCP-231` | missing | superseded | "The whole predicate is unwritten" — proxy.rs:4592 is the predicate, with every obligation the row lists: presence-not-truthiness override (:4599-4605), `true` ⇒ always (:4608), non-array/empty ⇒ never (:4610), the legacy residue with the `-`→`_` injection and the collision test (:4622-4650), and the two scopes differing in one expression. Three production call sites (:1631, :2229, :2450) |

**Two out-of-scope observations this pass produced, flagged rather than acted on:**

* **`MCP-232` (critical) looks superseded too.** Its row says the gate "exists only as a `ProxyEnv`
  trait method with a test-only implementor". `proxy.rs:4787` is a free `pub async fn
  ensure_tool_call_approved(state, server, tool, args, origin, cancel)`, and section 15's header
  (:4539-4555) states the deliberate free-function-not-trait-method placement. Whoever owns the
  critical ledger should re-grade it; this plan does not.
* **The `13c` cluster the 13i plan declined to re-verify (`MCP-118`..`MCP-122`) sits next to fifteen
  13c rows that did move.** Its own caveat — "may have moved the same way" — should be treated as
  likely rather than possible.

### Work already owned by a queued task — do not schedule it twice

`.flux/todo` already carries five tasks that cover units in this set. Any wave below that names one
of these must *depend on* that task, not re-do it.

| task file | units it owns |
|---|---|
| `MCP_DISCOVERY_PAGINATION.md` | `MCP-119` (all four obligations) |
| `MCP_SESSION_RECOVERY.md` | `MCP-135` |
| `MCP_DIRECT_TOOLS_FILTERS.md` | `MCP-370` (critical) and with it `MCP-094`'s filter residual |
| `MCP_401_JSON_RPC_BODY.md` | the reachable half of `MCP-115`'s remaining OAuth-ladder gap |
| `MCP_CONFIG_LENIENT_TYPES.md` | the config type-model ruling that `MCP-070`/`MCP-145` hash through |

### The spine: two seams, and why almost everything is behind them

Two facts order this whole backlog, and neither is a unit's own fault.

**Seam 1 — nothing in this crate runs in a live session.** `McpExtension::on_session_start`
(extension.rs:455-479) bumps the generation and drains the previous owner, and then stops: it never
calls `lifecycle::shutdown_state` (which exists, unwired, at lifecycle.rs:1562), never builds the
new owner or OAuth runtime, and never starts initialization. So `runtime::initialize_mcp`
(runtime.rs:125) still has exactly one caller and it is a test (runtime.rs:403). Everything wave 5
built is reachable only from tests.

**Seam 2 — no request can be issued on a connection.** `ConnectionResource` exposes
`close`/`has_session_id`/`child_pid`/`stderr_detail` and nothing else, so `McpConnection`'s `Peer`
is unreachable outside `runtime.rs` (runtime.rs:2180 says so in situ). `NewConnection` has no field
for tools/resources/prompts and `ServerConnection::new` hardcodes them empty (server_manager.rs:808
— "**Populated by MCP-119**"), and the landing site is marked in the source at runtime.rs:2992.

```
  W1 (session spine)        W2 (live peer seam)
  MCP-008/009/010/011/014   MCP-119 + the resource widening + MCP-140's reconstructors
        │                         │
        ├──────────┬──────────────┼───────────────┬──────────────┐
        ▼          ▼              ▼               ▼              ▼
   W6 startup   W7 tool-       W8 live tool   W9 /mcp       (13i S2/E4/V3,
   connect      execution      surface        dispatcher     planned above)
   MCP-023/     MCP-043/164/   MCP-217        MCP-381 ▶ 386/387/
   025/029      207/214/214a/  (+395's host   388/390/392/398
                249            leg)
```

Everything not under those two arrows is startable today: **W3, W4, W5, W10, W11**.

### Waves

Grouped by shared obligation, never by file — the PR #30 rule. Where a wave's obligation is
genuinely one function or one byte-exact string, its units travel together even across three crates.
There is **no size column**: see the last bullet of [§What this plan does not
claim](#what-this-plan-does-not-claim-1).

| wave | units | obligation held in common | files | verification | may start |
|---|---|---|---|---|---|
| **W1 — the session spine** | `MCP-008`, `009`, `010`, `011`, `014` | one generation protocol: bump → snapshot → synchronous `begin_stop` → await → **triple** staleness re-check (`is_active` && `generation == my_gen` && `Arc::ptr_eq(init_task, promise)`) → commit → hook install, and its mirror image at shutdown. Splitting it puts the cancel in one agent's set and the re-check that makes the cancel meaningful in another's | `extension.rs` (:455 start, :565 shutdown), `lifecycle.rs` (:1562 `shutdown_state`, :1654 `shutdown_previous_generation` — both written, both unwired), `state.rs`, `runtime.rs` (:125) | `crates/cyrup-it/tests/mcp/activation.rs` — `MCP-014` names four assertions it must carry; plus the ablation the brief requires: disable the synchronous `begin_stop` and the replacement test must fail | **now** |
| **W2 — the live peer seam** | `MCP-119` *(queued)*, `MCP-140`'s reconstructor half | one connection record that carries a usable `Peer` **and** the catalog discovered through it. `MCP-119` alone has nowhere to put its results: the seam must widen in the same change | `runtime.rs` (:2180, :2992 marker), `server_manager.rs` (:744 record, :808 fields, :2626), `dirs.rs` (reconstructors beside the serialisers at :761-808), `lifecycle.rs` (:324 `refresh_tools` unbinds when this lands) | fixture-driven per failure arm (the queued task's AC4), **plus** the seam assertion: a test that issues a `tools/list` through `ConnectionResource` without reaching into `runtime.rs` | **now** — but the queued task must be widened to include the seam, or it cannot land |
| **W3 — one naming vocabulary** | `MCP-073`, `075`, `076` | one candidate set and one matcher, in two copies that must agree. `075`'s bug (proxy.rs:509 re-sanitises an **already**-sanitised `get_server_prefix`, where registration.rs:261 derives from `legacy_server_prefix`) and `076`'s (registration.rs:325 is a bare `Regex::new`, while proxy.rs:598 sets both ceilings) are the same defect class: two copies, one drifted. `073` is the consumer that makes the drift observable | `registration.rs` (:240-350), `proxy.rs` (:486-600) | table-driven equality between the two copies for the same input, so drift fails a test rather than a review; a ReDoS case for the ceilings — proxy.rs:5895-5908 already has one to copy | **now** |
| **W4 — one digest, one resolver** | `MCP-070`(rem.), `139`, `143`, `145`(rem.), `094`(rem., *queued*) | one byte-exact pre-image agreed by three crates. `MCP-139`'s axis 3 is named in situ at dirs.rs:69-72: this crate's `home_dir` is `$HOME`, the reader's is `CYRUP_HOME` → `HOME`, so a `~`-prefixed `cwd` hashes differently. `143` is the same shape one layer down — cyrup-ext's `interpolate_env_vars_with` (proc.rs:148-149) is still only `${VAR}` + `$env:VAR`, missing `{env:NAME}` | `dirs.rs`, `registration.rs` (:785-807), `ui.rs` (:1763), `cyrup-ext/src/caps/proc.rs`, `cyrup-ext/src/caps/proc/npx_resolver.rs`, `cyrup-ext-subagents/.../mcp_direct_tools.rs` | the cross-crate golden vector that already exists (mcp_direct_tools.rs:2213, `dirs::tests::golden_vector_stdio_server`) extended to the `~`-cwd and `{env:}` cases — **regenerated from upstream on node 22**, not hand-written | after the `MCP_CONFIG_LENIENT_TYPES` ruling (it decides what a non-string `env` hashes to, which this wave would otherwise pin wrong) |
| **W5 — the schema gate** | `MCP-092` | one dialect router and its two validators. Independent of everything | `config.rs` / a new validator module | upstream's own accept/reject corpus per dialect, plus the exact `Unsupported JSON Schema dialect: …` text | **now** — and cheaper than the row implies: `jsonschema` is already a dependency (Cargo.toml:118) |
| **W6 — startup: connect, build, notify, flush** | `MCP-023`, `025`, `029` | one pass over the startup connect results: the two-pass metadata build, the notifications derived from *its* counters, and the cache write that persists them. They are three steps of one function; the notification counts are unforgeable without the build | `runtime.rs` (`initialize_mcp`), `dirs.rs`, `state.rs` | byte-exact strings (`servers connected`, `tools skipped`, `Failed to connect to {name}`) asserted against upstream output, not against the spec prose | after **W1** and **W2** |
| **W7 — the direct-tool call path** | `MCP-043`, `164`, `207`, `214`, `214a`, `249` | one path from model to server: metadata names the tool (`207`), the executor runs the ordered state machine (`214`), the rmcp invocation is issued (`164`), auth recovery re-enters it (`214a`), the emitted `details` shape is frozen (`249`), and the tool the model actually reaches is the one that dispatches (`043` — today `registration::ProxyTool` is registered while `proxy::McpTool` is constructed only in tests). Split any of these off and the `details` schema drifts from the code that emits it | `proxy.rs` (:1465/1478 trait, :4954/4964 test-only impl), `registration.rs` (:1456 `McpToolDispatch`, :1504 `install`), `runtime.rs` | 13d's mode conformance cases scripted through `ProxyEnv`, which is what that seam is for; ablation on the approval call — with it disabled the denial case must fail | after **W2** (needs the `Peer`) and **W3** (needs the names) |
| **W8 — the live tool surface** | `MCP-217`, `395` | one dirty-flag round trip: fingerprint diff → late registration → downstream rebuild. `217`'s host seam **has landed** (HA-1); what is missing is the diff pass and the `deactivateTools` fallback. `395` is the same round trip for *commands*, and its downstream half does not exist in any crate — the 13i plan measured that: a grep for `mark_commands_dirty`/`take_commands_dirty`/`commands_dirty` across `cyrup-ext`, `cyrup-session-svc` and `cyrup-tui` returns nothing, and this pass did not re-run it | `extension.rs`, `registration.rs`, plus a **host-side** change in `cyrup-ext`/`cyrup-session-svc`/`cyrup-tui` for `395` | the mid-session demonstration `HOST_LATE_TOOL_REGISTRATION.md` says it could not run: register a tool after `init` and assert the model sees it in the next turn | after **W1** (the generation-swap race the HA-1 task flagged is unreachable until then). `395`'s host leg needs a task filing of its own — see below |
| **W9 — the `/mcp` dispatcher** | `MCP-381` first, then two groups — `386`/`388`/`390`/`392`, and `387`/`398` | one owner-fenced `commandCtx` and one connection-status vocabulary. `381` is the trunk: the prologue, the init-await preamble and the argument split every other handler assumes. `386`/`388`/`390`/`392` all *mutate or report* connection+credential state and share the `needs-auth` ladder (ui.rs:1388 records that the eight-rung derivation is the dispatcher's, not the panel's); `387`/`398` only read discovery and cache | `ui.rs` (:1401 `McpPanelCallbacks` is the seam, :4786 `TODO(MCP-394)`), `oauth.rs` (:3780 `TODO(MCP-334)`), a new command module | one scripted transcript per subcommand asserting the exact refusal/usage strings — `388`'s partial-failure string in particular distinguishes two outcomes and cannot be inferred | after **W1**+**W2**; `386`/`392` additionally after **W6** (they call the metadata build) |
| **W10 — error identity through the auth path** | `MCP-324`, `326` | one rule: an error's identity is its type, never its rendered text. `324` is a `starts_with(CREDENTIAL_STORE_PREFIX)` at oauth.rs:3582 that must become structural; `326` is an abort that must carry the identical reason value out of `combined_signal` (oauth.rs:2478) | `oauth.rs`, `errors.rs` | a test that changes the message text and leaves the classification unchanged — which fails today by construction | **now** |
| **W11 — the keyring helper subcommand** | `MCP-260` | one hidden-subcommand process contract. `crates/cyrup/src/mcp_keyring_helper_cmd.rs` **does not exist** (confirmed by glob); credentials.rs:66/165/1658 names it as the missing half. 13i asks that this and `MCP-484`'s conformance subcommand be designed together, so they share the `argv[1]` pre-dispatch precedent (`__subagent-runner`, `__intercom-broker`, main.rs:115/131) | `crates/cyrup/src/mcp_keyring_helper_cmd.rs` (new), `crates/cyrup/src/lib.rs`, `crates/cyrup/src/main.rs` | a real re-exec under `keyctl session -`, plus the three timeout/exit-code fixtures `TODO(MCP-287)` names | **now**, co-designed with 13i's **C1** |
| **W12 — the proxy-mode corpus** | `MCP-196`, `191` | one measure of the mode surface: the 47-case suite and the permission-target question its auth cases raise. Both are about what the corpus *asserts*, not about new behaviour | 13d's test corpus, `proxy.rs` | case-count parity against upstream, reported as a number | after **W7**; `191` needs a ruling first |
| **not scheduled** | the 16 `superseded` rows | closed. Re-grade them in a status pass, do not staff them | — | — | — |

### Units whose blocker was HA-1 (AC3)

**HA-1 has landed and is not a blocker for anything in this set.**
`.flux/done/2026-08-22-14-00/HOST_LATE_TOOL_REGISTRATION.md` is `status: completed`, and the handle
is in the tree in a **narrower** shape than either option `MCP-037`'s row proposes — a capability
object with `owner` bound at construction (`LateRegistrar`, native.rs:768; `HostLateRegistrar`,
facade.rs:131), not a `Weak<ExtensionHost>`. That is why a grep for the row's own spellings
(`set_ext_host`) still returns nothing and why a reader could conclude the opposite.

The four units that ever cited it, and where they now sit:

| unit | was blocked on HA-1 | now |
|---|---|---|
| `MCP-037` | it **is** HA-1 | **closed** — re-grade the row |
| `MCP-217` | the registration handle | unblocked; scheduled in **W8**, gated on W1 only |
| `MCP-039` (medium) | `register_late_command` | unblocked — `facade.rs:724` exists |
| `MCP-395` | HA-1's command leg | **half unblocked**: the registry half exists, the downstream half (`commands_dirty` / rebuilding the `/` surface) exists in no crate. That is a host change in `cyrup-ext`/`cyrup-session-svc`/`cyrup-tui`, not MCP work, and **no task owns it** — recommend filing it as HA-1c before W8 starts, rather than letting an MCP agent discover it mid-wave |

The task file's own closing note names two things it left open and both are W1's: `initialize_mcp`
has no production caller, and the `sync_tool_surface`/generation-swap race is unreachable while
`on_session_start` is a stub. Neither is HA-1's residual.

### Collisions to decide once, before the waves start

The 13i plan's `lib.rs` problem has three analogues here, and each is a merge conflict between agents
who otherwise never meet:

1. **`runtime.rs::initialize_mcp`** — W1 gives it a caller, W6 fills its middle, W2 changes what its
   `ConnectionBuilder` returns. Nominate W1 to own the function's shape; W2 and W6 extend it.
2. **`server_manager.rs`** — W2 widens the record; 13i's **T2** has three flush sites already marked
   in it (:2337, :2479, :1432). Schedule them adjacently or serialise them.
3. **`proxy.rs`** — W3 edits the naming block (:486-600), W7 the execute path and the `ProxyEnv`
   trait (:1397-1560). Disjoint regions of one 7,594-line file; say so in both task files.

### Rulings needed, each named with what it unblocks

1. **The config type model** (`MCP_CONFIG_LENIENT_TYPES.md`, already filed) — gates **W4**. Pinning
   a digest before it is decided means re-pinning it after.
2. **`auth-start` / `auth-complete` permission targets** (`MCP-191`, filed `open-decision`) — gates
   the last third of **W12**.
3. **Whether `MCP-119`'s queued task also owns the `ConnectionResource` widening** — gates **W2**,
   and through it W6, W7 and W9. This is the single highest-leverage question in the backlog: as
   filed, the task cannot land, because discovery has nowhere to write its results.
4. **A re-clone of `pi-mcp-adapter`** — gates the *quality* of every wave, not its start. Without it
   no wave can measure, and every parity bug PR #30 found came from measuring.

### What this plan does not claim

* **Nothing here was executed.** Not one build, test, clippy run or upstream invocation. Every
  verdict is a reading of the tree at 2026-08-22, and carries exactly the false-positive risk this
  section just measured in someone else's readings.
* **15 of the 58 rows were not re-checked** (`MCP-009`, `010`, `014`, `025`, `029`, `043`, `068`,
  `115`, `191`, `196`, `214a`, `249`, `326`, `390`, `395`). Given that 47% of the 43 rows that
  *were* checked had moved, assume roughly one in three of those 15 has too — and re-check them at
  wave start rather than at planning time, when the answer will be staler still.
* **`MCP-068` was not placed in a wave.** Its three obligations (the `MCP_UI_DEBUG` logger
  bootstrap, and two others) share no obligation with anything else in this set; it is a single
  small unit and should be handed to whichever wave touches `config.rs` next, not staffed alone.
* **Sizes were deliberately omitted** rather than guessed. The census's own S/M/L is not carried on
  these rows, and inventing one would be the kind of number that survives into a schedule.
* The `13c` rows this pass marked `superseded` were verified by symbol and anchor comment, **not**
  by running their tests. A row that says "landed" in a doc comment is still a claim by the agent
  that wrote it; the three that would cost a wave if wrong are `MCP-100`, `MCP-231` and `MCP-116`,
  and each cites a line so the re-check is cheap.
