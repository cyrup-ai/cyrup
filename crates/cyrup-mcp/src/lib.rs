//! cyrup-mcp — the `pi-mcp-adapter` port, as one native `cyrup_ext::NativeExtension`.
//!
//! Upstream is `pi-mcp-adapter` **v2.26.1** (retargeted 2026-08-20 from v2.25.0, which is the tag
//! the plan was authored against); the plan is `docs/gap-analysis/13-cyrup-mcp.md` plus its section
//! files `13a`..`13i`. Every module below cites the upstream file it ports and the port unit
//! (`MCP-nnn`) that owns it.
//!
//! **Reading a citation.** A bare `file:line` in this crate resolves against **v2.25.0**, because
//! that is the tree the plan was read from; a citation written `@v2.26.1` resolves against the newer
//! tag. The version strings were deliberately *not* search-and-replaced: the plan carries 51 v2.25.0
//! citations whose line numbers are correct only at that tag, and rewriting the string without
//! re-resolving every line would silently invalidate all of them. The full retarget record — what
//! was already ported, what was inapplicable, what is newly ported, and the three units filed as
//! still-open work (MCP-027a, MCP-069a, MCP-115a) — is the *Retarget* section at the top of
//! `docs/gap-analysis/13-cyrup-mcp.md`.
//!
//! # The thesis: this is an extension, and the port changes nothing in core
//!
//! Upstream, `pi-mcp-adapter` is an *installed npm package* that pi's extension loader invokes —
//! not part of pi. Here it is a native built-in crate compiled into the binary and attached at the
//! three session-build arms of `crates/cyrup/src/main.rs` through
//! `SessionFactory::with_native_extension`, exactly the shape `cyrup-ext-subagents`,
//! `cyrup-permission-system` and `cyrup-intercom` already take. A native extension is **not**
//! sandboxed: [`cyrup_ext::host::HostServices`] is the capability surface a *WASM guest* is confined
//! to, while this crate links `rmcp`, `tokio`, `keyring` and `reqwest` directly and reaches for
//! `HostServices` only where it genuinely touches the host — drawing UI, notifying, reading session
//! state, honouring cancellation, registering tools and commands.
//!
//! # The design's central trick: cache-first registration
//!
//! `installMcpAdapter` (`index.ts`) runs to completion with **no `await` anywhere** and **no MCP
//! server contacted**. It reads `mcp.json` off disk, reads `<agent_dir>/mcp-cache.json` off disk,
//! and from those two files alone registers the full model-visible surface: one direct tool per
//! cached MCP tool and resource, one slash command per cached MCP prompt, the `mcp` gateway tool,
//! `/mcp`, `/mcp-auth`, and the `--mcp-config` flag. Only *then*, deferred by one macrotask tick,
//! does it consider spawning anything, and only if some server declares
//! `lifecycle: "eager" | "keep-alive"`. Everything else connects lazily on first call.
//!
//! A porter who implements "connect servers, then register tools" reproduces none of this: startup
//! would block on N subprocess handshakes, the system prompt would change shape between runs, and
//! the provider's prompt-cache prefix would be invalidated on every reconnect — which is exactly
//! what `settings.freezeDirectTools` exists to prevent. [`registration`] is the module that keeps
//! the trick, and it is why [`extension::McpExtension`]'s `init` performs its disk reads with
//! `std::fs` (upstream's `readFileSync`) rather than `tokio::fs`: nothing may block the session
//! build on the reactor.
//!
//! # The one structural difference, and it is an ordering inversion
//!
//! In pi the extension factory runs **once per process** and `session_start` fires repeatedly on
//! the same closure, which is why `lifecycleGeneration`, `currentOwner` and `currentOAuthRuntime`
//! are module-scoped mutable slots. In cyrup a session replacement **builds the replacement first**
//! and only then tears the old one down: `AgentSessionRuntime::new_session_with` calls
//! `SessionFactory::build_with_parent` — which re-runs `ExtensionHost::load_native_with_services` →
//! `NativeExtension::init` on the **same `Arc<dyn NativeExtension>`** — and *then* fires
//! `SessionShutdown` on the outgoing session before `SessionStart` on the new one. The real cyrup
//! order on a replacement is therefore:
//!
//! ```text
//! init()          for generation N+1   <- fresh ExtensionHost, fresh InitApi, fresh registry
//! SessionShutdown for generation N     <- the old runtime's teardown + metadata flush
//! SessionStart    for generation N+1
//! ```
//!
//! pi's "one factory, many `session_start`s" becomes cyrup's "one object, many `init`s, one
//! `SessionStart` each, and the new `init` runs *before* the old shutdown". That inversion
//! (MCP-014) is the single most likely source of a subtle port defect in this subsystem, and it is
//! why [`extension::McpExtension`] carries a generation counter rather than trusting call order.
//!
//! # Scope: four surfaces are cut, deliberately
//!
//! 1. **The legacy HTTP+SSE transport.** rmcp 3.1.2 ships no SSE *client* transport at all, so
//!    `httpTransport: "sse"` survives as a field with one legal value and an `sse` value is
//!    rejected at config load with a named diagnostic rather than silently dropped.
//! 2. **MCP Apps / the UI extension, entirely.** This removes `state.uiServer`,
//!    `state.completedUiSessions`, `state.uiResourceHandler` and `state.consentManager` — four of
//!    the five fields that take `McpExtensionState` from 25 down to [`state::McpState`]'s twenty.
//! 3. **The raw unix-socket transport.** `ServerEntry` has no `socket` field.
//! 4. **`mcpScript` / the JavaScript worker.** RUST ONLY: there is no JS runtime to host it.
//!
//! The fifth cut field is `approvalEvents`, the pi-bus approval broker — subsumed by
//! `ExtHooks::before_tool_call` plus `cyrup-permission-system`'s existing, already fail-closed MCP
//! target derivation.
//!
//! # Module map
//!
//! The eleven modules below are the port of `pi-mcp-adapter`'s eleven activation-path files
//! (gap-analysis 13a), one Rust module per upstream file, plus [`errors`] for the taxonomy every
//! one of them returns through:
//!
//! | module | upstream | owns |
//! |---|---|---|
//! | [`extension`] | `index.ts` | the [`cyrup_ext::native::NativeExtension`] impl and the construction gate |
//! | [`registration`] | `index.ts` (the sync body) | everything `init()` registers from disk caches |
//! | [`runtime`] | `init.ts` | `initializeMcp` — the staged runtime build |
//! | [`owner`] | `runtime-owner.ts` | [`owner::McpRuntimeOwner`] and the fenced services handle |
//! | [`abort`] | `abort.ts` | `throwIfAborted` / `abortable` / signal combination |
//! | [`state`] | `state.ts` | [`state::McpState`] — the twenty-field runtime record |
//! | [`lifecycle`] | `lifecycle.ts` | the reconnect / idle-shutdown health-check state machine |
//! | [`config`] | `config.ts`, `types.ts` | `mcp.json`: the six-source ladder, `ServerEntry`, `McpSettings` |
//! | [`agent_plugin`] | `agent-plugin-loader.ts` | the sandboxed third-party plugin translator |
//! | [`dirs`] | `agent-dir.ts` | `<agent_dir>` and every adapter-owned path under it |
//! | [`onboarding`] | `onboarding-state.ts` | `<agent_dir>/mcp-onboarding.json` |
//! | [`secrets`] | `utils.ts` | `${VAR}` interpolation and the `!`/`!!` command-secret grammar |
//! | [`request_headers_command`] | `request-headers-command.ts` | per-request HTTP header commands (v2.26.0) |
//!
//! Cut 2 adds the five modules that make that surface *do* something — the gateway tool, the
//! credential store, the OAuth flow, the result renderers and the panels:
//!
//! | module | upstream | owns |
//! |---|---|---|
//! | [`proxy`] | `proxy-modes.ts`, `mcp-tool.ts` | the `mcp` gateway tool and its nine modes (13d) |
//! | [`credentials`] | `mcp-auth.ts` | the keychain-backed credential store (13f) |
//! | [`oauth`] | `oauth.ts`, `mcp-auth-flow.ts`, `mcp-callback-server.ts` | the OAuth 2.1 flow runtime (13g) |
//! | [`renderers`] | `tool-result-renderer.ts`, `mcp-output-guard.ts`, `tool-registrar.ts` | result rendering, the output guard, resource materialization (13e/13h) |
//! | [`ui`] | `mcp-panel.ts`, `mcp-setup-panel.ts` | `/mcp`, `/mcp-auth` and the setup panel (13h) |
//!
//! No-panic policy (arch-00 §8) is enforced crate-wide via `[workspace.lints]`; this crate-level
//! `#![deny(...)]` mirrors `cyrup-ext`'s own explicit restatement of that convention, and matters
//! more here than in most crates: [`extension::McpExtension`]'s `init` **must never return `Err`** and
//! must never panic, because a native extension's failing `init` is a fatal startup diagnostic that
//! every mode arm turns into `dispose(); exit 1`. Upstream's `installMcpAdapter` cannot fail —
//! every disk read it performs is defensive — so a stray `{{{` in a user's `mcp.json` has to
//! degrade to an empty surface, never to a crash on a normal path (MCP-003).
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![forbid(unsafe_code)]

