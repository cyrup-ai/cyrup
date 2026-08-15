# `pi-mcp-adapter` → `cyrup-mcp`: how the port is executed, and how anyone knows it is right

The companion to the seam map. The seam map says **what** each surface becomes; this says **how the
work is sequenced, what fidelity means concretely, which layer of verification catches which class of
error, and what has to be true before anyone calls it done.** It is meant to be used as a checklist.

**Provenance.** Upstream is `pi-mcp-adapter` **v2.25.0**. cyrup is branch **`david/cyrup`**. The Rust
MCP SDK is **`rmcp` 3.1.2**, read from the checkout at `/Users/davidmaple/cyrup.ai/rmcp`
(`rmcp-v3.1.2-7-gf713ebd`). That is the whole of the provenance this document carries.

**The citation rule, stated once and binding everywhere below.** cyrup is referenced by **symbol and
file** — `ExtensionHost::register_late_tool` (`crates/cyrup-ext/src/facade.rs`). Never a line number,
never a commit sha, never "the working tree". Upstream is referenced by **file and symbol** —
`server-manager.ts`'s `createConnection` — with a line range only where one algorithm must be located
inside a large file and no symbol names it. §2 gives the reason, and it is a measurement, not a
preference.

**The documents this one is bound to.** The seam map is `13-cyrup-mcp.md`; its port units have their
bodies — upstream behaviour, cyrup mechanism, `verify` line — in nine section files published beside
it. Cite them by file name, never by section number:

| § | published as | subject |
|---|---|---|
| 01 | `13a-mcp-activation.md` | activation, lifecycle and the host seam |
| 02 | `13b-mcp-config.md` | configuration, the type model and errors |
| 03 | `13c-mcp-servers.md` | server manager, transports and the metadata cache |
| 04 | `13d-mcp-proxy-modes.md` | proxy modes and search ranking |
| 05 | `13e-mcp-tools.md` | tool registration, approval, output guard and rendering |
| 06 | `13f-mcp-credentials.md` | credential storage, keychain and consent |
| 07 | `13g-mcp-oauth.md` | the OAuth 2.1 flow and the callback server |
| 08 | `13h-mcp-tui.md` | the TUI panels, slash commands and prompts |
| 10 | `13i-mcp-protocol-and-verification.md` | sampling, elicitation, tracing and verification |

**There is no section 09.** The surface it would have held is MCP Apps, which is Cut 2.

---

## 1. Scope, and the acceptance criterion

### 1.1 What "ported" means, auditably

A port unit is **closed** when all five of these hold. Any one missing and it is open.