pub mod abort;
pub mod agent_plugin;
pub mod commands;
pub mod config;
pub mod credentials;
pub mod dirs;
pub mod dispatch;
pub mod elicitation;
pub mod errors;
pub mod extension;
pub mod lifecycle;
pub mod live;
pub mod oauth;
pub mod onboarding;
pub mod owner;
/// The `/mcp` panels' production callbacks (MCP-387, MCP-392) — crate-internal: the traits they
/// implement are `crate::ui`'s, and nothing outside this crate constructs one.
pub(crate) mod panel_host;
pub mod prompts;
pub mod proxy;
pub mod registration;
pub mod renderers;
pub mod request_headers_command;
pub mod runtime;
pub mod sampling;
pub mod schema;
pub mod secrets;
pub mod server_manager;
pub mod state;
pub mod trace;
pub mod ui;

pub use errors::{CleanupErrors, McpError, McpResult};
pub use extension::{EXTENSION_ID, McpExtension, mcp_extension_for_env};
pub use state::McpState;

// -------------------------------------------------------------------------------------------------
// Cut 2 surface. Each `pub use` below is the entry point one of the five cut-2 modules asked for at
// integration; the module itself stays `pub` so the long tail (33-method `ProxyEnv`, the panel
// models, the renderer primitives) is reachable by path without a hundred-line re-export block.
// -------------------------------------------------------------------------------------------------

/// The credential store (13f) — `mcp-auth.ts`'s keychain-backed `AuthEntry` record.
pub use credentials::{AuthEntry, AuthStoreError, McpAuthStore, OAuthCredentialStatus};
/// The OAuth flow runtime (13g) — `oauth.ts` / `mcp-auth-flow.ts`.
pub use oauth::{
    AuthStatus, AuthenticateOptions, HttpAuthProviderState, McpOAuthRuntime, McpOAuthStorage,
    authenticate, complete_auth, complete_auth_from_input, create_oauth_runtime, get_auth_status,
    get_valid_token, has_pending_auth, remove_auth, shutdown_oauth, start_auth, supports_oauth,
};
/// The `mcp` gateway tool and its nine modes (13d) — `proxy-modes.ts` / `mcp-tool.ts`.
pub use proxy::{MCP_TOOL_NAME, McpErrorCode, McpTool, ProxyCtx, ProxyEnv};
/// The inverse of the tool-name grammar (MCP-073) — `types.ts` `resolveServerFromToolName`. The
/// forward direction lives on [`registration`] with the rest of the grammar.
///
/// **Its consumer is not wired, deliberately** — see the declaration (`registration.rs:265-271`).
/// `cyrup-permission-system` is a *dependency* of this crate, not a dependent, so it cannot consume
/// this; it derives MCP targets with its own **suffix** test — `add_derived_mcp_server_targets`,
/// `cyrup-permission-system/src/manager.rs:985` — which is a different rule from this prefix one, and
/// reconciling the two is MCP-191. Re-exported so the pair reads as one grammar at the
/// crate root when that lands.
pub use registration::resolve_server_from_tool_name;
/// Tool-result rendering and the MCP output guard (13e/13h) — `tool-result-renderer.ts`,
/// `mcp-output-guard.ts`, `tool-registrar.ts`.
pub use renderers::{
    GuardedMcpOutput, MaterializedResources, McpContentBlock, McpOutputGuardOptions,
    McpToolRenderOptions, guard_mcp_output, render_call, render_result,
};
/// Environment interpolation and the `!`/`!!` command-secret grammar (13b, MCP-082/MCP-083) —
/// `utils.ts`. `resolve_command_secret` is the ONLY function in the crate that spawns a shell for a
/// configured value, and it is reachable only from a connect/auth path; every other caller wants
/// `interpolate_env_record`, which unescapes `!!` and leaves `!` unexecuted.
pub use secrets::{
    ResolvedHttpSecrets, interpolate_env_record, resolve_command_secret,
    resolve_command_secrets_record, resolve_env, resolve_http_secrets, resolve_stdio_env,
};
/// The `/mcp` panel and the setup panel (13h) — `mcp-panel.ts` / `mcp-setup-panel.ts`.
pub use ui::{
    DirectToolsChange, McpPanelModel, McpPanelResult, McpSetupPanelModel, open_mcp_panel,
    open_mcp_setup_panel,
};