1. **Behaviour.** The Rust reproduces the upstream behaviour for every input the upstream code
   distinguishes — including its error paths, its ordering, and its output strings byte-for-byte
   (§3.2's exceptions are the only ones).
2. **Citation.** The Rust carries a doc comment naming the upstream **file, symbol and version**
   (§3.1). A reviewer can open one file and check the claim.
3. **Divergence ledger.** Every mechanism difference is recorded at the site with its cost, in the
   two-part `CYRUP-DELTA` form §3.4 fixes. A divergence that is not written down is a defect.
4. **Test.** At least one automated assertion fails if the behaviour regresses, at the layer §5 says
   catches that class. "Covered by an integration test that does not run in the merge gate" is not a
   test (§5.3 — this is a live defect in this workspace, not a hypothetical).
5. **No scaffold.** `rg 'SCAFFOLD\(MCP-NNN\)' crates/` returns zero for that id (§4.0).

### 1.2 The scope, and the four cuts — decisions of record, not deferrals

**cyrup supports exactly the subset of MCP that `rmcp` supports. Nothing more.** Four surfaces are
**cut by decision of the project owner**. They are recorded here, with reasons, so that a later pass
does not re-file them as gaps. They are settled in **ADR-0012** (§9).

| # | cut | why | the consequence to propagate |
|---|---|---|---|
| 1 | **Legacy HTTP+SSE transport** (2024-11-05 GET `/sse` → `endpoint` → POST), `shouldFallbackToSse`'s 404/405/406/415 downgrade, and every legacy-protocol-revision code path | rmcp 3.1.2 ships **no** SSE client transport at all — `crates/rmcp/src/transport.rs` exports `TokioChildProcess`, `StreamableHttpClientTransport` and `UnixSocketHttpClient` on the client side and nothing else. Supporting it means hand-writing a protocol transport, which is the one thing the dependency decision exists to avoid | Supported transports are exactly **stdio** and **streamable HTTP**. `ServerEntry.httpTransport` keeps only `"streamable-http"`; `"sse"` is **rejected at config load with a named diagnostic** — `agent-plugin-loader.ts` sets that field straight from a manifest's `type`, so a plugin declaring it is a live case that would otherwise appear configured and never connect. `ServerEntry.protocolVersion` is **not** part of this cut — it is protocol-era negotiation and maps 1:1 onto rmcp's `ClientLifecycleMode` |
| 2 | **MCP Apps / the UI extension, entirely** — `ui-server.ts`, `ui-session.ts`, `host-html-template.ts`, `ui-resource-handler.ts`, `ui-stream-types.ts`, `ui-app-bridge-helpers.ts`, `app-bridge.bundle.js`, `glimpse-ui.ts`, every `ui://` path, the local HTTP host server, the iframe bridge, `@modelcontextprotocol/ext-apps` | owner decision | **No `axum`.** No local HTTP *server* (the OAuth loopback listener is a different thing and stays). No app-initiated tool calls, so `McpToolApprovalOrigin`'s `"iframe"` variant goes. `consent-manager.ts` goes with it — its only production consumers are `ui-server.ts` and `ui-session.ts`. The tool-result renderer handles the standard content types and **not** `ui://`. `ui-tool-visibility.ts` **splits**: `extractUiToolVisibility` + `isUiToolVisibleToModel` are **kept** (dropping them would expose to the model tools the server explicitly marked app-only); `isUiToolCallableByApp` is cut |
| 3 | **Raw unix-socket transport** (`unix-socket-transport.ts`, `ServerEntry.socket`) | rmcp's `UnixSocketHttpClient` is streamable-HTTP-over-UDS, a different wire shape from the adapter's raw framed socket. rmcp does not ship the adapter's shape | `createConnection`'s invariant becomes **"exactly one of `command` or `url`"**; a config carrying `socket` produces a named diagnostic, not a silent skip. **The `socket` KEY stays in `computeServerHash`'s pre-image** — always absent, hashing as the `undefined` token, exactly as for most upstream servers. Dropping the key changes the digest for *every* server and voids the golden-vector fixture |
| 4 | **`mcpScript` / the JavaScript worker** — `mcp-code.ts`, `mcp-script-worker.mjs`, `skills/mcp-scripting`, `McpSettings.scriptMode`, `McpToolApprovalOrigin`'s `"script"` | owner decision | **This removes the only JS-engine question in the port.** No `rquickjs`, no vendored C, no `boa`, no JS-in-WASM. **Do not raise a JS engine anywhere, for anything.** With Cut 2 also gone, **`node` is not a production dependency**. `executeCall`'s `origin?: "proxy" \| "script"` keeps its shape and its `"proxy"` default; only the `"script"` call site disappears |

**Everything else is in scope**, and it is the large majority: server lifecycle and connection
management over stdio and streamable HTTP, the metadata cache, config, tool registration and naming,
tool approval, the output guard, result rendering, resources and templates, prompts, completions,
logging, progress and cancellation, structured content and output schemas, roots, sampling,
elicitation, the full OAuth 2.1 path, the OS keychain, the two TUI panels, slash commands, status and
notifications, and tracing.

**Three "in-scope" surfaces have nothing to port.** `grep` over v2.25.0 finds **zero** occurrences of
roots, `logging/setLevel` / `notifications/message`, and `completion/complete`. rmcp ships all three.
Wiring them is **new functionality, not a port**, and it is outside the 1:1 mandate. Recorded here so
a later pass does not file them as gaps. The same applies to resource subscriptions.

### 1.3 The thesis, and the test that protects it

**This is an extension, and it stays one. The port changes essentially nothing in cyrup's core.**
That is the defining property of `pi-mcp-adapter` upstream — an npm package pi loads, whose core knows
almost nothing about MCP — and the port preserves it. `crates/cyrup-mcp` is a **native built-in crate
compiled into the binary**, the same shape as `crates/cyrup-ext-subagents`, attached through
`SessionFactory::with_native_extension` / `SessionBuilder::with_native_extension` and loaded by
`ExtensionHost::load_native_with_services`.

**A native extension is not sandboxed.** `HostServices` is the capability surface a **WASM guest** is
confined to. A native crate links `rmcp`, `tokio`, `keyring`, `reqwest`, `opener` and the filesystem
**directly**, and reaches for `HostServices` only where it genuinely touches the host: drawing UI,
notifying, reading session state, honouring cancellation, registering tools and commands.

**Therefore: a claimed host-surface addition is an extraordinary claim, and it does not get scheduled
until it passes both halves of this test, in writing, at the site:**

> **(i)** The need is a genuine **host** concern — it mutates the agent's live tool array, draws into
> the terminal, or prompts the one human — and not something a native crate legitimately does itself.
> **(ii)** No existing verb serves it. Naming the verb you read and what it does instead is part of
> the claim.

Three additions survive that test, and only three. **HA-1** (a native has no handle to
`ExtensionHost::register_late_tool`, there is no `register_late_command` sibling, **and
`ExtensionHost::refresh_tools` does not report a native tier's late registration in the default
build**) is the only load-bearing one; it is two edits in two crates, `MCP-037` and `MCP-037a`.
**HA-2** (extension slash commands have no argument completions, for natives or in the TUI) is real
and secondary — `MCP-041`, seen again from the TUI as `MCP-382`. **HA-3** (the overlay seam carries no
geometry options) is cosmetic and owned by `MCP-368`. Each has an ADR (§9).

Two **wiring gaps** that are not new surfaces also survive, and they share a failure mode — each
compiles, returns a plausible value, and looks correct:

- `ExtensionHost::refresh_tools` returns the *guest* materializer's verdict while the tools-dirty flag
  is raised by both tiers, so a native late registration is consumed and reported as "nothing
  changed". One line. The seam map's **Finding 1** carries the full trace, and this is the reason
  HA-1's size moved from S–M to M.
- `LiveHostServices` does not override `HostServices::is_run_cancelled` (or `tools_expanded`), so the
  documented `ctx.signal` substitute returns the trait default `false` forever in production. One
  method body each, and the WASM bridge forwards to the same defaults, so both tiers are affected.

**Neither was findable by reading a single function.** Both were found by asking what the *caller*
receives, which is the check §1.1 clause 4 exists to force and which no amount of symbol-level
verification supplies.

### 1.4 Severity, the house scale

`critical` = **data loss, silent wrong output, a permission bypass, or a crash on a normal path.**
Four clauses, no fifth. **Blocking-ness is not severity** — "without this the subsystem is inert" is
scheduling information and belongs in the body of the item, never in its rating. The previous edition
rated 20% of items critical; the current set rates **21 of 433 (4.8%)**, and each names which clause
it meets. The full census is the seam map's; the counts in this document are computed from the same
nine section files and agree with it row for row.

---

## 2. Baselines, and the rule about what is pinned

Three fields per upstream, moving on three different triggers — the shape ADR-0006 mandates, because
conflating them is what produced both of the workspace's earlier baseline errors.

| | ported baseline | comparison tag | HEAD | citable? |
|---|---|---|---|---|
| **`pi-mcp-adapter`** | v2.25.0 (nothing landed yet; moves as port units close) | **v2.25.0** | **17 files / +543 / −69** past v2.25.0 | baseline and comparison tag: yes. HEAD: **no** |
| **`rmcp`** | — | **3.1.2** (`rmcp-v3.1.2-7-gf713ebd`) | 7 commits past the tag | comparison tag only |
| **cyrup** | — | **branch `david/cyrup`** | moves continuously | **nothing about cyrup is pinned — by rule** |

### 2.1 cyrup is a moving target and this document does not pin it

**The rule: reference cyrup by symbol and file. Never a line. Never a commit. Never "the working
tree".**

This is a measurement, not a stylistic preference. cyrup moved a whole commit *during* the previous
analysis of this same port, and its working tree is dirty as this is written. **37% of the previous
edition's cyrup line citations sat on files that had already drifted.** A plan anchored to
`file.rs:1234` is stale the day it is written, and — worse — it *looks* verified, because it carries a
citation. A symbol reference either resolves or fails loudly under `rg`; a line reference silently
points at the wrong code.

The same reasoning forbids "resolves at `<sha>`" and "read from disk at HEAD" provenance sentences.
They record when someone looked, which nobody can act on, at the cost of implying a currency the
document cannot have.

### 2.2 Upstream re-baseline cadence

Follow ADR-0006's procedure verbatim, with `pi-mcp-adapter` as a fifth upstream:

1. **Watch** at the top of every phase that names an upstream file, and weekly regardless:
   `git -C <pi-mcp-adapter> fetch --tags && git describe --tags --abbrev=0` plus
   `git rev-list --count v2.25.0..<latest>`.
2. **Trigger is a new tag. Only that.** A non-zero commit count is *information*; a tag is the
   *event*. The 17-file / +543 / −69 window past v2.25.0 is **deliberately unanalysed** — an untagged
   commit cannot answer "which side of the ported tag did this land on", so items in it are unfileable
   by the project's own rule.
3. **Census before trusting.** Count in-tree `pi-mcp-adapter@vX.Y.Z` citations before writing any new
   baseline number. Do not inherit the row. **This is not hypothetical for this port**: the two
   existing partial ports (`cyrup_ext::caps::proc::npx_resolver`,
   `cyrup_ext_subagents::exec::mcp_direct_tools`) cite upstream **with no version at all** — which is
   exactly the defect blind spot 3 names. Phase 2's first act is to stamp both with `@2.25.0`.
4. **Compute the re-anchor worklist mechanically**: intersect `git diff --name-only <old>..<new>` with
   the upstream paths this document and the seam map cite.
5. **Re-anchor by ADDING, never by rewriting.** Never write "identical at both tags".
6. **File only what the diff shows**, then move the comparison tag. The ported baseline moves only
   when the units close.

### 2.3 `rmcp` gets the same treatment on a much shorter clock

**`rmcp` 3.x is weeks old and moving fast.** Measured from the checkout's own tag dates:
`3.0.0-beta.1` 2026-07-23 → `3.0.0` 07-28 → `3.0.1` 07-29 → `3.1.0` 07-31 → `3.1.1` 08-05 → `3.1.2`
08-07. **Six releases in sixteen days**, and 3.1.2 is one week old as this is written.

Rules that follow:

- **Re-check `rmcp`'s latest version at every phase boundary**, not weekly and not at re-baseline
  time. `cargo search rmcp` / the checkout's `git fetch --tags`.
- **Pin the version and the feature set in `Cargo.toml`, and assert both in a test** (MCP-493). A
  silent feature-unification change is the failure mode that costs the most and shows the least.
- `rmcp`'s `VERSIONING.md` promises SemVer with `#[non_exhaustive]` on public structs "where
  practical", so a MINOR bump is normally additive. **The risk this cadence creates is a MAJOR landing
  mid-port.** If that happens: do not chase it inside a phase. Finish the phase against the pinned
  version, then take the bump as its own unit with the API diff read from the checkout.
- **`DEPENDENCY_POLICY.md` constrains nothing downstream** — it is rmcp's own selection / Dependabot /
  MSRV policy. The one clause worth carrying is "anything not needed by every user should sit behind a
  Cargo feature", which is why the port's feature list is as narrow as it is.

The pinned dependency line, read from `crates/rmcp/Cargo.toml`:

```toml
rmcp = { version = "3.1.2", default-features = false, features = [
  "client",                                      # = ["dep:tokio-stream"]; gates ClientHandler + Peer<RoleClient>
  "transport-child-process",                     # pulls process-wrap + tokio/process — NEW to Cargo.lock
  "transport-streamable-http-client-reqwest",    # pulls sse-stream, http, bytes, base64 — sse-stream NEW
  "reqwest",                                     # = ["__reqwest", "reqwest?/rustls"] — name the TLS backend
  "auth",                                        # = ["dep:async-trait", "dep:oauth2", "__reqwest", "dep:url"]
] }
```

`default-features = false` is **mandatory**: rmcp's default is `["base64", "macros", "server"]`, and
`server` pulls `transport-async-rw`, `schemars`, `pastey` and `uuid` for a role the adapter never
plays. `base64` returns transitively through `client-side-sse`, so nothing is lost. **`elicitation` is
NOT needed** — the feature is `["dep:url"]` and every `#[cfg(feature = "elicitation")]` in the tree is
server-side; the whole client elicitation surface is unconditional under `client`. `oauth2` 5.0.0 is
already in cyrup's lock file. `which-command` stays off — `npx_resolver` already does far more than
`which` for the one command shape that needs it.

**Two consequences of that feature set that will be hit on the first day, both verified in the
checkout.** First, **`ClientCapabilities::builder()` does not exist in this build.** The `builder!`
macro in `crates/rmcp/src/model/capabilities.rs` is guarded by
`#[cfg(any(feature = "server", feature = "macros"))]`, and the type is `#[non_exhaustive]`, so the
struct-literal fallback is illegal outside rmcp too. Use `ClientCapabilities::default()` and assign
the `pub` fields; the same applies to `SamplingCapability` and `ElicitationCapability`.
`StoredCredentials` and `StoredAuthorizationState` are `#[non_exhaustive]` **without** a `Default`, so
they go through `new(…)` plus their `with_*` builders — which lands on `MCP-290` and `MCP-291`
directly. Second, **sampling is soft-deprecated by SEP-2577 in 3.1.2**: `SamplingCapability`'s doc
comment says so and two `ClientCapabilitiesBuilder::enable_sampling_*` methods are `#[deprecated]`.
Neither is reachable from this feature set, `ClientHandler::create_message` is not deprecated, and the
capability functions end to end — **the port and the sampling units' severity are unaffected.** It is
recorded so that the `rmcp` bump which eventually removes it is recognised as breaking rather than
rediscovered, alongside the logging and roots deprecations already noted.

---

## 3. Fidelity rules

### 3.1 Cite the upstream file, symbol **and version** in a Rust doc comment

This is the in-tree convention, demonstrated by both existing ports of this same package. From
`crates/cyrup-ext/src/caps/proc/npx_resolver.rs`:

> *"Direct Rust port of `pi-mcp-adapter/npx-resolver.ts`'s `resolveNpxBinary` … Real consumer wiring:
> `pi-mcp-adapter/server-manager.ts` — `createConnection` calls `resolveNpxBinary(command, args)`
> and, when it resolves, substitutes `command`/`args` BEFORE constructing `StdioClientTransport`."*

and from `crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs`:

> *"This is the direct analogue of pi's module: `resolve_mcp_direct_tool_names` is
> `resolveMcpDirectToolNames`; `compute_mcp_server_hash` is `computeMcpServerHash` … The one
> deliberate deviation from a line-for-line port is directory resolution … so the directory context is
> factored into an injectable `McpDirs`."*

Both do the two things that matter — name the upstream symbol, and state **and justify** every
mechanism divergence. **This port inherits that convention with one correction: the version token is
mandatory.** Both existing files cite `npx-resolver.ts:34-69` and `mcp-direct-tool-allowlist.ts` with
no version anywhere, which is the exact failure mode structural blind spot 3 names — a citation that
reads as verified and cannot be re-resolved. The form:

```rust
//! Port of `pi-mcp-adapter@2.25.0/metadata-cache.ts`'s `computeServerHash`.
```

Line numbers are **optional** in a doc comment and **forbidden** without a version token beside them.
A bare `foo.ts:412` is not a citation; `foo.ts:412 @2.25.0` is.

### 3.2 Message strings and constants are byte-identical

Every user-facing string, every model-facing string, every numeric constant, every default, every
error code, every ordering. Specifically and non-negotiably:

- The regenerated `mcp` tool description — **it is the prompt-cache key**, so a whitespace change is a
  cache miss on every session.
- The `details.error` vocabulary: 35 codes upstream → **32 surviving the cuts → 31 with a producer**
  (`unsafe_pattern` survives as a documented no-producer variant, §5.6).
- The three OAuth callback HTML pages, U+2014 em dash and 2000 ms auto-close included.
- The 13 elicitation coercion message templates across 15 throw sites.
- The output guard's ` B` / ` KiB` / ` MiB` formatting and its truncation notice.
- The five `resolveCommandSecret` failure literals and four context strings.
- The twelve `extractOAuthConfig` validation messages.

**The only sanctioned string changes in the entire port**, each with a reason that is not "it reads
better":

| change | reason |
|---|---|
| `buildProxyDescription`'s header sentence loses *"When one request needs several MCP calls with logic between them, use mcpScript."* | Cut 4 — advertising a tool that is not registered |
| `buildProxyDescription` loses the `mcp({ action: "ui-messages" })` usage line | Cut 2 |
| the `action` property description narrows from *"Action: 'ui-messages', 'auth-start', or 'auth-complete'"* to *"Action: 'auth-start' or 'auth-complete'"* | Cut 2 — leaving it advertises a mode that now silently falls through to `executeStatus` |
| two model-facing strings say **"Pi"** (`buildProxyDescription`'s header; `executeCall`'s native-tool message) → **"cyrup"** | host identity; ADR-0024 |
| the endpoint probe gains **one** ladder arm classifying a legacy-SSE-only endpoint as an unsupported transport | Cut 1 — otherwise the probe reports "endpoint responded with an MCP event stream" attached to a connect failure, which is worse than no probe |

Every one of those is in this table. If a sixth appears, it is a defect until it is added here with a
reason.

### 3.3 Port the mechanism, not the vibe

Where upstream reaches for a subprocess, an IPC hop, a signal or a re-exec, **the port reaches for the
same thing.** An "idiomatic Rust" redesign of a mechanism is not a port and does not ship without
explicit sign-off. The three places this bites hardest, all already settled:

- **`mcp-keyring-helper.cjs`** — upstream runs `spawnSync("keyctl", ["session", "-", "node",
  "mcp-keyring-helper.cjs"])`, talking JSON over stdin/stdout with a 10 s timeout. The mechanism being
  ported is *"re-run the keyring op inside a fresh `keyctl` session over a JSON stdio protocol"* — not
  *"run node"*. The port re-execs `std::env::current_exe()` under a hidden `__mcp-keyring-helper`
  subcommand: **same `keyctl session -` hop, same argv shape, same stdio protocol, same timeout, same
  trigger regex, no node.** An in-process keyring library is precisely what cannot recover from a
  revoked session keyring, so the hop is not vestigial. Precedent for the re-exec:
  `crates/cyrup/src/intercom_broker_cmd.rs` and `crates/cyrup/src/subagent_runner_cmd.rs`, both
  pre-dispatched from `main.rs` before any clap parsing.
- **`--mcp-config`** — upstream's `registerFlag` exists only for `--help`; the *value* is read straight
  off `process.argv` by `getConfigPathFromArgv`. The port does exactly that: `InitApi::register_flag`
  for the help text, `std::env::args()` for the value. There is no flag-read-back gap and inventing
  one would be the divergence.
- **`abort.ts` / `runtime-owner.ts`** — one owner token per lifecycle generation, children per
  request, on a `tokio_util::sync::CancellationToken` tree. `cyrup_core::CancelToken` *is*
  `tokio_util::sync::CancellationToken`, the exact type `rmcp::serve_client_with_ct` takes.

### 3.4 Record every forced mechanism divergence, with its cost, in the two-part form

The workspace harmonised two conflicting rules into one (ADR-0002 rule 7 × ADR-0008 §A.3): a
divergence marker carries **both** halves — the **versioned upstream citation** and the **owning ADR
path or `MCP-NNN` id** — and both are checked by the single `cargo xtask lint-citations` pass. For
this port:

```rust
// CYRUP-DELTA: pi-mcp-adapter@2.25.0 mcp-auth.ts `keyringAuthSecretStore` loads the native
// binding dynamically from a 12-triple table; Rust backends link at compile time, so the
// table has no counterpart. COST: none behaviourally — but the ERROR CONDITION it guarded
// (locked keychain, no D-Bus session, no default store) must still produce the exact
// sentence, now attached to `keyring::Entry::store_status()`. Owner: MCP-252 / ADR-0020.
```

Three properties, each load-bearing: the divergence is at the **site**, not in a document nobody
opens; the **cost** is named, so a reviewer can judge it; and there is an **owner**, so it cannot be
orphaned (blind spot 4).

**There is no "accepted divergence" category in this workspace.** A mechanism difference that costs
behaviour stays on the list as work. Recording it is disclosure, not approval.

### 3.5 The scaffold rule (this is how "no unapproved deferrals" is enforced)

Phase 0 is allowed to stand up incomplete code so that a real tool call round-trips. Nothing else is.

- A scaffold is marked `// SCAFFOLD(MCP-NNN)` naming the unit that replaces it.
- Phase 0's definition of done includes the census `rg -c 'SCAFFOLD\(MCP-' crates/cyrup-mcp/`, and
  publishing the list.
- Every later phase's definition of done includes `rg 'SCAFFOLD\(MCP-NNN\)' crates/` returning **zero**
  for each of its own ids.
- **A `todo!()`, an `unimplemented!()`, a silent `Ok(())` arm, or a stub that always errors is not a
  scaffold — it is an incomplete build.** A registered tool that always errors is strictly worse than
  an absent one.

### 3.6 Determinism and ordering are part of behaviour

Two mechanical traps this port walks into repeatedly, both already diagnosed:

- **Map ordering.** `serde_json` is built without `preserve_order` tree-wide, so `serde_json::Map` is
  a `BTreeMap`. `cyrup-mcp` therefore deserialises config and metadata into its own
  `indexmap::IndexMap`-backed types (`indexmap` is already in the lock file) and **never** through
  `serde_json::Value`, so that servers, tools and metadata keep **config insertion order** the way
  upstream's plain objects do. The one exception is the fixed `mcp` tool schema, whose property order
  is alphabetised by `serde_json` — accepted, recorded, and *not* fixable by pre-rendering a
  `&'static str` (parsing re-normalises).
- **Elicitation property order.** Do **not** hand-write a `MapAccess` deserializer for this: rmcp's
  `ElicitationSchema` deserialises through `ElicitationSchemaWire` whose `properties` is an
  `IndexMap`, and the `From` impl fills `ElicitationSchema::property_order`. Iterate that field. The
  `BTreeMap` is the trap.

### 3.7 Reuse what already exists; do not re-port it

Four cyrup assets are **fixed contracts, not suggestions**, and one more is a reuse chore:

| asset | relationship |
|---|---|
| `cyrup_ext::caps::proc::npx_resolver` | already a full port of `npx-resolver.ts` (same `CACHE_VERSION`, 24 h TTL, 30 s force-cache timeout). Reuse via a one-line `pub` promotion; six confirmed gaps against v2.25.0 are separate units |
| `cyrup_ext_subagents::exec::mcp_direct_tools` | an existing **reader** of `<agent_dir>/mcp-cache.json` at schema version 1 with the 7-day rule, and an existing port of `computeMcpServerHash`. `cyrup-mcp` is the **writer** of a file that already has a reader |
| `cyrup_permission_system::manager`'s `create_mcp_permission_targets` | already knows the `mcp` tool's exact argument shape and derives MCP permission targets from it. **Renaming a parameter silently changes which permission rules apply** |
| `cyrup_provider::auth::oauth::callback` | a complete loopback HTTP callback listener with `CallbackServerConfig::{fixed, ephemeral, with_host, advertising, with_cancel}` and a handler contract (`CallbackOutcome::Continue`) that makes it persistent and multi-tenant. Reused, not rebuilt |
| `cyrup_tools::truncate` / `cyrup_tools::output` | the dual byte/line truncation model and the temp-file spill pattern the output guard needs |

---

## 4. The phase plan

### 4.0 The ordering criterion, stated so that disagreeing is cheap

> **Order by the earliest point at which a real MCP server does real work in a real session**, subject
> to two hard constraints: **(a)** no phase may depend on a later phase, and **(b)** a phase that
> writes a file another crate already reads lands before anything else that writes that file.
> Everything reachable only through a remote *authenticated* server comes after everything reachable
> through a local stdio server.

Three separable claims. Disagree with (a) and you are arguing the dependency graph is wrong — check
it. Disagree with (b) and you are arguing the `mcp-cache.json` contract is not load-bearing — it is,
and §7.4 says why. Disagree with the value ranking and you get a different order for phases 5-8 only;
0-4 are forced by (a) and (b).

**Every one of the 433 port units appears in exactly one phase.** The census is §4.14. A unit in two
phases is double-booked work; a unit in none is orphaned work, which is exactly what structural blind
spot 4 describes. **A phase that schedules an id which does not exist is the third failure of the same
kind**, and this plan has committed it: an earlier edition scheduled fourteen deleted section-04 ids
in Phase 6, one of which was a ruling the scope decision had already made. §4.14 now names every
absent id with its reason, and the check is mechanical — the id set of this section must equal the id
set of the seam map's port-unit table.

**Verdict distribution across the whole set**, counted from the nine section files with a compound
verdict attributed to its first-listed term: **298** `hand-written`, **37** `rmcp`, **33**
`extension-owned`, **27** `host-verb`, **17** `open-decision` (each with an ADR in §9), **11** `cut`,
**9** `host-addition` — three distinct host surfaces (HA-1 across `MCP-037` + `MCP-037a` + `MCP-039` +
`MCP-193` + `MCP-217` + `MCP-395`, HA-2 across `MCP-041` + `MCP-382`, HA-3 at `MCP-368`), with a tenth
unit, `MCP-152`, carrying an HA-1 leg behind a `hand-written` primary. **No protocol code is
hand-written at all** — that is the headline consequence of the scope decision, and it is the single
biggest sizing input to this plan.

---

### Phase 0 — Walking skeleton, and the instrument

**One real MCP server, over stdio, one real tool registered and callable end-to-end, before any
breadth work.** Nothing in this backlog has been observed; this is the phase that makes an
observation possible.

**The server, named:** `@modelcontextprotocol/server-everything`, spawned as
`npx -y @modelcontextprotocol/server-everything`. It is the server rmcp's own client example
(`examples/clients/src/everything_stdio.rs`) drives, and it exercises tools, prompts, resources,
sampling, elicitation, progress and logging from one process — so it stays useful for the whole port,
not just this phase. **Hermetic backstop for CI:** rmcp's `examples/servers/src/counter_stdio.rs`,
which needs no network and no npm.

**Closes (15):** MCP-001, MCP-050, MCP-101, MCP-102, MCP-112, MCP-131, MCP-148, MCP-216, MCP-289,
MCP-482, MCP-489, MCP-491, MCP-493, MCP-494, MCP-495.

**Prerequisites:** none. This is the first thing anyone does.

**Definition of done.**
1. `crates/cyrup-mcp` exists as a workspace member, is a `NativeExtension` with `is_ambient = true`
   and `decides_project_trust = false`, and is attached in `crates/cyrup/src/main.rs` through
   `SessionFactory::with_native_extension`.
2. The pinned `rmcp` line of §2.3 is in `Cargo.toml`, and **MCP-493's manifest-policy test asserts the
   exact feature list**, so a future `--all-features` or a transitive unification cannot silently
   widen it.
3. `TokioChildProcess` spawns the named server with `env`, `cwd` and `.stderr(Stdio::piped())`;
   the stderr tail is captured; `graceful_shutdown` runs on teardown and **no orphan survives**.
4. **One tool from that server is registered through `InitApi::register_tool` during `init()` and is
   callable by the model, and someone has watched it happen in a real terminal.**
5. The scaffold census is published. Expected scaffolds and their owners: config loading
   (SCAFFOLD → MCP-052), content transform, text arm only (→ MCP-220), naming, verbatim only
   (→ MCP-200).
6. The four decisions this phase cannot proceed without are written as ADRs: **ADR-0012** (scope),
   **ADR-0013** (rmcp), **ADR-0021** (seam-test home + merge-gate shape), **ADR-0023** (fixtures).
7. MCP-482's **cut census** is published: for each of the four cuts, the list of upstream files and
   symbols it removes, and the list of `__tests__` files that go with it (12 of 96 pin cut surfaces
   only; `ui-tool-visibility.test.ts` splits 2/1; **84 vitest + 5 node:test files remain in scope**).

**Verification.** A live pty transcript of the tool call. `ps` before and after showing no orphaned
child. The manifest test. `rg -c 'SCAFFOLD\(MCP-' crates/cyrup-mcp/` published.

**Branch — this phase is a decision point, not a formality.**
- *The tool does not reach the model* → the extension seam is misunderstood and everything after
  phase 4 is mis-sized. Re-read `ExtensionHost::load_native_with_services` before continuing.
- *The child orphans* → the npx pre-resolution question (MCP-103, MCP-105) moves ahead of phase 3.
- *`rmcp` cuts a MAJOR during this phase* → take the bump here, where nothing is built on it yet.

---

### Phase 1 — Config, the type model, secrets and errors

Everything downstream reads config. Nothing else can be right if this is wrong, and the two
security-critical units of the whole port live here.

**Closes (48):** MCP-002, MCP-047, MCP-048, MCP-049, MCP-051…MCP-070, MCP-077…MCP-093,
MCP-095…MCP-099, MCP-342, MCP-349.

*(MCP-054 and MCP-080 are `cut` records kept in the sequence rather than deleted, so the cut is
auditable. MCP-342 and MCP-349 are section-07 ids for the same subjects as MCP-082 and MCP-083; they
land here so the duplication resolves in one place rather than being implemented twice.)*

**Prerequisites:** Phase 0.

**The two criticals, and which clause each meets.**
- **MCP-053** — `mergeServerMaps`' URL-bound credential stripping. Losing it ships a **global** file's
  bearer token to a **project** file's new endpoint. *Permission bypass.*
- **MCP-083** — `!` / `!!` command-secret resolution. Resolving at merge / hash / preview time rather
  than at connect time means merely **listing** config in a repo containing a hostile `.mcp.json`
  executes arbitrary shell. *Permission bypass.*

**Definition of done.**
1. `<agent_dir>/mcp.json` is read as **JSONC**, through `cyrup_permission_system::jsonc` — the same
   parser `cyrup_permission_system::manager::read_configured_mcp_server_names` uses on the same file,
   so both crates parse it identically by construction.
2. The six-source precedence ladder with its dedup guards and its
   kind/importKind/shared/scope/writePath quintuple; the seven host-config import families, including
   `opencode`'s git-root walk and merge-all and `codex`'s three snake_case remaps.
3. All **23** `McpSettings` keys with the **read-site predicate** for each — not the documented
   default. (`notifyOnStartupConnect` is `!== false`; `idleTimeout` is
   `typeof === "number" ? v : 10`, so `0` disables; `collapsedResultLines` is a `1|2|3` whitelist, not
   a clamp.) All **28** `ServerEntry` fields.
4. Both cut config shapes produce a **named load-time diagnostic**: `httpTransport: "sse"` (Cut 1) and
   `socket` (Cut 3). Silently ignoring either is a defect, not a simplification.
5. `interpolateEnvVars` implements **all three** upstream syntaxes — `${X}`, `$env:X`, **`{env:X}`**.
   Both existing in-tree copies implement two; the third is missing from each.
6. The atomic raw-config writer, the five config writers and their `preview*` twins.
7. **ADR-0017 accepted and executed** — `<agent_dir>` resolution consolidated onto one resolver that
   `cyrup-permission-system` and `cyrup-mcp` share, so the permission gate and the extension cannot
   enumerate different server sets; and the `~/.pi/agent` migration question settled. **ADR-0018
   accepted** — whether the two project-scoped config sources honour project trust.
8. Error taxonomy on `thiserror`; the logger as a `tracing` adapter with all four levels and the
   `MCP_LOG_LEVEL` filter.
9. **The security gate of §7.1 passes.**

**Verification.** Unit tests in-crate for every pure function. A hostile-`.mcp.json` test proving
`getMcpDiscoverySummary` spawns **no** subprocess. A golden config fixture set covering all six
sources and all seven import families. `jsonschema` bumped with `should_validate_formats(true)` on
**both** dialect validators and the `ajv-formats` delta registered.

---

### Phase 2 — Naming, the metadata cache, and the cross-crate contract

**This phase exists because another crate is already reading the file this port writes.** It lands
before anything writes `mcp-cache.json`, per ordering constraint (b).

**Closes (24):** MCP-071…MCP-076, MCP-094, MCP-139…MCP-147, MCP-200…MCP-206, MCP-219.

**Prerequisites:** Phase 1 (the hash pre-image is built from `ServerEntry`).

**The three criticals, and the clause each meets — all three are the same clause.**
`cyrup_ext_subagents::exec::mcp_direct_tools` already reads this file. Each of these produces a
**silently empty subagent tool allowlist with no error anywhere** — *silent wrong output.*

- **MCP-141** — the in-tree reader hashes **11** fields; upstream hashes **14** (`socket`,
  `protocolVersion`, `includeTools` absent) and hashes `url` **raw** rather than through
  `resolveServerUrl`.
- **MCP-142** — `stable_stringify` maps absent → `"null"`, where upstream emits the **bare
  nine-character token `undefined`**. Because the identity object always carries all 14 keys, this
  changes the digest for essentially **every** server.
- **MCP-146** — resource tools are named `read_<name>` upstream and `get_<name>` in the in-tree
  reader.

Two more sit beside them at `high`: **MCP-143** (the third interpolation pattern) and **MCP-144**
(`!` / `!!` semantics inside hashed values — the reader skips `interpolateSecretExpression`, so `!!X`
is not un-escaped and `!cmd` *is* interpolated).

**Definition of done.**
1. One **shared naming module** both `cyrup-mcp` and `cyrup-ext-subagents` depend on (ADR-0016),
   implementing the four `ToolPrefix` modes, hyphen-**preserving** `sanitizeServerPrefix` with
   per-code-point hex escaping, `formatToolName`, `resolveServerFromToolName` **with its ambiguity
   fail-safe**, `getToolNameCandidates` (18 insertions, 5 + 13, deduped size data-dependent), glob
   matching, and `read_` resource naming.
2. `<agent_dir>/mcp-cache.json` is written at **`CACHE_VERSION = 1`** — **not renumbered** — with the
   UI fields (`uiResourceUri`, `uiStreamMode`) reserved, absent and ignored, and **`uiVisibility`
   still written and still read**, because `reconstructToolMetadata` filters on it.
3. **The five coordinated edits to `mcp_direct_tools` land as ONE change** with the golden-vector
   fixture: hashed field set 11 → 14, the `undefined` token, the third interpolation pattern, `!`/`!!`
   semantics, `get_` → `read_`. Each independently changes the digest or the name for essentially
   every server; landing them separately produces a window in which the cache is wrong in a *different*
   way.
4. Both existing partial ports are stamped with `@2.25.0` citations (§2.2 step 3).
5. **ADR-0016** (naming grammar) is accepted.

**Verification.** A **golden-vector fixture generated from the TypeScript** — server configs in,
digests and formatted names out — asserted by both crates. This is the only layer that catches this
class (§5.2). Plus: a round-trip test writing a cache from `cyrup-mcp` and resolving `mcp:` selectors
through `cyrup_ext_subagents::exec::mcp_direct_tools` against it.

---

### Phase 3 — Transports, the server manager and connection lifecycle

**Closes (36):** MCP-100, MCP-103…MCP-111, MCP-113…MCP-121, MCP-123…MCP-130, MCP-132…MCP-138,
MCP-149, MCP-498.

*(MCP-110, MCP-111 and MCP-121 are `cut` records. MCP-122 — URL-elicitation acceptance tracking —
moves to Phase 10 with the rest of elicitation, so this phase has no forward dependency.)*

**Prerequisites:** Phases 1, 2.

**Definition of done.**
1. `McpServerManager` with all five race guards and the full public API: `connections`,
   `connectPromises`, `reconnectPromises`, `closePromises`, `closeGenerations`, `connectAttempts`.
2. Streamable HTTP through `StreamableHttpClientTransport` + `StreamableHttpClientTransportConfig`;
   header, bearer and `!command`-secret resolution; the HTTP attempt loop with its **verbatim arm
   order** — abort-cleanup-aggregate rethrow FIRST, abort check SECOND, implicit-challenge THIRD,
   generic 401 FOURTH — the SSE-fallback arm having been the last and now being the `throw err` that
   already preceded it.
3. `reinit_on_expired_session` **off**, and `withSessionRecovery` ported literally (ADR-0026 — on, it
   stacks two independent one-shot retries and hides the reconnect the adapter wanted).
4. Protocol-era negotiation onto `ClientLifecycleMode::{Initialize, Auto, Discover}` with **ADR-0019**
   settled — a stdio server that *exits* on `server/discover` fails under rmcp's same-transport probe,
   and upstream ships a fixture for exactly that case.
5. npx pre-resolution wired **in the connection builder** (upstream's position), not inside
   `ProcCaps::spawn` (where cyrup's copy currently sits, on the guest path); the six confirmed
   resolver gaps closed, including **exact package-version pinning** (MCP-105 — today
   `npx -y srv@1.2.3` spawns whatever has the newest mtime).
6. The endpoint probe survives whole and **gains** the Cut-1 ladder arm of §3.2.
7. Lifecycle modes `eager`/`lazy`/`keep-alive`/`lazy-keep-alive`, the idle sweep, the 60 s failure
   backoff, and `gracefulShutdown`'s memoised wait on the in-flight check.

**Verification.** Seam tests in `cyrup-it`'s **`bin`** target (process spawning) against rmcp's
`counter_stdio` and `counter_streamhttp` example servers, plus the two child-process host harnesses
(MCP-498). Kill-the-server tests for `isTerminatedSession` and `withSessionRecovery`. A test that a
`socket` or `httpTransport: "sse"` config produces the named diagnostic and **not** a connection.

---

### Phase 4 — Activation, the session lifecycle and the host seam

**Closes (33):** MCP-003…MCP-015, MCP-017…MCP-035, MCP-046.

*(MCP-016 — the sampling/elicitation wiring gates — moves to Phase 10, so this phase has no forward
dependency.)*

**Prerequisites:** Phases 1, 2, 3.

**The one critical.** **MCP-003** — a native extension's `init()` returning `Err` is a fatal startup
diagnostic that exits 1. A malformed `mcp.json` must therefore **never** fail `init()`; it must
register the surface it can from disk caches and report the rest. *Crash on a normal path.*

**Definition of done.**
1. `init()` registers the entire tool and command surface **from disk caches, synchronously, and never
   fails** — which is what upstream itself does (`syncDirectTools(earlyConfig, earlyCache)` runs
   before any server connects).
2. `McpRuntimeOwner`, the abort helpers, and generation fencing on an `AtomicU64` + a
   `CancellationToken` tree; the `session_start` generation protocol with abort-before-await; the
   `session_shutdown` handler; `shutdownState`'s four-step sequence **preserving the metadata-flush
   error in preference to the shutdown error**.
3. Owner cleanups registered in **exact LIFO order** — `lifecycle.gracefulShutdown()` → `shutdownOAuth`
   → `cleanupMaterializedBinaryResources` — so an in-flight OAuth callback can be refused cleanly
   after the servers close.
4. The two-pass startup metadata build; the bounded startup connect pass; `updateMetadataCache`'s
   write rules; `flushMetadataCache` on shutdown.
5. **The `is_run_cancelled` wiring gap is closed** — `LiveHostServices` overrides it against the live
   run token. One method body. Four port units read it (MCP-007, MCP-033, MCP-040, MCP-046) and all
   four are silently wrong without it.
6. `updateStatusBar`'s three footer verbosities through `HostServices::set_status(key, Option<&str>)`,
   uncoloured — upstream's own no-theme branch.

**Verification.** Seam tests in `cyrup-it`'s **`session_svc`** target: a session start/shutdown/restart
cycle proving the generation counter fences a stale initialisation; a malformed-`mcp.json` test proving
the binary starts and reports rather than exiting 1; an Esc-during-startup-connect test proving the
abort reaches the child.

---

### Phase 5 — The tool surface: registration, approval, the output guard, rendering

The largest user-visible payload in the port, and the phase where **HA-1** is either built or
consciously accepted.

**Closes (49):** MCP-036, MCP-037, MCP-037a, MCP-038, MCP-045, MCP-193, MCP-197, MCP-199,
MCP-207…MCP-212, MCP-214, MCP-214a, MCP-217, MCP-217a, MCP-217b, MCP-218, MCP-220…MCP-246, MCP-248,
MCP-249.

*(MCP-209 is a `cut` record. MCP-037, MCP-037a, MCP-193 and MCP-217 are four ids for HA-1 across three
sections; they land together so the duplication resolves once, and **MCP-037a is not optional if
MCP-037 is built** — see below. MCP-215 — `attemptDirectAutoAuth` — moves to Phase 8 with OAuth; this
phase implements the `needs-auth` **refusal** path, which is exactly upstream's behaviour with
`autoAuth` off.)*

**Prerequisites:** Phases 2, 3, 4.

**The two criticals.**
- **MCP-212** — `resolveDirectTools`' builtin-collision drop. Without it an MCP server's tool
  **silently replaces a cyrup built-in** (`ExtensionRegistry::active_tools` lets a registered tool
  override). *Silent wrong output.*
- **MCP-232** — `ensureToolCallApproved`. An approval gate that cannot distinguish a **cancelled
  dialog** from a **headless session** fails open. *Permission bypass.*

**Definition of done.**
1. `buildToolMetadata` with its exact ordering, including that a hidden tool does not claim its name;
   `formatSchema`'s exact annotation key order; `resolveDirectTools`' five warnings and 75-tool
   advisory.
2. The 13-step direct-tool executor in its **exact order**: disabled-server check → owned-signal
   composition → `lazyConnect` → auth handling → connection assertion → `ensureToolCallApproved` →
   request options → `withSessionRecovery`-wrapped `tools/call` → content transform → output guard →
   error/abort mapping → in-flight decrement → `touch`. Every `details.error` code and message string
   byte-exact.
3. `transformMcpContent` for **every** standard content type — text, image, audio, resource,
   resource_link — plus the structured-content fallback. Binary-resource materialisation with all
   four limits, `0o600` in a `0o700` tempdir, exclusive create, and the cleanup drain with its retry.
4. `mcp-output-guard.ts` in full: 50 KiB / 2000 lines / 16 KiB defaults, the kill switch's tri-state,
   `reserveBudget`, `truncateHead`'s partial line, `truncateStringToBytes`' continuation-byte backoff,
   the notice wording, `saveArtifact`'s spill, the summary schema's depth-2 / 20-entry bounds.
5. The renderer through `InitApi::register_tool_renderer` + `NativeExtension::{render_call,
   render_result}`, `compact`/`boxed` shells, `collapsedResultLines`.
6. **HA-1 is decided by ADR-0014 and the decision is executed, in both halves.** If built: a defaulted
   `NativeExtension::set_ext_host(Weak<ExtensionHost>)` called beside the existing
   `set_host_services`, plus a `register_late_command` sibling — **and** the one-line correction to
   `ExtensionHost::refresh_tools` (`MCP-037a`) without which the handle changes nothing at all. If
   accepted: `disableProxyTool` is documented as **unsupported**, and the degradation is written into
   the user-facing docs — on a cold cache the first session exposes only the `mcp` proxy tool and
   direct tools appear next session.
7. **The security gates of §7.2 and §7.3 pass.**

**The trap in item 6, stated so nobody half-builds it.** `refresh_tools` returns the *guest*
materializer's verdict, and the `wasm-host` arm of that materializer iterates only the guest
descriptor map — a different map from the one a native tool lands in. The default feature set is
`wasm-host`. So in a production build a native late registration raises the dirty flag,
`refresh_tools` consumes it, reports `false`, and `AgentSession::refresh_extension_tools` returns
without a diagnostic; `take_tools_dirty` is a swap, so the signal is gone. The tool reappears only at
the next session build. Building `MCP-037` alone therefore produces a handle that silently does
nothing, while the extension and the `/mcp` panel both believe the tool is live — which is worse than
not building it. The seam map's Finding 1 has the full trace.

**Verification.** Unit tests for every pure function (naming, glob, schema formatting, the truncation
arithmetic). A seam test in `session_svc` proving a registered direct tool reaches the live agent's
tool array. **For `MCP-037a` that seam test runs twice — `--features wasm-host` and without — and both
runs must reach `agent.tools()` on the next turn.** A builtin-collision test asserting `read` from an
MCP server does **not** shadow the built-in. An approval test asserting a cancelled dialog and a
headless session are distinguishable and both **deny**.

---

### Phase 6 — The `mcp` gateway tool, the proxy modes and search ranking

After the cuts this phase owns exactly **one** model-facing tool with **nine** dispatch arms
(`status`, `list`, `search`, `describe`, `instructions`, `connect`, `call`, `auth-start`,
`auth-complete`).

**Closes (33):** MCP-043, MCP-044, MCP-151, MCP-152, MCP-153…MCP-161, MCP-163, MCP-164, MCP-165,
MCP-169…MCP-178, MCP-192, MCP-194, MCP-195, MCP-196, MCP-198, MCP-213, MCP-247.

*(MCP-044 is a `cut` record, kept in the sequence so the cut is auditable rather than invisible.
MCP-162, MCP-167 and MCP-168 — the auth-facing modes — move to Phase 8. **Fourteen ids that earlier
editions of this plan scheduled here no longer exist and are not work:** MCP-150, MCP-166 and
MCP-179…MCP-190 — see §4.14. In particular **MCP-180 was "decide how cyrup executes adapter-authored
JavaScript"**, which is the exact ruling Cut 4 exists to eliminate, and scheduling it in the document
engineers execute from would have reopened it.)*

**Prerequisites:** Phases 2, 3, 5.

**The one critical.** **MCP-163** — `executeCall`'s resolution state machine. Its fail-closed ambiguity
gate is the only thing stopping first-match resolution from **silently routing a call to the wrong
server's same-named tool**. *Silent wrong output.*

**Definition of done.**
1. The `mcp` tool registered with the **exact** 12-property JSON Schema, the five permission-read
   property names (`tool`, `server`, `connect`, `describe`, `search`) unchanged, and `action`'s
   description carrying the Cut-2 narrowing of §3.2.
2. `buildProxyDescription` regenerated byte-for-byte minus the two cut lines, including the usage
   block's column alignment. It is the prompt-cache key.
3. Search ranking to reimplementation depth: weights 12/10/8/5/5, `MIN_STEM_LENGTH` 4, per-field
   phrase bonuses ×14/×9/×6, per-token ×4/×2/×1 with the deliberately asymmetric stem rule, the
   keyword phrase bonus as a max-over-phrases added once, the coverage gate (≤2 tokens needs 1.0,
   longer needs 0.6, bypassed by any phrase match), final +25 / +round(cov×10), +8 first-token-in-name,
   +20 whole-field-exact, sort by score desc then the ADR-0024-adjacent tie-break, and `paginate`'s
   clamping.
4. The regex search path on `regex`'s linear-time engine with **explicit** `size_limit` /
   `dfa_size_limit`, surfacing compile failure as `invalid_pattern`. The `unsafe_pattern` code stays
   in the enum as a documented no-producer variant, so 31 of 32 codes are reachable and the 32nd is
   accounted for rather than missing.
5. The `details.error` conformance table frozen (MCP-169).

**Verification.** The **46 portable proxy-mode conformance cases** (47 upstream, minus the three
`ui-messages` cases and the 20 `mcp-code` cases which are cut; one case — "rejects
catastrophic-backtracking regex queries" — is **re-specified rather than ported**, because the attack
it tests is structurally impossible against `regex`). All **11** search-ranking cases port verbatim.

---

### Phase 7 — Credential storage and the OS keychain

**Zero host additions. Nothing in this phase consults `HostServices` at all** — a native crate opens
its own keychain handles, spawns its own subprocess and writes its own files. The seam-map thesis holds
here without qualification.

**Closes (40):** MCP-250…MCP-278, MCP-280…MCP-288, MCP-290, MCP-291.

*(MCP-272, MCP-273, MCP-274 are `cut` records — `ConsentManager`, `ConsentError` and the process-scoped
consent state go with Cut 2, their only production consumers being `ui-server.ts` and `ui-session.ts`.
Their full behavioural contract is recorded anyway, because the denied-server asymmetry and the
always-mode one-shot exist nowhere else in writing. MCP-279 was dropped as dead scaffolding and is
deliberately absent from the sequence.)*

**Prerequisites:** Phase 1 (agent-dir resolution, error taxonomy).

**The two criticals.**
- **MCP-264** — URL binding and the mutators' sibling-purge rule. Failing it presents a credential
  minted for one authorization server to another, and reuses a PKCE verifier across contexts.
  *Permission bypass.*
- **MCP-277** — secret leakage through `Debug`, logs and errors. *Permission bypass, with
  log-retention blast radius.*

**Definition of done.**
1. `rmcp::transport::auth::{CredentialStore, StateStore}` implemented over the OS keychain, one store
   instance per server bound to that server's account key (the trait is keyless by design, so this is
   the natural shape, not a workaround).
2. **ADR-0020 accepted**, settling three things at once: the service name (`cyrup.mcp.oauth`, with a
   one-time **read-only** import that deliberately does **not** delete the source, because the payload
   is not wire-compatible with upstream's and a co-installed `pi-mcp-adapter` still owns it); the
   keychain-mandatory posture; and which Linux credential store links — which decides whether
   MCP-260/261/262/287 are live code or must be **cut in the same breath**.
3. The chunking manifest (Windows `CRED_MAX_CREDENTIAL_BLOB_SIZE`), the chunked read path,
   stale-chunk cleanup ordering, and `chunkCount` bounded at 64 with anything larger treated as "not a
   manifest".
4. The `keyctl session -` re-exec under a hidden `__mcp-keyring-helper` subcommand in
   `crates/cyrup/src/`, beside the two existing precedents, with the same one-shot JSON stdio
   protocol, the same 10 s timeout, and the same revoked-keyring cause-chain regex. **No node.**
5. The legacy plaintext `tokens.json` one-time import **and delete** — noting the deliberate asymmetry
   with the keychain importer, which must not delete.
6. Serialized per-server read-modify-write on a `tokio::sync::Mutex`, copying
   `cyrup_provider::auth::store::CredentialStore::modify`'s shape (the mutex half only, not its
   `FileLock` half).
7. MCP credentials **never** reach `auth.json` — `cyrup_config::auth::AuthStore`'s `Credential` record
   is `ProviderId`-keyed with no `serverUrl`, no PKCE slot and a millisecond expiry; it cannot hold an
   MCP credential without a schema change and a plaintext downgrade.
8. **The security gate of §7.1 passes** for this phase's half.

**Verification.** The 17-test storage acceptance suite and the 13-test cache acceptance suite, run
against `keyring_core::mock::Store` with `set_error` (ungated, and it replaces four of upstream's five
hand-rolled fault-injection backends). Two subprocess tests behind an explicitly declared `[[test]]`
target. **A leakage test that constructs a fully-populated credential record and asserts the rendered
`Debug`, `Display` and `tracing` forms contain no token substring.**

---

### Phase 8 — OAuth 2.1, the callback server, and the auth-facing modes

**The port hand-writes no OAuth protocol code.** Verified first-hand in the checkout:
`crates/rmcp/src/transport/auth.rs` ships RFC 9728, RFC 8414/OIDC, RFC 7591, PKCE S256 (always, never
`plain`), RFC 8707 resource binding, the RFC 9207 `iss` gate, SEP-2207 `offline_access`, SEP-991 CIMD,
SEP-835 scope upgrade, auto-refresh with refresh-token preservation, and the client-credentials grant.

**Closes (51):** MCP-162, MCP-167, MCP-168, MCP-191, MCP-215, MCP-300…MCP-333, MCP-335…MCP-341,
MCP-343…MCP-347.

*(MCP-334 — the `/mcp-auth` command surface — moves to Phase 11 with the other commands. MCP-342 and
MCP-349 landed in Phase 1. MCP-348 was dropped as dead scaffolding and is deliberately absent.)*

**Prerequisites:** Phases 3, 7.

**The one critical.** **MCP-306** — the callback request handler's eight branches, and specifically its
`state` checks. *Permission bypass.*

**Definition of done.**
1. The loopback listener built on `cyrup_provider::auth::oauth::callback` — **reused, not rebuilt**.
   `CallbackHandler` returning `CallbackOutcome::Continue` never settles the server's one-shot, which
   is what makes the listener persistent and multi-tenant; the `state`-keyed oneshot map and the
   refcount are adapter code on top.
2. The bind/rebind/strict-port state machine with both "cannot be switched while authorizations are
   pending" strings; the `localhost` vs `127.0.0.1` bind question settled per §9.1.
3. The three callback HTML pages verbatim.
4. **The three named rmcp deltas closed, each ~10-40 lines and each with its `CYRUP-DELTA` block:**
   `initialize_from_store` restores only `client_id`, so the DCR client secret and redirect URI must be
   re-applied through `configure_client`; rmcp's `ClientRegistrationRequest` body is fixed and drops
   `client_uri` / `logo_uri` / confidential `client_secret_post`, so the registration POST is performed
   in `cyrup-mcp` and the result handed to `configure_client`; rmcp's client-auth selection follows the
   TypeScript SDK's rule, not the adapter's, so `token_endpoint_auth_methods_supported` is injected via
   `set_metadata` before `configure_client` when the AS published none and a secret is configured.
5. `skipIssuerMetadataValidation` maps to `set_allow_missing_issuer` with the **narrowing named**: rmcp
   tolerates a *missing* issuer but still fails a *mismatched* one.
6. The manual/headless paste leg, the callback-versus-paste race, the 5-minute abandoned-flow timer,
   `authenticate`'s in-flight dedup, and `startAuth`'s ordering with its five stale-registration
   branches and four aggregate-cleanup phase strings.
7. `attemptAutoAuth` / `attemptDirectAutoAuth` with the single-shot latch; `executeAuthStart` /
   `executeAuthComplete`; the `authRequiredMessage` templating.
8. **ADR-0025** records that `mcp({action:"auth-start"|"auth-complete", server:"x"})` derives
   `mcp_list` — a baseline auto-allow target — which is faithful to pi and is filed upstream rather
   than hardened here without separate sign-off.

**Verification.** The node:test OAuth suite ported as a **serialised** group (MCP-492). Hermetic
end-to-end against rmcp's `complex_auth_streamhttp` example server, which is a **complete
self-contained authorization server** — DCR, authorize form, token endpoint — so no third-party IdP is
needed (§6.3). A test that a callback carrying a wrong or absent `state` settles **nothing**.

---

### Phase 9 — Conformance: the protocol gate

**Closes (7):** MCP-483, MCP-484, MCP-485, MCP-486, MCP-487, MCP-488, MCP-492.

**Prerequisites:** Phases 3, 8.

Placed here because the conformance client suite is streamable-HTTP + OAuth: it cannot run before
those exist, and it must run before the adapter layer is built on top of them. Full wiring in §5.4.

**Definition of done.** A hidden `cyrup mcp conformance-driver` subcommand; both versioned suites green
from an **empty** expected-failures baseline; the baseline file written from an observed run, never
copied; the runner's port strategy settled by ADR-0022; and MCP-488's explicit record of **what
conformance does not cover** (see §5.4).

---

### Phase 10 — Sampling and elicitation

**Closes (26):** MCP-016, MCP-122, MCP-450…MCP-472, MCP-496.

**Prerequisites:** Phases 3, 4, 5.

**The one critical.** **MCP-455** — the two sampling approval gates. Inverting one is a *permission
bypass*: a remote MCP server would drive a nested completion, on the user's credentials, without
consent.

**Definition of done.**
1. `ClientHandler::create_message` and `create_elicitation` overridden; capabilities advertised as
   `{ sampling: {}, elicitation: { form: {}, url? : {} } }` with `allowUrl == (mode == tui)`.
2. **Sampling runs through `cyrup-provider` directly** — `cyrup_provider::catalog::{builtin_catalog,
   load_catalog}` for the candidate set and the completion path for the call. This is the faithful
   port: upstream imports `complete` from `pi-ai/compat` and bypasses pi's host API entirely, so a
   host verb here would be the divergence. `HostServices::{models, scoped_models, current_model}`
   supplies the session's own view.
3. **The six unsupported-sampling-feature rejections in order.** `"task" in params` becomes
   **structural**, not a runtime string check: `CreateMessageRequestParams` has no `task` field, and
   task augmentation is the `io.modelcontextprotocol/tasks` extension negotiated through
   `ClientCapabilities.extensions` — which `cyrup-mcp` never declares.
4. `handleFormElicitation`'s gate, review loop and edit picker; `collectValidField`'s per-field
   re-prompt; `coerceAndValidateFormValues` with **JS `Number()` semantics**; the final schema
   assertion with `format` as an **assertion**, not an annotation. Properties iterated in
   `ElicitationSchema::property_order` (§3.6).
5. `handleUrlElicitation` with its three `-32602` rejections
   (`rmcp::model::ErrorData::invalid_params`), the `notifications/elicitation/complete` dedupe and its
   notice, and `handleUrlElicitationRequired` for the `-32042` array.
6. Every dialog holds `HostServices::human_interaction_lock` **and** the dispatcher budget via
   `HostCtx::begin_human_wait`, so a slow human answer cannot fail-**open** the permission gate and a
   permission prompt and an MCP dialog can never both be on screen.

**Verification.** Unit tests for the coercion matrix and the 13 message templates. Hermetic end-to-end
against rmcp's `sampling_stdio`, `elicitation_stdio` and `elicitation_enum_inference` example servers.
**MCP-496: live-pty verification of every elicitation widget and both sampling gates** — see §5.6,
this is not optional.

---

### Phase 11 — Prompts, the panels, slash commands, tracing and status

**The largest phase in the plan, and deliberately the last substantive one.** It owns the whole of
`13h-mcp-tui.md` plus the tracing units, and it is the only phase whose definition of done cannot be
discharged by any automated gate this document describes.

**Closes (68):** MCP-350a, MCP-039, MCP-040, MCP-041, MCP-042, MCP-334, MCP-351…MCP-372, MCP-363a,
MCP-374…MCP-399 (including MCP-385a, MCP-394a, MCP-395a, MCP-397a), MCP-473…MCP-481.

*(MCP-373 — `glimpse-ui.ts` — was retired under Cut 2 and is deliberately absent; it is recorded in
`13h-mcp-tui.md`'s *Out of scope*. The section-08 units are large: `MCP-351`…`MCP-380` are the two
panels, `MCP-381`…`MCP-399` the slash commands and prompts. Six of them are `critical`.)*

**Prerequisites:** Phases 4, 5, 8. In practice everything — a panel that lists servers, tools,
credentials and connection status has a dependency on every subsystem that produces one.

**Definition of done.**
1. `/mcp` and `/mcp-auth` registered through `InitApi::register_command`, dispatched by
   `NativeExtension::execute_command` at **command tier** so `ControlOp::Reload` is legal from
   `/mcp setup`. The eight-arm switch with its two structurally divergent arms (`logout` with no
   argument and `setup`/panel after a reload **return** rather than break; `disable`/`enable` is the
   only pair that asks the user to `/reload` instead of reloading for them), the per-side-effect owner
   fence, and the argument-splitting rule — `reconnect` takes `parts[1]`, `logout`/`disable`/`enable`
   take the joined remainder, so `/mcp logout my server` targets `"my server"`. All eleven
   `/mcp-auth` messages.
2. The three panels — `/mcp` status, `/mcp setup`, `/mcp-auth` picker — implemented as
   `cyrup_ext::InteractiveOverlay` and opened with `HostServices::open_overlay`. Precedents to copy:
   `cyrup_ext_subagents::tui::fleet_overlay::FleetOverlay` for async work spawned out of an overlay
   and drained back into it, and `cyrup_permission_system::config_modal::PermissionSystemSettingsOverlay`
   for reading a result off an `Arc`-shared object after `open_overlay` returns — which is the only
   mechanism by which `McpPanelResult` can escape a `bool` return (`MCP-369`, `MCP-394`).
3. **HA-2 decided by ADR-0015 and executed**: either the native dispatch arm on
   `ExtensionHost::command_completions` plus the `cyrup-tui` `autocomplete::slash_context` consumer,
   or the accepted degradation (the commands work typed in full; `<TAB>` does not complete server
   names). Note the third leg: the TUI has **no argument-completion context at all** today —
   `slash_context` returns `None` at the first space — so this is a from-zero design, not a wiring
   change.
4. **HA-3 decided by ADR-0015** and owned by `MCP-368`: overlay geometry options, or the accepted
   cosmetic delta — the host draws no border, so the panels self-centre their 82/92-column content
   inside the 95% rect and the only visible difference is the width of the `Clear`. The *height*-clip
   half is not the host's and is discharged by `MCP-366` and `MCP-377`.
5. MCP prompts as slash commands: the cache-backed half at `init` (`MCP-395a`), and the live half
   (`MCP-395`) only if HA-1's command leg was built — which is three additions, not one (a post-`init`
   registry write, a dirty/refresh pair, and a catalog rebuild signal). Record the smaller delta
   either way: `slash_command_catalog` stamps `source: "extension"` on every extension command, so
   prompt commands surface labelled `Extension` rather than `Prompt`.
6. `McpTraceEvent` schema v1 with its exact key set **and insertion order**; `redactTraceText`
   including its provably-dead third branch; `McpTraceWriter`'s latching caps, injectable fs and
   serialized append queue; `TracingTransport<T>` as an `rmcp::transport::Transport` decorator.
7. Panel keybindings read from `<agent_dir>/keybindings.json` by the crate itself (`MCP-363`) — a
   native crate links `cyrup-config`, and that is the only way a user's `mcp.panel.save` is visible.
   This is **not** a host addition and must not be filed as one.

**The `TracingTransport` safety proof, already done, recorded so nobody re-derives it.**
`serve_client_with_ct_inner` is generic over `T: Transport<RoleClient>` and inspects no concrete type;
`ClientLifecycleMode::{Initialize, Discover, Auto}` probes `server/discover` over the **same**
transport. A newtype wrapper is safe. **The one surviving consequence:**
`DynamicTransportError::{is, downcast}` key on `TypeId::of::<T>()`, so wrapping changes the **error**
identity — that is the thing to test.

**Verification.** **Live-pty runs of all three panels, and nothing in this phase is done until it has
been run in a real terminal.** §5.6 is the rule and it is not negotiable: ratatui `TestBackend` tests
pass while the assembled application has layout and empty-state bugs, and this phase is the port's
largest concentration of exactly that risk. Seven units — `MCP-355`, `MCP-359`, `MCP-362`, `MCP-366`,
`MCP-368`, `MCP-369` and `MCP-377` — carry the requirement in their own `verify` lines; treat those as
the minimum list, not the whole one. The specific things a `TestBackend` will not tell you: the tool
tree's empty state, the fuzzy filter with zero matches, the token-estimate column at 82 columns, the
60-second inactivity auto-cancel, the height clip, and whether `Clear` erases more transcript than
intended. Plus a trace-JSONL golden test asserting key order.

---

### Phase 12 — Differential verification, the test census, and the merge gate

**Closes (3):** MCP-490, MCP-497, MCP-499.

**Prerequisites:** everything.

*(MCP-497 — coverage tracking — is a `cut` record. MCP-490 is a **meta-unit**: each phase ports its own
share of the 84 in-scope vitest files as it goes, and MCP-490 closes when the census shows every
in-scope file accounted for as ported, re-specified, or explicitly declined with a reason.)*

**Definition of done.**
1. The vitest census is complete and published: 96 upstream `__tests__` files → 12 pin cut surfaces
   only → 1 splits at the function boundary → **84 vitest + 5 node:test files in scope**, each marked
   ported / re-specified / declined-with-reason.
2. The trace-JSONL **differential harness** (§5.5) runs, against the fixture set of §6, with the cut
   surfaces excluded by construction.
3. The merge gate is the two-command form and is green: `cargo nextest run --workspace && cargo test
   --workspace --doc`. `--all-features` appears nowhere (guardrail G3).
4. The second gate — the integration suite — runs `cyrup-it` with `--features it` and is green, with
   the environment scrubbed of ambient credentials (guardrail R5).

---

### 4.14 The id census

| phase | ids | count |
|---:|---|---:|
| 0 | 001, 050, 101, 102, 112, 131, 148, 216, 289, 482, 489, 491, 493, 494, 495 | 15 |
| 1 | 002, 047–049, 051–070, 077–093, 095–099, 342, 349 | 48 |
| 2 | 071–076, 094, 139–147, 200–206, 219 | 24 |
| 3 | 100, 103–111, 113–121, 123–130, 132–138, 149, 498 | 36 |
| 4 | 003–015, 017–035, 046 | 33 |
| 5 | 036–038, 037a, 045, 193, 197, 199, 207–212, 214, 214a, 217, 217a, 217b, 218, 220–246, 248, 249 | 49 |
| 6 | 043, 044, 151–161, 163–165, 169–178, 192, 194–196, 198, 213, 247 | 33 |
| 7 | 250–278, 280–288, 290, 291 | 40 |
| 8 | 162, 167, 168, 191, 215, 300–333, 335–341, 343–347 | 51 |
| 9 | 483–488, 492 | 7 |
| 10 | 016, 122, 450–472, 496 | 26 |
| 11 | 350a, 039–042, 334, 351–372, 363a, 374–399, 385a, 394a, 395a, 397a, 473–481 | 68 |
| 12 | 490, 497, 499 | 3 |
| | **total** | **433** |

**Absent ids, each with its reason.** Three different kinds of absence are collapsed into one list
here because the failure they guard against is the same: a plan that schedules work nobody intends to
do.

| id(s) | why absent |
|---|---|
| **MCP-150** | a tool-surface index tracker. Dead scaffolding — the index it tracked is this census |
| **MCP-166** | `executeUiMessages`. **Cut 2.** The surviving nine-arm dispatch is `MCP-153`'s; an `action:"ui-messages"` call falls through to `executeStatus`, exactly as an unrecognised action already did |
| **MCP-179…MCP-190** | the entire `mcpScript` worker. **Cut 4.** `MCP-180` was *"decide how cyrup executes adapter-authored JavaScript"* — the ruling Cut 4 exists to eliminate. These are deleted rather than kept as auditable `cut` records **precisely so that no phase can schedule them**; §1.2's cut row is the audit trail instead |
| **MCP-279**, **MCP-348** | dropped as dead scaffolding, not cut |
| **MCP-373** | `glimpse-ui.ts`, a macOS-only native-webview viewer whose only caller is `ui-session.ts`. **Cut 2**; recorded in `13h-mcp-tui.md`'s *Out of scope* |
| **MCP-350** | the section-08 tracker, carrying the poll-repaint decision. Written; **excluded from the counts as a tracker**, per the convention that a row proposing no schedulable work is not planned against. `MCP-350a` was the other half of this row's debt and is now a counted unit in `13h-mcp-tui.md` |
| **MCP-292…MCP-299**, **MCP-400…MCP-449** | never allocated. There is no section 09 — the surface it would have held is MCP Apps, which is Cut 2 |

**A gap in the numbering is not evidence of a deletion** — that convention is inherited from the
gap-analysis directory, and the table above is the check that establishes it. The stronger check,
which an earlier edition of this document failed: **the id set of §4.14 must equal the id set of the
seam map's port-unit table, exactly.** Anything in this plan and not in that table is scheduled work
that nobody specified; anything in that table and not here is specified work that nobody scheduled.

---

## 5. Verification, in layers — and which layer catches what

Six layers. Each is listed with **what it catches**, **what it structurally cannot catch**, and where
it runs. A unit's test obligation (§1.1 clause 4) is discharged at the layer that catches its class,
not at whichever layer is cheapest.

### 5.1 In-crate unit tests — `crates/cyrup-mcp/src/**` under `#[cfg(test)]`

**Catches:** every pure function, and that is the large majority of this port — naming and glob
matching, the 18-expression candidate set, search ranking arithmetic, the coercion matrix, JSON-Schema
validation dispatch, the truncation arithmetic and its notice format, `stableStringify`, `ts-shape`
rendering, terminal sanitising, the config precedence ladder, the settings-merge predicates, the
`details.error` table.

**Cannot catch:** anything about assembly. A function can be perfect and never be called; a tool can be
registered and never reach the agent; a panel can render correctly to a `TestBackend` and be invisible
on a real screen.

**Rules that bind here** (TEST-ARCHITECTURE §4): **R2 — no `std::env::set_var` / `remove_var`**, which
is enforced by `clippy.toml`'s `disallowed-methods` and *fails the build*. Upstream's tests set env
vars freely; the in-tree answer is the injectable-directory pattern
`cyrup_ext_subagents::exec::mcp_direct_tools::McpDirs` already uses, and `cyrup-mcp` must take the same
shape from the start rather than retrofitting it. **R1** — tempdir per test, no fixed paths.

### 5.2 Golden vectors generated from the TypeScript

**Catches:** the cross-crate contract, and nothing else does. `computeServerHash`'s digest,
`stableStringify`'s `undefined` token, the four `ToolPrefix` modes, `sanitizeServerPrefix`'s
hex-escaping, `read_` resource naming, `getToolNameCandidates`' data-dependent deduped size (3 / 7 /
12 — **a fixed-cardinality test cannot pass**).

**How:** a one-off script run against `pi-mcp-adapter@2.25.0` emitting `(config in) → (digest, names
out)` as a checked-in fixture. Both `cyrup-mcp` and `cyrup-ext-subagents` assert against the **same**
fixture file, so the two crates cannot drift from each other or from upstream independently.

**Cannot catch:** anything the fixture does not enumerate. Generate it from the upstream test corpus
plus every `KNOWN_SERVER_PRESETS` entry, not from hand-written examples.

**This is the only layer that catches the port's three criticals in Phase 2**, which is why Phase 2's
definition of done names it explicitly.

### 5.3 Seam tests in `cyrup-it` — and the gating defect they inherit

**Catches:** spawned children and their teardown, the session-build arms, registration reaching the
live agent, the broker-shaped concerns, real sockets.

**Where they can live, and why it is constrained.** `crates/cyrup-it` is the workspace's one
integration crate. Two facts govern placement:

- **`autotests = false` and every `[[test]]` target carries `required-features = ["it"]`.** That is
  deliberate — it is the only manifest lever that skips a target under *any* package-selection flag —
  but **the consequence is that the default merge gate does not build or run any of them.** A test
  parked there and nowhere else satisfies nobody's definition of done (§1.1 clause 4).
- **Guardrail G2 caps the workspace at seven `[[test]]` targets**, and the seven are already taken:
  `subagents`, `intercom`, `ext`, `permission`, `session_svc`, `bin`, `misc`. An eighth requires a
  written justification in `docs/TEST-ARCHITECTURE.md` and **only two justifications are accepted**: a
  crate-level `#![cfg(...)]` the rest of the suite must not get, or process isolation because the
  target aborts, panics on unwind, installs a global handler, or mutates process-global state. **MCP
  seam tests meet neither.**

**Therefore:** MCP seam tests fold into the existing targets **by the seam they cross** — process-
spawning ones into **`bin`**, session-driving ones into **`session_svc`** — with **one** exception, the
two keyring subprocess tests of Phase 7, which need an explicitly declared target and *do* meet the
process-isolation justification. **ADR-0021** settles this and the merge-gate question together, since
they are the same question asked twice.

**Rules that bind here:** **R4** — no fixed ports, bind `:0` and read the assignment back. This
directly touches the OAuth callback listener's tests and the conformance runner's port strategy
(§5.4). **R5** — ambient credentials are scrubbed and the scrub is asserted; the integration suite
already carries guard tests that fail when `TOGETHER_API_KEY`, `GITHUB_TOKEN` and friends leak in.

### 5.4 The `@modelcontextprotocol/conformance` harness — the protocol gate

**This is demonstrated, not inferred.** The rmcp checkout ships `conformance/src/bin/client.rs` and a
`.github/workflows/conformance.yml` that runs:

```
npx -y @modelcontextprotocol/conformance@0.2.0-alpha.10 client \
  --command "$(pwd)/target/debug/conformance-client" \
  --suite all --spec-version {2025-11-25,2026-07-28}
```

**The harness can be pointed at a Rust binary, and this is how.** The client contract, read from
`conformance/src/bin/client.rs`:

| input | meaning |
|---|---|
| `argv[1]` | the server URL to connect to |
| `MCP_CONFORMANCE_SCENARIO` | scenario name; defaults to `initialize` |
| `MCP_CONFORMANCE_CONTEXT` | JSON context for the scenario |
| `MCP_CONFORMANCE_PROTOCOL_VERSION` | the revision to negotiate |
| `MCP_CONFORMANCE_TIMEOUT_SECS` | per-scenario timeout |

**Wiring for cyrup:** a hidden `cyrup mcp conformance-driver` subcommand, pre-dispatched from
`crates/cyrup/src/main.rs` alongside `__intercom-broker` and `__subagent-runner`, reading exactly that
contract and driving `cyrup-mcp`'s own connection path — **not** a bare `rmcp` client, or the run
proves nothing about the adapter.

**What it covers.** The client suite connects over **streamable HTTP** and exercises: initialize,
`tools/call`, JSON-Schema `$ref` handling, elicitation defaults, **`sse-retry`**, request metadata,
request-state, HTTP standard/custom/invalid headers, and **thirty `auth/*` scenarios** — the four
metadata variants, CIMD, scope derivation from `WWW-Authenticate` and from `scopes_supported`, all
three token-endpoint auth methods, 2025-03-26 backcompat, `offline_access`, the eight `iss` cases,
scope step-up and its retry limit, AS migration, pre-registration, resource mismatch, and both
client-credentials grants.

**What it structurally cannot cover, and this is the point of writing it down (MCP-488):**

- **stdio.** The driver takes a URL. Every stdio concern — `TokioChildProcess`, env/cwd resolution,
  stderr tail, orphan avoidance, npx pre-resolution — is invisible to it.
- **The entire adapter layer.** Multi-server management, the lifecycle modes, reconnect, session
  recovery, the metadata cache, tool naming, approval, the output guard, rendering, the proxy modes,
  the panels. Conformance tests the *wire*; the wire is `rmcp`'s and is already gated upstream.
- Anything about cyrup's session, permission or extension seams.

**Cut surfaces in the matrix: there are none in the versioned client suites.** Worth stating because
the obvious reading is wrong: **`sse-retry` is SSE stream *resumption inside streamable HTTP*** — rmcp
runs it with `StreamableHttpClientTransport` — and is **not** the cut legacy two-endpoint transport. It
stays in the matrix and must not be dropped along with Cut 1. Consequently:

- **The versioned suites run with an EMPTY expected-failures baseline**, exactly as rmcp's CI does, and
  every failure is a real failure.
- Only the **extensions** suite carries a baseline, and rmcp's has four informational client entries
  (`auth/enterprise-managed-authorization`, `auth/dpop`, `auth/dpop-nonce`, `auth/wif-jwt-bearer`).
- **Do not copy rmcp's baseline file.** The format is strict in both directions — *"a listed scenario
  that starts passing fails the build as a stale entry"* — so an inherited entry is a landmine. **Start
  empty and write the file from an observed run** (MCP-486).

**Version:** track rmcp's pin (`0.2.0-alpha.10`), not upstream's `0.1.16` — `--suite` and
`--spec-version` come from the newer line. **Runner shape:** upstream refuses `--suite` because its
pre-registered OAuth clients bind a fixed callback port; rmcp runs `--suite all` in parallel because
its driver does not. ADR-0022 decides which cyrup's driver is — and choosing `:0` per rule R4 drops
both the sequential constraint and the ephemeral-port probe.

### 5.5 The differential harness — the only oracle for the adapter layer

**Catches:** everything §5.4 cannot. Run the Node adapter and the Rust port against the **same** set of
servers, with the **same** config, and diff observable output.

**The diff surface, in order of signal:**
1. **The JSONL protocol trace** (`mcp-trace.ts`'s output). It is already a schema with a fixed key set
   and insertion order, already redacted, and it captures every request and response the adapter makes
   — which is precisely "did the two implementations talk to the server the same way".
2. **The `mcp-cache.json` written by each.** Byte-comparable after normalising timestamps.
3. **Tool-result envelopes** — the `details` object of every `mcp` tool call and every direct-tool
   call, including the `details.error` code.
4. **The regenerated proxy-tool description**, byte-compared modulo the five sanctioned string changes
   of §3.2.

**Exclusions, by construction:** anything reaching a cut surface. The runner refuses configs carrying
`socket` or `httpTransport: "sse"`, never invokes `mcpScript`, and never requests a `ui://` resource —
so a diff is always a real divergence and never a scope artefact.

**The dependency this creates, and it is the only reason it might not exist:** it requires
`pi-mcp-adapter` to stay checked out and installable in CI as a reference implementation. **ADR-0027**
decides that. If the answer is no, this layer disappears and the adapter layer has no oracle beyond
§5.1 and §5.2 — which is a real reduction in confidence and should be recorded as one rather than
absorbed silently.

### 5.6 Live-pty verification — and the house rule

> **A TUI port is not done until it has been run in a real terminal.** A ratatui `TestBackend` result
> is not admissible and does not close anything.

This is not boilerplate; it is a measured finding of this workspace. `TUI-055` — no indicator renders
for the entire 10-20 s of a compaction — is invisible to every static read: the source sets the
indicator and looks correct, and only running it shows the band never reaches the screen. Of seventeen
items driven through a real pty in the workspace's repro pass, **sixteen were confirmed to exist and
only three survived unchanged** — in each of the other thirteen the *verdict* was right and the
*picture of the screen* was wrong, and the picture is what a fix gets written against.

**What must be driven live in this port:**

| surface | phase | what a static read cannot see |
|---|---|---|
| `/mcp` status panel | 11 | the tool tree's empty state; the fuzzy filter with zero matches; the token-estimate column at 82 columns; whether `Clear` erases more transcript than intended (HA-3) |
| `/mcp setup` onboarding panel | 11 | the same at 92 columns, plus the multi-step flow's back/forward |
| `/mcp-auth` server picker | 11 | the picker with zero servers, one server, and more servers than fit |
| every elicitation widget | 10 | enum single- and multi-select, boolean, string, number, integer; the re-prompt loop on a coercion failure; the review-and-edit picker (MCP-496) |
| both sampling approval gates | 10 | the request preview's truncation at real widths; that Escape maps to **deny**, not to accept (MCP-455) |
| the footer status segment | 4 | all three verbosities, and that `None` actually clears it |
| the approval dialog | 5 | that a cancelled dialog is visibly distinguishable from a headless refusal (MCP-232) |

**Instrument validation is a first-class step, not a preliminary.** The workspace's repro pass produced
**three instrument errors in one sitting**: `tail` hiding a failing line, `pgrep -f` matching its own
pattern and inventing 22 orphaned processes, and `tmux display-message '#{cursor_x}'` reporting a stale
hardware cursor while the app paints its caret as an SGR-7 cell. Before any live row is recorded:
prove the harness reports a **known failure** as a failure, prove the process census does not count
itself, and prove the screen-scrape reads the cell the app actually painted.

---

## 6. Test servers and fixtures

### 6.1 Real third-party servers — develop against these

| server | launch | what it exercises |
|---|---|---|
| **`@modelcontextprotocol/server-everything`** | `npx -y @modelcontextprotocol/server-everything` | tools, prompts, resources, sampling, elicitation, progress, logging — from one process. rmcp's own `everything_stdio.rs` example drives it, so the wiring is demonstrated. **This is the walking skeleton's server.** |
| **`mcp-server-git`** | `uvx mcp-server-git` | a real **Python** server — the non-npx spawn path, `uvx` resolution, and a different stderr shape. rmcp's `git_stdio.rs` drives it |
| **`chrome-devtools-mcp@1.6.0`** | `npx -y chrome-devtools-mcp@1.6.0` | it is in `KNOWN_SERVER_PRESETS`, and its **pinned version** is exactly the case MCP-105 exposes: the shipped resolver writes `package_version` and never reads it, so a pinned spec spawns whatever has the newest mtime |

Note on Cut 4: these are external third-party OS processes — which is what MCP *is* — not a JS runtime
inside cyrup. Nothing here reintroduces the JS-engine question.

### 6.2 Hermetic servers — rmcp ships them, and this is the finding that makes CI cheap

`rmcp/examples/servers/src/` contains a ready-made fixture set covering **exactly** the three surfaces
that are otherwise hardest to test:

| fixture | covers |
|---|---|
| `counter_stdio.rs`, `calculator_stdio.rs` | the stdio spine, no network, no npm — the CI backstop for Phase 0 |
| `counter_streamhttp.rs`, `counter_hyper_streamable_http.rs` | streamable HTTP |
| `prompt_stdio.rs`, `completion_stdio.rs`, `memory_stdio.rs` | prompts, completions, resources |
| `structured_output.rs` | output schemas and structured content — the one place rmcp does **no** client-side validation, so MCP-092's `jsonschema` path is what is under test |
| `progress_demo.rs` | progress notifications and cancellation |
| `subscriptions_streamhttp.rs` | resource subscriptions (unused by the adapter, useful as a negative) |
| **`sampling_stdio.rs`** | a server that *initiates* `sampling/createMessage` |
| **`elicitation_stdio.rs`**, **`elicitation_enum_inference.rs`** | a server that *initiates* `elicitation/create`, both form shapes |
| **`simple_auth_streamhttp.rs`** | static bearer token |
| **`complex_auth_streamhttp.rs`** | **a complete self-contained authorization server** — dynamic client registration, an authorize form, a token endpoint |
| **`cimd_auth_streamhttp.rs`** | CIMD / SEP-991 |

### 6.3 How the three hard surfaces are tested hermetically

- **OAuth.** `complex_auth_streamhttp` is a full AS in-process. No third-party IdP, no network egress,
  no manual browser step: bind it on `:0` (rule R4), point `cyrup-mcp` at it, and drive DCR → authorize
  → callback → token → refresh → scope-upgrade end to end. `cimd_auth_streamhttp` covers the CIMD
  branch; `simple_auth_streamhttp` covers the static-bearer branch. **The browser hop is the only part
  that is not hermetic**, and it is stubbed at the `opener` boundary — inject the open-URL sink rather
  than launching a browser, and assert the URL, then feed the callback directly.
- **Sampling.** `sampling_stdio` initiates the request; a stub provider on `cyrup-provider`'s seam
  returns a fixed completion. The approval gates are driven live (§5.6) and asserted headlessly for the
  auto-approve and refusal paths.
- **Elicitation.** `elicitation_stdio` + `elicitation_enum_inference` initiate every primitive shape.
  The coercion matrix and the re-prompt loop are unit-testable in-crate; the widget selection and the
  review/edit loop are live-pty.

### 6.4 The eight surviving upstream fixtures

Upstream ships nine stdio fixture servers; `mcp-code-server.mjs` goes with Cut 4, leaving **eight**.
**ADR-0023** decides their fate, and the calculus changed once the rmcp checkout was read: rmcp ships a
1 780-line Rust MCP server in `conformance/src/bin/server.rs` plus the `examples/servers` set above.
Recommendation: **hand-roll the three NDJSON handshake servers in Rust** (they test framing edge cases
and are trivial), **build the rest on `rmcp/server` as a `[dev-dependencies]`-only feature**, and keep
**at most one** `.mjs` for genuine third-party interop. That needs a ruling on test-only `node` and on
`rmcp/server` in dev-dependencies — both are in ADR-0023.

---

## 7. Security gates

Three named gates, plus a fourth that protects the scope decision. Each is a **merge blocker for the
phases named**, each is a short list of concrete checks, and each check is an automated assertion
rather than a review opinion.

### 7.1 The credential gate — blocks Phase 1, Phase 7 and Phase 8

| # | check | unit |
|---|---|---|
| C1 | A fully-populated credential record renders **no token substring** through `Debug`, `Display`, `tracing` at every level, or any error type's `source()` chain | MCP-277 |
| C2 | A stored credential is bound to its **server URL**, and a mutator purges siblings — a credential minted for AS *a* is never presented to AS *b*, and a PKCE verifier is never reused across contexts | MCP-264 |
| C3 | Credentials live **only** in the OS keychain. No plaintext fallback path exists, and the legacy `tokens.json` import deletes its source while the keychain import does **not** | MCP-256, MCP-281 |
| C4 | MCP credentials never reach `auth.json` — asserted structurally, by `cyrup_config::auth::Credential` having no field that could hold one | MCP-269 |
| C5 | A callback carrying a **wrong or absent `state`** settles nothing, returns the error page, and leaves the pending flow intact | MCP-306 |
| C6 | **`!command` secrets resolve at connect time only.** A hostile project `.mcp.json` in the cwd, plus `getMcpDiscoverySummary` / a config preview / a hash computation, spawns **zero** subprocesses | MCP-083, MCP-349 |
| C7 | `mergeServerMaps` strips `headers`, `bearerToken`, `bearerTokenEnv` and `oauth` when a later layer rebinds the `url` — with `oauth: false` surviving | MCP-053 |
| C8 | Every secret-bearing field is excluded from the trace, and `redactTraceText` is exercised on a payload containing each | MCP-474 |

### 7.2 The output-guard gate — blocks Phase 5

| # | check | unit |
|---|---|---|
| O1 | Byte and line caps are enforced on **every** result path, including `details.mcpResult`, and a result exceeding both is truncated by both | MCP-226, MCP-227 |
| O2 | Spill artifacts are `0o600` files inside a `0o700` `mkdtemp`, created exclusively (`wx`), and are swept | MCP-228, MCP-224 |
| O3 | The truncation notice is emitted with its exact wording and its ` B` / ` KiB` / ` MiB` format | MCP-227 |
| O4 | The `MCP_OUTPUT_GUARD` kill switch's **tri-state** behaves as upstream — including that the disabled state is not the same as an absent variable | MCP-225 |
| O5 | Image pass-through does not bypass the caps unboundedly | MCP-226 |
| O6 | Binary-resource materialisation honours all four limits: 10 MiB per resource, the session byte cap, the session file cap, and the per-call cap | MCP-223 |
| O7 | The result-summary schema's depth-2 / 20-entry bounds hold against a deeply nested adversarial structured result | MCP-229 |

### 7.3 The permission gate — blocks Phase 5 and Phase 6

| # | check | unit |
|---|---|---|
| P1 | The `mcp` tool's schema carries **exactly** the five property names `create_mcp_permission_targets` reads — `tool`, `server`, `connect`, `describe`, `search` — asserted against the permission crate's own expectations, not against a copy | MCP-192, MCP-247 |
| P2 | A `before_tool_call` handler that traps, panics or exhausts its budget **denies**, exercising `EventKind::ToolCall::fails_closed()` | MCP-232 |
| P3 | A **cancelled** approval dialog and a **headless** session are distinguishable, and **both deny** | MCP-232 |
| P4 | A slow human answer does not fail the gate open — `HostCtx::begin_human_wait` suspends the dispatcher budget across every MCP dialog | MCP-471 |
| P5 | A permission prompt and an MCP approval can never both be on screen — every dialog takes `HostServices::human_interaction_lock` | MCP-471 |
| P6 | An MCP server's tool named `read` (or any built-in name) is **dropped**, not registered, and does not shadow the built-in | MCP-212 |
| P7 | The `auth-start` / `auth-complete` target derivation is recorded as behaving as pi does, with the divergence-or-not decided by ADR-0025 rather than left implicit | MCP-191 |

### 7.4 The scope gate — blocks every phase, and costs nothing

A grep census in CI. It exists because a cut surface that leaks back in is invisible to every other
check, and because structural blind spot 6 is specifically about exclusions nobody re-examines.

```bash
# no cut surface may reappear in the production crate
! rg -q 'axum'                     crates/cyrup-mcp/Cargo.toml
! rg -q 'ui://'                    crates/cyrup-mcp/src/
! rg -qi 'mcpScript|mcp_script'    crates/cyrup-mcp/src/
! rg -qi 'rquickjs|boa_engine|deno_core|v8' crates/cyrup-mcp/Cargo.toml
! rg -q 'SseClientTransport|transport-sse-client' crates/cyrup-mcp/
# and, in the other direction: both cut config shapes must produce a NAMED diagnostic
rg -q 'httpTransport.*sse'         crates/cyrup-mcp/src/   # the rejection must exist
rg -q 'socket'                     crates/cyrup-mcp/src/   # the rejection must exist
```

The last two lines are the important half and are easy to get backwards: **cutting a transport does not
remove the config value from the world.** A user's `mcp.json`, and every Agent Plugin manifest, can
still carry `type: sse` or a `socket` path. Silence there is the failure; a named diagnostic is the
requirement.

---

## 8. Definition of done

Auditable. Every line is a check someone can run or a document someone can open.

1. **All 433 port units are closed** under §1.1's five clauses — behaviour, versioned citation,
   divergence ledger, test at the right layer, no scaffold. The per-phase census of §4.14 accounts for
   every id exactly once.
2. **`rg 'SCAFFOLD\(MCP-' crates/` returns nothing.**
3. **`rg 'TODO|FIXME|todo!\(|unimplemented!\(' crates/cyrup-mcp/src/` returns nothing** that is not an
   upstream-mirrored comment with a `CYRUP-DELTA` block and an owner.
4. **The scope gate (§7.4) is green**, and each of the four cuts is recorded in ADR-0012 with its
   reason and its propagated consequences.
5. **All three security gates (§7.1-7.3) are green.**
6. **The merge gate is green in its two-command form** — `cargo nextest run --workspace && cargo test
   --workspace --doc` — and `--all-features` appears nowhere (guardrail G3).
7. **The integration gate is green** — `cargo nextest run -p cyrup-it --features it` — with the
   environment scrubbed of ambient credentials, and the MCP seam tests living in `bin` and
   `session_svc` rather than in an eighth target (guardrail G2 still reports ≤ 7, plus the one
   justified keyring target if ADR-0021 takes it).
8. **Both conformance suites are green from an empty baseline**, at both `--spec-version` values, and
   the baseline file that exists was written from an observed run.
9. **The differential harness (§5.5) reports zero diffs** on the fixture set of §6, or every remaining
   diff is in the §3.2 sanctioned-change table.
10. **The vitest census is complete**: 84 in-scope vitest files + 5 node:test files each marked ported,
    re-specified, or declined-with-reason; the 12 cut-surface files and the one split file recorded as
    such.
11. **Every surface named in §5.6's live-pty table has been driven in a real terminal**, with a
    transcript or an asciinema cast attached, and the instrument was validated first.
12. **The cross-crate contract holds**: the golden-vector fixture passes from **both**
    `cyrup-mcp` and `cyrup-ext-subagents`, and a cache written by the former resolves `mcp:` selectors
    through the latter.
13. **Every ADR in §9 is `accepted`**, and every `open-decision` unit cites the ADR that settled it.
14. **The three host additions are dispositioned in writing** — built, or accepted with the degradation
    documented in the user-facing guide. "Not yet" is not a disposition.
15. **`is_run_cancelled` is overridden in `LiveHostServices`**, and a test proves a run cancellation is
    observed by an MCP tool call in flight.
16. **The upstream baseline row is current**: `git describe --tags` on `pi-mcp-adapter` re-run, the
    ported baseline moved to v2.25.0 by census rather than by assertion, and rmcp's version re-checked
    against the pinned line.
17. **`docs/guide/` documents the user-visible surface**, including every accepted degradation and each
    of the four cuts, so a user who reads pi's MCP documentation and then cyrup's is not surprised.

---

## 9. Decisions to record — the ADR docket

**One numbered sequence, continuing from the workspace's existing ADRs, which end at ADR-0011.** The
next free number is **ADR-0012**, and this port claims **0012 through 0027**. Numbers are never reused
and never renumbered. Cite one by **path** (`docs/adr/ADR-0012-mcp-port-scope.md`), never by the bare
token. Every ADR ends with a *How to reverse this* section naming the sentence a maintainer would have
to say, what would change in the tree, and the cost of the rejected option — the convention
`docs/adr/README.md` establishes.

**The scope decision is first, deliberately.** It is the one that makes the rest sizable, and it is the
one a later pass is most likely to re-open by accident.

| ADR | question | options | recommendation |
|---|---|---|---|
| **0012** — MCP port scope: the rmcp subset and the four cuts | Does cyrup support exactly the MCP subset `rmcp` supports, cutting the legacy HTTP+SSE transport, MCP Apps, the raw unix socket, and `mcpScript`? | (a) the four cuts as stated; (b) cuts 2-4 only, hand-writing an SSE client transport; (c) no cuts | **(a)** — already decided by the project owner. Recorded here so a later pass reads a decision rather than a gap. Note that (b) means hand-writing a protocol transport, which is the one thing the dependency decision exists to avoid, and that cuts 1 and 3 coincide exactly with rmcp's own two gaps |
| **0013** — `rmcp` as the protocol layer | Which SDK, at which version, with which features, re-checked how often? | (a) `rmcp` 3.1.2 client-only with the five named features; (b) a wider feature set; (c) hand-roll on cyrup's own HTTP/process capabilities | **(a)**, with the feature list asserted by a manifest test and the version re-checked **at every phase boundary** — six rmcp releases landed in sixteen days. (c) is refuted: `ProcCaps`/`HttpCaps` are the WASM-guest grants, not a native crate's spawn/HTTP path |
| **0014** — HA-1: the native handle to late tool and command registration, **and the broken link behind it** | A native extension has no handle to `ExtensionHost::register_late_tool`, and there is no `register_late_command` sibling. **Separately, `ExtensionHost::refresh_tools` reports "nothing changed" for a native late registration in the default `wasm-host` build, so the handle alone reaches nothing.** Build both, or accept the degradation? | (a) defaulted `NativeExtension::set_ext_host(Weak<ExtensionHost>)` called beside the existing `set_host_services`, plus the command sibling, **plus the one-line `refresh_tools` fix**; (b) the same, with the handle delivered as two defaulted `HostServices` methods backed by a late-attach sink, the shape `set_overlay_sink` already uses; (c) accept | **(a) or (b) — build it, both halves.** Everything downstream is complete and live (`refresh_tools` → `AgentSession::{refresh_extension_tools, next_turn_tools, push_active_tools}` reaches the running agent at every turn boundary), and the WASM tier already has the handle through its `registration` WIT import — a two-tier asymmetry the wrong way round. **But "only the handle is missing" is wrong**, and an earlier edition of this ADR said so: `refresh_tools` returns the guest materializer's verdict, and the `wasm-host` arm iterates only the guest descriptor map, so the native tier's raised dirty flag is consumed and reported as `false` with no diagnostic. `MCP-037` is the handle; **`MCP-037a` is the fix**, and shipping the first without the second yields a registration that silently does nothing while the panel shows it as live. Size is **M across two crates**, not S. Under (c): `disableProxyTool` becomes unsupported and the cold-cache degradation is documented — and `MCP-037a` should still be fixed, because it is a latent defect in `cyrup-ext` independent of this port |
| **0015** — HA-2 and HA-3: command completions and overlay geometry | Extension slash commands have no argument completions (no native dispatch arm, no TUI consumer). `open_overlay` carries no geometry options. Build either? | HA-2: (a) build the native arm on `ExtensionHost::command_completions` plus the `cyrup-tui` consumer; (b) accept. HA-3: (a) `OverlayOptions` threaded through `open_overlay` and `box_rect`, `Default` = today's constants; (b) accept | **HA-2 (a)** — the declaration half already exists and does nothing, which is why no native uses it; medium, across two crates, and every extension command with arguments benefits. **HA-3: either.** It is cosmetic — the host draws no border, so the panels self-centre and the only visible difference is the `Clear` width. It **does** have an owning unit — `MCP-368`, rated `low`, which owns the geometry half and explicitly disowns the height-clip half to `MCP-366` and `MCP-377`. **Do not let HA-3 hold up the port** |
| **0016** — the MCP naming grammar: one module, two consumers | `cyrup-mcp` writes tool names; `cyrup-ext-subagents` reads `mcp:` selectors and expects them. The two grammars disagree six ways. Which wins? | (a) `cyrup-mcp` conforms to the in-tree reader; (b) upgrade the reader to `pi-mcp-adapter@2.25.0`'s grammar; (c) a shared naming module both crates depend on, upgrading the consumer in the same change | **(c)**. Doing nothing is not defensible: the failure is a **silently empty subagent allowlist** with no error anywhere. The divergences are hyphen replacement vs preservation, a missing `mcp` prefix mode, no dot replacement, `get_` vs `read_`, exact-match-only exclusion with no globs and no `includeTools`, plus `BTreeMap` ordering vs config insertion order and a missing disabled check |
| **0017** — `<agent_dir>`, `mcp.json`, and `~/.pi/agent` | Is `~/.pi/agent` a supported migration source for cyrup's `mcp.json` / `mcp-cache.json`? | (a) `~/.cyrup/agent` only, plus a one-way migration; (b) dual-read from a resolver **shared** by `cyrup-mcp` and `cyrup-permission-system`; (c) a permanent extra discovery source | **(a)**. Resolution itself is settled (`ConfigDirs::agent_dir`); the hazard is that `cyrup-permission-system` resolves `<agent_dir>/mcp.json` **independently**, so a fallback living only inside `cyrup-mcp` makes the permission gate enumerate a *different* server set than the extension runs — permissions too permissive or too strict, with no error. Fold in the second half: **consolidate the three disagreeing agent-dir resolvers**, two of which diverge precisely under the `CYRUP_AGENT_DIR` / `CYRUP_HOME` configurations CI and subagent isolation use |
| **0018** — project trust and the project-scoped MCP config sources | Do `<cwd>/.mcp.json` and `<cwd>/.cyrup/mcp.json` honour project trust? | (a) mirror upstream — no gate at all; (b) drop both sources when the project is untrusted and report them as present-but-untrusted in `/mcp` status | **(b)**, with the divergence recorded. A project-local config can name an arbitrary stdio `command` and an `!`-prefixed `env` value that runs a shell command at connect; cyrup's `SettingsManager` already skips the whole project layer for an untrusted project, and `HostServices::is_project_trusted` makes gating a one-call change. This is a policy choice, not a missing capability |
| **0019** — negotiation against a discover-intolerant stdio server | rmcp's `serve_client_with_lifecycle` probes `server/discover` on the **same** transport and returns `Legacy` only from a correlated JSON-RPC error; a child that *exits* produces a transport error and there is no fallback. Upstream ships a fixture for exactly that server | (a) adopt `ClientLifecycleMode` as-is and record the loss; (b) adopt it for HTTP and, for **stdio + `auto` only**, spawn a disposable sibling child, run `Discover` on it, then open the real child pinned to the negotiated revision; (c) hand-roll negotiation | **(b)** — the only option preserving upstream's observable behaviour, bounded to one config arm, and it needs nothing from cyrup's core. (a) only with explicit sign-off that discover-intolerant servers may break. (c) is ruled out by ADR-0013 |
| **0020** — the keychain: service name, posture, and the Linux store | What service name, must the keychain be mandatory, and which Linux credential store links? | service: (a) `cyrup.mcp.oauth` with a read-only import; (b) reuse `pi-mcp-adapter.oauth`. posture: (a) keychain-mandatory as upstream; (b) a plaintext fallback. Linux: (a) `keyring` with `v1` (Secret Service, **not** keyutils); (b) `keyring` with `cli` plus explicit `keyring-core`; (c) `keyring-core` plus the three native stores directly, per keyring 4.x's own guidance for an application | **service (a)** — near-forced, because the port's payload (rmcp `StoredCredentials`) is **not wire-compatible** with upstream's `AuthEntry`, so writing under the old name would destroy a co-installed `pi-mcp-adapter`'s credentials. **posture (a)** — no cyrup MCP credential exists today, so adopting it regresses nothing. **Linux (c)**, falling back to (b) if the `keyring = 4.1.6` line must be preserved verbatim. **This decision is load-bearing beyond itself:** under Linux option (a), a revoked kernel session keyring cannot occur, so MCP-260/261/262/287 become dead code and must be **cut in the same breath** — and a headless Linux box with no D-Bus has no store at all, which is exactly the environment upstream's recovery path serves |
| **0021** — where the MCP seam tests live, and the merge gate's shape | `cyrup-it` caps at seven `[[test]]` targets accepting only two justifications, and every target is `required-features = ["it"]` so the default gate does not build them | (a) an eighth target — refused by G2, MCP meets neither justification; (b) fold into `bin` (process-spawning) and `session_svc` (session-driving), plus one justified target for the keyring subprocess tests; (c) put everything in `src/` under `#[cfg(test)]` | **(b)**, and settle the second half in the same ADR: the merge gate is the **two-command** form (`cargo nextest run --workspace && cargo test --workspace --doc`), because a single `cargo test --workspace` silently drops doctests. This is a pre-existing workspace ambiguity the port surfaces rather than creates |
| **0022** — the conformance harness: version, runner shape, baseline | Which harness version, fixed or ephemeral callback port, and what baseline? | version: (a) track rmcp's `0.2.0-alpha.10`; (b) upstream's `0.1.16`. runner: (a) fixed port, sequential, with an ephemeral-port probe; (b) bind `:0` per rule R4, `--suite all`, parallel. baseline: (a) empty, written from an observed run; (b) inherit upstream's | **version (a)** — `--suite` and `--spec-version` come from the newer line. **runner (b)** — binding `:0` drops both the sequential constraint *and* the probe. **baseline (a)** — near-forced, and stated as an instruction rather than a question: the format fails the build on a **stale passing** entry, so an inherited file is a landmine |
| **0023** — fixtures, and test-only `node` | What replaces the eight surviving upstream `.mjs` fixture servers, and may `node` appear in the test environment at all? | (a) keep them as `.mjs`, spawning `node` in tests; (b) rewrite all eight in Rust on `rmcp/server` as a **dev-dependency**; (c) split — hand-roll the three NDJSON handshake servers, build the rest on `rmcp/server`, keep at most one `.mjs` for genuine third-party interop | **(c)**. Requires a ruling on test-only `node` (the conformance CLI itself runs under `npx`, which is a third-party test harness, not a cyrup runtime) and on `rmcp/server` in `[dev-dependencies]`. Note the calculus changed once the checkout was read: rmcp ships a 1 780-line Rust MCP server plus a dozen example servers |
| **0024** — client identity and host branding in model-facing text | Upstream announces `{ name: "pi-mcp-<server>", version: "1.0.0" }` and two model-facing strings say "Pi" | (a) rename to `cyrup-mcp-<server>` / "cyrup"; (b) keep pi's identity to inherit any server allow-list | **(a)**. Misrepresenting the client to a remote server in order to inherit an allow-list is worse than being refused. Record it so a "why does this server reject us" report has an answer. Same ADR settles the OAuth client-metadata branding defaults and the `localeCompare` tie-break (a hand-written ASCII collator with a documented precondition, since it orders only equal-score results and a hint list) |
| **0025** — direct MCP tools and the `mcp` permission category | `cyrup_permission_system`'s `mcp` arm fires only when the tool name is exactly `mcp`, so a **direct** MCP tool lands in the arbitrary-extension-tools arm with `target: None`. And `auth-start`/`auth-complete` derive `mcp_list`, a baseline auto-allow target | (a) port the upstream split as-is and document plainly that `defaultPolicy.mcp = "deny"` does **not** deny direct MCP tool calls; (b) teach `check_permission` to recognise MCP-owned names | **(a)** for the port, with (b) filed as a cyrup **product** follow-up rather than a port unit — it needs the naming module of ADR-0016 first. The `auth-start` derivation is faithful to pi; reproduce it and file it upstream, with hardening behind separate sign-off |
| **0026** — session recovery: `reinit_on_expired_session` | rmcp's transport can transparently recover an expired session; the adapter has its own `withSessionRecovery` | (a) transport flag **off**, port `withSessionRecovery` literally; (b) flag on, drop the wrapper; (c) both | **(a)**. The flag covers upstream's 404-with-session arm and nothing else — not the 400/`-32000` arm, not the `ProtocolError` arm, and not the manager-level reconnect that `onNeedsAuth` and `SessionRecoveryAuthRequiredError` hang off. (c) stacks two independent one-shot retries and lets the transport's silent retry hide the reconnect the adapter wanted |
| **0027** — the differential oracle | Does `pi-mcp-adapter` stay checked out and installable in CI as a reference implementation? | (a) yes — enables the trace-JSONL differential harness; (b) no | **(a)**. It is the cheapest available oracle for **exactly** what conformance cannot cover — stdio, multi-server, lifecycle, reconnect, the whole adapter layer. Under (b) that layer has no oracle beyond unit tests and golden vectors, which is a real reduction in confidence and must be recorded as one |

### 9.1 Recorded as divergences, not ADRs

Small observable differences that need writing down but do not change the shape of any work. They live
in the fidelity ledger at their sites (§3.4), not in `docs/adr/`.

- Tool-schema property order is alphabetised (`serde_json` without `preserve_order`). A pre-rendered
  `&'static str` does **not** help — parsing re-normalises.
- `.pi/mcp-traces/` → `.cyrup/mcp-traces/`, settled by the same line that settles `mcp.json`'s path.
- The `unsafe_pattern` `details.error` code survives with **no producer**: `regex`'s linear-time
  guarantee makes catastrophic backtracking structurally impossible, so there is no rejection to
  report. What is *not* reproduced, exactly: compile-time and memory blowup remain possible and must be
  bounded explicitly with `size_limit` / `dfa_size_limit`, surfaced as `invalid_pattern`; the
  `unsafe_pattern` **diagnostic** disappears, so a model that relied on it to learn its pattern was bad
  now gets results instead; and JS `RegExp` accepts backreferences and lookaround that `regex` rejects,
  which become `invalid_pattern` where upstream would have compiled them. Name the dialect difference
  in `/mcp` help text rather than pretending it is the same dialect.
- rmcp sends no `MCP-Protocol-Version` header on discovery, and appends `offline_access` but not
  `prompt=consent`.
- The callback listener **binds `127.0.0.1` and advertises `localhost`** through the existing
  `CallbackServerConfig::advertising`. Upstream's literal mechanism resolves `localhost` and binds
  every returned address, which needs a multi-listener accept loop; binding *and* advertising
  `127.0.0.1` breaks redirect URIs already registered with providers as `localhost`. The residual —
  an IPv6-only `localhost` — is named rather than solved, and the literal mechanism is what to
  restore if a report arrives.
- Under rmcp a `!command` secret resolves once per token leg rather than up to three times.
- Sampling `metadata` pass-through is dropped: `cyrup-provider`'s `StreamOptions` has no metadata
  field, no upstream test pins it, and adding one widens a crate this port otherwise only reads.
- `PI_MCP_ADAPTER_KEYRING_RECOVERY_NODE` disappears because it names a JavaScript interpreter and the
  re-execed program is now `current_exe()`; the `_KEYCTL` and `_HELPER` switches both survive, the
  latter now naming a program rather than a script. Recorded so a reader who finds five of six switches
  ported does not read the sixth as an oversight.

---

## 10. How this document is kept honest

### 10.1 The six structural blind spots, and how each is defended against *for this port*

The gap-analysis README records six blind spots, each found the hard way, each a property of the
*method* rather than of any one pass. They apply to this port too, and generic acknowledgement is
worthless — here is the specific countermeasure for each, with the artefact that carries it.

**1. An item-driven analysis cannot see behaviour nobody wrote an item for.**
*Defence: this port is surface-driven by construction, and there is a per-phase census to prove it.*
The seam map carries a **file-by-file disposition of the entire upstream package** — every file has a
verdict (port / cut / split / not-applicable), so there is no file nobody looked at. On top of that,
each phase publishes three consumer censuses at its close:
- **the symbol census** — for each upstream file the phase owns, every exported symbol maps to a port
  unit, a cut, or a recorded no-op;
- **the environment-variable census** — 20 upstream names → **17** after Cut 2 removes `MCP_UI_DEBUG`,
  `MCP_UI_VIEWER` and `GLIMPSE_BINARY`; each survivor has a read site;
- **the `details.error` census** — 35 codes → 32 surviving → 31 with a producer, and the 32nd
  (`unsafe_pattern`) explicitly accounted for rather than silently missing.
Treat every count in this document as a **floor**.

**2. A carve-out gets applied far too broadly.** *Defence: a cut invocation must carry the grep that
justifies it.* The analogue of ADR-0001's over-application here is the four scope cuts, and the seam
map already caught **three** false cuts by asking "does this symbol actually belong to the cut
surface?": `ui-tool-visibility.ts` **splits** rather than dying; `tool-result-renderer.ts` contains no
`ui://` code at all and survives whole; `consent-manager.ts` genuinely dies while `tool-approval.ts`
beside it genuinely lives. **Rule: before cutting a symbol, record the grep establishing that its only
callers are cut files.** And the corollary — *not enabling a feature does not make its hazards moot* —
is why §7.4's second half exists: cutting the SSE transport does not remove `type: sse` from anyone's
config file, so both cut config shapes must produce a **named** diagnostic.

**3. The recorded baseline is itself an unverified claim, and a wrong one silently reclassifies work.**
*Defence: §2's three-field table, ADR-0006's census-not-inheritance rule, and a shorter clock for
rmcp.* This is not hypothetical here — **both existing in-tree ports of this package cite upstream with
no version token at all**, which is the exact shape of the defect. Phase 2 stamps them. And the moving
target is doubled: `pi-mcp-adapter` moves on a tag cadence, `rmcp` moved **six times in sixteen days**,
so its version is re-checked at every phase boundary and pinned by a manifest test.

**4. A cross-cutting document can orphan work no area file owns.** *Defence: the §4.14 census, and an
owner for every residual.* There are exactly two cross-cutting artefacts for this port — the seam map
and this document — and **every one of the 433 units appears in exactly one phase**, which is
mechanically checkable. Every host addition and every open decision carries an **ADR number**; there is
no unowned row. The two deliberately-absent ids (MCP-279, MCP-348) are named in §4.14 so a gap in the
numbering reads as a record rather than as evidence of a deletion.

**5. "Has a consumer" is too weak a test for the unwired class.** *Defence: the consumer census is a
test, not a sweep.* The three failure shapes the README names all have instances in this port, and each
has a check: *advertised but unimplemented* → the `action` property's description **must** change with
Cut 2, or the tool advertises a mode that silently falls through (§3.2); *implemented but unadvertised*
→ every one of the 23 `McpSettings` keys is pinned to its **read-site predicate**, not its documented
default; *delivered but never rendered* → §5.6's live-pty table, which exists because a panel that
renders correctly to a `TestBackend` can be invisible on a real screen. HA-2 is the canonical instance
in cyrup's own tree: `InitApi::add_autocomplete` exists, records an opt-in, and **nothing consumes it**
— which is why no native extension has ever noticed.

**6. A surface dismissed as out of scope is invisible to every later pass, and the dismissal is never
re-examined.** *Defence: the exclusions are published, with a reason per entry, and they are a tracked
unit.* §1.2's cut table is exactly the countermeasure the README prescribes — an explicit exclusion
list with a reason per entry, rather than one line in a sweep nobody re-opens. MCP-482 makes the cut
census a **tracked deliverable of Phase 0**, listing per cut the upstream files, the symbols, and the
test files that go with it. Every phase publishes what it excluded, including its negative results:
**"read, nothing found" is worth as much as a unit**, and three surfaces in this port are exactly that
(roots, MCP logging, MCP completions — all shipped by rmcp, all implemented by nobody upstream).

### 10.2 What closes a unit, and what does not

**Closure requires reading the Rust at the current branch and the TypeScript at v2.25.0.** Both sides,
in the same sitting, by the person marking it closed.

**Explicitly not evidence of closure:**
- a commit message asserting the fix — treated as a **hypothesis**;
- a passing test that does not run in either gate (§5.3 — this is a live workspace defect, and a test
  parked behind `required-features = ["it"]` and nowhere else satisfies nothing);
- a `TestBackend` result standing in for a live run (§5.6);
- an in-source ADR or requirement id that cannot be read from this workspace — the workspace's standing
  rule, and it cost `TUI-019` months of a wrong severity;
- "identical at both tags" — never write it; byte-identical bodies do not imply identical offsets, and
  the shift is often non-uniform within one file.

**The inverted duty on any re-verification pass:** the primary instruction is to **refute** every
`closed` claim, on the grounds that a wrongly-closed unit deletes a real defect from the backlog and
nobody looks again. That scepticism keeps paying in this workspace — two separate re-baselines found
follow-on defects *inside the code that closed* an earlier item.

**Write down the falsification condition on any closure that rests on an argument rather than an
observation.** The workspace named this failure mode only after `TOOL-042` was closed against the wrong
signal — its fix cut a leak rate from ~12% to ~1.0% without stopping it — and it was reopenable **only
because its closure had recorded what would falsify it.** Several closures in this port will rest on
arguments (the `TracingTransport` type-safety proof; "rmcp validates nothing client-side here"; "no
consumer exists for an MCP status event"). Each must carry its own falsification condition.

### 10.3 How a refutation is recorded

**IDs are never renumbered, merged or deleted.** A refuted unit keeps its `MCP-NNN`, keeps its place in
§4.14's census, and gains:

1. **The verdict** — `refuted`, with the date and who read what.
2. **What was actually read**, on both sides, by symbol.
3. **What the premise said and why it is false.** Not "no longer applies" — the specific claim, and the
   specific evidence.
4. **The downstream repair.** A refutation that changes a phase's contents, an ADR's recommendation, or
   another unit's premise says so explicitly, because a refutation nobody propagates is a second error.

Two refutation shapes are known in advance and should be labelled as such, because they need different
follow-ups: **doc staleness** (the fix landed and nobody reconciled — cheap, and the answer is to
reconcile every phase rather than every four) and **genuine analysis error** (the premise about
upstream is false — expensive, because anything built on it is also wrong). A third shape was named
only recently: **a closure validated against the wrong signal** (§10.2).

**Expect a residue.** The workspace's measured item-level error rate is **≈12% across six editions**,
and the honest reading is that this is the method's floor rather than a defect to be driven out. This
port's analysis has already refuted dozens of its own predecessor's claims — including a `critical`
that was half wrong. **Treat every unit in the seam map as a lead to verify, not a fact**, and treat a
refutation as a success.

### 10.4 When to re-run the analysis

| trigger | what to re-run |
|---|---|
| **`pi-mcp-adapter` cuts a new tag** | ADR-0006's six-step procedure, scoped by `git diff --name-only v2.25.0..<new>` intersected with the paths this document and the seam map cite. File only what the diff shows; move the **comparison** tag; move the **ported** baseline only as units close |
| **`rmcp` cuts a MINOR** | re-read the changed modules in the checkout; re-run the manifest-policy test; check nothing moved from unconditional to feature-gated. Normally additive under rmcp's `VERSIONING.md` |
| **`rmcp` cuts a MAJOR** | **stop and take the bump as its own unit.** Do not chase it inside a phase. Read the API diff from the checkout, not the changelog |
| **every phase boundary** | re-check rmcp's latest version; publish the phase's three censuses (§10.1 defence 1); publish what the phase excluded and its negative results |
| **any host addition is built** | re-run the two-part test of §1.3 against the *shipped* surface, not the proposal. A surface that grew during implementation is a different decision |
| **≥2 units in one phase come back refuted** | **re-plan before continuing.** At that rate the phase's premise is suspect, not its items, and the cheap move is to re-read the upstream files the phase owns before writing more code |
| **the assembled binary shows a layout or empty-state defect no unit describes** | unit-by-unit parity is the wrong frame for that surface; pivot to end-to-end bring-up for it. This is a real branch, and it has fired before in this workspace |
| **a cut surface is proposed for re-entry** | it is an ADR-0012 reversal, and ADR-0012's *How to reverse this* section is the work order. It is not a gap, and it is not a new item |

### 10.5 The standing instruction

**Treat the brief as a lead.** Two of three agents in the workspace's most recent sweep found a
load-bearing error in their own **assignment text** rather than in the code — one a prescribed
mechanism that would have shipped a budget nothing enforced, one a **fabricated upstream citation in
the orchestrator's own brief**, caught by an agent opening the file. Fabricated citations are not
confined to the analysis documents.

Applied here: **this document and the seam map are both wrong somewhere.** When the code contradicts
them, the code wins, and the contradiction gets written down by the person who found it, under §10.3.
