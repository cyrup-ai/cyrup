//! `initializeMcp` — the runtime build (`init.ts`; 13a §8–§13, MCP-015…MCP-026).
//!
//! This is the function that turns a configuration into a live runtime: it builds the server
//! manager, wires the sampling and elicitation gates, registers the owner's cleanups in their exact
//! LIFO order, bootstraps the metadata cache, registers each server's lifecycle, rehydrates
//! metadata from a hash-valid cache entry, runs the bounded startup connect pass, and returns the
//! [`McpState`] the extension commits.
//!
//! # MCP-015 — snapshot before the first await, and why
//!
//! Upstream's own comment: *"Pi guards ExtensionContext getters after reload. Snapshot all values
//! that can be used by asynchronous work before the first await."* Seven reads
//! (`configPath`, `cwd`, `hasUI`, `mode`, `rawUi`, `modelRegistry`, `initialSignal`) plus two
//! derivations (`ui = createOwnedUi(rawUi, owner)` and
//! `runtimeSignal = combineAbortSignals(owner.signal, initialSignal)`). [`ContextSnapshot`] is that
//! snapshot, taken by the caller *before* this function is ever polled — which is stricter than
//! taking it here, and is why it is a parameter rather than something this function reads.
//!
//! # The two live closures that are deliberately not snapshotted
//!
//! The sampling gate's `getCurrentModel` and `getSignal` are **live closures over `ctx`**,
//! owner-guarded on each call: `owner.isActive() ? ctx.model : undefined` and
//! `owner.isActive() ? combineAbortSignals(owner.signal, ctx.signal) : owner.signal`. They must stay
//! live — a sampling request arriving twenty minutes into a session has to see the model the user
//! selected five minutes ago, not the one at init.
//!
//! # This file has two halves, and they meet only at the seam
//!
//! Everything above `The wire` is the **runtime build**: one function, called once per generation,
//! that turns configuration into an [`McpState`]. Everything below it is the **connection**: how a
//! [`crate::config::ServerEntry`] becomes an `rmcp` transport, how that transport becomes an
//! initialised client, and what the client does when a server talks back unprompted
//! ([`McpClientHandler`]). The halves share no state — the second is a set of pure builders and one
//! handler type that `server_manager` composes — which is deliberate: the connection half must be
//! testable without an `McpState`, an owner, or a tokio reactor, and most of it is.
//!
//! # Cleanup LIFO order after Cut 2
//!
//! `lifecycle.graceful_shutdown()` → `shutdown_oauth` → (registered later, by `startInitialization`)
//! `cleanup_materialized_binary_resources`. Registered in that order, run in reverse. The
//! `uiServer.close(reason)` cleanup is **Cut 2** and has no replacement.

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::CancelToken;
use cyrup_ext::HostServices;

use crate::config::{McpConfig, ServerLifecycle};
use crate::dirs::McpDirs;
use crate::errors::McpResult;
use crate::lifecycle::McpLifecycleManager;
use crate::owner::{McpRuntimeOwner, OwnedServices};
use crate::state::{
    AuthStorageOptions, McpServerManager, McpState, McpStateParts, OAuthRuntime, OpenBrowser,
    SendMessage,
};

/// Everything the extension context can supply, captured **before the first await** (MCP-015).
///
/// `rawUi` is not here: it is consumed immediately into the fenced [`OwnedServices`] handle, which
/// is what actually crosses the await.
#[derive(Clone)]
pub struct ContextSnapshot {
    /// `--mcp-config`'s value, if the user passed one. Read from argv, not the flag store — see
    /// [`crate::config::config_path_from_argv`].
    pub config_path: Option<PathBuf>,
    /// The session working directory. `resolveConfigPath`'s base.
    pub cwd: PathBuf,
    /// `ctx.hasUI`. Gates elicitation entirely and sampling unless auto-approve is on.
    pub has_ui: bool,
    /// `ctx.mode`. `allowUrl` for URL elicitation is `mode === "tui"` specifically, not `hasUI`.
    pub mode: String,
    /// `ctx.signal` — the caller's own cancellation, combined with the owner's into
    /// `runtimeSignal`. `None` where the host supplies none.
    pub initial_signal: Option<CancelToken>,
    /// The live capability backend, stashed by `set_host_services` before `init`. Fenced into
    /// [`OwnedServices`] before it is handed to anything asynchronous.
    pub services: Option<Arc<dyn HostServices>>,
}

impl ContextSnapshot {
    /// `isTuiMode(ctx)` — exported from `init.ts` and exactly `ctx.hasUI && ctx.mode === "tui"`.
    /// Not the same test as `has_ui`: URL elicitation requires a real TUI, not merely a UI.
    ///
    /// **No caller, by design.** The one place upstream asks this — the URL-elicitation guard — is
    /// answered here by the host instead, as a capability probe: `crate::ui` takes a `false` from
    /// `HostServices::open_overlay` as "no TUI" rather than asking the snapshot. The predicate is
    /// kept because it is `isTuiMode`'s exact test and the two must not drift.
    #[must_use]
    pub fn is_tui_mode(&self) -> bool {
        self.has_ui && self.mode == "tui"
    }
}

/// `createMcpAdapter(options)`'s options, as far as [`initialize_mcp`] consumes them.
#[derive(Clone, Default)]
pub struct InitializeOptions {
    /// `options.config` — a programmatic configuration that **replaces** discovery.
    /// Cloned at factory time and again per call upstream, so a caller cannot mutate a live
    /// runtime through the object it passed in.
    pub programmatic_config: Option<McpConfig>,
    /// `options.oauthRuntime`. When `None`, this generation **owns** the runtime it creates and
    /// registers `shutdownOAuth` as an owner cleanup; when `Some`, it does not.
    pub oauth_runtime: Option<Arc<OAuthRuntime>>,
    /// The credential vault this generation authenticates through. `None` — production — builds
    /// [`crate::credentials::McpAuthStore::new`] from `dirs` and the resolved
    /// `authStorageOptions`.
    ///
    /// It exists because the backend selector is an **environment** switch
    /// ([`crate::credentials::TEST_AUTH_STORE_ENV`]) and edition 2024 made `std::env::set_var`
    /// `unsafe`, with std's own conclusion that a multithreaded program must not call it at all —
    /// the same reason [`crate::McpExtension::with_home`] exists rather than a `CYRUP_HOME` write.
    /// Without this seam nothing outside `cyrup-mcp`'s own `#[cfg(test)]` modules can reach a
    /// session whose vault is not the host keychain, so on any machine with no OS credential store
    /// (a container, CI, a headless Linux box) every `auth: "oauth"` HTTP server in a real session
    /// fails its connect with `NoDefaultStore` and the failure is unprovable.
    pub auth_store: Option<crate::credentials::McpAuthStore>,
}

/// `initializeMcp(pi, ctx, owner, options)` — build the live runtime for one generation.
///
/// **Minimal body (MCP-016…MCP-026 fill it).** What is landed now is the shape every later unit
/// hangs off and must not re-derive: the snapshot parameter, the owner-fenced UI handle, the
/// combined runtime signal, the manager/lifecycle construction order, the cleanup registration
/// order, and the [`McpState`] commit. The staged connect pass, the two-pass metadata build, the
/// cache bootstrap and the sampling/elicitation gates are the work.
///
/// Wiring order, reproduced from 13a §8 so a filler does not have to re-read it:
///
/// 1. `config` — the programmatic clone, else [`crate::config::load_mcp_config`]
/// 2. `authStorageOptions = getAuthStorageOptions(settings.oauthDir, cwd)`
/// 3. `ownsOAuthRuntime = options.oauthRuntime === undefined`
/// 4. `manager = new McpServerManager(cwd)` + its five setters
/// 5. the **sampling gate**: `settings.sampling !== false && (has_ui || samplingAutoApprove)`
/// 6. the **elicitation gate**: `settings.elicitation !== false && has_ui`, `allowUrl = is_tui_mode`
/// 7. `lifecycle = new McpLifecycleManager(manager, hasPendingAuth)`
/// 8. allocate the live maps and sets
/// 9. build the state
/// 10. if `ownsOAuthRuntime`: `owner.addCleanup(shutdownOAuth)`
/// 11. `manager.setMetadataListChangedListener(...)`
/// 12. `owner.addCleanup(lifecycle.gracefulShutdown)`
pub async fn initialize_mcp(
    owner: Arc<McpRuntimeOwner>,
    dirs: McpDirs,
    snapshot: ContextSnapshot,
    options: InitializeOptions,
) -> McpResult<Arc<McpState>> {
    // Step 1 — the config this generation runs. `loadMcpConfig` cannot fail (MCP-003).
    let config = options.programmatic_config.clone().unwrap_or_else(|| {
        crate::config::load_mcp_config(&dirs, snapshot.config_path.as_deref())
    });

    // MCP-015's two derivations. The fenced handle is built BEFORE anything asynchronous can hold
    // it, which is the whole point: what crosses the await is already inert-on-stop.
    // `const rawUi = hasUI ? ctx.ui : undefined` (`init.ts:104-106`). The `has_ui` gate is
    // load-bearing and not cosmetic: `McpState::dialog()` is `None` exactly when this is `None`, and
    // that is the ONLY thing that distinguishes "this session has no interactive UI" from "the user
    // was asked and declined". Deriving `ui` straight from `snapshot.services` — which the host
    // supplies whether or not a human can see it — made `NoInteractiveSession` unreachable in
    // production and collapsed MCP-232's two answers into one (the safe one, but the wrong one).
    let ui = snapshot
        .has_ui
        .then(|| snapshot.services.clone())
        .flatten()
        .map(|services| Arc::new(OwnedServices::new(services, Arc::clone(&owner))));
    let runtime_signal =
        crate::abort::combine(&owner.token(), snapshot.initial_signal.as_ref());

    // Steps 2-4. `getAuthStorageOptions(settings.oauthDir, cwd)` — and **only** `settings.oauthDir`:
    // `$MCP_OAUTH_DIR` and the `<agent_dir>/mcp-oauth` default are the store's own precedence ladder
    // ([`crate::credentials::McpAuthStore::auth_base_dir`]), so pre-resolving a base dir here would
    // pin the lowest rung and make the environment override unreachable (MCP-265).
    let auth_storage_options = AuthStorageOptions::from_settings(
        config
            .settings
            .as_ref()
            .and_then(|settings| settings.oauth_dir.as_deref()),
        &snapshot.cwd,
    );
    let owns_oauth_runtime = options.oauth_runtime.is_none();
    // `createOAuthRuntime(signal)` under the generation's own token, so a session replacement
    // aborts every in-flight login it started and disturbs no other's (MCP-301, MCP-344).
    let oauth_runtime = options
        .oauth_runtime
        .clone()
        .unwrap_or_else(|| crate::oauth::create_oauth_runtime(Some(&owner.token())));
    // `new McpServerManager(cwd)` — with `createConnection` supplied. [`ConnectionBuilder`] is that
    // seam's body (MCP-101/109/113/114/115/115a), and this line is the one place in the crate that
    // installs it; everywhere else the manager still carries `UnbuiltConnectionFactory` and every
    // connect fails loudly by design. `McpServerManager::new`/`default` keep the unbuilt factory
    // deliberately: they are `server_manager.rs`'s constructors and every test in that module
    // scripts its own factory, so flipping their default would be a change to a file this unit does
    // not own for no behaviour anyone depends on.
    //
    // **REACH.** This is now on the shipping path. `McpExtension::start_initialization` — the port
    // of `index.ts:292`'s `startInitialization` — builds this future and spawns its driver, and
    // `McpExtension::on_session_start` calls that on every `SessionStart`. So a server listed in
    // `mcp.json` connects through this line in production, not only under test.
    //
    // It is SPAWNED, never awaited by the handler, and that is not incidental: `cyrup_ext`'s
    // native dispatch budget drops a handler future that outlives it, and the subprocess
    // handshakes below routinely do. See `McpExtension::start_initialization`'s doc comment.
    //
    // `with_handler_factory` it now gets too, and that is what puts the manager's own hook bag on
    // every connection: `manager_handler_factory` upgrades the `Weak` per call and reads the
    // sampling and elicitation slots live. Of the three hooks it can carry, sampling is installed
    // (step 5 below); elicitation is step 6, waiting on `elicitation.rs` (MCP-121/122), and
    // list-changed is MCP-120.
    //
    // `with_auth_provider` it DOES get, and that is this line's whole point. The generation's
    // credential vault is built here and handed to [`StoredCredentialAuth`], which the one
    // production `ConnectionBuilder` carries into every HTTP connect. Before it,
    // `ConnectionBuilder::new`'s default [`NoStoredCredentials`] stood, and an HTTP server whose
    // credential was ALREADY in the store still ended at `needs-auth`: the outcome upstream reaches
    // on a first login, and the wrong one for a returning user, who must connect on attempt one
    // with no prompt.
    //
    // **One instance, published.** The handle is cloned into
    // [`crate::server_manager::McpServerManager::set_auth_store`] below, so
    // [`crate::live::RuntimeEnv::auth_options`] authenticates through the same vault the ladder
    // reads rather than a fresh one per operation. That is what makes the entry cache coherent
    // across a login (see the setter's own doc), and it is why `options.auth_store` is enough to
    // make a whole session hermetic.
    //
    // Constructing the store is pure — `McpAuthStore::new` selects a backend and allocates a cache,
    // it does not touch the keychain — so a session with no HTTP server pays nothing for this, and
    // a stdio-only configuration never reaches a read.
    let auth_store = options.auth_store.clone().unwrap_or_else(|| {
        crate::credentials::McpAuthStore::new(dirs.clone(), auth_storage_options.clone())
    });
    // `definition.oauth?.skipIssuerMetadataValidation === true`, resolved once per generation
    // because the provider is handed a server *name* and has no configuration of its own. Only the
    // `true` entries are recorded; absent reads as `false`, which is also the answer for a name
    // that is not in the config at all.
    let skip_issuer: std::collections::HashSet<String> = config
        .mcp_servers
        .iter()
        .filter(|(_, entry)| skip_issuer_metadata_validation(entry))
        .map(|(name, _)| name.clone())
        .collect();
    let auth_provider: Arc<dyn HttpAuthProvider> = Arc::new(StoredCredentialAuth::new(
        auth_store.clone(),
        Arc::clone(&oauth_runtime),
        runtime_signal.clone(),
        skip_issuer,
    ));
    // `ctx.modelRegistry`. `default_models` spans EVERY built-in provider, which is what
    // `getAvailable()` spans; one installed provider's catalogue would be narrower and is the bug
    // 13i names at :930. Built once and shared by `Arc` — the sampling hook re-reads it per request.
    let models = Arc::new(cyrup_provider::default_models(
        cyrup_provider::CreateModelsOptions::default(),
    ));

    // The late-bound back-reference the sampling hook reads the generation's dialog through.
    // Upstream's hooks close over `ui` directly (`init.ts:126-141`) because they are created before
    // `state`; here the dialog is `McpState::dialog()` — the ONE production constructor (MCP-471) —
    // because it also carries `human_wait_ctx`, which only the state has. `Weak`, never `Arc`: the
    // state owns the manager, the manager owns the hooks.
    let session: Arc<SessionSlot> = Arc::new(SessionSlot::default());

    // `Arc::new_cyclic` because the factory needs the manager and the manager needs the factory.
    // An `Arc` in the closure would be a cycle that never drops and leaks every generation's
    // connection table; `manager_handler_factory` takes the `Weak` and upgrades per call, which is
    // also what makes `closeAll`'s null-out observable to a connect racing a shutdown.
    let manager = Arc::new_cyclic(|weak: &std::sync::Weak<McpServerManager>| {
        let builder = ConnectionBuilder::new(Some(snapshot.cwd.clone()))
            .with_auth_provider(auth_provider)
            .with_handler_factory(crate::server_manager::manager_handler_factory(weak.clone()));
        McpServerManager::with_factory(Some(snapshot.cwd.clone()), Arc::new(builder))
    });
    // Step 4's setters, now that `McpServerManager` is real (MCP-100). Four of the eight are
    // resolvable here; the other four are not this step's:
    //
    // * `setMetadataListChangedListener` is **step 11** — installed after the state commits, so a
    //   hook fired mid-build cannot see a half-installed surface (MCP-011/MCP-030);
    // * `setSamplingConfig` is step 5's gate and is wired with its handler immediately below;
    //   `setElicitationConfig` is step 6's and still has no writer (MCP-121/MCP-122);
    // * `setTraceConfig` has no counterpart at all — `mcp-trace.ts` is MCP-133, unported.
    //
    // `runtimeSignal` is combined **once per generation**, which is what makes
    // `crate::abort::combine`'s one-forwarder-task-per-pair cost affordable (13a §8).
    // Cloned rather than moved: §11's `parallelLimit` connect pass hands the same combined token
    // to every `manager.connect`, and this setter consumes one.
    manager.set_runtime_signal(Some(runtime_signal.clone()));
    manager.set_default_request_timeout_ms(
        config
            .settings
            .as_ref()
            .and_then(|settings| settings.request_timeout_ms),
    );
    manager.set_auth_storage_options(auth_storage_options.clone());
    manager.set_oauth_runtime(Arc::clone(&oauth_runtime));
    // The ninth setter, and not upstream's: it publishes the vault built two statements above so
    // `RuntimeEnv::auth_options` authenticates through the SAME instance the connect ladder reads.
    // Upstream needs no equivalent because its `authEntryCache` is module-global; this port made the
    // cache an instance field, which is what creates the obligation.
    manager.set_auth_store(auth_store);

    let settings = config.settings_or_default();

    // Step 4b — `setTraceConfig(settings.trace)` (`init.ts:122`). The writer is minted here rather
    // than in the manager because deriving its path needs `dirs`, which the manager does not have;
    // one writer is built per generation, so the byte and event budgets are session-global.
    //
    // Built unconditionally when a `trace` block exists, even if `enabled` is false: a per-server
    // `trace: true` turns tracing on for that server against a global default of off, which is the
    // `??` in `is_mcp_trace_enabled`. Construction touches no file system — the directory and the
    // truncate happen on the first line actually written.
    let trace_settings = settings.trace.clone();
    let trace_writer = trace_settings.as_ref().map(|trace| {
        Arc::new(crate::trace::TraceWriter::new(
            crate::trace::trace_file_path(&dirs, trace, &crate::trace::random_suffix()),
            settings.trace_max_bytes(),
            settings.trace_max_events(),
            Arc::new(crate::trace::RealTraceFs),
        ))
    });
    manager.set_trace_config(trace_settings, trace_writer);

    // Step 5 — `init.ts:124-134`. `sampling !== false && (hasUI || samplingAutoApprove)`.
    //
    // This is `set_sampling_config`'s FIRST production caller. Until it, `bare_handler_factory`'s
    // `sampling: None` stood at every connection, so `build_client_capabilities` advertised no
    // sampling capability to any server, no conforming server ever sent `sampling/createMessage`,
    // and `McpClientHandler::create_message` answered `METHOD_NOT_FOUND` unconditionally.
    if settings.sampling(snapshot.has_ui) {
        let options = Arc::new(crate::sampling::SamplingOptions {
            auto_approve: settings.sampling_auto_approve(),
            has_ui: snapshot.has_ui,
            session: Arc::clone(&session),
            models: Arc::clone(&models),
            owner: Arc::clone(&owner),
        });
        manager.set_sampling_config(Some(Arc::new(move |server: String, params| {
            let options = Arc::clone(&options);
            Box::pin(async move {
                crate::sampling::handle_sampling_request(&options, &server, params).await
            })
        })));
    }

    // Step 6 — `init.ts:135-141`. `elicitation !== false && hasUI`, `allowUrl = mode === "tui"`.
    //
    // This is `set_elicitation_config`'s FIRST production caller, and it is what makes
    // `create_elicitation` do anything but rmcp's default `Decline`. The `allow_url` half is
    // `is_tui_mode`, which is stricter than `has_ui`: a non-TUI surface has nowhere sensible to
    // hand a browser handoff back to.
    if settings.elicitation(snapshot.has_ui)
        && let Some(ui) = ui.as_ref()
    {
        let validators = Arc::new(crate::schema::ValidatorCache::default());
        let accepted = Arc::downgrade(&manager);
        let options = Arc::new(crate::elicitation::ElicitationOptions {
            allow_url: snapshot.is_tui_mode(),
            session: Arc::clone(&session),
            launcher: Arc::new(crate::oauth::OpenerLauncher) as Arc<dyn crate::oauth::BrowserLauncher>,
            // `options.onUrlAccepted` — the registry write the completion notice's dedupe reads.
            // `Weak`, so the hook the manager owns does not own the manager back; a dead weak is a
            // no-op, which is the right answer for a generation that has already been torn down.
            on_url_accepted: Arc::new(move |server: &str, id: &str| {
                if let Some(live) = accepted.upgrade() {
                    live.remember_url_elicitation(server, id);
                }
            }),
            validators,
        });
        let handler = {
            let options = Arc::clone(&options);
            Arc::new(move |server: String, params| {
                let options = Arc::clone(&options);
                Box::pin(async move {
                    crate::elicitation::handle_elicitation_request(&options, &server, params).await
                }) as BoxFuture<'static, Result<ElicitResult, ErrorData>>
            }) as ElicitationHook
        };
        let notify = {
            // The FENCED handle: a stale generation's notice must not paint into the session that
            // replaced it. `OwnedServices::notify` degrades to `()` once the owner stops.
            let ui = Arc::clone(ui);
            Arc::new(move |message: &str, kind| {
                cyrup_ext::HostServices::notify(ui.as_ref(), message, kind);
            }) as NotifyHook
        };
        manager.set_elicitation_config(Some(ElicitationConfig {
            mode: ElicitationMode {
                allow_url: snapshot.is_tui_mode(),
            },
            handler,
            notify,
        }));
    }

    // Step 7. `hasPendingAuth` is the OAuth runtime's, so an authenticating server is never reaped.
    let lifecycle = Arc::new(McpLifecycleManager::new(Arc::clone(&manager), Arc::new(|_| false)));
    lifecycle.set_global_idle_timeout(config.settings_or_default().idle_timeout_minutes());

    // Steps 8-9.
    let open_browser: OpenBrowser = {
        let owner = Arc::clone(&owner);
        Arc::new(move |_url: String| {
            let owner = Arc::clone(&owner);
            Box::pin(async move {
                // Guarded on BOTH sides of the await, as upstream is (13a §8 step 9).
                owner.throw_if_inactive()?;
                let result = Ok(());
                owner.throw_if_inactive()?;
                result
            })
        })
    };
    let send_message: SendMessage = {
        let owner = Arc::clone(&owner);
        Arc::new(move |message: String| {
            // `if (!owner.isActive()) return;` then `pi.sendMessage(...)`. The owner check IS the
            // guard — a stale generation must not inject into the session that replaced it.
            if owner.is_active() {
                tracing::debug!("MCP: send_message not yet wired — dropping {} bytes", message.len());
            }
        })
    };

    let state = Arc::new(McpState::new(McpStateParts {
        owner: Arc::clone(&owner),
        manager,
        lifecycle: Arc::clone(&lifecycle),
        config,
        programmatic_config: options.programmatic_config,
        oauth_runtime,
        auth_storage_options,
        ui,
        open_browser,
        send_message,
    }));

    // The hooks minted above can now resolve the generation's dialog. Bound AFTER the state exists
    // and BEFORE step 11's connect pass, so no hook can observe a half-built generation.
    session.bind(&state);

    // Steps 10, 11 and 12. The two cleanups are registered in this order so the LIFO run order is
    // `gracefulShutdown` -> `shutdownOAuth`, and step 11 sits BETWEEN them — the position this
    // function's own wiring list gives it and the position `init.ts:198-207` has.
    if owns_oauth_runtime {
        let oauth_runtime = Arc::clone(&state.oauth_runtime);
        owner.add_cleanup(Box::new(move || {
            Box::pin(async move {
                crate::oauth::shutdown_oauth(&oauth_runtime).await;
                Ok(())
            })
        }));
    }

    // Step 11 — the list-changed listener (MCP-017), installed AFTER the state commits so a hook
    // fired mid-build cannot see a half-installed surface, and BEFORE the connect pass so a
    // `tools/list_changed` arriving during startup is honoured rather than dropped
    // (`init.ts:200-206`).
    //
    // `preserveEmptyResources: false` is the load-bearing detail: THIS empty `resources/list` is
    // authoritative and must overwrite the cache, where §12's startup write must not.
    state.manager.set_metadata_list_changed_listener(Some({
        let state = Arc::clone(&state);
        let dirs = dirs.clone();
        Arc::new(move |server: &str, reason: &str| {
            if !state.owner.is_active() {
                return;
            }
            crate::live::update_server_metadata(&state, server);
            crate::live::update_metadata_cache(
                &state,
                &dirs,
                server,
                crate::live::MetadataCacheOptions { preserve_empty_resources: false },
            );
            state.notify_tool_metadata_updated(server, reason);
            crate::live::update_status_bar(&state);
        })
    }));

    owner.add_cleanup(Box::new(move || {
        Box::pin(async move {
            lifecycle.graceful_shutdown().await;
            Ok(())
        })
    }));

    // ── §8 tail — the zero-enabled-servers early return (MCP-018) ───────────────────────────
    // No cache work, no lifecycle, no health checks — just the notice, the published snapshot and
    // the state. The notice is gated on `allServerEntries.length > 0 && hasUI`
    // (`init.ts:217-223`), so a config with NO servers at all says nothing and a headless run says
    // nothing.
    if state.config.enabled_servers().next().is_none() {
        let all = state.config.mcp_servers.len();
        if all > 0
            && let Some(ui) = state.ui.as_ref()
        {
            HostServices::notify(
                ui.as_ref(),
                &format!("MCP: All {all} server(s) are disabled"),
                cyrup_ext::NotifyKind::Info,
            );
        }
        state.publish_status(crate::live::create_mcp_status_snapshot(&state));
        return Ok(state);
    }

    // ── §9 — cache bootstrap (MCP-019) ─────────────────────────────────────────────────────
    // The two-way split IS the unit (`init.ts:228-239`). Collapsing "no usable cache" into one arm
    // turns the corrupt-cache path from cheap into a connect storm.
    //
    // The PROBE is [`crate::dirs`]', the READ is [`crate::registration`]'s lenient reader, and the
    // WRITE is [`crate::dirs`]'. That asymmetry is deliberate: the strict reader answers `None` for
    // a file the lenient one parses fine, and rewriting on THAT would destroy the very cache
    // `resolve_direct_tools` and `resolve_cached_prompts` registered this session's surface from.
    let cache_path = dirs.metadata_cache();
    let cache_file_exists = cache_path.exists();
    let mut cache = crate::registration::load_metadata_cache(&dirs);
    let mut bootstrap_all = false;
    if !cache_file_exists {
        // No file at all — a first run. Every enabled server is a startup connect this once, so
        // the next launch has a cache to register a direct-tool surface from.
        bootstrap_all = true;
        save_empty_metadata_cache(&cache_path);
    } else if cache.is_none() {
        // A file that exists and does not parse. Truncate it, but do NOT set `bootstrap_all`: a
        // corrupt cache must stay cheap rather than becoming a connect storm.
        save_empty_metadata_cache(&cache_path);
        cache = Some(crate::registration::MetadataCache {
            version: crate::registration::METADATA_CACHE_VERSION,
            servers: indexmap::IndexMap::new(),
        });
    }

    // ── §10 — per-server lifecycle registration (MCP-020) + rehydration (MCP-021) ──────────
    for (name, definition) in state.config.enabled_servers() {
        let mode = definition.lifecycle_mode();
        // `persistsAfterFirstSpawn` is `eager | lazy-keep-alive` (`init.ts:245`) — NOT
        // [`ServerLifecycle::is_prewarmed`], which is `eager | keep-alive` and answers §11's
        // question instead. The two sets differ and swapping them is silent.
        let persists = matches!(mode, ServerLifecycle::Eager | ServerLifecycle::LazyKeepAlive);
        // `definition.idleTimeout ?? (persistsAfterFirstSpawn ? 0 : undefined)` (`init.ts:246`) —
        // the `?? 0` is what stops an eager or lazy-keep-alive server ever idling out by default.
        let idle_timeout = definition.idle_timeout.or_else(|| persists.then_some(0.0));
        state.lifecycle.register_server(
            name,
            definition.clone(),
            idle_timeout
                .map(|minutes| crate::lifecycle::LifecycleOverrides { idle_timeout: Some(minutes) }),
        );
        // ONLY `keep-alive` at registration (`init.ts:252`); `lazy-keep-alive` waits for its first
        // successful connect, which is [`McpLifecycleManager::mark_keep_alive_after_connect`].
        if marks_keep_alive_at_registration(mode) {
            state.lifecycle.mark_keep_alive(name);
        }
        // Step 6 — rehydrate from a hash-valid entry (`init.ts:256-269`).
        // [`crate::registration::valid_entry`] IS `cachedEntry && isServerCacheValid(entry, def)`;
        // it is not re-derived here, because a second hash path is the reader/writer drift the
        // cache seam exists to prevent.
        if let Some(cache) = cache.as_ref()
            && let Some(entry) = crate::registration::valid_entry(Some(cache), name, definition)
        {
            crate::live::rehydrate_from_cache(&state, name, definition, entry, cache);
        }
    }

    // ── §11 — the bounded startup connect pass (MCP-022 / MCP-087 / MCP-130) ───────────────
    let startup: Vec<(String, ServerEntry)> = state
        .config
        .enabled_servers()
        .filter(|(_, definition)| bootstrap_all || definition.lifecycle_mode().is_prewarmed())
        .map(|(name, definition)| (name.clone(), definition.clone()))
        .collect();

    if let Some(ui) = state.ui.as_ref()
        && !startup.is_empty()
    {
        let text = crate::ui::format_mcp_status(
            &state.config,
            &format!("connecting to {} servers...", startup.len()),
        );
        HostServices::set_status(ui.as_ref(), "mcp", text.as_deref());
    }

    // `{name, definition, connection, error}` (`init.ts:284-299`), as a tuple. `error` is `Some`
    // for a real failure AND for needs-auth (which carries the byte-exact `/mcp-auth` line); BOTH
    // are `None` for an abort on a live signal, which pass two skips silently.
    //
    // One deliberate divergence: upstream rethrows the abort when `owner.signal.aborted`
    // (`init.ts:294`), aborting the whole `parallelLimit`. Here the arm answers `(None, None)` and
    // the `throw_if_inactive` immediately below turns the same condition into the same `Err` — the
    // only difference is that the remaining in-flight connects finish first, and they are already
    // racing a cancelled runtime signal.
    let results = crate::live::parallel_limit(
        startup.clone(),
        crate::live::STARTUP_CONNECT_CONCURRENCY,
        |(name, definition)| {
            let manager = Arc::clone(&state.manager);
            let signal = runtime_signal.clone();
            async move {
                match manager.connect(&name, &definition, Some(&signal)).await {
                    Ok(connection) if connection.status() == ConnectionStatus::NeedsAuth => {
                        // BYTE-EXACT (`init.ts:288`). The `/mcp-auth {name}` form is what the user
                        // copies; a reworded line is a support burden, not a style choice.
                        let message = format!("OAuth authentication required. Run /mcp-auth {name}.");
                        (name, definition, None, Some(message))
                    }
                    Ok(connection) => (name, definition, Some(connection), None),
                    Err(error) if crate::abort::is_abort_error(&error, Some(&signal)) => {
                        (name, definition, None, None)
                    }
                    Err(error) => (name, definition, None, Some(error.to_string())),
                }
            }
        },
    )
    .await;

    // `if (initialSignal?.aborted) return state;` (`init.ts:301`) — BEFORE the owner check, and it
    // returns `Ok`, not `Err`. This is the FIFTH exit from this function: a caller-cancelled init
    // hands back the state it built rather than failing.
    if snapshot.initial_signal.as_ref().is_some_and(CancelToken::is_cancelled) {
        return Ok(state);
    }
    // MCP-046 checkpoint 1 (`init.ts:302`).
    owner.throw_if_inactive()?;

    // ── §12 — the two-pass metadata build (MCP-023) ────────────────────────────────────────
    let prefix = state.config.tool_prefix();

    // Pass one, over EVERY successful connection first (`init.ts:304-325`): a SIMPLE prefixed
    // list — no `includeTools`/`excludeTools` filtering, no collision resolution, no visibility
    // check — because it IS the collision universe pass two resolves against. Building it with
    // [`crate::registration::build_tool_metadata`] would be circular: that function's answer for
    // one server depends on this map's entry for every other.
    let mut startup_known: indexmap::IndexMap<String, Vec<crate::proxy::ToolMetadata>> =
        indexmap::IndexMap::new();
    for (name, definition, connection, _) in &results {
        let Some(connection) = connection.as_ref() else { continue };
        let effective_prefix = crate::registration::resolve_tool_prefix(Some(definition), prefix);
        let mut metadata: Vec<crate::proxy::ToolMetadata> = connection
            .tools()
            .iter()
            .filter(|tool| !tool.name.is_empty())
            .map(|tool| crate::proxy::ToolMetadata::new(
                crate::registration::format_tool_name(&tool.name, name, effective_prefix),
                tool.name.to_string(),
                tool.description.as_deref().unwrap_or_default(),
            ))
            .collect();
        // `definition.exposeResources !== false ? … : []` (`init.ts:313`), and the `resource?.name
        // && resource?.uri` guard that goes with it.
        if definition.expose_resources() {
            for resource in &connection.resources() {
                if resource.name.is_empty() || resource.uri.is_empty() {
                    continue;
                }
                let original_name = crate::registration::resource_base_tool_name(&resource.name);
                metadata.push(crate::proxy::ToolMetadata {
                    name: crate::registration::format_tool_name(
                        &original_name,
                        name,
                        effective_prefix,
                    ),
                    original_name,
                    description: resource
                        .description
                        .clone()
                        .unwrap_or_else(|| format!("Read resource: {}", resource.uri)),
                    resource_uri: Some(resource.uri.clone()),
                    ui_visibility: None,
                    input_schema: None,
                });
            }
        }
        startup_known.insert(name.clone(), metadata);
    }

    // Pass two, per server (`init.ts:327-362`).
    for (name, definition, connection, error) in &results {
        // MCP-046 checkpoint 2 (`init.ts:328`) — at the TOP of each iteration, so a stop observed
        // mid-pass leaves the remaining servers untouched instead of half-committed.
        owner.throw_if_inactive()?;

        // `if (error || !connection)` — a needs-auth result carries BOTH a `None` connection and a
        // message, and a live-signal abort carries neither.
        let connection = match connection.as_ref().filter(|_| error.is_none()) {
            Some(connection) => connection,
            None => {
                // `if (initialSignal?.aborted) continue;` FIRST (`init.ts:330`), before anything is
                // recorded: a cancelled init must not poison the next sixty seconds of every
                // server's availability.
                if snapshot.initial_signal.as_ref().is_some_and(CancelToken::is_cancelled) {
                    continue;
                }
                // `if (error) recordFailure(...)` — the abort arm has no message and records
                // nothing.
                if let Some(message) = error.as_deref() {
                    crate::live::record_failure(&state, name, message);
                }
                let display = crate::ui::sanitize_terminal_text(
                    error.as_deref().unwrap_or("Unknown connection failure"),
                );
                let line = format!("MCP: Failed to connect to {name}: {display}");
                if let Some(ui) = state.ui.as_ref() {
                    HostServices::notify(ui.as_ref(), &line, cyrup_ext::NotifyKind::Error);
                }
                // Upstream emits the notice AND `console.error` (`init.ts:336`), because a
                // headless run has only the second. This crate's established rendering of
                // `console.error` is `tracing::error!` (`lifecycle.rs`'s health-check catch does
                // the same) — writing raw to stderr from inside a TUI session would corrupt the
                // display that `ui.notify` already served.
                tracing::error!("{line}");
                continue;
            }
        };

        // `buildToolMetadata(..., startupKnownMetadata, true)` — the startup snapshot, NOT
        // `state.toolMetadata`, and `includeMissingConfiguredCandidates: true` with it.
        let built = crate::registration::build_tool_metadata(
            &connection.tools(),
            &connection.resources(),
            definition,
            name,
            prefix,
            Some(&state.config.mcp_servers),
            Some(&startup_known),
            true,
        );
        let failed_tools = built.failed_tools.len();
        if let Ok(mut map) = state.tool_metadata.lock() {
            map.insert(name.clone(), built.metadata);
        }
        if let Ok(mut counts) = state.resource_counts.lock() {
            counts.insert(name.clone(), connection.resources().len());
        }
        // `if (!connection.promptDiscoveryFailed)` — only a live `prompts/list` writes the map and
        // joins the live set (`init.ts:340-343`).
        if !connection.prompt_discovery_failed() {
            let prompts = crate::registration::reconstruct_prompt_metadata(
                name,
                &connection.prompts(),
                prefix,
                Some(definition),
            );
            if let Ok(mut map) = state.prompt_metadata.lock() {
                map.insert(name.clone(), prompts);
            }
            if let Ok(mut live) = state.prompt_metadata_live.lock() {
                live.insert(name.clone());
            }
        }
        // `if (connection.instructions) … else delete` (`init.ts:344-348`) — a TRUTHY test, so an
        // EMPTY string DELETES.
        if let Ok(mut map) = state.server_instructions.lock() {
            match connection.instructions().filter(|text| !text.is_empty()) {
                Some(text) => {
                    map.insert(name.clone(), text.to_string());
                }
                None => {
                    map.shift_remove(name);
                }
            }
        }
        crate::live::update_metadata_cache(
            &state,
            &dirs,
            name,
            crate::live::MetadataCacheOptions::preserving(),
        );
        state.notify_tool_metadata_updated(name, "startup");
        state.lifecycle.mark_keep_alive_after_connect(name);

        if failed_tools > 0
            && let Some(ui) = state.ui.as_ref()
        {
            HostServices::notify(
                ui.as_ref(),
                &format!("MCP: {name} - {failed_tools} tools skipped"),
                cyrup_ext::NotifyKind::Warning,
            );
        }
    }

    // ── §13 — the startup summary (`init.ts:364-372`) ──────────────────────────────────────
    let connected_count =
        results.iter().filter(|(_, _, connection, _)| connection.is_some()).count();
    let failed_count = results.iter().filter(|(_, _, _, error)| error.is_some()).count();
    if let Some(ui) = state.ui.as_ref()
        && connected_count > 0
        && state.config.settings_or_default().notify_on_startup_connect()
    {
        let total_tools = total_tool_count(&state);
        // `{total}` is `startupServers.length`, NOT the config count: a lazy server that was never
        // in this pass is not a server that failed to connect.
        let message = if failed_count > 0 {
            format!(
                "MCP: {connected_count}/{} servers connected ({total_tools} tools)",
                startup.len()
            )
        } else {
            format!("MCP: {connected_count} servers connected ({total_tools} tools)")
        };
        HostServices::notify(ui.as_ref(), &message, cyrup_ext::NotifyKind::Info);
    }

    // ── §14 — the `MCP_DIRECT_TOOLS` bootstrap (MCP-026) ───────────────────────────────────
    let env_direct = std::env::var("MCP_DIRECT_TOOLS").ok();
    // `envDirect !== "__none__"` skips the WHOLE block (`init.ts:375`), which is a different shape
    // from [`direct_tools_override`]'s `Some(vec![])`: the RAW value is tested first, and only then
    // normalised for the predicate.
    if env_direct.as_deref() != Some(DIRECT_TOOLS_NONE_SENTINEL) {
        // Re-read the cache from disk rather than reusing §9's value, exactly as upstream does
        // (`init.ts:376`): §12 has just rewritten it, and a server whose entry landed there is no
        // longer missing.
        let current_cache = crate::registration::load_metadata_cache(&dirs);
        let env_override = direct_tools_override(env_direct.as_deref());
        let missing = crate::registration::missing_configured_direct_tool_servers(
            &state.config,
            current_cache.as_ref(),
            env_override.as_deref(),
        );
        if !missing.is_empty() {
            // `filter(name => !results.some(r => r.name === name && r.connection))`
            // (`init.ts:382`) — a server §11 already connected is cached and not missing.
            let pending: Vec<String> = missing
                .into_iter()
                .filter(|name| {
                    !results.iter().any(|(other, _, connection, _)| {
                        other == name && connection.is_some()
                    })
                })
                .collect();
            let bootstrap = crate::live::parallel_limit(
                pending,
                crate::live::STARTUP_CONNECT_CONCURRENCY,
                |name| {
                    let state = Arc::clone(&state);
                    let dirs = dirs.clone();
                    let signal = runtime_signal.clone();
                    async move {
                        // `if (!definition) throw new Error(...)` (`init.ts:387`) — thrown INSIDE
                        // the try, so it lands in the same catch a connect failure does.
                        let Some(definition) = state.config.mcp_servers.get(&name).cloned() else {
                            let message = format!("MCP server \"{name}\" is not configured");
                            crate::live::record_failure(&state, &name, &message);
                            tracing::debug!(
                                "MCP: direct-tools bootstrap failed for {name}: {}",
                                crate::ui::sanitize_terminal_text(&message)
                            );
                            return (name, false);
                        };
                        match state.manager.connect(&name, &definition, Some(&signal)).await {
                            Ok(connection)
                                if connection.status() == ConnectionStatus::NeedsAuth =>
                            {
                                (name, false)
                            }
                            Ok(_) => {
                                crate::live::update_server_metadata(&state, &name);
                                crate::live::update_metadata_cache(
                                    &state,
                                    &dirs,
                                    &name,
                                    crate::live::MetadataCacheOptions::preserving(),
                                );
                                state.notify_tool_metadata_updated(&name, "direct-tools-bootstrap");
                                state.lifecycle.mark_keep_alive_after_connect(&name);
                                crate::live::clear_failure(&state, &name);
                                (name, true)
                            }
                            Err(error) if crate::abort::is_abort_error(&error, Some(&signal)) => {
                                (name, false)
                            }
                            Err(error) => {
                                let message = error.to_string();
                                crate::live::record_failure(&state, &name, &message);
                                tracing::debug!(
                                    "MCP: direct-tools bootstrap failed for {name}: {}",
                                    crate::ui::sanitize_terminal_text(&message)
                                );
                                (name, false)
                            }
                        }
                    }
                },
            )
            .await;
            let bootstrapped: Vec<String> =
                bootstrap.into_iter().filter_map(|(name, ok)| ok.then_some(name)).collect();
            // MCP-046 checkpoint 3 (`init.ts:411`), INSIDE the `missingCacheServers.length > 0`
            // arm — not outside it.
            owner.throw_if_inactive()?;
            if !bootstrapped.is_empty()
                && let Some(ui) = state.ui.as_ref()
            {
                // BYTE-EXACT upstream (`init.ts:413`), and deliberately so. 13a MCP-026 offers the
                // alternative of registering the new surface here and saying "are now available"
                // instead — but that requires `McpExtension::sync_tool_surface`, and this function
                // holds no extension handle: `initialize_mcp`'s inputs are the owner, the dirs, the
                // context snapshot and the options. The registration belongs with
                // `install_surface_sync`'s caller (MCP-011), and the message must not change
                // before it: promising availability that never arrives is worse than the restart.
                HostServices::notify(
                    ui.as_ref(),
                    &format!(
                        "MCP: direct tools for {} will be available after restart",
                        bootstrapped.join(", ")
                    ),
                    cyrup_ext::NotifyKind::Info,
                );
            }
        }
    }

    // ── §15 — lifecycle callbacks (MCP-027) ────────────────────────────────────────────────
    // FIVE, not three (`init.ts:418-451`). Omitting `health_restored` leaves a recovered server
    // marked `failed` for the full 60 s; omitting `auth_required` leaves a `needs-auth` server
    // marked `failed`. Every body opens with the owner guard: that is what keeps a generation-N
    // timer from writing into generation N+1.
    let on_reconnect: crate::lifecycle::ReconnectCallback = {
        let state = Arc::clone(&state);
        let dirs = dirs.clone();
        Arc::new(move |server: String| {
            let state = Arc::clone(&state);
            let dirs = dirs.clone();
            Box::pin(async move {
                if !state.owner.is_active() {
                    return Ok(());
                }
                crate::live::update_server_metadata(&state, &server);
                crate::live::update_metadata_cache(
                    &state,
                    &dirs,
                    &server,
                    crate::live::MetadataCacheOptions::preserving(),
                );
                state.notify_tool_metadata_updated(&server, "lifecycle-reconnect");
                crate::live::clear_failure(&state, &server);
                crate::live::update_status_bar(&state);
                Ok(())
            })
        })
    };
    state.lifecycle.set_reconnect_callback(on_reconnect);

    let on_reconnect_failure: crate::lifecycle::ReconnectFailureCallback = {
        let state = Arc::clone(&state);
        Arc::new(move |server: &str, error: &McpError| {
            if !state.owner.is_active() {
                return;
            }
            crate::live::record_failure(&state, server, &error.to_string());
            crate::live::update_status_bar(&state);
        })
    };
    state.lifecycle.set_reconnect_failure_callback(on_reconnect_failure);

    let on_health_restored: crate::lifecycle::HealthRestoredCallback = {
        let state = Arc::clone(&state);
        Arc::new(move |server: String| {
            let state = Arc::clone(&state);
            Box::pin(async move {
                if !state.owner.is_active() {
                    return Ok(());
                }
                crate::live::clear_failure(&state, &server);
                crate::live::update_status_bar(&state);
                Ok(())
            })
        })
    };
    state.lifecycle.set_health_restored_callback(on_health_restored);

    let on_auth_required: crate::lifecycle::AuthRequiredCallback = {
        let state = Arc::clone(&state);
        Arc::new(move |server: String| {
            let state = Arc::clone(&state);
            Box::pin(async move {
                if !state.owner.is_active() {
                    return Ok(());
                }
                crate::live::clear_failure(&state, &server);
                crate::live::update_status_bar(&state);
                Ok(())
            })
        })
    };
    state.lifecycle.set_auth_required_callback(on_auth_required);

    let on_idle_shutdown: crate::lifecycle::IdleShutdownCallback = {
        let state = Arc::clone(&state);
        Arc::new(move |server: &str| {
            if !state.owner.is_active() {
                return;
            }
            let minutes = effective_idle_timeout_minutes(&state, server);
            tracing::debug!("{server} shut down (idle {minutes}m)");
            crate::live::update_status_bar(&state);
        })
    };
    state.lifecycle.set_idle_shutdown_callback(on_idle_shutdown);

    // ── The tail (`init.ts:453-458`) ───────────────────────────────────────────────────────
    // MCP-046 checkpoint 4, the health checks, the `off` footer clear, and a PUBLISH — not an
    // `update_status_bar`, which would additionally WRITE the footer this path deliberately leaves
    // to whatever §11 last set.
    owner.throw_if_inactive()?;
    // `startHealthChecks(runtimeSignal)`. [`McpLifecycleManager::start`] takes the OWNER rather
    // than a token — it re-reads `owner.token()` itself — so the combined runtime signal is not
    // threaded here; the owner half of it is the half that stops the loop.
    state.lifecycle.start(&owner);
    if state.config.settings_or_default().mcp_footer_status() == crate::config::FooterStatus::Off
        && let Some(ui) = state.ui.as_ref()
    {
        HostServices::set_status(ui.as_ref(), "mcp", None);
    }
    state.publish_status(crate::live::create_mcp_status_snapshot(&state));
    Ok(state)
}

/// `saveMetadataCache({version: 1, servers: {}})` (`init.ts:233`, `:236`) — §9's two write arms.
///
/// [`crate::dirs::save_metadata_cache`] merges rather than replaces, which is what makes upstream's
/// one-entry writes non-destructive; an empty cache therefore truncates only because the strict
/// reader inside it rejects the same bytes the lenient one just did.
///
/// The failure is swallowed with a debug line, as `metadata-cache.ts`'s own try/catch is: a cache
/// that cannot be written is a slower next start, never a failed init.
fn save_empty_metadata_cache(path: &std::path::Path) {
    if let Err(error) =
        crate::dirs::save_metadata_cache(path, &crate::dirs::MetadataCache::default())
    {
        tracing::debug!("MCP: failed to bootstrap the metadata cache: {error}");
    }
}

/// `tool-metadata.ts:146` `totalToolCount(state)` — every server's tool count, summed.
///
/// Deliberately **not** [`crate::state::McpStatusSnapshot::total_tools`]: that one excludes
/// disabled servers and falls back to the live connection's list, where this is the flat sum over
/// `state.toolMetadata` that §13's summary line quotes.
fn total_tool_count(state: &McpState) -> usize {
    state
        .tool_metadata
        .lock()
        .map(|map| map.values().map(Vec::len).sum())
        .unwrap_or(0)
}

/// `getEffectiveIdleTimeoutMinutes(state, serverName)` (`init.ts:664-673`) — what §15's idle
/// shutdown line reports.
///
/// Three rungs, in upstream's order: the definition's own `idleTimeout`; `0` for a mode that
/// persists after its first spawn; then the global setting. An unconfigured server takes the global
/// directly, skipping both.
fn effective_idle_timeout_minutes(state: &McpState, server: &str) -> f64 {
    let global = || state.config.settings_or_default().idle_timeout_minutes();
    let Some(definition) = state.config.mcp_servers.get(server) else { return global() };
    if let Some(minutes) = definition.idle_timeout {
        return minutes;
    }
    if matches!(
        definition.lifecycle_mode(),
        ServerLifecycle::Eager | ServerLifecycle::LazyKeepAlive
    ) {
        return 0.0;
    }
    global()
}

/// `startLoadTimeInitialization`'s gate (MCP-012): connect at load **only** when some enabled
/// server declares `lifecycle: "eager" | "keep-alive"`. Everything else waits for its first call.
///
/// Landed now because [`McpExtension`][crate::extension::McpExtension]'s `init` consults it to decide whether to
/// spawn the pre-warm task at all, and getting this wrong costs every session a subprocess
/// handshake it did not need.
#[must_use]
pub fn needs_load_time_initialization(config: &McpConfig) -> bool {
    config.enabled_servers().any(|(_, entry)| entry.lifecycle_mode().is_prewarmed())
}

/// `MCP_DIRECT_TOOLS`'s "no servers" sentinel (MCP-013). Not an empty string: an empty value is
/// indistinguishable from unset in most shells, so upstream picked a token.
pub const DIRECT_TOOLS_NONE_SENTINEL: &str = "__none__";

/// Normalise `MCP_DIRECT_TOOLS` — `split(',').map(trim).filter(non-empty)`, with the
/// [`DIRECT_TOOLS_NONE_SENTINEL`] collapsing to "explicitly none" (MCP-013).
///
/// `None` means the variable is unset (no pinning); `Some(empty)` means the sentinel was given.
#[must_use]
pub fn direct_tools_override(raw: Option<&str>) -> Option<Vec<String>> {
    let raw = raw?;
    if raw.trim() == DIRECT_TOOLS_NONE_SENTINEL {
        return Some(Vec::new());
    }
    Some(
        raw.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Whether `lifecycle` should be marked keep-alive at **registration** time. Only `keep-alive` is;
/// `lazy-keep-alive` is marked after its first successful connect (13a §10 step 5).
#[must_use]
pub fn marks_keep_alive_at_registration(lifecycle: ServerLifecycle) -> bool {
    matches!(lifecycle, ServerLifecycle::KeepAlive)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::config::ServerEntry;

    fn config_with(lifecycle: Option<ServerLifecycle>, disabled: bool) -> McpConfig {
        let mut config = McpConfig::default();
        config.mcp_servers.insert(
            "s".to_string(),
            ServerEntry {
                lifecycle,
                disabled: disabled.then_some(true),
                ..Default::default()
            },
        );
        config
    }

    #[test]
    fn prewarm_gate_matches_the_two_lifecycles_and_skips_disabled() {
        assert!(!needs_load_time_initialization(&config_with(None, false)), "lazy is the default");
        assert!(needs_load_time_initialization(&config_with(Some(ServerLifecycle::Eager), false)));
        assert!(needs_load_time_initialization(&config_with(Some(ServerLifecycle::KeepAlive), false)));
        assert!(
            !needs_load_time_initialization(&config_with(Some(ServerLifecycle::LazyKeepAlive), false)),
            "lazy-keep-alive connects on first call, not at load"
        );
        assert!(
            !needs_load_time_initialization(&config_with(Some(ServerLifecycle::Eager), true)),
            "a disabled server never pre-warms"
        );
    }

    #[test]
    fn direct_tools_override_normalisation() {
        assert_eq!(direct_tools_override(None), None);
        assert_eq!(direct_tools_override(Some("__none__")), Some(Vec::new()));
        assert_eq!(
            direct_tools_override(Some(" a , ,b ")),
            Some(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn only_keep_alive_is_marked_at_registration() {
        assert!(marks_keep_alive_at_registration(ServerLifecycle::KeepAlive));
        assert!(!marks_keep_alive_at_registration(ServerLifecycle::LazyKeepAlive));
        assert!(!marks_keep_alive_at_registration(ServerLifecycle::Eager));
    }

    #[test]
    fn tui_mode_is_stricter_than_has_ui() {
        let snap = ContextSnapshot {
            config_path: None,
            cwd: PathBuf::from("/w"),
            has_ui: true,
            mode: "print".to_string(),
            initial_signal: None,
            services: None,
        };
        assert!(!snap.is_tui_mode());
        let tui = ContextSnapshot { mode: "tui".to_string(), ..snap };
        assert!(tui.is_tui_mode());
    }

    #[tokio::test]
    async fn zero_enabled_servers_returns_a_state_without_connecting() {
        let owner = Arc::new(McpRuntimeOwner::new());
        let dirs = McpDirs::new(PathBuf::from("/nonexistent/agent"), PathBuf::from("/w"));
        let snapshot = ContextSnapshot {
            config_path: None,
            cwd: PathBuf::from("/w"),
            has_ui: false,
            mode: "print".to_string(),
            initial_signal: None,
            services: None,
        };
        let state = initialize_mcp(owner, dirs, snapshot, InitializeOptions::default())
            .await
            .unwrap();
        assert!(state.config.mcp_servers.is_empty());
        assert!(state.owner.is_active());
    }

    /// The production wiring: the one `ConnectionBuilder` this function builds carries a
    /// **store-backed** [`HttpAuthProvider`], so an `auth: "oauth"` HTTP server reads the credential
    /// vault before it opens a socket.
    ///
    /// The signal is chosen to be environment-independent. The server's URL is a closed loopback
    /// port, so the only way this connect can fail with a *credential-store* message is if the
    /// vault was consulted first — and it is consulted first exactly because the explicit arm reads
    /// the store before the handshake. The seeded entry is deliberately unparseable, which is the
    /// one vault answer that is the same on a machine with a working keychain (the backend has no
    /// entry, the read falls through to this legacy file, and the file will not parse) and on one
    /// with none at all (the backend read itself fails). Both render
    /// `"... OAuth credentials for gated ..."`.
    ///
    /// With `ConnectionBuilder::new`'s default [`NoStoredCredentials`] — which is what this
    /// function installed before the provider was wired in — nothing reads the vault, the connect
    /// reaches the closed port, and the recorded failure is a connection refusal instead.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_explicit_oauth_server_reaches_the_credential_store_before_the_socket() {
        let dir = tempfile::tempdir().unwrap();
        let oauth_dir = dir.path().join("mcp-oauth");
        let server_dir = oauth_dir.join(crate::credentials::auth_entry_account("gated"));
        std::fs::create_dir_all(&server_dir).unwrap();
        std::fs::write(server_dir.join("tokens.json"), b"{ not json").unwrap();

        let mut config = McpConfig::default();
        config.mcp_servers.insert(
            "gated".to_string(),
            ServerEntry {
                // Port 1 on loopback: reserved, never listening, and refused immediately.
                url: Some("http://127.0.0.1:1/mcp".to_string()),
                auth: Some(crate::config::AuthMode::Named(crate::config::AuthKind::Oauth)),
                ..ServerEntry::default()
            },
        );
        config.settings = Some(crate::config::McpSettings {
            oauth_dir: Some(oauth_dir.display().to_string()),
            ..crate::config::McpSettings::default()
        });

        let owner = Arc::new(McpRuntimeOwner::new());
        let dirs = McpDirs::new(dir.path().to_path_buf(), dir.path().to_path_buf());
        let snapshot = ContextSnapshot {
            config_path: None,
            cwd: dir.path().to_path_buf(),
            has_ui: false,
            mode: "print".to_string(),
            initial_signal: None,
            services: None,
        };
        let state = initialize_mcp(
            owner,
            dirs,
            snapshot,
            InitializeOptions {
                programmatic_config: Some(config),
                ..InitializeOptions::default()
            },
        )
        .await
        .unwrap();

        let message = state
            .failure_messages
            .lock()
            .unwrap()
            .get("gated")
            .cloned()
            .expect("the startup connect recorded a failure");
        assert!(
            message.contains("OAuth credentials for gated"),
            "the vault was consulted before the socket; got {message}"
        );
    }
}

// =================================================================================================
// The wire — transports, the rmcp client and `ClientHandler` (13c §3.2–§3.10)
//
// Everything below this line is the *connection* half of the runtime: how a [`ServerEntry`] becomes
// a live `rmcp` transport, how that transport becomes an initialised client, and what the client
// does when the server talks back unprompted. It is deliberately free of connection *bookkeeping* —
// the registry, the single-flight maps, the generations and the retry ladders are
// `server_manager`'s (MCP-100, MCP-115, MCP-123…126) and consume the seams declared here.
//
// The four surfaces upstream has that do not exist here, so a reader does not go looking:
//
// * the legacy HTTP+SSE transport (**Cut 1** — rmcp 3.1.x ships no SSE *client* transport at all),
// * the raw framed unix socket (**Cut 3** — [`ServerEntry`] has no `socket` field),
// * the adapter-private UI stream-patch notification (**Cut 2** — see
//   [`STREAM_RESULT_PATCH_METHOD`]), and
// * `wrapTransportWithMcpTrace`, whose *composition* trick is what makes upstream's protocol probe
//   run on a disposable sibling process — see [`version_negotiation`] for the delta that costs.
// =================================================================================================

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::StreamExt as _;
// `http` and `sse-stream` are declared dependencies for exactly this reason: implementing
// `StreamableHttpClient` (here for [`SessionIdProbe`], in `request_headers_command.rs` for the
// signing decorator) means naming the trait's argument and return types, and a trait impl leaves no
// option to reach them by inference. See the crate's dependency note.
use http::{HeaderName, HeaderValue};
use sse_stream::{Error as SseError, Sse, SseStream};
// SEP-2577 deprecates `sampling/createMessage` protocol-wide and rmcp marks the types
// accordingly; `pi-mcp-adapter` ships a sampling handler and 1:1 parity is a hard rule, so the
// deprecation is acknowledged and suppressed rather than obeyed. rmcp's own `handler/client.rs`
// carries the identical `#![expect(deprecated)]` for the identical reason.
#[allow(deprecated)]
use rmcp::model::{
    ClientCapabilities, ClientInfo, CreateMessageRequestMethod, CreateMessageRequestParams,
    CreateMessageResult, CustomNotification, ElicitRequestParams, ElicitResult,
    ElicitationAction, ElicitationCapability, FormElicitationCapability, Implementation,
    ProtocolVersion, SamplingCapability, UrlElicitationCapability,
};
use rmcp::service::{
    ClientInitializeError, ClientLifecycleMode, MaybeSendFuture, NotificationContext, Peer,
    PeerRequestOptions, RequestContext, RoleClient, RunningService, ServiceError,
};
use rmcp::model::{ClientJsonRpcMessage, ClientRequest, JsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::common::http_header::{
    EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID, HEADER_SESSION_ID, JSON_MIME_TYPE,
};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, InsufficientScopeError, StreamableHttpClient,
    StreamableHttpClientTransportConfig, StreamableHttpError, StreamableHttpPostResponse,
};
use rmcp::transport::{ConfigureCommandExt, IntoTransport, StreamableHttpClientTransport,
    TokioChildProcess};
use rmcp::{ClientHandler, ErrorData};
use tokio::process::ChildStderr;

use crate::config::{HttpTransport, ProtocolVersionSetting, ServerEntry};
use crate::errors::McpError;
use crate::lifecycle::ConnectionStatus;
use crate::server_manager::{
    ConnectionFactory, ConnectionResource, CreateConnection, Discovery, NewConnection,
};

// -------------------------------------------------------------------------------------------------
// MCP-113 — transport selection and mutual exclusion (§3.2)
// -------------------------------------------------------------------------------------------------

/// The two transports that survive the cuts.
///
/// Upstream counts three (`command`, `url`, `socket`) and enforces "exactly one"; here the count is
/// two, because Cut 3 removed the raw unix socket outright — see [`socket_cut_diagnostic`] for the
/// string a configuration that still carries one has to be refused with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    /// `definition.command` — a child process speaking NDJSON over stdio (§3.3).
    Stdio,
    /// `definition.url` — streamable HTTP (§3.4). The only HTTP transport rmcp implements
    /// client-side, and post-Cut-1 the only one this port accepts.
    StreamableHttp,
}

/// `createConnection`'s mutual-exclusion check (`server-manager.ts:466-470`), minus `socket`.
///
/// Three properties are load-bearing and each has cost a porter a bug somewhere:
///
/// 1. **An empty string counts as unconfigured.** `{command: "", url: "http://x"}` is a *valid*
///    HTTP server, not an error — upstream's filter is
///    `typeof value === "string" && value.length > 0`.
/// 2. **`${name}` is unquoted in this one message**, unlike almost every other string in
///    `server-manager.ts`. Reproduced verbatim.
/// 3. **The check fires at connect time, not at parse time** (13b "Validation rules that fire at
///    connect time"). A two-transport entry loads fine and fails per-connection, which is why
///    `isServerCacheValid` has to wrap `computeServerHash` in a `try`/`catch`.
///
/// The returned error is [`McpError::Config`] rather than [`McpError::Server`] because `Config`'s
/// `Display` is `{0}` — the message already contains the server name and `Server`'s
/// `"{server}: {message}"` would print it twice.
///
/// # The two cut diagnostics
///
/// `httpTransport: "sse"` is refused here with the Cut-1 sentence. It *should* already have been
/// refused at config load (13b "Cut-value diagnostics"), and this is the second line of defence:
/// silence is the one unacceptable behaviour, because `agent-plugin-loader.ts` sets `httpTransport`
/// straight from a manifest's `type: "sse"`, so a plugin declaring SSE is a live, reachable case
/// that would otherwise appear configured and never connect.
///
/// # Errors
///
/// The exactly-one-transport error, or the Cut-1 legacy-SSE diagnostic.
pub fn select_transport(name: &str, entry: &ServerEntry) -> McpResult<TransportKind> {
    // `[definition.command, definition.url].filter(v => typeof v === "string" && v.length > 0)`.
    let command = configured(entry.command.as_deref());
    let url = configured(entry.url.as_deref());

    match (command, url) {
        (Some(_), None) => Ok(TransportKind::Stdio),
        (None, Some(_)) => {
            if entry.http_transport == Some(HttpTransport::Sse) {
                return Err(McpError::Config(sse_cut_diagnostic(name)));
            }
            Ok(TransportKind::StreamableHttp)
        }
        // Both or neither. Upstream's message named three transports; post-cut it names two, and
        // the `${name}` stays unquoted.
        _ => Err(McpError::Config(format!(
            "Server {name} must configure exactly one of command or url"
        ))),
    }
}

/// `typeof value === "string" && value.length > 0` — an empty string is *unconfigured*, not a
/// misconfigured transport. Note this is `is_empty`, not `trim().is_empty()`: upstream tests the
/// raw length, so a single space is a configured (and doomed) command.
#[must_use]
fn configured(value: Option<&str>) -> Option<&str> {
    value.filter(|candidate| !candidate.is_empty())
}

/// The **Cut 1** diagnostic, verbatim from 13c §3.2. Exposed so the config loader (13b, MCP-050…065)
/// refuses `httpTransport: "sse"` with the *same sentence* the connect path would, rather than a
/// second paraphrase of it.
#[must_use]
pub fn sse_cut_diagnostic(name: &str) -> String {
    format!(
        "MCP server \"{name}\" requests the legacy HTTP+SSE transport, which cyrup does not \
         support; use streamable HTTP."
    )
}

/// The **Cut 3** diagnostic, verbatim from 13c §3.2.
///
/// **Uncalled, and unreachable by construction.** [`ServerEntry`] has no `socket` field, so this
/// string can never be produced from a parsed entry; the only caller it could have is the config
/// loader, at the point where it still holds the raw document — and that loader drops an unknown
/// `socket` key silently rather than diagnosing it. Kept as the recorded Cut-3 wording, beside its
/// Cut-1 sibling [`sse_cut_diagnostic`], so the two sentences stay together and neither drifts.
#[must_use]
pub fn socket_cut_diagnostic(name: &str) -> String {
    format!(
        "MCP server \"{name}\" configures `socket`; cyrup supports only stdio (`command`) and \
         streamable HTTP (`url`)."
    )
}

// -------------------------------------------------------------------------------------------------
// MCP-102 — bounded stderr capture (§3.3)
// -------------------------------------------------------------------------------------------------

/// `MAX_CAPTURED_STDERR_BYTES` (`server-manager.ts:64`). 8 KiB, and the bound is applied to every
/// chunk *before* it is appended, so a child that writes a megabyte in one burst never causes a
/// megabyte allocation.
pub const MAX_CAPTURED_STDERR_BYTES: usize = 8 * 1024;

/// `MAX_CAPTURED_STDERR_LINES` (`server-manager.ts:65`). Only the **last three** non-empty lines
/// reach the user.
pub const MAX_CAPTURED_STDERR_LINES: usize = 3;

/// `boundedStderrChunk` (`server-manager.ts:96-112`), byte arm only.
///
/// Upstream has two arms because a Node `"data"` listener can hand back either a `Buffer` or a
/// `string`; a `tokio::process::ChildStderr` is a byte stream and only the `Buffer` arm is
/// reachable, which is also the arm that does not have to reason about chars-vs-bytes. The
/// string arm's whole purpose — *"limit string conversion before encoding"* — is satisfied here by
/// never decoding at all until [`stderr_tail_detail`].
#[must_use]
pub fn bounded_stderr_chunk(chunk: &[u8]) -> &[u8] {
    let start = chunk.len().saturating_sub(MAX_CAPTURED_STDERR_BYTES);
    chunk.get(start..).unwrap_or(chunk)
}

/// `appendStderrTail` (`server-manager.ts:114-122`) as an in-place ring.
///
/// Upstream allocates a fresh `Buffer` per chunk (`Buffer.concat` then a possible `subarray`); a
/// `VecDeque` gives the same last-N-bytes semantics with amortised O(1) appends and no reallocation
/// per stderr event, which matters because a chatty server emits one of these per line.
pub fn append_stderr_tail(tail: &mut VecDeque<u8>, chunk: &[u8]) {
    let bytes = bounded_stderr_chunk(chunk);
    if bytes.is_empty() {
        return;
    }
    tail.extend(bytes.iter().copied());
    let excess = tail.len().saturating_sub(MAX_CAPTURED_STDERR_BYTES);
    if excess > 0 {
        drop(tail.drain(..excess));
    }
}

/// The failure-message suffix built from a captured tail (`server-manager.ts:625-633`).
///
/// `tail.toString("utf8").trim()`, split on `/\r?\n/`, each line trimmed, empties dropped, the last
/// [`MAX_CAPTURED_STDERR_LINES`] joined by `" — "` (space, U+2014 EM DASH, space). `None` when
/// nothing survives, which is the arm that makes `debug: true` produce no `(...)` suffix at all —
/// in debug mode stderr is *inherited* by the host terminal and there is no tail to build from.
///
/// A multi-byte sequence split at the 8 KiB boundary becomes U+FFFD, exactly as Node's
/// `Buffer.toString("utf8")` does.
#[must_use]
pub fn stderr_tail_detail(tail: &VecDeque<u8>) -> Option<String> {
    if tail.is_empty() {
        return None;
    }
    let bytes: Vec<u8> = tail.iter().copied().collect();
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text
        .trim()
        .split('\n')
        .map(|line| line.trim_end_matches('\r').trim())
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let start = lines.len().saturating_sub(MAX_CAPTURED_STDERR_LINES);
    let detail = lines.get(start..).unwrap_or(&lines).join(" — ");
    Some(detail)
}

/// `` throw new Error(`${baseMessage} (${detail})`) `` — the enrichment applied to a stdio connect
/// failure when the child said something on the way down (`server-manager.ts:629-631`).
///
/// Returns `base` untouched when there is no tail, which is both the `debug: true` case and the
/// "child died silently" case.
#[must_use]
pub fn with_stderr_tail(base: &str, tail: &VecDeque<u8>) -> String {
    match stderr_tail_detail(tail) {
        Some(detail) => format!("{base} ({detail})"),
        None => base.to_string(),
    }
}

// -------------------------------------------------------------------------------------------------
// MCP-101 — the stdio transport (§3.3)
// -------------------------------------------------------------------------------------------------

/// Everything the stdio spawn needs, **already resolved**.
///
/// The resolution itself is deliberately not here — it is [`crate::secrets`], and
/// [`Self::resolve`] is the seam. `resolveEnv`'s full-environment copy and the `!`/`!!`
/// command-secret grammar are [`crate::secrets::resolve_stdio_env`] (MCP-083); `interpolateEnvVars`
/// over `args` and the npx/npm rewrite of `command`/`args` (MCP-103) stay with the caller, which is
/// why [`Self::resolve`] takes them already-built. Keeping the split here is what lets
/// [`spawn_stdio_transport`] stay a pure function of its inputs and be unit-tested without a shell.
pub struct StdioTransportSpec {
    /// Post-npx-resolution executable: `resolved.isJs ? "node" : resolved.binPath`, else
    /// `definition.command` untouched. Never interpolated — upstream interpolates `args`, not
    /// `command`.
    pub command: String,
    /// Post-npx-resolution, post-`interpolateEnvVars` arguments. Never `!command`-resolved.
    pub args: Vec<String>,
    /// `resolveEnv(...)`'s output: the **full** child environment, not a set of overrides.
    /// `StdioClientTransport`'s `env` option *replaces* the child environment, and `resolveEnv`
    /// copies all of `process.env` before layering, so [`spawn_stdio_transport`] clears and sets
    /// rather than merging — a `Command` that merely `.envs()`d would silently keep host variables
    /// a caller had deliberately dropped.
    pub env: HashMap<String, String>,
    /// `resolveConfigPath(definition.cwd) ?? this.defaultCwd`. `None` reproduces the *"key omitted
    /// entirely"* arm — the child inherits the parent's cwd.
    pub cwd: Option<PathBuf>,
    /// `definition.pluginDataDir`, `mkdir -p`'d **before** the spawn. Set only by
    /// [`crate::agent_plugin`]; it is `${PLUGIN_DATA}`, and a plugin whose first write creates it
    /// would race its own server.
    pub plugin_data_dir: Option<PathBuf>,
    /// `definition.debug`. `true` ⇒ stderr is **inherited** by the host terminal and there is no
    /// tail; `false` ⇒ piped, and the returned handle feeds [`append_stderr_tail`].
    pub debug: bool,
}

impl StdioTransportSpec {
    /// §3.3 step 7's `env` and the two fields read straight off the definition, resolved — the
    /// MCP-083 half of the stdio pre-flight.
    ///
    /// `command`, `args` and `cwd` arrive already-built because each belongs to a different unit:
    /// the npx/npm rewrite is MCP-103's, `interpolateEnvVars` over `args` is MCP-082's, and
    /// `resolveConfigPath(definition.cwd) ?? this.defaultCwd` is MCP-084's plus the manager's
    /// default. `env` is the one field that can execute a user's shell command, and it is the one
    /// this constructor owns.
    ///
    /// `base` is upstream's `process.env` — [`crate::secrets::process_env_snapshot`] in production,
    /// an explicit map in a test.
    ///
    /// # Errors
    ///
    /// The first `env` value whose `!command` fails, carrying
    /// `` MCP server "{server}" stdio env "{key}" `` — see [`crate::secrets::resolve_env`].
    pub fn resolve(
        entry: &ServerEntry,
        server_name: &str,
        command: String,
        args: Vec<String>,
        cwd: Option<PathBuf>,
        base: &HashMap<String, String>,
    ) -> McpResult<Self> {
        Ok(Self {
            command,
            args,
            env: crate::secrets::resolve_stdio_env(entry, server_name, base)?,
            cwd,
            // `if (definition.pluginDataDir) mkdirSync(...)` (`server-manager.ts:488`) — used raw,
            // NOT `resolveConfigPath`'d, because only `agent_plugin` writes it and it writes an
            // absolute path it built itself.
            plugin_data_dir: entry.plugin_data_dir.as_deref().map(PathBuf::from),
            debug: entry.debug == Some(true),
        })
    }
}

/// Spawn the child and hand back rmcp's stdio transport plus, in non-debug mode, the stderr handle.
///
/// The rmcp fit is exact and worth stating precisely, because it inverts the obvious reading of
/// upstream: `TokioChildProcessBuilder`'s **default** is `Stdio::inherit()` — upstream's
/// `debug: true` — so the port calls `.stderr(Stdio::piped())` on the `debug: false` arm rather than
/// the other way round. `spawn()` then returns `Option<ChildStderr>`, `Some` exactly when piped.
///
/// Ordering follows §3.3 step 5 → 7: the plugin data directory is created *before* the process
/// exists. Step 4's `throwIfAborted(signal)` sits one level up in the connection builder, between
/// npx resolution and this call.
///
/// # Errors
///
/// [`McpError::Io`] carrying the offending path: the plugin data directory when `mkdir -p` fails,
/// the command when the spawn does. A failed spawn is the common case — a mistyped `command` — and
/// the path is what makes it actionable.
pub fn spawn_stdio_transport(
    spec: &StdioTransportSpec,
) -> McpResult<(TokioChildProcess, Option<ChildStderr>)> {
    // §3.3 step 5. `mkdirSync(definition.pluginDataDir, { recursive: true })`.
    if let Some(dir) = spec.plugin_data_dir.as_ref() {
        std::fs::create_dir_all(dir).map_err(|source| McpError::Io {
            path: dir.clone(),
            source,
        })?;
    }

    let mut builder = TokioChildProcess::builder(tokio::process::Command::new(&spec.command)
        .configure(|command| {
            command.args(&spec.args);
            // The replace-not-merge semantics of `StdioClientTransport`'s `env` option. See
            // `StdioTransportSpec::env`.
            command.env_clear();
            command.envs(&spec.env);
            // §3.3 step 6: the key is *omitted entirely* when neither the definition nor the
            // manager supplies one, so `None` must not become `current_dir(".")`.
            if let Some(cwd) = spec.cwd.as_ref() {
                command.current_dir(cwd);
            }
        }));

    if !spec.debug {
        builder = builder.stderr(Stdio::piped());
    }

    builder.spawn().map_err(|source| McpError::Io {
        path: PathBuf::from(&spec.command),
        source,
    })
}

// -------------------------------------------------------------------------------------------------
// MCP-109 — the streamable HTTP transport (§3.4)
// -------------------------------------------------------------------------------------------------

/// Everything the HTTP transport needs, **already resolved** (§3.4 steps 1–7).
///
/// Steps 2–6 — `resolveCommandSecretsRecord` over the headers, the `bearerToken`/`bearerTokenEnv`
/// ladder and the `new Headers()` injection guard — are [`crate::secrets::resolve_http_secrets`]
/// (MCP-083), and [`Self::resolve`] is the seam onto it. Step 1, `resolveServerUrl`'s three throws,
/// is still MCP-084's and is why `url` arrives as a parameter. What reaches
/// [`build_http_transport_config`] is the finished pre-flight.
pub struct HttpTransportSpec {
    /// The `mcpServers` key, for the error strings.
    pub server: String,
    /// `resolveServerUrl(definition)` — interpolated and already proven to parse.
    pub url: String,
    /// The resolved header set, as an ordered `Vec` rather than a map so the transport cannot
    /// reorder it again. **Named delta:** upstream's order is `Object.entries(definition.headers)`,
    /// i.e. the order the keys were written in `mcp.json`, while [`ServerEntry::headers`] is a
    /// `BTreeMap` and hands them over alphabetically. Header *semantics* do not depend on order,
    /// so the only observable consequence is the order they appear on the wire.
    /// `Authorization` is *not* in here when a bearer token resolved — see [`Self::bearer_token`].
    pub headers: Vec<(String, String)>,
    /// The bearer token **without** the `Bearer ` prefix.
    ///
    /// Upstream writes `headers["Authorization"] = \`Bearer ${token}\``; rmcp owns that header
    /// through `auth_header` and applies it with `RequestBuilder::bearer_auth`, which produces the
    /// byte-identical header. Routing it through the typed field rather than the custom-header map
    /// is what keeps the SSE `GET` stream and the session `DELETE` authorized too — both are issued
    /// by the transport worker, not by the caller, and only `auth_header` reaches them.
    pub bearer_token: Option<String>,
}

impl HttpTransportSpec {
    /// §3.4 steps 2–6 applied to a definition — the MCP-083 half of the HTTP pre-flight.
    ///
    /// `url` arrives already-resolved: step 1 is `resolveServerUrl`, whose three throws are MCP-084's
    /// and which must run **before** this, since a URL with a missing environment variable has to
    /// fail before a secret command is ever spawned for it.
    ///
    /// # Errors
    ///
    /// A header or bearer `!command` that failed — carrying
    /// `` MCP server "{server}" HTTP header "{key}" `` or `` MCP server "{server}" HTTP bearer
    /// token `` — or the injection guard's invalid-header-value sentence. See
    /// [`crate::secrets::resolve_http_secrets`].
    pub fn resolve(
        entry: &ServerEntry,
        server_name: &str,
        url: String,
        env: &crate::credentials::EnvFn,
    ) -> McpResult<Self> {
        let resolved = crate::secrets::resolve_http_secrets(entry, server_name, env)?;
        Ok(Self {
            server: server_name.to_string(),
            url,
            headers: resolved.headers,
            bearer_token: resolved.bearer_token,
        })
    }
}

/// Build the streamable-HTTP transport's configuration
/// (`connectHttpClient`'s `new StreamableHTTPClientTransport(url, transportOptions)`).
///
/// # Two rmcp defaults, one of which is wrong for this port
///
/// * **`reinit_on_expired_session` defaults to `true`.** It performs one silent `initialize` replay
///   when the server 404s an expired session. The port turns it **off** and keeps it off: upstream
///   surfaces `isTerminatedSession` to `withSessionRecovery` (MCP-134/135), which re-runs discovery
///   and re-registers metadata. A transparent transport-level reinit would produce a live session
///   whose tool list the adapter never refreshed — the tools would keep resolving against a session
///   that no longer knows them.
/// * **`allow_stateless` defaults to `true`**, which *is* the upstream-equivalent value — named
///   only so the next reader does not "fix" it.
///
/// # Why this returns a config rather than a transport
///
/// `transportOptions.authProvider` has no field here: rmcp models OAuth by **wrapping the HTTP
/// client**, so the authorized transport is
/// `StreamableHttpClientTransport::with_client(AuthClient::new(client, manager), config)` — a
/// *different concrete type* from the unauthorized `StreamableHttpClientTransport<reqwest::Client>`.
/// Returning the config keeps both arms of MCP-115's ladder expressible from one builder, and keeps
/// this function free of a `reqwest` type name (`reqwest` is in the lock file transitively but is
/// not a declared dependency of this crate — see the crate's dependency note). Hand the result to
/// [`http_transport_with_client`], or to `StreamableHttpClientTransport::from_config` once `reqwest`
/// is declared: `from_config` applies rmcp's tuned client (idle pooling off to dodge the ~40 ms
/// Linux delayed-ACK stall, redirects off so custom headers cannot be replayed to a redirect
/// target), and hand-rolling `reqwest::Client::new()` silently loses both.
///
/// # Errors
///
/// [`McpError::Server`] when a header name or value is not representable on the wire.
/// [`crate::secrets::resolve_http_secrets`] has already validated the command-sourced ones with the
/// exact upstream sentence (MCP-083, §3.4 step 6); this arm therefore only fires for a *statically
/// configured* header that upstream would have let through to `fetch` and failed on later — a
/// **recorded divergence**: cyrup rejects it at transport construction, with a message that does not
/// falsely blame a command.
pub fn build_http_transport_config(
    spec: &HttpTransportSpec,
) -> McpResult<StreamableHttpClientTransportConfig> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(spec.url.clone());
    config.reinit_on_expired_session = false;

    for (name, value) in &spec.headers {
        // `custom_headers` is `HashMap<http::HeaderName, http::HeaderValue>`. `http` is not a
        // declared dependency of this crate, so both types are reached through inference from the
        // map's own type rather than named. `HeaderName`/`HeaderValue`'s `TryFrom<&str>` impls are
        // what reject CR/LF and control bytes — the same rejection `new Headers()` performs
        // upstream, which is the header-injection guard of §3.4 step 6.
        let header_name = match TryFrom::try_from(name.as_str()) {
            Ok(header_name) => header_name,
            Err(_) => return Err(invalid_header(&spec.server, name)),
        };
        let header_value = match TryFrom::try_from(value.as_str()) {
            Ok(header_value) => header_value,
            Err(_) => return Err(invalid_header(&spec.server, name)),
        };
        let _ = config.custom_headers.insert(header_name, header_value);
    }

    if let Some(token) = spec.bearer_token.as_ref() {
        config.auth_header = Some(token.clone());
    }

    Ok(config)
}

/// `new StreamableHTTPClientTransport(url, options)` for a caller-supplied HTTP client.
///
/// Generic over the client so the unauthorized arm (`reqwest::Client`) and MCP-115's authorized arm
/// (`AuthClient<reqwest::Client>`) share one construction path, and so this module needs no HTTP
/// client dependency of its own. [`connect_client`] accepts either without change.
#[must_use]
pub fn http_transport_with_client<C: StreamableHttpClient>(
    client: C,
    config: StreamableHttpClientTransportConfig,
) -> StreamableHttpClientTransport<C> {
    StreamableHttpClientTransport::with_client(client, config)
}

/// `(connection.transport as {sessionId?: string})?.sessionId != null` — `session-recovery.ts:59-65`,
/// as a [`StreamableHttpClient`] decorator.
///
/// # Why a decorator and not a field read
///
/// Upstream reads the transport: `get sessionId() { return this._sessionId; }` in the pinned SDK.
/// rmcp keeps the session id in a **local of the transport worker's `run` loop**
/// (`rmcp-3.1.4/src/transport/streamable_http_client.rs:893`) and `StreamableHttpClientTransport`
/// exposes no accessor at all, so there is no field to read. What *is* observable is the response
/// the session id arrives on: `post_message` returns
/// [`StreamableHttpPostResponse::Json`]/[`StreamableHttpPostResponse::Sse`] carrying the
/// `Mcp-Session-Id` the server sent, and the client is a seam this crate already occupies
/// ([`crate::request_headers_command::RequestHeadersCommandClient`] sits in exactly this position).
/// So the flag is set from the wire, once, by whichever response first carries a session id — which
/// for a stateful server is the `initialize` response, the same message upstream's `_sessionId` is
/// assigned from.
///
/// # What this buys, stated as the bug it fixes
///
/// The field used to be a hardcoded `true` under a comment quoting upstream's live read. A
/// streamable-HTTP server that returns **no** `Mcp-Session-Id` is legal — rmcp's `allow_stateless`
/// default is `true` and this port leaves it there — and upstream reads `undefined` for it, so
/// `shouldReconnectAfterRefresh`'s first line (`if (!hadSessionId) return false`) short-circuits.
/// With the constant, `lifecycle.rs:1090`'s `had_session_id` was always `true`, so a plain 404 from
/// a stateless server was classified as a terminated session on every health tick and drove a
/// reconnect-plus-rediscovery cycle upstream never performs.
#[derive(Clone)]
pub struct SessionIdProbe<C> {
    inner: C,
    seen: Arc<std::sync::atomic::AtomicBool>,
}

impl<C> SessionIdProbe<C> {
    /// Wrap `inner`, sharing one flag with every clone the transport makes of it.
    pub fn new(inner: C) -> Self {
        Self {
            inner,
            seen: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// The flag this probe writes. Cloned out **before** the client is handed to the transport,
    /// because the transport takes the client by value.
    #[must_use]
    pub fn flag(&self) -> Arc<std::sync::atomic::AtomicBool> {
        Arc::clone(&self.seen)
    }

    fn record(&self, response: &StreamableHttpPostResponse) {
        let carried = match response {
            StreamableHttpPostResponse::Json(_, session_id)
            | StreamableHttpPostResponse::Sse(_, session_id) => session_id.is_some(),
            // `Accepted` is a 202/204 with no headers worth reading, and any future variant is
            // likewise not a session-id carrier until this match is revisited (the enum is
            // `#[non_exhaustive]`).
            _ => false,
        };
        if carried {
            // `Relaxed` is enough: the flag is read once, after the handshake future has been
            // awaited to completion, and that await is the happens-before edge.
            self.seen
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

impl<C> StreamableHttpClient for SessionIdProbe<C>
where
    C: StreamableHttpClient + Sync,
{
    type Error = C::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let response = self
            .inner
            .post_message(uri, message, session_id, auth_header, custom_headers)
            .await?;
        self.record(&response);
        Ok(response)
    }

    /// Overridden rather than left to the trait default: the default would delegate to
    /// [`Self::post_message`] and drop the transport-wide SSE event-size limit the inner client
    /// enforces. Same reason for the `get_stream` pair below.
    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let response = self
            .inner
            .post_message_with_max_sse_event_size(
                uri,
                message,
                session_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await?;
        self.record(&response);
        Ok(response)
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        self.inner
            .get_stream(uri, session_id, last_event_id, auth_header, custom_headers)
            .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        self.inner
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        self.inner
            .delete_session(uri, session_id, auth_header, custom_headers)
            .await
    }
}

/// `DEFAULT_MAX_SSE_EVENT_SIZE` (`rmcp-3.1.4/src/transport/common/client_side_sse.rs:18`), which
/// rmcp keeps `pub(crate)`. Only [`UnauthorizedProbe::post_message`] needs it — the transport always
/// calls the `_with_max_sse_event_size` form
/// (`rmcp-3.1.4/src/transport/streamable_http_client.rs:773`, `:804`, `:867`, `:934`) — but
/// restating it there rather than delegating is what stops a direct caller of `post_message` from
/// bypassing the 401 classification.
const DEFAULT_MAX_SSE_EVENT_SIZE: usize = 16 * 1024 * 1024;

/// `validate_custom_header` (`rmcp-3.1.4/src/transport/common/http_header.rs:31-45`), inverted.
///
/// `MCP-Protocol-Version` is in rmcp's `RESERVED_HEADERS` but is explicitly allowed through (the
/// transport worker injects it post-init), so it is simply absent from this list. The SEP-2243
/// `Mcp-Method` / `Mcp-Name` / `Mcp-Param-*` headers are not reserved either and must keep passing:
/// `request_version_headers` puts them on the modern startup POST.
///
/// This rejection is what
/// [`request_headers_command.rs:63-67`](crate::request_headers_command) documents a derived header
/// hitting, so it must not weaken.
fn is_reserved_header(name: &HeaderName) -> bool {
    const RESERVED: [&str; 3] = ["accept", HEADER_SESSION_ID, HEADER_LAST_EVENT_ID];
    RESERVED
        .iter()
        .any(|reserved| name.as_str().eq_ignore_ascii_case(reserved))
}

/// `extract_scope_from_header` (`rmcp-3.1.4/src/transport/common/http_header.rs:50-73`), which rmcp
/// keeps `pub(crate)`. Rewritten slice-free because `clippy::indexing_slicing` is `deny` here;
/// behaviour is identical, including "an unterminated quoted value yields `None`" and "an empty
/// unquoted value yields `None`".
///
/// Byte offsets from the lowercased copy index the original safely: `to_ascii_lowercase` preserves
/// byte length, and `str::get` returns `None` rather than panicking off a char boundary.
fn scope_from_challenge(header: &str) -> Option<String> {
    const SCOPE_KEY: &str = "scope=";
    let start = header.to_ascii_lowercase().find(SCOPE_KEY)? + SCOPE_KEY.len();
    let value = header.get(start..)?;
    match value.strip_prefix('"') {
        Some(quoted) => {
            let end = quoted.find('"')?;
            quoted.get(..end).map(str::to_string)
        }
        None => {
            let end = value
                .find(|c: char| c == ',' || c == ';' || c.is_whitespace())
                .unwrap_or(value.len());
            let scope = value.get(..end)?;
            (!scope.is_empty()).then(|| scope.to_string())
        }
    }
}

/// The two requests that can be a client's *startup* message, and therefore the only two whose 401
/// can surface as a [`ClientInitializeError`].
///
/// `InitializeRequest` is [`ClientLifecycleMode::Initialize`]'s;
/// `DiscoverRequest` is `Discover`'s and `Auto`'s (`rmcp-3.1.4/src/service/client.rs:943-954`), both
/// of which [`version_negotiation`] reaches from `protocolVersion: "2026-07-28"` and `"auto"`.
/// Matching only `InitializeRequest` would leave this defect live for those two configurations.
fn is_handshake_request(message: &ClientJsonRpcMessage) -> bool {
    matches!(
        message,
        JsonRpcMessage::Request(request)
            if matches!(
                request.request,
                ClientRequest::InitializeRequest(_) | ClientRequest::DiscoverRequest(_)
            )
    )
}

/// `bounded_sse_stream` (`rmcp-3.1.4/src/transport/common/client_side_sse.rs:144-155`), which rmcp
/// keeps `pub(crate)` along with its `SseEventSizeLimiter`.
///
/// NAMED DELTA: rmcp caps each SSE **event**; this caps the **total** bytes of the handshake
/// response, i.e. strictly stricter. That is sound only because it is reached only for a handshake
/// POST, whose stream `expect_initialized`
/// (`rmcp-3.1.4/src/transport/streamable_http_client.rs:264-283`) drains to the first `Response`
/// message and then DROPS. Every other POST keeps rmcp's per-event limiter because every other POST
/// is delegated.
///
/// `std::io::Error` rather than a bespoke enum: `SseStream::from_bytes_stream` needs only
/// `E: std::error::Error` (`sse-stream-0.2.5/src/stream.rs:36-56`), `reqwest::Error` cannot be
/// constructed, and `io::Error` carries both arms without adding a type.
fn capped_sse_stream(
    response: reqwest::Response,
    max_sse_event_size: usize,
) -> BoxStream<'static, Result<Sse, SseError>> {
    let mut seen = 0_usize;
    let capped = response.bytes_stream().map(move |chunk| {
        let chunk = chunk.map_err(std::io::Error::other)?;
        seen = seen.saturating_add(chunk.len());
        if seen > max_sse_event_size {
            return Err(std::io::Error::other(format!(
                "handshake SSE response exceeded the maximum size of {max_sse_event_size} bytes"
            )));
        }
        Ok(chunk)
    });
    SseStream::from_bytes_stream(capped).boxed()
}

/// `error.status === 401` — the one bit rmcp's reqwest client throws away.
///
/// # Why this OWNS the POST instead of decorating it
///
/// [`SessionIdProbe`] and [`crate::request_headers_command::RequestHeadersCommandClient`] both sit
/// ABOVE `impl StreamableHttpClient for reqwest::Client` and delegate the send to it
/// (`runtime.rs:1017-1022`, `request_headers_command.rs:944-946`). What comes back to them is a
/// [`StreamableHttpPostResponse`] — a `ServerJsonRpcMessage`, a stream, and an `Option<String>`
/// session id, and NOTHING about the HTTP status. rmcp reads the status into a local at
/// `rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:243` and that local dies with
/// the frame, so the status cannot be decorated out of rmcp: it can only be kept by a client that
/// performs the POST itself.
///
/// # Why only the handshake
///
/// [`unauthorized_challenge`] has exactly one caller (`runtime.rs:2672`) and it takes a
/// [`ClientInitializeError`], so the startup POST is the only request whose 401 can start the OAuth
/// ladder. Every other POST is delegated to rmcp verbatim — same SSE limiter, same 202 handling,
/// same session-expiry — which keeps the blast radius at the handshake. The SSE cap in
/// [`capped_sse_stream`] is total-bytes rather than rmcp's per-event, and that is safe ONLY because
/// a handshake stream is drained to its first `Response` and dropped; a `tools/call` result stream
/// is not, which is the second reason non-handshake POSTs must stay delegated.
///
/// # Why `AuthRequired` and not a new shape
///
/// [`StreamableHttpError::AuthRequired`] is the currency the consumers already read, so nothing
/// downstream changes: [`unauthorized_challenge`] downcasts it at `runtime.rs:2021-2025` and returns
/// `Some(&required.www_authenticate_header)` — `Some("")` for a bare 401, which is exactly what
/// [`crate::oauth::on_unauthorized`] (`oauth.rs:3949-3964`) already expects. `AuthRequiredError::new`
/// is public (`rmcp-3.1.4/src/transport/streamable_http_client.rs:135-142`) and the variant carries
/// it as `#[source]` (`:203`), so the `source()` walk at `runtime.rs:2019-2030` finds it. rmcp's own
/// `ClientInitializeError::auth_challenge` (`…/service/client.rs:109-132`) reads the same type, and
/// so will `AuthClient` when section 05 lands it.
#[derive(Debug, Clone)]
pub struct UnauthorizedProbe {
    inner: reqwest::Client,
}

impl UnauthorizedProbe {
    /// Wrap the tuned client [`build_http_client`] produces.
    #[must_use]
    pub fn new(inner: reqwest::Client) -> Self {
        Self { inner }
    }

    /// A faithful port of `rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:190-324`
    /// with ONE behavioural difference — the 401 arm below — and two arms that cannot fire here:
    /// one kept anyway, one omitted, each stated at the point it sits.
    async fn post_handshake(
        &self,
        uri: &str,
        message: &ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<reqwest::Error>> {
        // Byte-for-byte the request rmcp would have sent (`:196-211`), in rmcp's order: ACCEPT, the
        // separate bearer channel, the custom headers under the same reserved-header rejection, the
        // session header, then `serde_json` of the message.
        let mut request = self.inner.post(uri).header(
            reqwest::header::ACCEPT,
            [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "),
        );
        if let Some(auth_header) = auth_header {
            request = request.bearer_auth(auth_header);
        }
        for (name, value) in custom_headers {
            if is_reserved_header(&name) {
                return Err(StreamableHttpError::ReservedHeaderConflict(name.to_string()));
            }
            request = request.header(name, value);
        }
        let session_was_attached = session_id.is_some();
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        }
        let response = request.json(message).send().await?;

        // ── THE FIX ──────────────────────────────────────────────────────────────────────────────
        // The status is read BEFORE the body, and 401 wins over the JSON-RPC shortcut whether or not
        // a challenge came with it. rmcp gates its own 401 arm on the header being present
        // (`:212-213`), which is why a bare 401 with a JSON body falls through to `:289`. Upstream
        // reads the status first and unconditionally
        // (`@modelcontextprotocol/client/dist/index.mjs:5333-5334`), and confines its own JSON-RPC
        // error passthrough to status 400 (`:5374-5381`).
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            let challenge = response
                .headers()
                .get(http::header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            return Err(StreamableHttpError::AuthRequired(AuthRequiredError::new(
                challenge,
            )));
        }
        // ── everything below mirrors rmcp `:227-324` ─────────────────────────────────────────────
        // 403 is NOT widened: `InsufficientScope` is reproduced exactly so a scope denial keeps
        // rmcp's vocabulary and stays a hard error, which is what `unauthorized_challenge`'s
        // `AuthRequiredError`-only downcast already enforces.
        if status == reqwest::StatusCode::FORBIDDEN
            && let Some(header) = response.headers().get(http::header::WWW_AUTHENTICATE)
        {
            let Ok(header) = header.to_str() else {
                return Err(StreamableHttpError::UnexpectedServerResponse(Cow::from(
                    "invalid www-authenticate header value",
                )));
            };
            return Err(StreamableHttpError::InsufficientScope(
                InsufficientScopeError::new(header.to_string(), scope_from_challenge(header)),
            ));
        }
        if matches!(
            status,
            reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        // UNREACHABLE ARM 1 of 2 — rmcp's `404 => SessionExpired` (`:250-252`) is KEPT, but it can
        // never fire here: the worker posts both startup requests with `session_id: None`
        // (`…/streamable_http_client.rs:870`, `:776`). Kept anyway, because it costs three lines and
        // an rmcp change that starts attaching one must not silently change meaning.
        if status == reqwest::StatusCode::NOT_FOUND && session_was_attached {
            return Err(StreamableHttpError::SessionExpired);
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
        let session_id = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let is_json = content_type
            .as_deref()
            .is_some_and(|ct| ct.starts_with(JSON_MIME_TYPE));

        // UNREACHABLE ARM 2 of 2 — rmcp's empty-success => Accepted (`:265-277`) requires the OUTGOING
        // message to be a Notification/Response/Error. This method only runs for a Request, so the
        // arm is unreachable by construction and is omitted rather than written-and-dead.

        if !status.is_success() {
            // Unchanged from rmcp `:278-299`, and deliberately so: a 400 carrying
            // `UNSUPPORTED_PROTOCOL_VERSION` is how `Discover` renegotiates
            // (`…/service/client.rs:980-981`). Only 401 was taken above.
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_owned());
            if is_json
                && let Ok(parsed) = serde_json::from_str::<ServerJsonRpcMessage>(&body)
                && matches!(parsed, JsonRpcMessage::Error(_))
            {
                return Ok(StreamableHttpPostResponse::Json(parsed, session_id));
            }
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("HTTP {status}: {body}"),
            )));
        }
        if content_type
            .as_deref()
            .is_some_and(|ct| ct.starts_with(EVENT_STREAM_MIME_TYPE))
        {
            return Ok(StreamableHttpPostResponse::Sse(
                capped_sse_stream(response, max_sse_event_size),
                session_id,
            ));
        }
        if is_json {
            // Same tolerance as rmcp `:308-318`: a body that is not a `ServerJsonRpcMessage` is
            // treated as an accept rather than a failure.
            return Ok(match response.json::<ServerJsonRpcMessage>().await {
                Ok(message) => StreamableHttpPostResponse::Json(message, session_id),
                Err(_) => StreamableHttpPostResponse::Accepted,
            });
        }
        Err(StreamableHttpError::UnexpectedContentType(content_type))
    }
}

impl StreamableHttpClient for UnauthorizedProbe {
    type Error = reqwest::Error;

    /// Routed through [`Self::post_message_with_max_sse_event_size`] rather than delegated to
    /// `self.inner`, so a caller that reaches for the non-`_with_max` form cannot bypass the 401
    /// classification. rmcp's transport never calls this one — see [`DEFAULT_MAX_SSE_EVENT_SIZE`].
    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        self.post_message_with_max_sse_event_size(
            uri,
            message,
            session_id,
            auth_header,
            custom_headers,
            DEFAULT_MAX_SSE_EVENT_SIZE,
        )
        .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        if is_handshake_request(&message) {
            return self
                .post_handshake(
                    &uri,
                    &message,
                    session_id,
                    auth_header,
                    custom_headers,
                    max_sse_event_size,
                )
                .await;
        }
        self.inner
            .post_message_with_max_sse_event_size(
                uri,
                message,
                session_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        self.inner
            .get_stream(uri, session_id, last_event_id, auth_header, custom_headers)
            .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        self.inner
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        self.inner
            .delete_session(uri, session_id, auth_header, custom_headers)
            .await
    }
}

fn invalid_header(server: &str, name: &str) -> McpError {
    McpError::Server {
        server: server.to_string(),
        message: format!("HTTP header \"{name}\" is not a valid header name/value pair"),
    }
}

// -------------------------------------------------------------------------------------------------
// MCP-117 — protocol-revision negotiation (§3.6)
// -------------------------------------------------------------------------------------------------

/// `resolveVersionNegotiation(definition)` (`server-manager.ts:83-95`) as an rmcp lifecycle mode.
///
/// | `protocolVersion` | upstream | here |
/// |---|---|---|
/// | absent / `"legacy"` | omit `versionNegotiation` entirely | [`ClientLifecycleMode::Initialize`] |
/// | `"auto"` | `{ mode: "auto" }` | [`ClientLifecycleMode::Auto`] |
/// | `"2026-07-28"` | `{ mode: { pin: "2026-07-28" } }` | [`ClientLifecycleMode::Discover`] |
///
/// | anything else | `` throw `Invalid MCP protocolVersion: ${String(…)}` `` | [`McpError::Config`] with that sentence |
///
/// # The fourth arm is reachable, and it is reachable *here*
///
/// It was not, and that was a divergence: [`ProtocolVersionSetting`] was a closed three-variant
/// enum behind `deserialize_with = "lenient"`, so `"2025-06-18"` was discarded at parse time. The
/// value never reached the digest (see [`ProtocolVersionSetting`]) and never reached this `match`
/// either, so a server pinning a real revision this build does not implement negotiated as
/// `legacy` in silence instead of failing to connect.
///
/// [`ProtocolVersionSetting::Other`] restores upstream's shape exactly: the deserialiser validates
/// nothing, `computeServerHash` hashes the value verbatim, and the throw happens **at connect**,
/// which is the one place `resolveVersionNegotiation` (`server-manager.ts:82-95`) performs it. That
/// is why this function is fallible; [`invalid_protocol_version_message`] is the sentence it
/// raises, and `String(definition.protocolVersion)` is [`ProtocolVersionSetting::as_js_string`].
///
/// # Errors
///
/// The entry pinned a `protocolVersion` that is not `"legacy"`, `"auto"` or `"2026-07-28"`.
///
/// # The named delta, and it is the sharpest one in this section
///
/// Upstream runs `server/discover` on a **disposable sibling process**: `mcp-trace.ts`'s
/// `wrapTransportWithMcpTrace` composes callbacks in place rather than returning a wrapper object
/// *precisely* so the SDK can still detect the base stdio transport and clone it for the probe.
/// rmcp does not do this — `serve_client_with_lifecycle` runs `discover_startup` (and, on `Auto`,
/// the legacy fallback) on the **same** `&mut transport` — and rmcp returns `DiscoverOutcome::Legacy`
/// only when the probe produced a complete, correlated JSON-RPC error, "i.e. the transport is in a
/// known-good state". A legacy stdio server that *exits* on `server/discover` therefore fails under
/// `Auto` where pi burned a sibling and connected anyway. Upstream ships a fixture for exactly this
/// case (`__tests__/fixtures/legacy-exits-on-discover-server.mjs`). `Auto` is also bounded by rmcp's
/// `DEFAULT_AUTO_DISCOVER_TIMEOUT` of 10 s, after which it falls back to legacy on the same
/// transport. `"legacy"` — the default — is unaffected: it never sends `server/discover` at all.
pub fn version_negotiation(entry: &ServerEntry) -> McpResult<ClientLifecycleMode> {
    Ok(match entry.protocol_version {
        // Byte-identical arms: `undefined` and `"legacy"` both send a plain `initialize`.
        None | Some(ProtocolVersionSetting::Legacy) => ClientLifecycleMode::Initialize,
        Some(ProtocolVersionSetting::Auto) => ClientLifecycleMode::Auto {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            // The version the fallback `initialize` announces. `LATEST` is what
            // `ClientLifecycleMode::Initialize` would have sent, so the fallback is byte-identical
            // to the `"legacy"` arm — which is what `{ mode: "auto" }` promises.
            legacy_version: Some(ProtocolVersion::LATEST),
        },
        Some(ProtocolVersionSetting::V20260728) => ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
        // `default:` — upstream's throw, at the moment upstream throws it.
        Some(ProtocolVersionSetting::Other(_)) => {
            return Err(McpError::Config(invalid_protocol_version_message(
                &entry
                    .protocol_version
                    .as_ref()
                    .map(ProtocolVersionSetting::as_js_string)
                    .unwrap_or_default(),
            )));
        }
    })
}

/// `` `Invalid MCP protocolVersion: ${String(definition.protocolVersion)}` `` — upstream's fourth
/// arm, verbatim.
///
/// Raised by [`version_negotiation`], which is where `resolveVersionNegotiation` raises it: at
/// **connect**, never at parse. `config.ts` does not validate `protocolVersion` at all, and
/// `computeServerHash` has already hashed the value verbatim by the time anything looks at it.
#[must_use]
pub fn invalid_protocol_version_message(raw: &str) -> String {
    format!("Invalid MCP protocolVersion: {raw}")
}

// -------------------------------------------------------------------------------------------------
// MCP-118 — client identity and capability advertisement (§3.5)
// -------------------------------------------------------------------------------------------------

/// The client-name prefix the **server sees**. Upstream sends `pi-mcp-<server>`; this is a
/// **recorded rename** — any MCP server that allow-lists the pi client name will not recognise
/// cyrup, and that is a deliberate, visible consequence rather than an oversight.
pub const CLIENT_NAME_PREFIX: &str = "cyrup-mcp-";

/// `{ name, version: "1.0.0" }` — the version half of the client identity, unchanged from upstream.
/// It is protocol identity, not the crate version: bumping cyrup must not change what a server sees.
pub const CLIENT_VERSION: &str = "1.0.0";

/// `buildClientCapabilities()` (`server-manager.ts:678-690`).
///
/// * `sampling: {}` **iff** a sampling handler is wired.
/// * `elicitation: { form: {} }` iff an elicitation handler is wired, plus `url: {}` iff
///   `allowUrl` — which `init.ts` sets from `mode === "tui"`, *not* from `hasUI`.
///
/// Servers gate features on this, so the on/off combinations are behaviour, not decoration.
///
/// # Recorded divergence: the empty case
///
/// Upstream omits the `capabilities` **key entirely** when both are absent. rmcp's
/// `InitializeRequestParams::capabilities` is not an `Option`, so the port sends `"capabilities":{}`.
/// Every field of [`ClientCapabilities`] is `skip_serializing_if`, so the object is empty and
/// semantically identical for any conforming server (the MCP schema requires the key), but the
/// frames are not byte-identical and a fixture asserting "key absent" cannot pass. There is no way
/// to express the omission through rmcp's typed handshake.
#[must_use]
#[allow(deprecated)]
pub fn build_client_capabilities(sampling: bool, elicitation: Option<ElicitationMode>) -> ClientCapabilities {
    // `ClientCapabilities` is `#[non_exhaustive]` and its `builder()` is generated behind the
    // `server`/`macros` features this crate does not enable — `Default` + field assignment is the
    // only construction route. Same for the two capability structs.
    let mut capabilities = ClientCapabilities::default();
    if sampling {
        capabilities.sampling = Some(SamplingCapability::default());
    }
    if let Some(mode) = elicitation {
        // `form: {}` is unconditional; `url: {}` rides on `allowUrl`.
        let mut elicit = ElicitationCapability::new().with_form(FormElicitationCapability::new());
        if mode.allow_url {
            elicit = elicit.with_url(UrlElicitationCapability::new());
        }
        capabilities.elicitation = Some(elicit);
    }
    capabilities
}

/// Whether an elicitation handler is wired, and whether it may open URLs.
///
/// `allow_url` is [`ContextSnapshot::is_tui_mode`] — `hasUI && mode === "tui"` — and is *stricter*
/// than `has_ui`: a print-mode session with a UI still refuses URL elicitation.
#[derive(Debug, Clone, Copy)]
pub struct ElicitationMode {
    /// `elicitationConfig.allowUrl`.
    pub allow_url: bool,
}

/// `new Client({ name: \`pi-mcp-${serverName}\`, version: "1.0.0" }, { capabilities })`
/// (`server-manager.ts:692-720`) — the `initialize` frame's client half.
#[must_use]
pub fn client_info(server: &str, capabilities: ClientCapabilities) -> ClientInfo {
    ClientInfo::new(
        capabilities,
        Implementation::new(format!("{CLIENT_NAME_PREFIX}{server}"), CLIENT_VERSION),
    )
}

// -------------------------------------------------------------------------------------------------
// MCP-120 / MCP-121 / MCP-122 — `ClientHandler`: what the server says unprompted (§3.10)
// -------------------------------------------------------------------------------------------------

/// The adapter-private notification method of **Cut 2** (`ui-stream-types.ts:6`).
///
/// MCP Apps / the UI extension are cut wholesale, so nothing handles this. It is named here for one
/// reason: a server that still emits it must land in [`ClientHandler::on_custom_notification`] and be
/// *dropped* — not logged above `debug`, and above all not treated as a protocol error, because an
/// unhandled notification that closed the connection would take a working server down mid-session.
pub const STREAM_RESULT_PATCH_METHOD: &str = "notifications/pi-mcp-adapter/result-patch";

/// `notifications/elicitation/complete` — the server telling us a browser interaction finished
/// (`server-manager.ts:731-740`).
///
/// rmcp models no first-class variant for it, so it arrives at
/// [`ClientHandler::on_custom_notification`] alongside the Cut-2 method above.
pub const ELICITATION_COMPLETE_METHOD: &str = "notifications/elicitation/complete";

/// Which list a `list_changed` notification is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    /// `notifications/tools/list_changed`.
    Tools,
    /// `notifications/prompts/list_changed`. The only kind that additionally clears
    /// `promptDiscoveryFailed`.
    Prompts,
    /// `notifications/resources/list_changed`.
    Resources,
}

impl ListKind {
    /// The reason string handed to `metadataListChangedListener`. **Byte-exact** — the metadata
    /// layer switches on it.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::Tools => "tools-list-changed",
            Self::Prompts => "prompts-list-changed",
            Self::Resources => "resources-list-changed",
        }
    }

    /// The `<kind>` in `` `MCP: <kind>/list_changed refresh failed for ${serverName}: ${message}` ``.
    #[must_use]
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Tools => "tools",
            Self::Prompts => "prompts",
            Self::Resources => "resources",
        }
    }

    /// The debug line emitted when the refresh itself fails (`server-manager.ts:752`, and its two
    /// siblings). Upstream logs and returns — a failed refresh never disturbs the live list.
    ///
    /// **No caller yet.** Its caller is the `list_changed` refresh arm, which needs
    /// `McpServerManager::refreshTools` — **MCP-120**, unported (see `crate::lifecycle`'s note that
    /// the arm fails closed until MCP-120 lands).
    #[must_use]
    pub fn refresh_failed_message(self, server: &str, message: &str) -> String {
        format!(
            "MCP: {}/list_changed refresh failed for {server}: {message}",
            self.wire_name()
        )
    }
}

/// Opaque per-client identity — upstream's `connection.client !== client` check.
///
/// Every `list_changed` handler upstream closes over the `Client` it was built for and refuses to
/// touch the connection map unless the map still holds *that* client. In TypeScript that is object
/// identity; here it is `Arc::ptr_eq` on a token minted once per handler and cloned into every
/// event. Without it, a notification from a client that lost a reconnect race would overwrite the
/// fresh connection's tool list with the stale server's.
#[derive(Clone)]
pub struct ClientIdentity(Arc<IdentityToken>);

/// The allocation whose address *is* the identity. Private, zero-sized, never read.
struct IdentityToken;

impl ClientIdentity {
    fn new() -> Self {
        Self(Arc::new(IdentityToken))
    }

    /// `connection.client === client`.
    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for ClientIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ClientIdentity({:p})", Arc::as_ptr(&self.0))
    }
}

/// A `list_changed` notification, with everything the manager needs to decide whether to act.
pub struct ListChangedEvent {
    /// The `mcpServers` key.
    pub server: String,
    /// Which list changed.
    pub kind: ListKind,
    /// The identity to `ptr_eq` against `connections[server]` before touching anything.
    pub identity: ClientIdentity,
    /// The live peer, so the hook can re-run `list_all_*` without a second lookup. rmcp's
    /// notification carries **no list** — unlike the TS SDK, which refreshes for you and hands back
    /// `(error, list)` — but `Peer` has already invalidated its own response cache by the time the
    /// handler runs, so the re-list is guaranteed fresh.
    pub peer: Peer<RoleClient>,
}

/// A `notifications/elicitation/complete`, already gated on the runtime signal and on `allowUrl`.
pub struct ElicitationCompleteEvent {
    /// The `mcpServers` key.
    pub server: String,
    /// `notification.params.elicitationId`.
    pub elicitation_id: String,
}

/// The identity-guarded refresh. Owned by the manager (MCP-120), which holds the connection map.
pub type ListChangedHook = Arc<dyn Fn(ListChangedEvent) -> BoxFuture<'static, ()> + Send + Sync>;

/// The URL-elicitation completion sink (MCP-122). The manager owns the accepted-id registry, so it
/// is the only place that can honour *"the notice fires only if `Set.delete` returned true"* — a
/// duplicate completion must be silent.
pub type ElicitationCompleteHook = Arc<dyn Fn(ElicitationCompleteEvent) + Send + Sync>;

/// `registerSamplingHandler(client, ...)` — `sampling/createMessage`. Section 05's body; this is the
/// seam it plugs into. See the import block for why the SEP-2577 deprecation is suppressed.
#[allow(deprecated)]
pub type SamplingHook = Arc<
    dyn Fn(String, CreateMessageRequestParams) -> BoxFuture<'static, Result<CreateMessageResult, ErrorData>>
        + Send
        + Sync,
>;

/// `registerElicitationHandler(client, ...)` — `elicitation/create`. Section 05's body.
pub type ElicitationHook = Arc<
    dyn Fn(String, ElicitRequestParams) -> BoxFuture<'static, Result<ElicitResult, ErrorData>>
        + Send
        + Sync,
>;

/// `options.ui.notify` — fire-and-forget in both implementations, which is why it returns `()` and
/// why a re-prompt dialog can open before its toast paints (same as upstream).
pub type NotifyHook = Arc<dyn Fn(&str, cyrup_ext::NotifyKind) + Send + Sync>;

/// `ServerElicitationConfig` (`elicitation-handler.ts:28`) — everything `createClient` needs to
/// build both elicitation hooks, minus the per-server name it splices in.
///
/// A named struct rather than a tuple because the completion notice needs a route out: upstream
/// reads `this.elicitationConfig.ui.notify` off the same object (`server-manager.ts:734`), and
/// splitting them is how the two drift.
#[derive(Clone)]
pub struct ElicitationConfig {
    /// `allowUrl` — `ContextSnapshot::is_tui_mode`, stricter than `has_ui`.
    pub mode: ElicitationMode,
    /// `registerElicitationHandler`'s body.
    pub handler: ElicitationHook,
    /// The completion notice's only route out.
    pub notify: NotifyHook,
}

impl std::fmt::Debug for ElicitationConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElicitationConfig").field("mode", &self.mode).finish_non_exhaustive()
    }
}

/// Construction parameters for [`McpClientHandler`]. A struct rather than a nine-argument `new`
/// because five of the fields are optional and three of those are closures.
pub struct McpClientHandlerParts {
    /// The `mcpServers` key. Every hook and every log line carries it.
    pub server: String,
    /// `runtimeSignal` — `combineAbortSignals(owner.signal, ctx.signal)`. The URL-elicitation
    /// completion handler is a **no-op once this is cancelled**, which is what stops a stale
    /// generation from notifying into the session that replaced it.
    pub runtime_signal: CancelToken,
    /// `elicitationConfig`, or `None` when no elicitation handler is wired.
    pub elicitation_mode: Option<ElicitationMode>,
    /// `samplingConfig ? registerSamplingHandler(...) : undefined`.
    pub sampling: Option<SamplingHook>,
    /// `elicitationConfig ? registerElicitationHandler(...) : undefined`.
    pub elicitation: Option<ElicitationHook>,
    /// The manager's identity-guarded refresh (MCP-120).
    pub list_changed: Option<ListChangedHook>,
    /// The manager's completion sink (MCP-122). Upstream registers the underlying notification
    /// handler **only when `allowUrl`**; the same gate is applied here at dispatch, so wiring the
    /// hook without `allow_url` is inert rather than wrong.
    pub elicitation_complete: Option<ElicitationCompleteHook>,
}

/// `createClient(serverName, definition)`'s client object, as an rmcp [`ClientHandler`].
///
/// # Why this is an `Arc` newtype
///
/// Upstream writes `let client: Client;` and then assigns it *after* the callbacks that close over
/// it — a self-reference that TypeScript permits because the closures do not run until the client
/// exists. Rust has no such window, so the shared state lives behind an `Arc` and the handler is a
/// cheap clone of the pointer: `serve_client_with_lifecycle_and_ct` takes the service **by value**,
/// the caller keeps a clone, and both observe the same [`ClientIdentity`]. That identity is the
/// whole point — it is what `Arc::ptr_eq` compares in the `list_changed` guard.
#[derive(Clone)]
pub struct McpClientHandler {
    shared: Arc<HandlerShared>,
}

struct HandlerShared {
    server: String,
    runtime_signal: CancelToken,
    allow_url: bool,
    info: ClientInfo,
    identity: ClientIdentity,
    sampling: Option<SamplingHook>,
    elicitation: Option<ElicitationHook>,
    list_changed: Option<ListChangedHook>,
    elicitation_complete: Option<ElicitationCompleteHook>,
}

impl McpClientHandler {
    /// Build the handler and, with it, the `initialize` frame's client half.
    ///
    /// The capability set is derived from the *wired hooks*, not from configuration: upstream's
    /// `buildClientCapabilities` tests `this.samplingConfig` / `this.elicitationConfig`, so a
    /// runtime with the config present but the handler absent would advertise a capability it
    /// cannot serve. Deriving from the hook makes that state unrepresentable.
    #[must_use]
    pub fn new(parts: McpClientHandlerParts) -> Self {
        let allow_url = parts
            .elicitation_mode
            .is_some_and(|mode| mode.allow_url);
        let capabilities = build_client_capabilities(
            parts.sampling.is_some(),
            parts.elicitation.is_some().then_some(ElicitationMode { allow_url }),
        );
        let info = client_info(&parts.server, capabilities);
        Self {
            shared: Arc::new(HandlerShared {
                server: parts.server,
                runtime_signal: parts.runtime_signal,
                allow_url,
                info,
                identity: ClientIdentity::new(),
                sampling: parts.sampling,
                elicitation: parts.elicitation,
                list_changed: parts.list_changed,
                elicitation_complete: parts.elicitation_complete,
            }),
        }
    }

    /// The `mcpServers` key this client speaks for.
    #[must_use]
    pub fn server(&self) -> &str {
        &self.shared.server
    }

    /// The identity every clone of this handler shares, and that the manager stores alongside the
    /// connection so a stale notification can be recognised.
    #[must_use]
    pub fn identity(&self) -> ClientIdentity {
        self.shared.identity.clone()
    }

    /// The `initialize` frame's client half, as it will be sent.
    #[must_use]
    pub fn info(&self) -> &ClientInfo {
        &self.shared.info
    }

    /// Shared body of the three `on_*_list_changed` arms (§3.10 — all three are identical in shape).
    ///
    /// The two upstream early returns that have no analogue here are worth naming: `if (error)` and
    /// `if (!list)` both exist because the TS SDK does the refresh *for* you and reports its result.
    /// rmcp's notification is bare, so the refresh — and therefore the failure and the null — moves
    /// into the hook, which is where [`ListKind::refresh_failed_message`] is emitted.
    fn dispatch_list_changed(
        &self,
        kind: ListKind,
        context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + MaybeSendFuture + 'static {
        let hook = self.shared.list_changed.clone();
        let event = ListChangedEvent {
            server: self.shared.server.clone(),
            kind,
            identity: self.identity(),
            peer: context.peer,
        };
        async move {
            let Some(hook) = hook else {
                tracing::debug!(
                    server = %event.server,
                    kind = kind.wire_name(),
                    "MCP: list_changed with no refresh hook wired; ignoring"
                );
                return;
            };
            hook(event).await;
        }
    }
}

#[allow(deprecated)]
impl ClientHandler for McpClientHandler {
    /// The `initialize` frame. See [`build_client_capabilities`] for the four on/off combinations
    /// and the one recorded divergence (`"capabilities":{}` where upstream omits the key).
    fn get_info(&self) -> ClientInfo {
        self.shared.info.clone()
    }

    /// `sampling/createMessage`. With no handler wired the capability was never advertised, so a
    /// conforming server never asks; an unconforming one gets rmcp's own `METHOD_NOT_FOUND`, which
    /// is exactly what the TS SDK returns for an unregistered request handler.
    fn create_message(
        &self,
        params: CreateMessageRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> impl std::future::Future<Output = Result<CreateMessageResult, ErrorData>> + MaybeSendFuture + '_
    {
        let hook = self.shared.sampling.clone();
        let server = self.shared.server.clone();
        async move {
            let Some(hook) = hook else {
                return Err(ErrorData::method_not_found::<CreateMessageRequestMethod>());
            };
            hook(server, params).await
        }
    }

    /// `elicitation/create`. The `Decline` fallback is rmcp's own default and is the right one: a
    /// client that cannot ask the user must not silently accept on their behalf.
    fn create_elicitation(
        &self,
        params: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> impl std::future::Future<Output = Result<ElicitResult, ErrorData>> + MaybeSendFuture + '_ {
        let hook = self.shared.elicitation.clone();
        let server = self.shared.server.clone();
        async move {
            let Some(hook) = hook else {
                return Ok(ElicitResult::new(ElicitationAction::Decline));
            };
            hook(server, params).await
        }
    }

    fn on_tool_list_changed(
        &self,
        context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + MaybeSendFuture + '_ {
        self.dispatch_list_changed(ListKind::Tools, context)
    }

    fn on_prompt_list_changed(
        &self,
        context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + MaybeSendFuture + '_ {
        self.dispatch_list_changed(ListKind::Prompts, context)
    }

    fn on_resource_list_changed(
        &self,
        context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + MaybeSendFuture + '_ {
        self.dispatch_list_changed(ListKind::Resources, context)
    }

    /// Everything rmcp has no first-class variant for — which, for this port, is two methods and a
    /// firm policy for everything else.
    ///
    /// * [`ELICITATION_COMPLETE_METHOD`] (MCP-122) — gated on `allowUrl` **and** on the runtime
    ///   signal, exactly as upstream's `if (this.runtimeSignal?.aborted) return`. The set-delete and
    ///   the *"only if delete returned true"* notice belong to the hook, because the registry is the
    ///   manager's.
    /// * [`STREAM_RESULT_PATCH_METHOD`] (MCP-121, Cut 2) — dropped at `debug`, deliberately.
    /// * anything else — dropped at `debug`. An unhandled notification must **never** fault the
    ///   connection; that is the property the Cut-2 verification asserts.
    fn on_custom_notification(
        &self,
        notification: CustomNotification,
        _context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + MaybeSendFuture + '_ {
        let server = self.shared.server.clone();
        let allow_url = self.shared.allow_url;
        let aborted = self.shared.runtime_signal.is_cancelled();
        let hook = self.shared.elicitation_complete.clone();
        async move {
            match notification.method.as_str() {
                ELICITATION_COMPLETE_METHOD => {
                    // `if (this.runtimeSignal?.aborted) return;` plus the `allowUrl` registration
                    // gate, collapsed into one dispatch-time test.
                    if aborted || !allow_url {
                        return;
                    }
                    let Some(elicitation_id) = notification
                        .params
                        .as_ref()
                        .and_then(|params| params.get("elicitationId"))
                        .and_then(serde_json::Value::as_str)
                    else {
                        tracing::debug!(
                            %server,
                            "MCP: elicitation/complete without a string elicitationId; ignoring"
                        );
                        return;
                    };
                    let Some(hook) = hook else { return };
                    hook(ElicitationCompleteEvent {
                        server,
                        elicitation_id: elicitation_id.to_string(),
                    });
                }
                STREAM_RESULT_PATCH_METHOD => {
                    // Cut 2. The UI extension is gone; the notification is not an error.
                    tracing::debug!(
                        %server,
                        "MCP: UI stream-patch notification ignored (MCP Apps are Cut 2)"
                    );
                }
                other => {
                    tracing::debug!(%server, method = %other, "MCP: unhandled notification ignored");
                }
            }
        }
    }
}

// -------------------------------------------------------------------------------------------------
// MCP-148 — the rmcp client construction itself
// -------------------------------------------------------------------------------------------------

/// `await client.connect(transport, requestOptions)` — initialise the connection or fail.
///
/// One call collapses three upstream mechanisms (§3.8):
///
/// * **`connectClientWithAbort`.** `serve_client_with_lifecycle_and_ct` owns the whole
///   initialise-or-fail path and races the token itself, returning
///   [`ClientInitializeError::Cancelled`].
/// * **The `abortCleanupPromises` `WeakMap`.** On `Err` the transport is dropped here, and
///   `TokioChildProcess`'s `ChildWithCleanup::drop` spawns a `kill()`, so no child survives an
///   aborted connect and there is no second `close()` to coordinate. What does *not* collapse is the
///   HTTP retry ladder's once-only teardown across attempts — that is adapter policy and stays in
///   `server_manager` (MCP-123).
/// * **`resolveVersionNegotiation`'s option.** Folded into the `lifecycle` argument; see
///   [`version_negotiation`].
///
/// `requestOptions` is split in two here. Its **signal** half is the `ct` argument. Its **timeout**
/// half is not expressible on this call — rmcp's connect takes no per-request timeout — so it is
/// applied one layer out, by [`connect_client_bounded`], which every connect arm calls instead of
/// this function. Do not call this one directly from a connect arm: an unbounded handshake is
/// exactly the defect `connect_client_bounded` exists to prevent, and `ct` is the **service's**
/// lifetime token rather than the handshake's — passing an attempt signal straight in closes the
/// connection the moment that attempt settles. [`detachable_from`] is what scopes it.
///
/// # Errors
///
/// [`ClientInitializeError`], returned unwrapped rather than mapped into [`McpError`]. The auth
/// ladder (MCP-115) needs `ClientInitializeError::auth_challenge()` — which walks the `source()`
/// chain for `AuthRequiredError`/`InsufficientScopeError` and hands back the `WWW-Authenticate`
/// header — and flattening the error here would destroy the one thing that makes the 401 predicate
/// typed instead of hand-written.
///
/// Boxed, not flattened. `ClientInitializeError` is 376 bytes, so returning it by value made
/// every `Result` on this path that wide (`clippy::result_large_err`). `Box` keeps the concrete
/// type — `auth_challenge()` still resolves through the deref — so the typed 401 predicate above
/// is untouched; only the error's placement changes, and only on the failure path.
pub async fn connect_client<T, E, A>(
    handler: McpClientHandler,
    transport: T,
    lifecycle: ClientLifecycleMode,
    ct: CancelToken,
) -> Result<RunningService<RoleClient, McpClientHandler>, Box<ClientInitializeError>>
where
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    rmcp::service::serve_client_with_lifecycle_and_ct(handler, transport, lifecycle, ct)
        .await
        .map_err(Box::new)
}

/// [`connect_client`] under `requestOptions.timeout` — the other half of
/// `abortable(client.connect(transport, requestOptions), signal)` (`server-manager.ts:660`).
///
/// # Why this exists at all
///
/// `CreateConnection::request_options` is computed by the manager (`server_manager.rs:1651`) once
/// per attempt and was, until this function, **read by nothing**. The consequence was not cosmetic:
/// a server that accepts the connection and then never answers `initialize` parked the connect
/// future — and with it the manager's per-name single-flight slot — until someone called `close`.
/// MEASURED through the real `McpServerManager` with entry
/// `{command:"sh", args:["-c","exec sleep 60"], requestTimeoutMs: 300}`: still hanging after 6 s.
/// Upstream fails that connect at 300 ms. `runtime.rs`'s own comment used to say only that
/// "`requestOptions` does not appear", which reads as "has no effect" rather than as "the handshake
/// is unbounded"; both statements are now written where they can be acted on.
///
/// # Two NAMED DELTAS in what the budget covers
///
/// 1. **Scope.** Upstream arms the timer around the `initialize` *request*
///    (`SdkError(RequestTimeout, "Request timed out", { timeout })`, the SDK's
///    `_setupTimeout(messageId, timeout, …)`); this arms it around rmcp's whole
///    `serve_client_with_lifecycle_and_ct`, which is `initialize` plus the
///    `notifications/initialized` send. The extra span is one buffered write on a transport that
///    has already answered, so the budget is very slightly stricter than upstream's, never looser.
/// 2. **`maxTotalTimeout` / `resetTimeoutOnProgress`** have no upstream analogue in
///    `buildRequestOptions` and are not applied.
///
/// `None` means no timeout, which is `normalizeRequestTimeoutMs`'s answer for an absent, zero,
/// negative, `NaN` or infinite value — see [`resolve_request_timeout`] for why that does **not**
/// fall back to the global.
///
/// # Errors
///
/// The inner `Result` is [`connect_client`]'s. The **outer** `Err` is the timeout, carrying the
/// budget that elapsed so the caller can render it; it is deliberately not a
/// [`ClientInitializeError`], because rmcp has no timeout variant and manufacturing one
/// (`ConnectionClosed`, say) would make a wedged server indistinguishable from a hung-up one.
pub async fn connect_client_bounded<T, E, A>(
    handler: McpClientHandler,
    transport: T,
    lifecycle: ClientLifecycleMode,
    ct: CancelToken,
    timeout: Option<Duration>,
) -> Result<Result<RunningService<RoleClient, McpClientHandler>, Box<ClientInitializeError>>, Duration>
where
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    // `signal?.addEventListener("abort", closeTransport, { once: true })` — armed for the connect,
    // and released by `connectClientWithAbort`'s `finally` the moment it returns. See
    // [`detachable_from`] for why holding it past that point closed every connection this crate
    // made.
    let (service, detach) = detachable_from(&ct);
    let outcome = match timeout {
        None => Ok(connect_client(handler, transport, lifecycle, service).await),
        // Dropping the `connect_client` future is what tears the half-built connection down, and it
        // is the same drop the abort path relies on: `serve_client_with_ct_inner` holds the
        // transport in a local, so the transport (and, for stdio, `ChildWithCleanup::drop`'s
        // `kill()`) goes with it.
        Some(budget) => {
            match tokio::time::timeout(budget, connect_client(handler, transport, lifecycle, service))
                .await
            {
                Ok(outcome) => Ok(outcome),
                Err(_elapsed) => Err(budget),
            }
        }
    };
    // The `finally`. Fired on BOTH arms, success and failure: on failure the transport is already
    // gone and this only reaps the joiner task.
    detach.cancel();
    outcome
}

/// A token that follows `attempt` **until the returned handle is cancelled**, and then stops —
/// `connectClientWithAbort`'s `addEventListener` / `finally { removeEventListener }` pair.
///
/// # The defect this exists to close, MEASURED
///
/// rmcp's `ct` argument is the **service's** lifetime token, not the handshake's: it is stored on
/// the `RunningService` and cancelling it ends the service task, which closes the transport. This
/// crate was handing it `CreateConnection::attempt` — a token [`crate::server_manager`] deliberately
/// cancels *on success*, in `AbortHandle::reap`, to release the parked `combine` joiners once the
/// attempt has settled. So every connection this crate made was torn down microseconds after it was
/// established, while the manager's bookkeeping still read `Connected`.
///
/// It was invisible until something actually used the peer. MEASURED against the real manager and a
/// real `sh` child: `tools/call` on the connection `connect` had just returned answered
/// `Err(TransportClosed)`, and the identical call through the factory with the attempt token left
/// unreaped answered `Ok(… "echoed:pong")`. Discovery (MCP-119) does not see it because it runs
/// inside `createConnection`, before the manager reaps.
///
/// Upstream cannot have this bug: its abort listener closes the transport and is removed by
/// `connectClientWithAbort`'s `finally`, so the connect signal governs the connect and nothing
/// after it (`server-manager.ts:646-674`). This restores that scoping — an abort *during* connect
/// still cancels rmcp's initialise (MCP-123 is untouched), and an abort that arrives after a
/// successful handshake is caught by [`discover`]'s `throwIfAborted`, which closes the resource
/// through the ordinary catch rather than by killing a live service out from under it.
///
/// Returns `(service_token, detach_handle)`. Cancelling the detach handle releases the link and
/// reaps the joiner; it never cancels the service token.
fn detachable_from(attempt: &CancelToken) -> (CancelToken, CancelToken) {
    let detach = CancelToken::new();
    let service = CancelToken::new();
    // Already aborted: no task, and the connect must still see a cancelled token.
    if attempt.is_cancelled() {
        service.cancel();
        return (service, detach);
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        // `combine`'s discipline: this crate denies `clippy::panic` and `tokio::spawn` panics
        // off-runtime. Degrading to the attempt token itself keeps the abort half working and
        // restores the old over-long lifetime, which is strictly better than a connect that cannot
        // be aborted at all. Unreachable in practice — this is an `async fn`, so a runtime exists.
        tracing::warn!("MCP: no tokio runtime to scope the connect signal on — using the attempt's");
        return (attempt.clone(), detach);
    };
    let watched = attempt.clone();
    let armed = service.clone();
    let released = detach.clone();
    handle.spawn(async move {
        tokio::select! {
            () = watched.cancelled() => armed.cancel(),
            // The `finally`, and the joiner's own exit path.
            () = released.cancelled() => {}
            () = armed.cancelled() => {}
        }
    });
    (service, detach)
}

/// `new SdkError(SdkErrorCode.RequestTimeout, "Request timed out", { timeout })` — the SDK's
/// per-request timer (`@modelcontextprotocol/client` `src-D_zzAWoS.mjs:6149`). `SdkError`'s
/// constructor is `super(message)`, so `error.message` is **exactly** `Request timed out`; the
/// budget rides in `data`, not in the text, and is therefore not appended here either.
///
/// It reaches the user through the same `McpError::Server` envelope and the same §3.3 step-8 stderr
/// tail as any other handshake failure, which is what a caller of `connect` sees upstream too.
pub const HANDSHAKE_TIMED_OUT: &str = "Request timed out";

fn handshake_timeout_error(server: &str, stderr: Option<&StderrPump>) -> McpError {
    let message = match stderr {
        Some(pump) => with_stderr_tail(
            HANDSHAKE_TIMED_OUT,
            &pump
                .tail
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        ),
        None => HANDSHAKE_TIMED_OUT.to_string(),
    };
    McpError::Server {
        server: server.to_string(),
        message,
    }
}

// -------------------------------------------------------------------------------------------------
// MCP-128 — request options (§3.13)
// -------------------------------------------------------------------------------------------------

/// `normalizeRequestTimeoutMs` (`server-manager.ts:1249-1253`): finite **and** strictly positive,
/// else no timeout.
///
/// The subtle half is in [`resolve_request_timeout`], not here.
#[must_use]
pub fn normalize_request_timeout_ms(timeout_ms: Option<f64>) -> Option<Duration> {
    let ms = timeout_ms?;
    if !ms.is_finite() || ms <= 0.0 {
        return None;
    }
    Some(Duration::from_secs_f64(ms / 1000.0))
}

/// `getResolvedRequestTimeoutMs(definition)` (`server-manager.ts:234-239`).
///
/// **The trap**: an invalid *per-server* `requestTimeoutMs` — `0`, negative, `NaN`, `Infinity` —
/// resolves to **no timeout at all**. It does *not* fall back to the global. Upstream's shape is
/// `if (definition?.requestTimeoutMs !== undefined) return normalize(...)`, so the global is only
/// consulted when the per-server key is **absent**; once present, whatever it normalises to wins.
/// A porter who writes `definition.or(global)` after normalising gets a plausible-looking
/// implementation that silently reinstates a 30-second cap the user disabled on purpose.
#[must_use]
pub fn resolve_request_timeout(entry: Option<&ServerEntry>, global_ms: Option<f64>) -> Option<Duration> {
    match entry.and_then(|entry| entry.request_timeout_ms) {
        Some(per_server) => normalize_request_timeout_ms(Some(per_server)),
        None => normalize_request_timeout_ms(global_ms),
    }
}

/// `buildRequestOptions(definition, signal)` (`server-manager.ts:241-256`).
///
/// Upstream returns `undefined` when *neither* a signal nor a timeout exists, and the object is
/// shared by the connect and all three discovery list calls. Here only the timeout half is
/// representable: [`PeerRequestOptions`] has no signal field, because rmcp cancels a request by
/// dropping its future — the `ownedSignal` half therefore lives in the `abortable(..)` wrapper
/// around each call rather than inside the options object.
///
/// `reset_timeout_on_progress` and `max_total_timeout` have no upstream analogue and stay at their
/// defaults.
#[must_use]
pub fn build_request_options(
    entry: Option<&ServerEntry>,
    global_ms: Option<f64>,
) -> Option<PeerRequestOptions> {
    resolve_request_timeout(entry, global_ms).map(PeerRequestOptions::with_timeout)
}

// =================================================================================================
// The `createConnection` body — MCP-101 (stdio), MCP-109/113/114/115/115a (HTTP)
//
// `server_manager.rs` owns the state machine around `createConnection` and declares it as the
// [`ConnectionFactory`] seam; this is that seam's body. Upstream it is one 180-line method
// (`server-manager.ts:442-624`) whose two arms share a catch; here the two arms are
// [`ConnectionBuilder::connect_stdio`] and [`ConnectionBuilder::connect_http_client`] and the shared
// catch is [`ConnectionBuilder::create_connection`], which is the one place the §3.3 step-8 stderr
// enrichment and the `MCP connection setup failed` wrapper live.
//
// What is deliberately NOT here, so a reader does not go looking:
//
// * **npx/npm pre-resolution (MCP-103).** `cyrup_ext::caps::proc::npx_resolver::resolve_npx_binary`
//   is `pub(super)` and its promotion is MCP-103's, in a crate this unit does not own. The call site
//   is marked below so landing MCP-103 is three lines.
// * **Discovery (MCP-119).** `NewConnection` has nowhere to put tools/resources/prompts —
//   `ServerConnection::new` hardcodes them empty — so a factory *cannot* deliver them through this
//   seam as it stands. Widening `NewConnection` is `server_manager.rs`'s change, not this one.
// * **`wrapTransportWithMcpTrace` (T-10/MCP-477).** No trace observer is threaded yet.
// =================================================================================================

/// `new McpOAuthProvider(serverName, serverUrl, extractOAuthConfig(definition), …)` — the provider
/// half of §3.4's ladder, as a seam.
///
/// **MCP-115 owns the state machine and the ordering; the provider itself is section 05's
/// `rmcp::transport::auth` work** (13c:1330). This trait is the line between them: the ladder
/// decides *when* a provider must exist and what a 401 means, and an implementation decides what
/// `Authorization` value — if any — that provider can produce right now.
///
/// # The default is not a stub, it is one of upstream's two real outcomes
///
/// Upstream constructs the provider with `{ onRedirect: async () => {} }`, so when the SDK's
/// `auth()` decides a browser round trip is required it completes with `"REDIRECT"`, nothing is
/// opened, and the transport raises `UnauthorizedError` again. The connect therefore ends at
/// `needs-auth` for every server with no usable stored credential — which is exactly what
/// [`NoStoredCredentials`] produces. What the default cannot reproduce is the *other* outcome: a
/// server whose credential IS in the store connects on the retry. Binding that arm is a matter of
/// calling [`crate::oauth::get_valid_token`], which needs an
/// [`crate::oauth::AuthenticateOptions`] — the storage handle and the OAuth runtime — and those are
/// held by the manager, not by this crate's transport layer.
pub trait HttpAuthProvider: Send + Sync + std::fmt::Debug + 'static {
    /// The `Authorization` value this attempt should carry, **without** the `Bearer ` prefix (rmcp
    /// applies the scheme through `auth_header`), or `None` when the provider has nothing to offer.
    ///
    /// `challenge` is the `WWW-Authenticate` header of the 401 that promoted an implicit provider,
    /// and is `None` on the `explicit` arm — upstream's provider ignores it entirely, while rmcp's
    /// `AuthorizationRequest::with_challenge` consumes it, so it is carried rather than dropped.
    fn authorize<'a>(
        &'a self,
        server: &'a str,
        url: &'a str,
        challenge: Option<&'a str>,
    ) -> BoxFuture<'a, McpResult<Option<String>>>;

    /// `invalidateAuthEntryCache(name)` (`mcp-auth.ts`) — forget the cached entry so the next read
    /// reloads secure storage. The **once-per-episode policy** is the ladder's, not this
    /// implementation's: see [`ConnectionBuilder::connect_http_client`]'s `invalidated` flag
    /// (MCP-116).
    fn invalidate_auth_entry_cache(&self, server: &str);
}

/// The default [`HttpAuthProvider`]: no credential is available, ever.
///
/// See [`HttpAuthProvider`] for why this is upstream-faithful rather than inert — it reproduces the
/// `onRedirect: async () => {}` outcome, which is the one a first-time login takes.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoStoredCredentials;

impl HttpAuthProvider for NoStoredCredentials {
    fn authorize<'a>(
        &'a self,
        _server: &'a str,
        _url: &'a str,
        _challenge: Option<&'a str>,
    ) -> BoxFuture<'a, McpResult<Option<String>>> {
        Box::pin(async { Ok(None) })
    }

    fn invalidate_auth_entry_cache(&self, _server: &str) {}
}

/// The production [`HttpAuthProvider`]: MCP-115's *"a server whose credential is already in the
/// store connects"* arm, bound to [`crate::credentials::McpAuthStore`] through
/// [`crate::oauth::get_valid_token`].
///
/// [`NoStoredCredentials`] reproduces one of upstream's two outcomes — the first login, where the
/// provider has nothing to offer and the connect ends at `needs-auth`. This is the other one, and
/// it is what a *returning* user gets on every session after that: the credential is read out of
/// the vault **before** the handshake POST is built, so an `auth: "oauth"` server connects on
/// attempt one, with no 401, no browser and no prompt.
///
/// # What it deliberately does not do
///
/// * **No browser, ever.** [`crate::oauth::get_valid_token`] performs at most a token *refresh*;
///   the authorization-code round trip belongs to the tool layer's `attempt_auto_auth`, which is
///   the only place allowed to open a window from inside a turn. A launcher installed here would
///   fire from inside a connect, which is precisely the fence that keeps a startup connect pass
///   from spawning one browser tab per configured server.
/// * **No once-per-episode policy.** [`Self::invalidate_auth_entry_cache`] evicts unconditionally.
///   The flag that makes eviction fire at most once per connect episode is
///   [`ConnectionBuilder::connect_http_client`]'s `invalidated` (MCP-116), and duplicating it here
///   would invert the bug that flag exists to prevent: an eviction that never fires again after the
///   first connect of the process, so a rotated credential is never re-read.
/// * **No `challenge` plumbing.** `get_valid_token`'s refresh resolves authorization-server
///   metadata with the proactive `.well-known` walk;
///   [`crate::oauth::AuthenticateOptions::challenge`] is consumed only by `start_auth`, which this
///   type never reaches. Setting it would be a field nothing reads.
///
/// # Why [`crate::oauth::get_valid_token`] and not a bare store read
///
/// Upstream's `McpOAuthProvider.tokens()` hands the stored token back as-is and lets the TS SDK's
/// `auth()` loop refresh it. There is no such loop behind `auth_header` here — rmcp's
/// streamable-HTTP transport takes a **static** bearer for the lifetime of the connection — so this
/// is the only place a refresh can happen. Without it an expired-but-refreshable credential would
/// 401 on every session and the returning user would silently become a first-time login forever.
///
/// # The four ways `authorize` can answer, and which are quiet
///
/// | cause | answer | connect outcome |
/// |---|---|---|
/// | no entry, or an entry bound to a different URL | `Ok(None)` | `needs-auth` — the first-login entry |
/// | a live (or refreshed) token | `Ok(Some(token))` | connected |
/// | the keychain is unreachable, or an entry will not parse | `Err(CredentialStore)` | the connect fails **loudly** |
/// | a refresh failed on the network | `Ok(None)` after an `error` log | `needs-auth` |
///
/// The third row is the one that must not be swallowed: a broken keychain answered as `Ok(None)`
/// is indistinguishable from "you have never logged in", so the user is sent to authenticate, the
/// flow writes a credential that cannot be read back, and the loop never terminates.
/// [`crate::oauth::get_valid_token`] already draws that line — abort errors and errors carrying the
/// credential-store marker rethrow, everything else logs and answers `None` — so `authorize`
/// propagates with `?` rather than classifying anything itself.
pub struct StoredCredentialAuth {
    /// The generation's vault. [`crate::credentials::McpAuthStore`] is `Clone`-shares-state, so
    /// this handle and every other clone of the same store are one entry cache over one backend —
    /// which is what makes [`Self::invalidate_auth_entry_cache`] evict something a later read will
    /// actually miss on.
    store: crate::credentials::McpAuthStore,
    /// The same vault behind the trait object [`crate::oauth::AuthenticateOptions`] takes. Built
    /// once at construction so `authorize` allocates nothing per connect attempt; it shares
    /// `store`'s inner `Arc`, so the two are the same object and not two views of one backend.
    storage: Arc<dyn crate::oauth::McpOAuthStorage>,
    /// This generation's OAuth runtime, handed to every [`crate::oauth::AuthenticateOptions`] this
    /// provider builds — see [`Self::authorize`] for why it is never left `None`.
    runtime: Arc<OAuthRuntime>,
    /// `runtimeSignal`: the owner/context token [`initialize_mcp`] combines once per generation, so
    /// a session replacement aborts an in-flight refresh instead of letting it write to a vault the
    /// next generation already owns.
    signal: CancelToken,
    /// The configured servers whose `oauth.skipIssuerMetadataValidation` is `true`, precomputed by
    /// [`skip_issuer_metadata_validation`]. A name set rather than a config clone: the trait hands
    /// `authorize` a server *name*, and this is the only per-server value
    /// [`crate::oauth::get_valid_token`] consumes. Absent means `false`, which is also what an
    /// unknown name gets — an unconfigured server cannot reach the ladder in the first place.
    skip_issuer: std::collections::HashSet<String>,
}

impl StoredCredentialAuth {
    /// Bind a provider to one generation's vault, OAuth runtime and abort.
    #[must_use]
    pub fn new(
        store: crate::credentials::McpAuthStore,
        runtime: Arc<OAuthRuntime>,
        signal: CancelToken,
        skip_issuer: std::collections::HashSet<String>,
    ) -> Self {
        let storage: Arc<dyn crate::oauth::McpOAuthStorage> = Arc::new(store.clone());
        Self {
            store,
            storage,
            runtime,
            signal,
            skip_issuer,
        }
    }
}

/// Hand-written because [`crate::credentials::McpAuthStore`] has no `Debug` and a derived one would
/// be a credential-shaped hole in every `{:?}` on a [`ConnectionBuilder`], whose own `Debug` prints
/// its provider. What is printed here is a directory path and two counts — never an entry, never a
/// token.
impl std::fmt::Debug for StoredCredentialAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredCredentialAuth")
            .field("base_dir", &self.store.auth_base_dir())
            .field("servers_with_skip_issuer", &self.skip_issuer.len())
            .finish_non_exhaustive()
    }
}

impl HttpAuthProvider for StoredCredentialAuth {
    fn authorize<'a>(
        &'a self,
        server: &'a str,
        url: &'a str,
        _challenge: Option<&'a str>,
    ) -> BoxFuture<'a, McpResult<Option<String>>> {
        Box::pin(async move {
            let mut options = crate::oauth::AuthenticateOptions::new(Arc::clone(&self.storage));
            // ALWAYS `Some`. `get_valid_token` opens with `get_runtime(options.runtime.as_ref())`,
            // and `get_runtime(None)` resurrects the module-level legacy runtime and inserts its id
            // into the process-global live-runtime set that only `shutdown_oauth` removes — so a
            // `None` here would leak one live-runtime id per connect attempt and wedge the shared
            // callback listener open for the life of the process.
            //
            // The secondary effect is the one this path wants anyway: once this generation's
            // runtime has been shut down, `get_runtime` answers `Err(McpError::Aborted)`, so a
            // connect racing a session teardown fails as an **abort** rather than as `needs-auth`
            // — which is what the ladder's abort arm and the startup pass both already expect.
            options.runtime = Some(Arc::clone(&self.runtime));
            options.signal = Some(self.signal.clone());
            options.skip_issuer_metadata_validation = self.skip_issuer.contains(server);
            // `launcher` stays the default and both hooks stay `None`: nothing on this path reaches
            // a browser, and installing one would breach the fence in this type's doc comment.
            // `challenge` stays `None` for the reason recorded there too.
            let tokens = crate::oauth::get_valid_token(server, url, &options).await?;
            // NEVER log `tokens`, and never fold it into an error: `McpTokens` derives `Debug` and
            // `access_token` is a plain `String`, so a single `?tokens` on this line would put a
            // live bearer token in the log. The value leaves only through the return, into
            // `config.auth_header`, which rmcp applies with `bearer_auth`.
            //
            // Bare, without the `Bearer ` prefix — [`HttpAuthProvider::authorize`]'s contract.
            Ok(tokens.map(|tokens| tokens.access_token))
        })
    }

    fn invalidate_auth_entry_cache(&self, server: &str) {
        // Unconditional, by design — see this type's doc comment on MCP-116.
        self.store.invalidate_cache(server);
    }
}

/// `createClient(serverName, definition)` — how the builder obtains its [`McpClientHandler`].
///
/// A seam because every hook the handler carries (`registerSamplingHandler`,
/// `registerElicitationHandler`, the three `listChanged.onChanged` callbacks, the URL-elicitation
/// completion sink) is owned by the **manager**, which holds the connection map the identity guards
/// compare against. The builder knows the server name and the runtime signal and nothing else.
pub type HandlerFactory = Arc<dyn Fn(&str, &CancelToken) -> McpClientHandler + Send + Sync>;

/// The one-shot back-reference from a manager hook to the generation that created it.
///
/// `OnceLock` rather than a `Mutex`: it is written exactly once, by [`initialize_mcp`], before any
/// connection can exist, and read from arbitrary rmcp tasks thereafter. A read before the write
/// yields `None`, which every consumer already has to handle — it is the same answer a headless
/// generation gives.
#[derive(Default)]
pub struct SessionSlot(std::sync::OnceLock<std::sync::Weak<McpState>>);

impl SessionSlot {
    /// Called once, by [`initialize_mcp`], the moment the state commits.
    pub fn bind(&self, state: &Arc<McpState>) {
        let _ = self.0.set(Arc::downgrade(state));
    }

    /// The generation's dialog, or `None` for a headless or already-torn-down generation. `None` is
    /// upstream's `!state.ui`, and every consent gate must read it as "cannot ask", never
    /// "approved".
    #[must_use]
    pub fn dialog(&self) -> Option<crate::owner::McpDialog> {
        self.0.get()?.upgrade()?.dialog()
    }

    /// `getCurrentModel()`'s body, through the **fenced** handle: a stopped generation reports
    /// `None` rather than the dead session's model.
    #[must_use]
    pub fn current_model(&self) -> Option<String> {
        let state = self.0.get()?.upgrade()?;
        let ui = state.ui.as_ref()?;
        cyrup_ext::HostServices::current_model(ui.as_ref())
    }
}

impl std::fmt::Debug for SessionSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionSlot").field("bound", &self.0.get().is_some()).finish()
    }
}

/// The hookless handler — sampling and elicitation are not advertised, and `list_changed`
/// notifications are logged and dropped. What [`ConnectionBuilder::new`] installs until a manager
/// supplies its own.
#[must_use]
pub fn bare_handler_factory() -> HandlerFactory {
    Arc::new(|server: &str, runtime_signal: &CancelToken| {
        McpClientHandler::new(McpClientHandlerParts {
            server: server.to_string(),
            runtime_signal: runtime_signal.clone(),
            elicitation_mode: None,
            sampling: None,
            elicitation: None,
            list_changed: None,
            elicitation_complete: None,
        })
    })
}

/// `isUnauthorizedHttpError(error)` (`server-manager.ts:73-75`) **and** the challenge it carries.
///
/// Upstream is `error instanceof UnauthorizedError || (error instanceof SdkHttpError && error.status
/// === 401)` — **401 only**. rmcp offers `ClientInitializeError::auth_challenge()`, which is nearly
/// this but not quite: it matches `AuthRequiredError` (401) *and* `InsufficientScopeError` (403),
/// because rmcp's reactive-OAuth flow treats an insufficient-scope 403 as actionable too. Upstream
/// does not: a 403 falls past both 401 arms, past `shouldFallbackToSse` (403 is not in its
/// `[404, 405, 406, 415]`), and out of `connectHttpClient` as a hard error.
///
/// So the predicate here walks the same `source()` chain rmcp's does and downcasts **only**
/// `AuthRequiredError`. That is one hand-written walk against the plan's "the 401 predicate is not
/// hand-written", and the reason is stated so it can be overruled: using `auth_challenge()` as-is
/// would turn every scope-denied 403 into an OAuth retry and then into `needs-auth`, which is a
/// different user-visible outcome from upstream's hard failure on a path that touches credentials.
///
/// `Some("")` is a real value, and it is [`bare_unauthorized`]'s answer: a 401 with **no**
/// `WWW-Authenticate` header still authorizes the retry, and the challenge is genuinely absent
/// rather than empty-by-accident. [`crate::oauth::on_unauthorized`] takes `Option<&str>` precisely
/// so that case is expressible.
///
/// # The bare 401 is why this function is not just the `AuthRequiredError` downcast
///
/// rmcp builds `AuthRequiredError` **only** inside
/// `if response.status() == UNAUTHORIZED && let Some(header) = response.headers().get(WWW_AUTHENTICATE)`
/// (`rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:212-226` for POST, `:97-111`
/// for GET). A 401 with no challenge header therefore never becomes that type, and a downcast-only
/// predicate answers `None` for it — which sends a permanently-401 server down arm 7 as a hard
/// connect failure and never offers the user `/mcp-auth`.
///
/// **On the handshake POST that gate is no longer the one this crate runs through.**
/// [`UnauthorizedProbe`] owns that one request and types **every** 401 on it as
/// [`StreamableHttpError::AuthRequired`], challenge header or not, so the first branch of the walk
/// below — the `AuthRequiredError` downcast — is what actually claims a bare 401 on `initialize` or
/// `server/discover` today. The widening is still load-bearing for every leg that stays delegated to
/// rmcp: the `notifications/initialized` POST, whose bare 401 arrives as
/// `UnexpectedServerResponse("HTTP 401 …")` and is claimed by [`bare_unauthorized`].
///
/// Upstream has no such gate: `isUnauthorizedHttpError` is
/// `error instanceof UnauthorizedError || (error instanceof SdkHttpError && error.status === 401)`
/// (`server-manager.ts:73-75`) — the header plays no part. Confirmed in the pinned SDK: with no
/// `authProvider` a 401 falls past the `response.status === 401 && this._authProvider` guard to
/// `throw new SdkHttpError(..., { status: response.status })`, and with a provider a 401 lacking
/// `www-authenticate` still reaches `throw markAuthSeamEscape(new UnauthorizedError())`.
///
/// MEASURED before this widening, against a loopback fixture answering every `initialize` with
/// `HTTP/1.1 401 Unauthorized` and no `WWW-Authenticate`, an implicit-OAuth entry and a counting
/// provider: one attempt, no retry, no `needs-auth`, no `invalidateAuthEntryCache` — the connect
/// failed with `unexpected server response: HTTP 401 Unauthorized`.
///
/// The `InsufficientScopeError` exclusion survives the widening untouched: a 403 becomes
/// [`StreamableHttpError::InsufficientScope`] when it carries a challenge and an
/// `UnexpectedServerResponse` reading `HTTP 403 …` when it does not, and [`bare_unauthorized`]
/// matches neither.
#[must_use]
pub fn unauthorized_challenge(error: &ClientInitializeError) -> Option<&str> {
    // The walk has to START at `DynamicTransportError::error`, not at `error.source()`.
    // MEASURED: `ClientInitializeError::TransportError`'s `#[error("Send message error {error},
    // when {context}")]` carries **no** `#[source]` and its field is not named `source`, so
    // thiserror generates no `source()` edge and a chain walk rooted at the `ClientInitializeError`
    // finds nothing. The first draft of this function did exactly that and every 401 fell through
    // to the hard-error arm — a permanent-401 server produced a connect failure instead of
    // `needs-auth`, which is the outcome MCP-115's verify line is about. rmcp's own
    // `auth_challenge` (`service/client.rs:110-131`) starts at `error.error.as_ref()` for this
    // reason, and so does this.
    let transport = match error {
        ClientInitializeError::TransportError { error, .. } => error,
        // A 401 in the fallback leg is still actionable. Reachable only under
        // `ClientLifecycleMode::Auto` (`protocolVersion: "auto"`).
        ClientInitializeError::LegacyFallbackFailed { fallback, .. } => {
            return unauthorized_challenge(fallback);
        }
        _ => return None,
    };
    unauthorized_in_chain(transport.error.as_ref())
}

/// The `source()` walk itself, over any error chain — [`unauthorized_challenge`]'s second half.
///
/// Split out because discovery needs the identical predicate on a **different** root type. A 401 on
/// `tools/list` never becomes a [`ClientInitializeError`]: the handshake is long over, so it arrives
/// as [`ServiceError::TransportSend`] wrapping a `DynamicTransportError` whose `error` field is the
/// same boxed transport error [`unauthorized_challenge`] walks. Upstream's `isUnauthorizedHttpError`
/// is one predicate applied at both sites (`server-manager.ts:73-75`, used by `createConnection`'s
/// catch *and* by `fetchAllResources`/`fetchAllPrompts`), and this is that one predicate.
///
/// Returns the `WWW-Authenticate` challenge, `Some("")` for a bare 401 — see [`bare_unauthorized`]
/// for why an empty challenge is a real value — and `None` when no link in the chain is a 401.
fn unauthorized_in_chain<'a>(root: &'a (dyn std::error::Error + 'static)) -> Option<&'a str> {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(root);
    while let Some(current) = source {
        if let Some(required) =
            current.downcast_ref::<rmcp::transport::streamable_http_client::AuthRequiredError>()
        {
            return Some(&required.www_authenticate_header);
        }
        if bare_unauthorized(current) {
            return Some("");
        }
        source = std::error::Error::source(current);
    }
    None
}

/// rmcp's rendering of `HTTP {status}: {body}` for a status it has no variant for
/// (`streamable_http_client.rs:296`, `format!("HTTP {status}: {body}")` with `status:
/// reqwest::StatusCode`, whose `Display` is `401 Unauthorized`).
const UNEXPECTED_UNAUTHORIZED_PREFIX: &str = "HTTP 401 ";

/// Is this link in the chain a 401 that rmcp did **not** turn into an `AuthRequiredError`?
///
/// # Why one arm is a string prefix, stated rather than hidden
///
/// rmcp keeps the status on the error in exactly one of its two 401 shapes:
///
/// * [`StreamableHttpError::Client`] wraps the `reqwest::Error` from `error_for_status()`
///   (`…/reqwest/streamable_http_client.rs:128`), which still carries `.status()`. This is the
///   GET/SSE leg. TYPED — no parsing. It is **defence in depth, not a live path**: the GET stream is
///   opened from a detached `JoinSet` (`…/streamable_http_client.rs:685-712`) long after the
///   handshake has settled, so its error never becomes a [`ClientInitializeError`] and never reaches
///   this predicate through [`unauthorized_challenge`]'s single caller.
/// * [`StreamableHttpError::UnexpectedServerResponse`] carries a `Cow<str>` and **nothing else**.
///   The status is only in the text, so matching it is a prefix test against rmcp's own format
///   string. There is no typed channel to prefer: the variant is `(Cow<'static, str>)`.
///
///   This is **no longer the `initialize` POST**: [`UnauthorizedProbe`] owns the handshake POST and
///   types its 401s as [`StreamableHttpError::AuthRequired`] before rmcp's body-reading path can
///   collapse them. What it still covers is real and reachable — the `notifications/initialized`
///   POST, which is sent inside `serve_client_with_lifecycle_and_ct`, is **not** a handshake request
///   and so stays delegated to rmcp verbatim. A bare 401 on that POST becomes
///   `UnexpectedServerResponse("HTTP 401 …")` and surfaces as
///   `ClientInitializeError::transport::<T>(error, "send initialized notification")`
///   (`rmcp-3.1.4/src/service/client.rs:912`), which is exactly a shape
///   [`unauthorized_challenge`] walks.
///
/// The fragility is real and bounded — if rmcp changes that format the predicate silently narrows
/// back to the header-carrying 401, which is the behaviour this function was written to fix. It is
/// pinned by `the_401_predicate_still_refuses_every_other_status`, which constructs rmcp's rendering
/// directly and asserts both halves of the prefix test, so an rmcp upgrade that changes the wording
/// fails a test rather than a user. `a_bare_401_with_no_challenge_still_reaches_the_oauth_ladder`
/// no longer pins this arm — its 401 is on the handshake POST, which [`UnauthorizedProbe`] types.
#[must_use]
pub fn bare_unauthorized(error: &(dyn std::error::Error + 'static)) -> bool {
    let Some(http) = error.downcast_ref::<
        rmcp::transport::streamable_http_client::StreamableHttpError<reqwest::Error>,
    >() else {
        return false;
    };
    match http {
        rmcp::transport::streamable_http_client::StreamableHttpError::Client(client) => {
            client.status() == Some(reqwest::StatusCode::UNAUTHORIZED)
        }
        rmcp::transport::streamable_http_client::StreamableHttpError::UnexpectedServerResponse(
            message,
        ) => message.starts_with(UNEXPECTED_UNAUTHORIZED_PREFIX),
        _ => false,
    }
}

/// The tuned `reqwest::Client` `StreamableHttpClientTransport::from_config` would have built.
///
/// rmcp's `default_http_client` is **private**, so a port that needs to wrap the client — this one
/// does, twice: [`crate::request_headers_command::RequestHeadersCommandClient`] and, when section 05
/// lands it, `AuthClient` — has to rebuild it. Both settings are load-bearing and are copied from
/// `rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:381-387` with its reasons:
///
/// * `pool_max_idle_per_host(0)` — idle pooling causes ~40 ms stalls from TCP delayed ACK on Linux
///   when a previous response body was not fully consumed before reuse.
/// * `redirect(Policy::none())` — so caller-supplied custom headers (which here include resolved
///   **secrets**) cannot be replayed to a redirect target. This one is a security property, not a
///   latency one.
///
/// # Errors
///
/// [`McpError::Other`] when the TLS backend fails to initialise. rmcp `expect()`s here; a
/// connect-time failure is a better answer than a panic inside a session.
pub fn build_http_client() -> McpResult<reqwest::Client> {
    reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| McpError::other(format!("Failed to build the MCP HTTP client: {error}")))
}

/// `{ client, transport }` — the live half of a connection, as `server_manager`'s
/// [`ConnectionResource`].
///
/// # Why this is not `StdioChildConnection`
///
/// `server_manager::StdioChildConnection` adopts a raw `TokioChildProcess` and calls
/// `graceful_shutdown` on it. That shape only exists for a caller that never handed the transport to
/// rmcp. This builder *does* — `serve_client_with_lifecycle_and_ct` takes the transport **by value**
/// — so the child is owned by the `RunningService` from the handshake onward and `close()` has to go
/// through it. Closing the service cancels its task, which closes the transport, which is
/// `TokioChildProcess::close` → the same `graceful_shutdown`. One owner, one close.
pub struct McpConnection {
    /// `None` once shutdown has completed, which is what makes [`ConnectionResource::close`]
    /// idempotent. A `tokio::sync::Mutex` because the guard is held **across** the close await, for
    /// the reason `StdioChildConnection` states at length: a cancelled close must leave the service
    /// where the next caller finds it, not an empty slot that answers `Ok(())` having killed
    /// nothing.
    service: tokio::sync::Mutex<Option<RunningService<RoleClient, McpClientHandler>>>,
    identity: ClientIdentity,
    peer: Peer<RoleClient>,
    has_session_id: bool,
    pid: Option<u32>,
    /// `stderrTail` — `None` for HTTP and for `debug: true` stdio, both of which have no pipe.
    stderr: Option<StderrPump>,
}

/// The `"data"` listener of §3.3 step 8 and the task that drains it.
struct StderrPump {
    tail: Arc<std::sync::Mutex<VecDeque<u8>>>,
    task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// How long [`StderrPump::settle`] waits for the drain task to reach EOF before giving up on it.
///
/// Not an upstream constant — upstream has no wait at all; see [`StderrPump::settle`] for why this
/// port needs one and why the number is not load-bearing.
const STDERR_SETTLE: Duration = Duration::from_millis(250);

impl StderrPump {
    /// Let the drain task finish before the tail is read.
    ///
    /// **This is a named delta, and it exists because the port measured a race upstream cannot
    /// have.** Node delivers `"data"` on the same event loop that drives the transport, so by the
    /// time `createConnection`'s catch runs, everything the child already wrote has been appended
    /// and `stderrTail` is complete. Here the drain is a separate tokio task, and
    /// `connect_client`'s error can win the race — MEASURED: with the suite under load, the
    /// assertion that a failed connect carries `(…)` failed intermittently because the tail was
    /// still empty when the message was built.
    ///
    /// Waiting is correct rather than merely convenient: `serve_client_with_lifecycle_and_ct`
    /// closes the transport on every failure path, which kills the child, which EOFs the pipe, so
    /// the task this awaits is already finishing. [`STDERR_SETTLE`] bounds the one case where it is
    /// not — a child that survived the transport close — and a quarter of a second on a connect
    /// that has already failed is not a latency anyone can observe.
    async fn settle(&self) {
        let handle = self
            .task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let Some(handle) = handle else { return };
        let aborter = handle.abort_handle();
        // `timeout` DROPS the `JoinHandle` on expiry, and dropping a `JoinHandle` detaches rather
        // than cancels — hence the explicit abort. Without it a child that never EOFs leaves one
        // task parked on a pipe for the life of the process.
        if tokio::time::timeout(STDERR_SETTLE, handle).await.is_err() {
            aborter.abort();
        }
    }
}

impl std::fmt::Debug for McpConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpConnection")
            .field("pid", &self.pid)
            .field("has_session_id", &self.has_session_id)
            .field(
                "live",
                &self.service.try_lock().map_or(true, |slot| slot.is_some()),
            )
            .finish()
    }
}

impl McpConnection {
    /// The `Peer` every request goes through, for a caller that already holds the concrete type.
    ///
    /// A caller holding an `Arc<dyn ConnectionResource>` — which is all the manager ever has — takes
    /// [`ConnectionResource::peer`] instead; that impl is this method wrapped in `Some`, so the two
    /// hand back the same handle and this one is the single place the field is read. The trait
    /// method has to be an `Option` because resources with no client behind them implement it too;
    /// here the peer is a struct field set at the handshake, so the `Option` would always be `Some`
    /// and is not worth making a caller unwrap.
    #[must_use]
    pub fn peer(&self) -> &Peer<RoleClient> {
        &self.peer
    }

    /// The handler identity `list_changed` guards `ptr_eq` against (MCP-120).
    #[must_use]
    pub fn identity(&self) -> ClientIdentity {
        self.identity.clone()
    }
}

impl ConnectionResource for McpConnection {
    fn close(&self) -> BoxFuture<'_, McpResult<()>> {
        Box::pin(async move {
            if let Some(pump) = self.stderr.as_ref()
                && let Some(task) = pump
                    .task
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
            {
                task.abort();
            }
            let mut slot = self.service.lock().await;
            let Some(service) = slot.as_mut() else {
                return Ok(());
            };
            // `RunningService::close(&mut self)` cancels the service task and awaits it, and that
            // task is what runs `transport.close()`. `cancel(self)` would consume, which a
            // `&self` trait method cannot do — this is why the slot is an `Option` behind a mutex
            // rather than the service itself.
            let outcome = service.close().await;
            drop(slot.take());
            outcome.map(|_| ()).map_err(|join| {
                McpError::other(format!("MCP connection close failed: {join}"))
            })
        })
    }

    fn has_session_id(&self) -> bool {
        self.has_session_id
    }

    fn child_pid(&self) -> Option<u32> {
        self.pid
    }

    fn peer(&self) -> Option<&Peer<RoleClient>> {
        Some(McpConnection::peer(self))
    }

    fn stderr_detail(&self) -> Option<String> {
        let pump = self.stderr.as_ref()?;
        stderr_tail_detail(
            &pump
                .tail
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

/// `createConnection`'s two arms and the catch they share — the [`ConnectionFactory`] this port
/// installs in place of `UnbuiltConnectionFactory`.
///
/// Everything the manager reads off `this` inside `createConnection` is a field here:
/// `this.defaultCwd`, the client construction, and (through [`HttpAuthProvider`]) the
/// `authStorageOptions` / `oauthRuntime` the OAuth provider would be built from.
pub struct ConnectionBuilder {
    default_cwd: Option<PathBuf>,
    env: crate::credentials::EnvFn,
    /// `{ ...process.env }` — the map `resolveEnv` copies before layering the per-server overrides.
    /// A snapshot rather than a lookup because `StdioClientTransport`'s `env` option **replaces**
    /// the child environment, so the whole set has to be enumerable.
    base_env: HashMap<String, String>,
    home: PathBuf,
    handler: HandlerFactory,
    auth: Arc<dyn HttpAuthProvider>,
}

impl std::fmt::Debug for ConnectionBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionBuilder")
            .field("default_cwd", &self.default_cwd)
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

impl ConnectionBuilder {
    /// `new McpServerManager(defaultCwd)`'s half of the connection builder.
    ///
    /// `default_cwd` is upstream's `this.defaultCwd` — the session working directory, used only when
    /// `definition.cwd` is absent, and `None` reproduces the *"omit the key entirely"* arm of §3.3
    /// step 6.
    #[must_use]
    pub fn new(default_cwd: Option<PathBuf>) -> Self {
        Self {
            default_cwd,
            env: crate::credentials::process_env(),
            base_env: crate::secrets::process_env_snapshot(),
            home: crate::dirs::home_dir(),
            handler: bare_handler_factory(),
            auth: Arc::new(NoStoredCredentials),
        }
    }

    /// Install the manager's `createClient` — the hooks of §3.5, §3.10 and §3.16.
    ///
    /// **No caller yet.** This is the seam named in this module's builder note as the one thing the
    /// builder does not yet get; the handler bodies it installs are **section 05**'s
    /// (MCP-118 / 120 / 122 — sampling, `list_changed` refresh and elicitation-complete), so until
    /// one of those lands every runtime is built on [`bare_handler_factory`].
    #[must_use]
    pub fn with_handler_factory(mut self, handler: HandlerFactory) -> Self {
        self.handler = handler;
        self
    }

    /// Install the OAuth provider seam (MCP-115). See [`HttpAuthProvider`].
    #[must_use]
    pub fn with_auth_provider(mut self, auth: Arc<dyn HttpAuthProvider>) -> Self {
        self.auth = auth;
        self
    }

    /// Override `process.env` and `os.homedir()`. Tests only — production reads the real ones, and
    /// edition 2024 makes `std::env::set_var` `unsafe`, which is why this is a seam and not a
    /// global.
    ///
    /// `env` is the *lookup* (`interpolateEnvVars`, `resolveServerUrl`, `resolveBearerToken`) and
    /// `base_env` is the *snapshot* `resolveEnv` copies; upstream both are `process.env` and here
    /// they are separate because an `EnvFn` cannot be enumerated.
    #[must_use]
    pub fn with_environment(
        mut self,
        env: crate::credentials::EnvFn,
        base_env: HashMap<String, String>,
        home: PathBuf,
    ) -> Self {
        self.env = env;
        self.base_env = base_env;
        self.home = home;
        self
    }
}

/// What one turn of §3.4's ladder produced.
enum HttpAttempt {
    /// `{ status: "connected", client, transport }`.
    Connected(Arc<McpConnection>),
    /// `{ status: "failed", client, transport, error }`. The client and transport are **not**
    /// carried: rmcp's `serve_client_with_lifecycle_and_ct` has already closed the transport on
    /// every failure path, so there is nothing left for the ladder to hand back or close. That is
    /// the whole of the difference between this and upstream's tuple — see
    /// [`ConnectionBuilder::connect_http_client`]'s "what rmcp owns" note.
    /// Boxed because `ClientInitializeError` is ~376 bytes and the success arm is a pointer;
    /// `clippy::large_enum_variant` is right that an unboxed union would cost every attempt the
    /// size of its own worst case.
    Failed(Box<ClientInitializeError>),
    /// `requestOptions.timeout` elapsed before the handshake settled — a rejection upstream too
    /// (`SdkError(RequestTimeout, "Request timed out")`), so it takes the same ladder path a
    /// non-401 failure takes: past arm 3's abort check, past the 401 arms (it is not one), out at
    /// arm 7. It is a separate variant only because rmcp has no timeout `ClientInitializeError`.
    TimedOut,
}

/// `connectHttpClient`'s return — `{ client, transport, status, credentialsInvalidated }`.
#[derive(Debug)]
pub struct HttpConnection {
    /// The live half, or — on the `needs-auth` arm — a resource with nothing to close.
    pub resource: Arc<dyn ConnectionResource>,
    /// `"connected"` or `"needs-auth"`.
    pub status: ConnectionStatus,
    /// `credentialsInvalidated`, possibly set by this connect's own 401 handling (MCP-116).
    pub credentials_invalidated: bool,
}

impl ConnectionBuilder {
    /// The stdio arm of `createConnection` (`server-manager.ts:472-503`), §3.3.
    ///
    /// Upstream's order is load-bearing and is reproduced exactly:
    ///
    /// 1. `client = this.createClient(name, definition)` — which runs `resolveVersionNegotiation`,
    ///    so an invalid `protocolVersion` throws **before a child is spawned**. A port that built
    ///    the transport first would leave a process behind on that throw.
    /// 2. `args = (definition.args ?? []).map(interpolateEnvVars)` — `args` are interpolated and
    ///    **never** `!command`-resolved; `command` is neither.
    /// 3. the npx/npm rewrite (MCP-103 — see the marker below).
    /// 4. `throwIfAborted(signal)`.
    /// 5. `mkdirSync(definition.pluginDataDir, { recursive: true })`.
    /// 6. `cwd = resolveConfigPath(definition.cwd) ?? this.defaultCwd`.
    /// 7. `new StdioClientTransport({...})` — which is where `resolveEnv` runs, and therefore where
    ///    a `!command` env value executes.
    /// 8. the `"data"` listener that feeds the bounded stderr tail.
    ///
    /// Steps 5–7 are [`spawn_stdio_transport`] and [`StdioTransportSpec::resolve`]; this method is
    /// the ordering and the two rewrites.
    ///
    /// # Errors
    ///
    /// `Invalid MCP protocolVersion: …`; the abort reason; [`McpError::Io`] for the plugin data
    /// directory or the spawn; one of `resolveCommandSecret`'s five sentences for an `env` value;
    /// or the handshake failure itself.
    async fn connect_stdio(&self, request: &CreateConnection) -> McpResult<Arc<McpConnection>> {
        let entry = request.definition.as_ref();
        let name = request.name.as_str();

        // Step 1. `createClient` — the throw has to precede the spawn.
        let lifecycle = version_negotiation(entry)?;
        let handler = (self.handler)(name, &request.request);
        let identity = handler.identity();

        // Step 2. Note `command` is used raw: upstream maps `interpolateEnvVars` over `args` only.
        let command = entry.command.clone().unwrap_or_default();
        let args: Vec<String> = entry
            .args
            .iter()
            .flatten()
            .map(|arg| crate::credentials::interpolate_env_vars(arg, &self.env))
            .collect();

        // Step 3 — MCP-103, NOT PORTED. Upstream:
        //   if (command === "npx" || command === "npm") {
        //     const resolved = await resolveNpxBinary(command, args, signal);
        //     if (resolved) { command = resolved.isJs ? "node" : resolved.binPath;
        //                     args = resolved.isJs ? [resolved.binPath, ...resolved.extraArgs]
        //                                          : resolved.extraArgs;
        //                     logger.debug(`${name} resolved to ${resolved.binPath} (skipping npm parent)`); } }
        // `cyrup_ext::caps::proc::npx_resolver::resolve_npx_binary` is `pub(super)` in a crate this
        // unit does not own; MCP-103 is its `pub` promotion plus these five lines. Until then the
        // tracked child of an `npx` server is the npm launcher, and §3.12's single-pid kill leaves
        // the real server orphaned — the residual `StdioChildConnection`'s doc already names.

        // Step 4.
        crate::abort::throw_if_aborted(&request.attempt, None)?;

        // Step 6. `resolveConfigPath(definition.cwd) ?? this.defaultCwd` — and `None` is the
        // *"key omitted entirely"* arm, not `current_dir(".")`.
        let cwd = entry
            .cwd
            .as_deref()
            .map(|raw| PathBuf::from(crate::dirs::resolve_config_path(raw, &self.env, &self.home)))
            .or_else(|| self.default_cwd.clone());

        // Steps 5 + 7.
        //
        // `resolve` is moved off the async worker deliberately, and this is the one place in the
        // builder where that matters. `StdioTransportSpec::resolve` → `secrets::resolve_stdio_env`
        // → `resolve_command_secret` is a `std::process::Command` spawn polled with
        // `std::thread::sleep`, bounded by `COMMAND_SECRET_TIMEOUT` (10 s) and cancellable by
        // nothing. Run inline it would hold a tokio worker for up to ten seconds **inside the
        // manager's single-flight connect future** — a `futures::Shared` first polled on the
        // detached tail task (`server_manager.rs`'s `handle.spawn(tail)`) — during which
        // `close`/`close_all`'s abort could not preempt the attempt, which is precisely the
        // guarantee wave 4's concurrency tests were measured on (they were measured against
        // `UnbuiltConnectionFactory`, which returned instantly, so nothing there would have caught
        // it). Upstream's `spawnSync` blocks node's whole event loop, so leaving it inline would be
        // arguable *parity*; it is still the one way this `createConnection` body can weaken a
        // guarantee the rest of the crate relies on, so it does not stay inline.
        //
        // What `spawn_blocking` does NOT buy: the command itself is still uncancellable and still
        // runs to its own timeout after an abort. That is parity — a `spawnSync` cannot be
        // interrupted either — and the point of the wrapper is the worker thread, not the child.
        let spec = {
            let definition = Arc::clone(&request.definition);
            let server_name = name.to_string();
            let base_env = self.base_env.clone();
            match tokio::task::spawn_blocking(move || {
                StdioTransportSpec::resolve(
                    &definition,
                    &server_name,
                    command,
                    args,
                    cwd,
                    &base_env,
                )
            })
            .await
            {
                Ok(spec) => spec?,
                // The closure cannot panic under this crate's lint policy and the runtime is alive
                // (we are running on it), so this arm is defensive. Reported rather than unwrapped:
                // a `JoinError` here is a local failure, and it names the step it happened in.
                Err(_join) => {
                    return Err(McpError::Server {
                        server: name.to_string(),
                        message: "MCP stdio environment resolution failed to run".to_string(),
                    });
                }
            }
        };
        let (process, stderr) = spawn_stdio_transport(&spec)?;
        let pid = process.id();

        // Step 8. Started BEFORE the handshake: between the spawn and the first read the child can
        // already have filled the 64 KiB pipe, and a child blocked in `write` never answers
        // `initialize`. The task is aborted by `McpConnection::close`.
        let stderr = stderr.map(start_stderr_pump);

        // `await this.connectClientWithAbort(client, transport, requestOptions, signal)`.
        // `requestOptions` is split: its signal half is the `ct` argument, its timeout half is
        // [`connect_client_bounded`]'s budget. A wedged child — one that accepts the pipe and never
        // answers `initialize` — fails here at `requestTimeoutMs` instead of parking this future
        // and the manager's single-flight slot for that name forever.
        // The one instrumentation point for stdio. `maybe_traced` is a passthrough when the manager
        // handed no writer, so an untraced server pays one enum discriminant and nothing else.
        let process = crate::trace::maybe_traced(
            process,
            name,
            crate::trace::TraceTransportKind::Stdio,
            request.trace.clone(),
        );
        let handshake = connect_client_bounded(
            handler,
            process,
            lifecycle,
            request.attempt.clone(),
            request.request_options.as_ref().and_then(|options| options.timeout),
        )
        .await;
        let handshake = match handshake {
            Ok(handshake) => handshake,
            Err(_budget) => {
                // Same order as the failure arm below: let the drain settle so the tail is whole,
                // then report. The child is already dying — dropping the `connect_client` future
                // dropped the transport, and `ChildWithCleanup::drop` spawns `kill()`.
                if let Some(pump) = stderr.as_ref() {
                    pump.settle().await;
                }
                return Err(handshake_timeout_error(name, stderr.as_ref()));
            }
        };
        match handshake {
            Ok(service) => Ok(Arc::new(McpConnection {
                peer: service.peer().clone(),
                service: tokio::sync::Mutex::new(Some(service)),
                identity,
                has_session_id: false,
                pid,
                stderr,
            })),
            Err(error) => {
                // What happens to the child here, stated exactly, because "rmcp cleans up" is not
                // precise enough to rely on: `serve_client_with_ct_inner` holds the transport in a
                // local, so every `?` and the `tokio::select!` cancellation arm **drop** it — and
                // `TokioChildProcess`'s `ChildWithCleanup::drop`
                // (`rmcp-3.1.4/src/transport/child_process.rs:45-57`) spawns a fire-and-forget
                // `kill()`. NAMED DELTA: that is a SIGKILL with no graceful window, where upstream's
                // catch runs `client.close()` and the TS SDK escalates close-stdin → 2 s → SIGTERM →
                // 2 s → SIGKILL. A stdio server that would have flushed state on SIGTERM does not
                // get the chance when its *handshake* fails. It is bounded to the failed-connect
                // path — a successful connection is torn down through `McpConnection::close`, which
                // is `graceful_shutdown` — and `a_failed_handshake_leaves_no_child_behind` pins that
                // the child does die.
                //
                // What is NOT rmcp's at all is the drain task: it has to be allowed to finish before
                // the tail is read, and then stopped.
                if let Some(pump) = stderr.as_ref() {
                    pump.settle().await;
                }
                Err(initialize_error(name, &error, stderr.as_ref()))
            }
        }
    }

    /// `connectHttpClient(definition, serverName, requestOptions, signal, traceObserver,
    /// credentialsInvalidated)` (`server-manager.ts:826-970`) — §3.4 steps 1–7 and the attempt
    /// ladder, MCP-109/113/114/115/115a.
    ///
    /// # The ladder's arm order IS the specification
    ///
    /// 1. connected ⇒ return;
    /// 2. the abort-cleanup aggregate ⇒ **rethrow**, never retried;
    /// 3. `if (signal?.aborted) throwIfAborted(signal)`;
    /// 4. implicit-deferred + 401 ⇒ construct the provider and retry the **same** kind;
    /// 5. any other 401 ⇒ `needs-auth` when OAuth is supported (invalidating the credential cache
    ///    at most once per episode), else rethrow;
    /// 6. ~~SSE fallback~~ — Cut 1;
    /// 7. `throw result.error`.
    ///
    /// [`crate::oauth::on_unauthorized`] is arms 4 and 5 as one function; the order of everything
    /// around it is here.
    ///
    /// # What rmcp owns, and what that costs
    ///
    /// Upstream's `attempt` has its own catch that closes the failed attempt's client and, when
    /// *that* close fails, raises `MCP HTTP connection cleanup failed`
    /// ([`McpError::HttpCleanupFailed`]). In this port `serve_client_with_ct_inner` holds the
    /// transport in a local and **drops** it on every failure path — a drop cannot report a
    /// failure, so there is no separate cleanup outcome to observe: **neither [`McpError::HttpCleanupFailed`] nor
    /// [`McpError::AbortCleanupFailed`] has a producer on this path**, and arm 2 is therefore
    /// written-but-unreachable. That is MCP-123's residual verbatim ("the once-only cleanup handle
    /// for the HTTP retry ladder … and the cleanup-failure-versus-connect-failure distinction"),
    /// not something this unit closed and not something it can close from here.
    ///
    /// # Errors
    ///
    /// `resolveServerUrl`'s two sentences; any of `resolveCommandSecret`'s five; the
    /// invalid-header-value sentence; `Invalid MCP protocolVersion: …`; the request-headers-command
    /// validation sentences; the abort reason; or the handshake failure.
    pub async fn connect_http_client(
        &self,
        request: &CreateConnection,
    ) -> McpResult<HttpConnection> {
        let entry = request.definition.as_ref();
        let name = request.name.as_str();

        crate::abort::throw_if_aborted(&request.attempt, None)?;

        // Step 1. `const serverUrl = resolveServerUrl(definition)!` — throws before any secret
        // command is spawned for a URL that cannot resolve.
        let Some(server_url) =
            crate::credentials::resolve_server_url(entry.url.as_deref(), &self.env)?
        else {
            // `resolveServerUrl` returns `undefined` only for an absent `url`, and
            // `select_transport` has already established there is one. `!` upstream.
            return Err(McpError::Config(format!(
                "Server {name} must configure exactly one of command or url"
            )));
        };

        // Steps 2–6: `hasCommandHeader`, `resolveCommandSecretsRecord`, `commandBearer`, the bearer
        // ladder and the `new Headers()` injection guard — all of `crate::secrets`.
        let spec = HttpTransportSpec::resolve(entry, name, server_url.clone(), &self.env)?;

        // `server-manager.ts:868-870`: built **once per connect**, above `attempt`, and spread into
        // every attempt's transport options. The plan's cyrup note says "inside the retry closure";
        // the .ts says outside, and outside is what makes the eager `resolvedCommand(config)`
        // validation (`request-headers-command.ts:309`) fail the CONNECT exactly once rather than
        // once per attempt. The decorator is reused across attempts because the thing that must be
        // fresh per attempt is the MCP client, not the HTTP one.
        let http_client = UnauthorizedProbe::new(build_http_client()?);
        let signing_client = match entry.request_headers_command.clone() {
            Some(config) => Some(crate::request_headers_command::RequestHeadersCommandClient::new(
                http_client.clone(),
                config,
                Some(request.attempt.clone()),
            )?),
            None => None,
        };

        // `let authState: HttpAuthProviderState = supportsOAuth(definition) ? … : { disabled }`.
        let mut auth_state = crate::oauth::initial_http_auth_state(entry);
        let mut invalidated = request.credentials_invalidated;

        loop {
            // `const authProvider = "provider" in authState ? authState.provider : undefined;`
            // and, with it, the token that provider would attach. Explicit OAuth reads the store on
            // the FIRST attempt; implicit OAuth reaches here only after a 401 has proven it needed
            // to — which is the whole point of the two-state split (MCP-115).
            let oauth_token = match &auth_state {
                crate::oauth::HttpAuthProviderState::Explicit => {
                    self.auth.authorize(name, &server_url, None).await?
                }
                crate::oauth::HttpAuthProviderState::ImplicitChallenged { challenge } => {
                    self.auth
                        .authorize(name, &server_url, challenge.as_deref())
                        .await?
                }
                crate::oauth::HttpAuthProviderState::Disabled
                | crate::oauth::HttpAuthProviderState::ImplicitDeferred => None,
            };

            let attempt = self
                .http_attempt(request, &spec, entry, &http_client, signing_client.as_ref(), oauth_token)
                .await?;

            let failure = match attempt {
                // Arm 1.
                HttpAttempt::Connected(connection) => {
                    return Ok(HttpConnection {
                        resource: connection,
                        status: ConnectionStatus::Connected,
                        credentials_invalidated: invalidated,
                    });
                }
                HttpAttempt::Failed(error) => Some(error),
                // The handshake budget elapsed. Carried to the same place a non-401 failure goes,
                // rather than returned from `http_attempt`, so arm 3 still runs first: a `close`
                // racing a timeout is an abort upstream, because `attempt`'s rejection reaches the
                // ladder and the aborted check precedes everything that inspects the error.
                HttpAttempt::TimedOut => None,
            };

            // Arm 2 — `if (result.error instanceof AggregateError && message === "MCP connection
            // abort cleanup failed") throw result.error;`. Unreachable here; see the method doc.

            // Arm 3 — `if (signal?.aborted) throwIfAborted(signal);`. Before the 401 arms, so a
            // close racing a 401 is an abort and not a `needs-auth`.
            crate::abort::throw_if_aborted(&request.attempt, None)?;

            let Some(error) = failure else {
                // Arm 7 for the timeout: it is not a 401, so no OAuth arm can claim it.
                return Err(handshake_timeout_error(name, None));
            };

            // Arms 4 and 5.
            let Some(challenge) = unauthorized_challenge(&error) else {
                // Arm 6 is Cut 1; arm 7 — `throw result.error`.
                return Err(initialize_error(name, &error, None));
            };
            match crate::oauth::on_unauthorized(&auth_state, Some(challenge)) {
                crate::oauth::UnauthorizedAction::RetryOnce(next) => {
                    auth_state = next;
                    continue;
                }
                crate::oauth::UnauthorizedAction::NeedsAuth => {
                    // MCP-116's once-per-episode guard. The flag rides back out on the connection
                    // record and returns on the next `connect`, so a retry loop cannot repeatedly
                    // discard a good cached credential.
                    if !invalidated {
                        self.auth.invalidate_auth_entry_cache(name);
                        invalidated = true;
                    }
                    return Ok(HttpConnection {
                        // Upstream returns the failed attempt's `{client, transport}` here so the
                        // manager can close them later; rmcp has already dropped both by the time
                        // the error surfaces, so the honest resource is one with nothing to close.
                        // The consequence is bounded: an HTTP transport's drop closes its worker
                        // and its reqwest connections, and there is no child process on this arm.
                        resource: crate::server_manager::InertResource::new(),
                        status: ConnectionStatus::NeedsAuth,
                        credentials_invalidated: invalidated,
                    });
                }
                crate::oauth::UnauthorizedAction::HardError => {
                    return Err(initialize_error(name, &error, None));
                }
            }
        }
    }

    /// One turn of the ladder — upstream's `attempt(kind)` closure, minus the `kind` (Cut 1 leaves
    /// one transport).
    ///
    /// A fresh MCP client per attempt, exactly as upstream: `const client =
    /// this.createClient(serverName, definition)` is *inside* `attempt`, so the retry after an
    /// implicit-OAuth challenge does not reuse a client that has already seen a failed
    /// `initialize`.
    async fn http_attempt(
        &self,
        request: &CreateConnection,
        spec: &HttpTransportSpec,
        entry: &ServerEntry,
        http_client: &UnauthorizedProbe,
        signing_client: Option<
            &crate::request_headers_command::RequestHeadersCommandClient<UnauthorizedProbe>,
        >,
        oauth_token: Option<String>,
    ) -> McpResult<HttpAttempt> {
        let name = request.name.as_str();
        let mut config = build_http_transport_config(spec)?;

        // `transportOptions.authProvider`'s effect on the wire. PRECEDENCE, measured from the
        // pinned SDK rather than assumed: `StreamableHTTPClientTransport._commonHeaders`
        // (`node_modules/@modelcontextprotocol/client/dist/index.mjs:5034-5049`) writes the
        // provider's `Authorization` into a plain object and then spreads `requestInit.headers`
        // OVER it inside a single `new Headers({...headers, ...extraHeaders})`. One `Headers`
        // object, therefore exactly one `Authorization` value, and the CONFIGURED one wins.
        //
        // # Why this needs two clauses and not one
        //
        // **rmcp carries the bearer in a separate `auth_header` channel from the custom-header
        // map, so every path that can produce an `Authorization` has to clear the other channel
        // explicitly.** `auth_header` is applied with `RequestBuilder::bearer_auth` and the map with
        // `builder.header(name, value)` — and both of those APPEND
        // (`rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:196-203`, `:29-37`).
        // Upstream has one channel and `set` semantics; this port has two channels and `append`
        // semantics, so parity is not the default and has to be restored at every producer. This is
        // the THIRD instance of the same defect class: wave 2 fixed it one layer down in
        // `secrets::resolve_http_secrets` (a resolved `bearerToken` strips a configured
        // `Authorization` out of the map), `request_headers_command::apply_derived` fixed it for a
        // signing command's derived header, and this is the OAuth token's.
        //
        // MEASURED before this clause existed, entry `{url, auth:"oauth", headers:{"Authorization":
        // "Static abc123"}}` with a provider returning `from-store`, against a loopback fixture
        // recording raw request bytes:
        //   authorization values = ["Bearer from-store", "Static abc123"]
        // — two values where upstream sends one, i.e. a credential the config did not name was
        // additionally disclosed to the server.
        //
        // The combination is reachable by config, not hypothetical: `supports_oauth` answers true
        // for `auth: "oauth"` **before** any custom-header gate (`crate::oauth::supports_oauth`,
        // mirroring `mcp-auth-flow.ts:940-953`, whose own doc says "even with custom headers"), and
        // `auth: "oauth"` means no `spec.bearer_token`, so nothing else strips the map's entry. A
        // previous revision of this comment argued the collision was impossible because
        // `supportsOAuth` is false whenever `auth === "bearer"`; that reasoning covers only the
        // `auth_header` channel and misses the `custom_headers` one entirely.
        if config
            .custom_headers
            .contains_key(&http::header::AUTHORIZATION)
        {
            // The configured header wins, matching the spread order above. `oauth_token` is dropped
            // rather than installed — dropping it is what makes the count on the wire one.
        } else if config.auth_header.is_none() {
            config.auth_header = oauth_token;
        }

        // `skipIssuerMetadataValidation` is gated on a provider EXISTING upstream
        // (`server-manager.ts:898-903`)
        // (`authProvider !== undefined && definition.oauth !== false &&
        //   definition.oauth?.skipIssuerMetadataValidation === true`). rmcp's streamable-HTTP
        // transport config has no such field — issuer-metadata validation belongs to
        // `rmcp::transport::auth`'s discovery, which is section 05's — so the flag is read here and
        // handed to nothing.
        //
        // RECORDED, and now live rather than predicted: the provider IS wired
        // ([`StoredCredentialAuth`], installed at `initialize_mcp`), and this flag reaches it only
        // through [`crate::oauth::get_valid_token`]'s refresh path — not through discovery on a
        // first login. So a server that genuinely needs `skipIssuerMetadataValidation` still fails
        // its first authorization-server discovery. Closing that means threading the flag into the
        // discovery call, and this is the line that has to grow the argument.
        let _skip_issuer_metadata_validation = skip_issuer_metadata_validation(entry);

        // `this.createClient(serverName, definition)` — fresh per attempt.
        let lifecycle = version_negotiation(entry)?;
        let handler = (self.handler)(name, &request.request);
        let identity = handler.identity();

        // `requestOptions.timeout`, per attempt — upstream's object is built once and passed to
        // every `client.connect`, so each turn of the ladder gets the full budget rather than a
        // share of one. See [`connect_client_bounded`].
        let budget = request.request_options.as_ref().and_then(|options| options.timeout);
        // [`SessionIdProbe`] wraps whichever client this attempt uses, so `has_session_id` below is
        // a read of what the server actually sent rather than a constant. The flag is cloned out
        // BEFORE the probe is handed to the transport, which takes its client by value.
        let (outcome, session_id) = match signing_client {
            Some(signing) => {
                let probe = SessionIdProbe::new(signing.clone());
                let session_id = probe.flag();
                (
                    connect_client_bounded(
                        handler,
                        crate::trace::maybe_traced(
                            http_transport_with_client(probe, config),
                            &request.name,
                            crate::trace::TraceTransportKind::StreamableHttp,
                            request.trace.clone(),
                        ),
                        lifecycle,
                        request.attempt.clone(),
                        budget,
                    )
                    .await,
                    session_id,
                )
            }
            None => {
                let probe = SessionIdProbe::new(http_client.clone());
                let session_id = probe.flag();
                (
                    connect_client_bounded(
                        handler,
                        crate::trace::maybe_traced(
                            http_transport_with_client(probe, config),
                            &request.name,
                            crate::trace::TraceTransportKind::StreamableHttp,
                            request.trace.clone(),
                        ),
                        lifecycle,
                        request.attempt.clone(),
                        budget,
                    )
                    .await,
                    session_id,
                )
            }
        };

        let Ok(outcome) = outcome else {
            return Ok(HttpAttempt::TimedOut);
        };

        Ok(match outcome {
            Ok(service) => HttpAttempt::Connected(Arc::new(McpConnection {
                peer: service.peer().clone(),
                service: tokio::sync::Mutex::new(Some(service)),
                identity,
                // `(transport as {sessionId?: string})?.sessionId != null` (`session-recovery.ts`)
                // — a LIVE read, off the `Mcp-Session-Id` the handshake response carried. See
                // [`SessionIdProbe`] for why it is observed at the client rather than the
                // transport, and for the session-recovery bug the previous hardcoded `true` caused
                // for stateless servers.
                has_session_id: session_id.load(std::sync::atomic::Ordering::Relaxed),
                pid: None,
                stderr: None,
            })),
            Err(error) => HttpAttempt::Failed(error),
        })
    }
}

/// `definition.oauth !== false && definition.oauth?.skipIssuerMetadataValidation === true` —
/// `server-manager.ts:898-903`. See `ConnectionBuilder::http_attempt` for why it currently has no
/// consumer.
#[must_use]
pub fn skip_issuer_metadata_validation(entry: &ServerEntry) -> bool {
    matches!(
        entry.oauth.as_ref(),
        Some(crate::config::OAuthSetting::Config(config))
            if config.skip_issuer_metadata_validation == Some(true)
    )
}

/// Start the §3.3 step-8 drain task over a piped `stderr`.
fn start_stderr_pump(mut stderr: ChildStderr) -> StderrPump {
    let tail = Arc::new(std::sync::Mutex::new(VecDeque::new()));
    let sink = Arc::clone(&tail);
    // `tokio::spawn` panics off-runtime and this crate denies `clippy::panic`; no runtime means no
    // drain, which degrades the tail rather than the connection.
    let task = tokio::runtime::Handle::try_current().ok().map(|handle| {
        handle.spawn(async move {
            use tokio::io::AsyncReadExt as _;
            let mut buffer = [0_u8; 8192];
            loop {
                match stderr.read(&mut buffer).await {
                    Ok(0) | Err(_) => return,
                    Ok(read) => {
                        let Some(chunk) = buffer.get(..read) else { return };
                        let mut tail =
                            sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        append_stderr_tail(&mut tail, chunk);
                    }
                }
            }
        })
    });
    StderrPump {
        tail,
        task: std::sync::Mutex::new(task),
    }
}

/// `createConnection`'s catch, §3.3 step 8: the failure message with the child's last three
/// non-empty stderr lines appended as `(a — b — c)`.
///
/// In `debug: true` mode stderr is inherited by the host terminal, so there is no tail and no
/// suffix — which is why `stderr` is an `Option` rather than an always-present buffer.
fn initialize_error(
    server: &str,
    error: &ClientInitializeError,
    stderr: Option<&StderrPump>,
) -> McpError {
    let base = error.to_string();
    let message = match stderr {
        Some(pump) => with_stderr_tail(
            &base,
            &pump
                .tail
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        ),
        None => base,
    };
    McpError::Server {
        server: server.to_string(),
        message,
    }
}

// -------------------------------------------------------------------------------------------------
// MCP-119 — discovery: `tools/list`, `resources/list`, `prompts/list` (§3.9)
// -------------------------------------------------------------------------------------------------

/// One list call's failure: what the request itself returned, or `requestOptions.timeout` expiring.
///
/// The two are kept apart because they answer the 401 predicate differently — a timeout is never a
/// 401, and folding it into `ServiceError` would make [`unauthorized_list_failure`] guess.
#[derive(Debug)]
enum ListFailure {
    /// `requestOptions.timeout` elapsed with no response.
    ///
    /// Carries nothing: the budget is not in the message upstream produces — `SdkError`'s
    /// constructor is `super(message)` and the timeout rides in `data` — and it is already known to
    /// the only code that could want it, which computed it two lines earlier.
    Timeout,
    /// rmcp answered, and the answer was an error.
    Service(ServiceError),
}

impl std::fmt::Display for ListFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The SDK's per-request timer message, byte for byte — the same one the handshake's
            // budget produces, because upstream arms the same timer over both.
            Self::Timeout => f.write_str(HANDSHAKE_TIMED_OUT),
            Self::Service(error) => write!(f, "{error}"),
        }
    }
}

/// `isUnauthorizedHttpError(error)` against a discovery failure.
///
/// A 401 that arrives *after* the handshake is not a [`ClientInitializeError`]; rmcp delivers it as
/// [`ServiceError::TransportSend`] around a `DynamicTransportError` whose boxed `error` is the very
/// same transport error [`unauthorized_challenge`] inspects. So the chain walk is shared and only
/// the unwrapping differs. Every other `ServiceError` — a protocol error from the server, a closed
/// transport, an unexpected response — is not a 401 and must not be treated as one, or a server
/// that merely errors on `resources/list` would be sent down the OAuth path.
fn unauthorized_list_failure(failure: &ListFailure) -> bool {
    match failure {
        ListFailure::Timeout => false,
        ListFailure::Service(ServiceError::TransportSend(error)) => {
            unauthorized_in_chain(error.error.as_ref()).is_some()
        }
        ListFailure::Service(_) => false,
    }
}

/// One `list_all_*` call under `requestOptions.timeout`.
///
/// # NAMED DELTA — what the budget covers
///
/// `CreateConnection::request_options` is upstream's single `RequestOptions` object, handed to
/// every `client.listTools(cursor, requestOptions)` call, so upstream arms the timer **per page**.
/// rmcp's `Peer::list_all_*` own their cursor loops (13c's MCP-119 row names them as the port of
/// `fetchAll*`) and take no per-request options, so the budget is armed around the **whole list**
/// instead: a paginated catalog gets one budget for all its pages rather than one per page. That is
/// strictly stricter than upstream, never looser, and it is what keeps a server that answers
/// `initialize` and then stalls on `tools/list` from parking the connect — and with it the
/// manager's per-name single-flight slot — for as long as the child lives.
///
/// A dropped list future is safe to abandon: with no subscription and no progress-reset watcher,
/// `send_request_with_option_and_subscription` registers nothing in the peer's side tables
/// (`rmcp-3.1.4/src/service.rs:859-931`), so the only thing left behind is one oneshot the service
/// loop drops when the late response arrives.
///
/// The one thing upstream's per-request timer does that this does not is send
/// `notifications/cancelled` for the abandoned request; rmcp only emits that from
/// `RequestHandle::await_response`'s own timeout arm, which the typed `list_all_*` helpers do not
/// route through. A server that keeps building a tool list nobody will read is the cost, bounded by
/// the connection being torn down immediately afterwards.
async fn bounded_list<T, F>(budget: Option<Duration>, list: F) -> Result<T, ListFailure>
where
    F: std::future::Future<Output = Result<T, ServiceError>>,
{
    let Some(budget) = budget else {
        return list.await.map_err(ListFailure::Service);
    };
    match tokio::time::timeout(budget, list).await {
        Ok(outcome) => outcome.map_err(ListFailure::Service),
        Err(_elapsed) => Err(ListFailure::Timeout),
    }
}

/// `Promise.all([fetchAllTools, fetchAllResources, fetchAllPrompts])` and the `instructions` read
/// that precedes it (§3.9) — the body of MCP-119.
///
/// # The three per-list failure policies, which are not the same policy
///
/// * **tools** — `fetchAllTools` has no `catch` at all. Unconditional, and every error propagates
///   into `createConnection`'s catch, which closes the half-built connection. A server whose
///   `tools/list` fails does not become a connection with no tools; it fails to connect.
/// * **resources** — capability-gated on `serverCapabilities.resources`, and when the capability is
///   absent **no request is sent**. On error: rethrow if the request signal aborted, rethrow a 401,
///   otherwise swallow to `[]` with no log.
/// * **prompts** — capability-gated on `.prompts`. Same two rethrows, otherwise `[]` **plus**
///   `prompt_discovery_failed = true` and one `debug`-level line. The flag is the whole point: it
///   is what stops `init.ts`'s §12 pass from publishing an empty prompt map as authoritative.
///
/// # `join!`, deliberately, not `try_join!`
///
/// `try_join!` short-circuits on the first error and drops its siblings mid-flight. That would let
/// a resources failure — which upstream *swallows* — cancel the tools list, turning a connection
/// that should have succeeded with `resources: []` into a hard failure. The three policies above
/// only exist if all three futures are allowed to finish. This is the one place in the file where
/// the choice of combinator is a behavioural decision rather than a stylistic one.
///
/// # Errors
///
/// [`McpError::Aborted`] when the request signal fired, and [`McpError::Server`] — carrying the
/// child's stderr tail, §3.3 step 8 — for a tools failure or a rethrown 401.
async fn discover(
    request: &CreateConnection,
    resource: &dyn ConnectionResource,
) -> McpResult<Discovery> {
    // `throwIfAborted(signal)`. Upstream's is at the top of the try; the one that matters after a
    // successful handshake is this — a `close` that raced the handshake must not leave a live child
    // behind just because it arrived a microsecond late, and it must not spend a `tools/list` on a
    // connection that is already being torn down.
    crate::abort::throw_if_aborted(&request.attempt, None)?;

    let name = request.name.as_str();
    let Some(peer) = resource.peer() else {
        // Not reachable from either connect arm — both hand `post_handshake` an `McpConnection`,
        // whose `peer()` is a struct field set at the handshake. It is an error rather than an
        // empty catalog because the two are not the same claim: `[]` would assert that the server
        // was asked and answered nothing, and this is the case where it was never asked.
        return Err(discovery_error(
            name,
            "MCP discovery cannot run: the connection carries no client",
            resource,
        ));
    };

    // `client.getServerCapabilities?.()` and `client.getInstructions?.()` — both off the
    // `InitializeResult` rmcp stored at the handshake. `None` is a peer that has not completed one,
    // which cannot happen here; reading it as "advertised nothing" keeps the gate closed, which is
    // the safe direction: an ungated `resources/list` against a server that never offered the
    // capability is a request upstream provably does not send.
    let info = peer.peer_info();
    let (has_resources, has_prompts, instructions) = match info.as_deref() {
        Some(info) => (
            info.capabilities.resources.is_some(),
            info.capabilities.prompts.is_some(),
            info.instructions.clone(),
        ),
        None => (false, false, None),
    };

    let budget = request.request_options.as_ref().and_then(|options| options.timeout);
    let (tools, resources_result, prompts_result) = tokio::join!(
        bounded_list(budget, peer.list_all_tools()),
        async {
            if !has_resources {
                return None;
            }
            Some(bounded_list(budget, peer.list_all_resources()).await)
        },
        async {
            if !has_prompts {
                return None;
            }
            Some(bounded_list(budget, peer.list_all_prompts()).await)
        },
    );

    // `if (requestOptions?.signal?.aborted) throwIfAborted(requestOptions.signal)` — upstream tests
    // the REQUEST signal, not the attempt controller's, and `CreateConnection::request` is exactly
    // that token. Hoisted above the three reductions because it is the same test in both catches
    // and it must win over the swallow-to-`[]` arms: an aborted discovery is a failure, not a
    // server with no resources.
    let aborted = request.request.is_cancelled();

    // `fetchAllTools` — no catch. Errors propagate.
    let tools = tools.map_err(|failure| {
        if aborted {
            return McpError::Aborted(crate::abort::ABORTED_FALLBACK_REASON.to_string());
        }
        discovery_error(name, &failure.to_string(), resource)
    })?;

    // `fetchAllResources` — swallow to `[]`, except on abort and 401.
    let resources = match resources_result {
        Some(Ok(resources)) => resources,
        // The capability was absent, so nothing was sent and `[]` is the answer with no request on
        // the wire — which is the observable half of the gate.
        None => Vec::new(),
        Some(Err(failure)) => {
            if aborted {
                return Err(McpError::Aborted(
                    crate::abort::ABORTED_FALLBACK_REASON.to_string(),
                ));
            }
            if unauthorized_list_failure(&failure) {
                return Err(discovery_error(name, &failure.to_string(), resource));
            }
            Vec::new()
        }
    };

    // `fetchAllPrompts` — same two rethrows, and the `failed` flag on everything else.
    let (prompts, prompt_discovery_failed) = match prompts_result {
        Some(Ok(prompts)) => (prompts, false),
        None => (Vec::new(), false),
        Some(Err(failure)) => {
            if aborted {
                return Err(McpError::Aborted(
                    crate::abort::ABORTED_FALLBACK_REASON.to_string(),
                ));
            }
            if unauthorized_list_failure(&failure) {
                return Err(discovery_error(name, &failure.to_string(), resource));
            }
            // BYTE-EXACT: `` logger.debug(`MCP: prompts/list failed: ${message}`) ``.
            tracing::debug!("MCP: prompts/list failed: {failure}");
            (Vec::new(), true)
        }
    };

    Ok(Discovery {
        tools,
        resources,
        prompts,
        prompt_discovery_failed,
        instructions,
    })
}

/// [`initialize_error`]'s counterpart for a failure raised after the handshake, where the stderr
/// tail is reachable through the resource rather than through a local [`StderrPump`].
///
/// Same §3.3 step-8 enrichment, same `(a — b — c)` rendering: upstream's catch appends the tail to
/// whatever error reaches it, and a discovery failure reaches exactly that catch. For an HTTP
/// connection [`ConnectionResource::stderr_detail`] is `None` and the message is the base text.
fn discovery_error(server: &str, base: &str, resource: &dyn ConnectionResource) -> McpError {
    let message = match resource.stderr_detail() {
        Some(detail) => format!("{base} ({detail})"),
        None => base.to_string(),
    };
    McpError::Server {
        server: server.to_string(),
        message,
    }
}

impl ConnectionFactory for ConnectionBuilder {
    fn create(&self, request: CreateConnection) -> BoxFuture<'static, McpResult<NewConnection>> {
        // The builder is cheap to clone by `Arc` and the future must be `'static`; every field is
        // either an `Arc` or a small owned value.
        let builder = Self {
            default_cwd: self.default_cwd.clone(),
            env: Arc::clone(&self.env),
            base_env: self.base_env.clone(),
            home: self.home.clone(),
            handler: Arc::clone(&self.handler),
            auth: Arc::clone(&self.auth),
        };
        Box::pin(async move { builder.create_connection(request).await })
    }
}

impl ConnectionBuilder {
    /// `createConnection` end to end — transport selection, the two arms, and the shared catch.
    ///
    /// # The shared catch, and what produces the arm it exists for
    ///
    /// Upstream's catch closes the half-built connection and, when *that* close fails, wraps
    /// everything in `MCP connection setup failed` ([`McpError::SetupFailed`]). Reaching that arm
    /// needs a post-handshake step that can fail, and upstream's is **discovery**
    /// (`Promise.all([fetchAllTools, fetchAllResources, fetchAllPrompts])`, MCP-119). Since that
    /// unit landed this builder has it: [`Self::post_handshake`] runs [`discover`] inside the try,
    /// so a server whose `tools/list` fails and whose subsequent `close` *also* fails raises
    /// `SetupFailed` on the ordinary path.
    ///
    /// The narrower producer that predates it is still there and still real: the abort check at the
    /// top of [`discover`] is itself a post-handshake step that can fail, and when a `close` racing
    /// a successful handshake trips it *and* the `resource.close()` after it also fails
    /// (`McpConnection::close` returns `Err` on a `JoinError` from `service.close()`), the same
    /// wrapper fires.
    ///
    /// `McpServerManager::close_inner`'s pending-connect rethrow arm consumes it and is live:
    /// MCP-124 landed the variant, [`McpError::is_cleanup_failure`] matches it, and
    /// `From<&ManagerError>` keeps the class across the public boundary.
    async fn create_connection(&self, request: CreateConnection) -> McpResult<NewConnection> {
        crate::abort::throw_if_aborted(&request.attempt, None)?;

        // `configuredTransports.length !== 1` — MCP-113, already ported.
        match select_transport(&request.name, request.definition.as_ref())? {
            TransportKind::Stdio => {
                let connection = self.connect_stdio(&request).await?;
                self.post_handshake(&request, connection as Arc<dyn ConnectionResource>, false)
                    .await
            }
            TransportKind::StreamableHttp => {
                let http = self.connect_http_client(&request).await?;
                if http.status == ConnectionStatus::NeedsAuth {
                    // `if (httpConnection.status === "needs-auth") return { … }` — the early return
                    // skips the try block entirely, so no discovery and no notification handlers.
                    return Ok(NewConnection {
                        resource: http.resource,
                        status: ConnectionStatus::NeedsAuth,
                        credentials_invalidated: http.credentials_invalidated,
                        // Nothing was listed: the early return skips the try block, so there is no
                        // `tools/list` on this arm and `[]` is a fact rather than a placeholder.
                        discovery: Discovery::default(),
                    });
                }
                self.post_handshake(&request, http.resource, http.credentials_invalidated)
                    .await
            }
        }
    }

    /// The body of `createConnection`'s `try` after the handshake, and its catch.
    ///
    /// The try is [`discover`] — the abort check, `client.getInstructions?.()` and the `Promise.all`
    /// over the three lists (§3.9). Everything it raises flows into the catch below.
    ///
    /// Still **not** here, and named so a reader does not assume otherwise:
    /// `attachAdapterNotificationHandlers` and the identity-guarded `client.onclose`, which are
    /// MCP-120's and MCP-131's respectively. Their absence costs a refresh on `list_changed`, not a
    /// catalog: what a server publishes at connect time is now fetched and installed.
    async fn post_handshake(
        &self,
        request: &CreateConnection,
        resource: Arc<dyn ConnectionResource>,
        credentials_invalidated: bool,
    ) -> McpResult<NewConnection> {
        let error = match discover(request, resource.as_ref()).await {
            Ok(discovery) => {
                return Ok(NewConnection {
                    resource,
                    status: ConnectionStatus::Connected,
                    credentials_invalidated,
                    discovery,
                });
            }
            Err(error) => error,
        };

        // `const cleanupResults = await Promise.allSettled([abortCleanup ?? client.close()]);`
        let mut cleanup_failures: Vec<McpError> = Vec::new();
        if let Err(cleanup) = resource.close().await {
            cleanup_failures.push(cleanup);
        }
        if cleanup_failures.is_empty() {
            return Err(error);
        }
        // `reportedError = new AggregateError([error, ...cleanupFailures], "MCP connection setup failed")`
        let mut children = Vec::with_capacity(cleanup_failures.len() + 1);
        children.push(error);
        children.append(&mut cleanup_failures);
        Err(McpError::SetupFailed(crate::errors::CleanupErrors::from(
            children,
        )))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod wire_tests {
    use super::*;

    fn entry() -> ServerEntry {
        ServerEntry::default()
    }

    // --- MCP-113 --------------------------------------------------------------------------------

    #[test]
    fn empty_string_counts_as_unconfigured() {
        let mut e = entry();
        e.command = Some(String::new());
        e.url = Some("http://x".to_string());
        assert_eq!(select_transport("s", &e).unwrap(), TransportKind::StreamableHttp);
    }

    #[test]
    fn two_transports_and_none_produce_the_exact_message() {
        let expected = "Server s must configure exactly one of command or url";

        let mut both = entry();
        both.command = Some("a".to_string());
        both.url = Some("b".to_string());
        assert_eq!(select_transport("s", &both).unwrap_err().to_string(), expected);

        assert_eq!(select_transport("s", &entry()).unwrap_err().to_string(), expected);
    }

    #[test]
    fn legacy_sse_is_refused_with_the_cut_one_diagnostic_and_opens_nothing() {
        let mut e = entry();
        e.url = Some("http://x".to_string());
        e.http_transport = Some(HttpTransport::Sse);
        let err = select_transport("s", &e).unwrap_err();
        assert_eq!(err.to_string(), sse_cut_diagnostic("s"));
        assert!(err.to_string().contains("legacy HTTP+SSE transport"));
    }

    #[test]
    fn streamable_http_is_the_accepted_value() {
        let mut e = entry();
        e.url = Some("http://x".to_string());
        e.http_transport = Some(HttpTransport::StreamableHttp);
        assert_eq!(select_transport("s", &e).unwrap(), TransportKind::StreamableHttp);
    }

    #[test]
    fn the_two_cut_diagnostics_name_their_cut_subject() {
        assert!(socket_cut_diagnostic("s").contains("configures `socket`"));
        assert!(socket_cut_diagnostic("s").contains("streamable HTTP (`url`)"));
    }

    // --- MCP-102 --------------------------------------------------------------------------------

    #[test]
    fn a_one_mib_burst_is_bounded_before_it_is_appended() {
        let burst = vec![b'x'; 1024 * 1024];
        assert_eq!(bounded_stderr_chunk(&burst).len(), MAX_CAPTURED_STDERR_BYTES);

        let mut tail = VecDeque::new();
        append_stderr_tail(&mut tail, &burst);
        append_stderr_tail(&mut tail, &burst);
        assert_eq!(tail.len(), MAX_CAPTURED_STDERR_BYTES);
    }

    #[test]
    fn a_split_multibyte_sequence_becomes_a_replacement_char_not_a_panic() {
        // 8 KiB + 1 of a 3-byte sequence: the boundary lands mid-character.
        let mut burst = vec![b'a'; MAX_CAPTURED_STDERR_BYTES - 1];
        burst.extend_from_slice("€".as_bytes());
        let mut tail = VecDeque::new();
        append_stderr_tail(&mut tail, &burst);
        assert_eq!(tail.len(), MAX_CAPTURED_STDERR_BYTES);
        // Whatever survives must still decode; `from_utf8_lossy` is what Node's toString does.
        let detail = stderr_tail_detail(&tail).unwrap();
        assert!(!detail.is_empty());
    }

    #[test]
    fn the_suffix_is_the_last_three_non_empty_lines_joined_by_an_em_dash() {
        let mut tail = VecDeque::new();
        append_stderr_tail(&mut tail, b"zero\r\n\n  one  \r\ntwo\nthree\n\n");
        assert_eq!(stderr_tail_detail(&tail).as_deref(), Some("one \u{2014} two \u{2014} three"));
        assert_eq!(with_stderr_tail("boom", &tail), "boom (one \u{2014} two \u{2014} three)");
    }

    #[test]
    fn an_empty_or_whitespace_tail_adds_no_suffix() {
        let empty = VecDeque::new();
        assert_eq!(stderr_tail_detail(&empty), None);
        assert_eq!(with_stderr_tail("boom", &empty), "boom");

        let mut blank = VecDeque::new();
        append_stderr_tail(&mut blank, b"   \n\r\n  ");
        assert_eq!(stderr_tail_detail(&blank), None, "debug-mode parity: no tail, no `(...)`");
    }

    // --- MCP-117 --------------------------------------------------------------------------------

    #[test]
    fn absent_and_legacy_are_the_same_lifecycle() {
        let absent = version_negotiation(&entry()).expect("no revision pinned");
        let mut legacy = entry();
        legacy.protocol_version = Some(ProtocolVersionSetting::Legacy);
        assert!(matches!(absent, ClientLifecycleMode::Initialize));
        assert!(matches!(
            version_negotiation(&legacy).expect("legacy"),
            ClientLifecycleMode::Initialize
        ));
    }

    #[test]
    fn auto_and_pin_map_to_their_rmcp_modes() {
        let mut auto = entry();
        auto.protocol_version = Some(ProtocolVersionSetting::Auto);
        match version_negotiation(&auto).expect("auto") {
            ClientLifecycleMode::Auto { preferred_versions, legacy_version } => {
                assert_eq!(preferred_versions, vec![ProtocolVersion::V_2026_07_28]);
                assert_eq!(legacy_version, Some(ProtocolVersion::LATEST));
            }
            other => panic!("expected Auto, got {other:?}"),
        }

        let mut pinned = entry();
        pinned.protocol_version = Some(ProtocolVersionSetting::V20260728);
        match version_negotiation(&pinned).expect("pinned") {
            ClientLifecycleMode::Discover { preferred_versions } => {
                assert_eq!(preferred_versions, vec![ProtocolVersion::V_2026_07_28]);
            }
            other => panic!("expected Discover, got {other:?}"),
        }
    }

    #[test]
    fn the_invalid_protocol_version_string_is_byte_exact() {
        assert_eq!(
            invalid_protocol_version_message("2019-01-01"),
            "Invalid MCP protocolVersion: 2019-01-01"
        );
    }

    /// `resolveVersionNegotiation`'s `default:` arm, at the moment upstream reaches it.
    ///
    /// This could not be written before [`ProtocolVersionSetting::Other`] existed: the value was
    /// discarded by `deserialize_with = "lenient"` at parse time, so a server pinning a revision
    /// this build does not implement negotiated as `legacy` in silence — and, worse, hashed as if it
    /// had pinned nothing, which is the digest divergence
    /// `cyrup_ext_subagents::exec::mcp_direct_tools::tests::a_protocol_revision_the_writer_used_to_reject_now_agrees_on_the_digest`
    /// closes. The constraint the fix had to respect is exactly this test: the value must survive
    /// the deserialiser **and** still be refused at connect.
    ///
    /// Every expected sentence is `` `Invalid MCP protocolVersion: ${String(v)}` `` evaluated on
    /// node 22, not transcribed — including `[object Object]` for an object and `1,2` for an array,
    /// which are `String()`'s answers and not JSON.
    #[test]
    fn an_unknown_revision_throws_upstreams_sentence_at_connect() {
        for (json, message) in [
            (r#"{"command":"x","protocolVersion":"2025-06-18"}"#, "Invalid MCP protocolVersion: 2025-06-18"),
            (r#"{"command":"x","protocolVersion":5}"#, "Invalid MCP protocolVersion: 5"),
            (r#"{"command":"x","protocolVersion":1.5}"#, "Invalid MCP protocolVersion: 1.5"),
            (r#"{"command":"x","protocolVersion":true}"#, "Invalid MCP protocolVersion: true"),
            (r#"{"command":"x","protocolVersion":null}"#, "Invalid MCP protocolVersion: null"),
            (r#"{"command":"x","protocolVersion":[1,2]}"#, "Invalid MCP protocolVersion: 1,2"),
            (r#"{"command":"x","protocolVersion":{"a":1}}"#, "Invalid MCP protocolVersion: [object Object]"),
            (r#"{"command":"x","protocolVersion":""}"#, "Invalid MCP protocolVersion: "),
        ] {
            let entry: ServerEntry = serde_json::from_str(json).expect("parses — the loader is not the validator");
            assert!(
                matches!(entry.protocol_version, Some(ProtocolVersionSetting::Other(_))),
                "the deserialiser must not have dropped it: {json}"
            );
            assert_eq!(
                version_negotiation(&entry).expect_err(json).to_string(),
                message,
                "{json}"
            );
        }

        // The three known revisions are untouched by the passthrough arm.
        for (json, expected) in [
            (r#"{"command":"x","protocolVersion":"legacy"}"#, ProtocolVersionSetting::Legacy),
            (r#"{"command":"x","protocolVersion":"auto"}"#, ProtocolVersionSetting::Auto),
            (r#"{"command":"x","protocolVersion":"2026-07-28"}"#, ProtocolVersionSetting::V20260728),
        ] {
            let entry: ServerEntry = serde_json::from_str(json).expect("entry");
            assert_eq!(entry.protocol_version, Some(expected), "{json}");
            assert!(version_negotiation(&entry).is_ok(), "{json}");
        }
    }

    // --- MCP-118 --------------------------------------------------------------------------------

    #[test]
    fn capabilities_cover_all_four_on_off_combinations() {
        let none = build_client_capabilities(false, None);
        assert!(none.sampling.is_none() && none.elicitation.is_none());
        assert_eq!(serde_json::to_value(&none).unwrap(), serde_json::json!({}));

        let sampling_only = build_client_capabilities(true, None);
        assert_eq!(
            serde_json::to_value(&sampling_only).unwrap(),
            serde_json::json!({ "sampling": {} })
        );

        let form_only = build_client_capabilities(false, Some(ElicitationMode { allow_url: false }));
        assert_eq!(
            serde_json::to_value(&form_only).unwrap(),
            serde_json::json!({ "elicitation": { "form": {} } })
        );

        let both = build_client_capabilities(true, Some(ElicitationMode { allow_url: true }));
        assert_eq!(
            serde_json::to_value(&both).unwrap(),
            serde_json::json!({ "sampling": {}, "elicitation": { "form": {}, "url": {} } })
        );
    }

    #[test]
    fn the_client_name_is_the_recorded_rename() {
        let info = client_info("github", ClientCapabilities::default());
        assert_eq!(info.client_info.name, "cyrup-mcp-github");
        assert_eq!(info.client_info.version, "1.0.0");
    }

    // --- MCP-120 / MCP-122 ----------------------------------------------------------------------

    #[test]
    fn the_three_reason_strings_are_byte_exact() {
        assert_eq!(ListKind::Tools.reason(), "tools-list-changed");
        assert_eq!(ListKind::Prompts.reason(), "prompts-list-changed");
        assert_eq!(ListKind::Resources.reason(), "resources-list-changed");
        assert_eq!(
            ListKind::Prompts.refresh_failed_message("s", "boom"),
            "MCP: prompts/list_changed refresh failed for s: boom"
        );
    }

    fn handler(allow_url: bool, hook: Option<ElicitationCompleteHook>) -> McpClientHandler {
        McpClientHandler::new(McpClientHandlerParts {
            server: "s".to_string(),
            runtime_signal: CancelToken::new(),
            elicitation_mode: Some(ElicitationMode { allow_url }),
            sampling: None,
            elicitation: None,
            list_changed: None,
            elicitation_complete: hook,
        })
    }

    #[test]
    fn two_handlers_never_share_an_identity_and_a_clone_always_does() {
        let a = handler(false, None);
        let b = handler(false, None);
        assert!(a.identity().ptr_eq(&a.clone().identity()));
        assert!(!a.identity().ptr_eq(&b.identity()));
    }

    #[test]
    fn capabilities_follow_the_wired_hooks_not_the_configuration() {
        // No elicitation hook wired => the capability is not advertised even though a mode was set.
        let h = handler(true, None);
        assert!(h.info().capabilities.elicitation.is_none());
        assert!(h.info().capabilities.sampling.is_none());
    }

    // --- MCP-101 --------------------------------------------------------------------------------

    #[tokio::test]
    async fn stdio_spawn_creates_the_plugin_dir_pipes_stderr_and_replaces_the_environment() {
        use tokio::io::AsyncReadExt;

        let dir = tempfile::tempdir().unwrap();
        let plugin_data = dir.path().join("nested/plugin-data");
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());

        let spec = StdioTransportSpec {
            command: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                // Two lines on stderr, then exit: the value the override supplied, and whether a
                // host variable leaked through. `resolveEnv` builds the child's WHOLE environment,
                // so `HOME` — which this test process has and which a POSIX shell does not invent
                // for itself the way it invents `PATH` — must not be visible unless the spawn
                // merged instead of replacing.
                "printf '%s\\n%s\\n' \"$FOO\" \"${HOME:-unset}\" 1>&2".to_string(),
            ],
            env,
            cwd: Some(dir.path().to_path_buf()),
            plugin_data_dir: Some(plugin_data.clone()),
            debug: false,
        };

        let (transport, stderr) = spawn_stdio_transport(&spec).unwrap();
        assert!(plugin_data.is_dir(), "pluginDataDir is mkdir -p'd before the spawn");

        let mut stderr = stderr.expect("debug: false pipes stderr and hands back the handle");
        let mut captured = Vec::new();
        let _ = stderr.read_to_end(&mut captured).await.unwrap();

        let mut tail = VecDeque::new();
        append_stderr_tail(&mut tail, &captured);
        assert_eq!(stderr_tail_detail(&tail).as_deref(), Some("bar \u{2014} unset"));
        assert_eq!(
            with_stderr_tail("MCP connection setup failed", &tail),
            "MCP connection setup failed (bar \u{2014} unset)"
        );

        drop(transport);
    }

    #[tokio::test]
    async fn debug_mode_inherits_stderr_so_there_is_no_tail_to_build_from() {
        let spec = StdioTransportSpec {
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "exit 0".to_string()],
            env: HashMap::new(),
            cwd: None,
            plugin_data_dir: None,
            debug: true,
        };
        let (transport, stderr) = spawn_stdio_transport(&spec).unwrap();
        assert!(stderr.is_none(), "debug: true is rmcp's default Stdio::inherit()");
        drop(transport);
    }

    #[tokio::test]
    async fn a_missing_command_reports_the_path_it_could_not_spawn() {
        let spec = StdioTransportSpec {
            command: "/nonexistent/cyrup-mcp-fixture".to_string(),
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
            plugin_data_dir: None,
            debug: false,
        };
        // `TokioChildProcess` is not `Debug`, so `unwrap_err` is unavailable here.
        let Err(err) = spawn_stdio_transport(&spec) else {
            panic!("spawning a nonexistent command must fail");
        };
        assert!(
            err.to_string().contains("/nonexistent/cyrup-mcp-fixture"),
            "the failing path is what makes a mistyped `command` actionable: {err}"
        );
    }

    // --- MCP-109 --------------------------------------------------------------------------------

    #[test]
    fn http_config_pins_the_uri_disables_reinit_and_routes_the_bearer_through_auth_header() {
        let spec = HttpTransportSpec {
            server: "s".to_string(),
            url: "https://example.test/mcp".to_string(),
            headers: vec![("X-A".to_string(), "1".to_string())],
            bearer_token: Some("hunter2".to_string()),
        };
        let config = build_http_transport_config(&spec).unwrap();
        assert_eq!(&*config.uri, "https://example.test/mcp");
        assert!(
            !config.reinit_on_expired_session,
            "rmcp defaults this ON; MCP-135 owns session recovery, not the transport"
        );
        assert!(config.allow_stateless, "rmcp's default, and the upstream-equivalent value");
        assert_eq!(
            config.auth_header.as_deref(),
            Some("hunter2"),
            "the token goes in WITHOUT the `Bearer ` prefix"
        );
        assert_eq!(config.custom_headers.len(), 1);
    }

    #[test]
    fn a_header_value_carrying_a_newline_cannot_inject_a_second_header() {
        let spec = HttpTransportSpec {
            server: "s".to_string(),
            url: "https://example.test/mcp".to_string(),
            headers: vec![("X-A".to_string(), "a\r\nX-Evil: 1".to_string())],
            bearer_token: None,
        };
        let err = build_http_transport_config(&spec).unwrap_err();
        assert!(err.to_string().contains("X-A"), "the offending header is named: {err}");
    }

    // --- MCP-083, the two deferred call sites -----------------------------------------------------

    #[test]
    fn the_stdio_spec_resolves_its_env_through_secrets_and_reports_the_failing_key() {
        let base: HashMap<String, String> =
            [("HOST_ONLY".to_string(), "kept".to_string())].into_iter().collect();

        let mut e = entry();
        e.command = Some("/bin/true".to_string());
        e.env = Some(
            [
                ("TOKEN".to_string(), "!printf hunter2".to_string()),
                ("PLAIN".to_string(), "x".to_string()),
            ]
            .into_iter()
            .collect(),
        );
        e.plugin_data_dir = Some("/tmp/cyrup-mcp-plugin-data".to_string());

        let spec = StdioTransportSpec::resolve(
            &e,
            "srv",
            "/bin/true".to_string(),
            vec!["--x".to_string()],
            None,
            &base,
        )
        .unwrap();
        assert_eq!(spec.env.get("TOKEN").map(String::as_str), Some("hunter2"));
        assert_eq!(spec.env.get("PLAIN").map(String::as_str), Some("x"));
        assert_eq!(
            spec.env.get("HOST_ONLY").map(String::as_str),
            Some("kept"),
            "`resolveEnv` copies the WHOLE base environment before layering"
        );
        assert_eq!(spec.plugin_data_dir.as_deref(), Some(std::path::Path::new("/tmp/cyrup-mcp-plugin-data")));
        assert!(!spec.debug, "`debug` absent is `false`, i.e. stderr piped");

        // A failing secret is an error carrying the `stdio env` context string — never an empty
        // environment variable the child would silently authenticate with.
        e.env = Some([("TOKEN".to_string(), "!exit 9".to_string())].into_iter().collect());
        // `StdioTransportSpec` is not `Debug` (it carries a resolved environment), so the failure is
        // destructured rather than `unwrap_err`'d — the same shape the spawn tests above use.
        let Err(err) = StdioTransportSpec::resolve(
            &e,
            "srv",
            "/bin/true".to_string(),
            Vec::new(),
            None,
            &base,
        ) else {
            panic!("an unresolvable stdio env secret must never become a value");
        };
        assert_eq!(
            err.to_string(),
            "Failed to resolve MCP server \"srv\" stdio env \"TOKEN\": command exited with code 9"
        );
    }

    #[test]
    fn the_http_spec_resolves_headers_and_the_bearer_through_secrets() {
        use crate::config::{AuthKind, AuthMode};

        let env = crate::credentials::process_env();
        let mut e = entry();
        e.url = Some("https://example.test/mcp".to_string());
        e.headers = Some(
            [("X-Token".to_string(), "!printf abc".to_string())]
                .into_iter()
                .collect(),
        );
        e.auth = Some(AuthMode::Named(AuthKind::Bearer));
        e.bearer_token = Some("!printf hunter2".to_string());

        let spec = HttpTransportSpec::resolve(
            &e,
            "srv",
            "https://example.test/mcp".to_string(),
            &env,
        )
        .unwrap();
        assert_eq!(spec.server, "srv");
        assert_eq!(
            spec.headers,
            vec![("X-Token".to_string(), "abc".to_string())],
            "the header command ran and its stdout is the value"
        );
        assert_eq!(
            spec.bearer_token.as_deref(),
            Some("hunter2"),
            "WITHOUT the `Bearer ` prefix, which `auth_header` adds"
        );

        // And the resolved spec still passes the transport builder unchanged.
        let config = build_http_transport_config(&spec).unwrap();
        assert_eq!(config.auth_header.as_deref(), Some("hunter2"));
        assert_eq!(config.custom_headers.len(), 1);
    }

    // --- MCP-128 --------------------------------------------------------------------------------

    #[test]
    fn an_invalid_per_server_timeout_means_no_timeout_even_when_a_global_is_set() {
        let with = |ms: Option<f64>| {
            let mut e = entry();
            e.request_timeout_ms = ms;
            e
        };

        for bad in [0.0_f64, -1.0, f64::NAN, f64::INFINITY] {
            let e = with(Some(bad));
            assert_eq!(resolve_request_timeout(Some(&e), None), None);
            assert_eq!(
                resolve_request_timeout(Some(&e), Some(30_000.0)),
                None,
                "an invalid per-server value must NOT reinstate the global ({bad})"
            );
        }

        let pinned = with(Some(5_000.0));
        assert_eq!(
            resolve_request_timeout(Some(&pinned), Some(30_000.0)),
            Some(Duration::from_millis(5_000))
        );

        let absent = with(None);
        assert_eq!(
            resolve_request_timeout(Some(&absent), Some(30_000.0)),
            Some(Duration::from_millis(30_000)),
            "the global is consulted only when the per-server key is absent"
        );
        assert_eq!(resolve_request_timeout(Some(&absent), None), None);
        assert!(build_request_options(Some(&absent), None).is_none());
        assert_eq!(
            build_request_options(Some(&pinned), None).and_then(|o| o.timeout),
            Some(Duration::from_millis(5_000))
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod connection_builder_tests {
    use super::*;
    use std::collections::HashMap;

    // ── fixtures ──────────────────────────────────────────────────────────────────────────────
    //
    // `sh -c` everywhere, which is the same fixture runtime `server_manager::tests::child_process`
    // already uses and the same one `secrets::resolve_command_secret` spawns in production — so
    // nothing here is a new host dependency.

    /// A stdio "server" that writes one environment variable to **stderr** and exits non-zero.
    ///
    /// It fails the handshake on purpose: that is the path §3.3 step 8 decorates, so one fixture
    /// asserts both what the child's environment actually was and that the tail reaches the user.
    const ENV_TO_STDERR: &str = "printf '%s' \"$K\" >&2; exit 3";

    /// A minimal MCP server: answers `initialize` and echoes its cwd and its arguments back in
    /// `instructions`, which is where the test reads them from. Deliberately free of `${`, `$env:`
    /// and `{env:` so that passing it through `interpolateEnvVars` (which `args` go through) cannot
    /// rewrite the script itself.
    const TINY_MCP: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{},"serverInfo":{"name":"fixture","version":"1"},"instructions":"%s|%s"}}\n' "$id" "$PV" "$(pwd)" "$*"
      ;;
    *'"method":"notifications/'*) : ;;
    *)
      if [ -n "$id" ]; then printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"; fi
      ;;
  esac
done
"#;

    fn env_fn(pairs: &[(&str, &str)]) -> crate::credentials::EnvFn {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        Arc::new(move |key: &str| map.get(key).cloned())
    }

    /// A `base_env` with only what `sh` needs, so the "replace, don't merge" semantics of
    /// `StdioClientTransport`'s `env` option are observable: anything the child sees beyond `PATH`
    /// came from `definition.env`.
    fn base_env() -> HashMap<String, String> {
        [("PATH".to_string(), std::env::var("PATH").unwrap_or_default())]
            .into_iter()
            .collect()
    }

    fn builder() -> ConnectionBuilder {
        ConnectionBuilder::new(None).with_environment(
            env_fn(&[("HOME", "/home/fixture"), ("TOKEN", "s3cret")]),
            base_env(),
            PathBuf::from("/home/fixture"),
        )
    }

    fn request(name: &str, entry: ServerEntry) -> CreateConnection {
        CreateConnection {
            trace: None,
            name: name.to_string(),
            definition: Arc::new(entry),
            attempt: CancelToken::new(),
            request: CancelToken::new(),
            credentials_invalidated: false,
            request_options: None,
        }
    }

    /// [`request`] with the manager's own `buildRequestOptions(definition, requestSignal)` applied,
    /// so a test exercises the same `request_options` a real `connect` would hand the factory.
    fn timed_request(name: &str, entry: ServerEntry) -> CreateConnection {
        let request_options = build_request_options(Some(&entry), None);
        CreateConnection {
            request_options,
            ..request(name, entry)
        }
    }

    fn stdio_entry(script: &str, args: &[&str]) -> ServerEntry {
        let mut entry = ServerEntry {
            command: Some("sh".to_string()),
            ..ServerEntry::default()
        };
        let mut argv = vec!["-c".to_string(), script.to_string(), "sh".to_string()];
        argv.extend(args.iter().map(|arg| (*arg).to_string()));
        entry.args = Some(argv);
        entry
    }

    fn record(pairs: &[(&str, &str)]) -> crate::config::StringRecord {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<std::collections::BTreeMap<String, String>>()
            .into()
    }

    // ── MCP-101 · §3.3 ────────────────────────────────────────────────────────────────────────

    /// `resolveEnv(definition.env, name, definition.literalEnv === true)`'s three arms, observed
    /// from **inside the child** rather than from the resolver's return value: the environment the
    /// process actually got is the thing that matters, and it is what a `.envs()`-instead-of-
    /// `.env_clear()` port would get wrong.
    ///
    /// Each case also exercises §3.3 step 8, since the child writes to stderr and dies: the
    /// connect failure carries `(<value>)`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_child_environment_is_resolved_exactly_as_resolve_env_specifies() {
        // `literalEnv: true` — verbatim, no interpolation, no command execution.
        let mut entry = stdio_entry(ENV_TO_STDERR, &[]);
        entry.env = Some(record(&[("K", "${HOME}")]));
        entry.literal_env = Some(true);
        let error = builder()
            .connect_stdio(&request("literal", entry))
            .await
            .expect_err("the fixture exits 3");
        assert!(
            error.to_string().ends_with("(${HOME})"),
            "literalEnv must not interpolate: {error}"
        );

        // Without `literalEnv`, `${HOME}` interpolates.
        //
        // NAMED RESIDUAL, and the reason this arm reads the *real* `HOME` while the `args` test
        // below reads the seam's: `secrets::resolve_command_secret` — which
        // `resolveEnv`'s non-literal arm goes through — takes no `EnvFn`; it resolves against
        // `secrets::PROCESS_ENV`. `ConnectionBuilder::with_environment`'s `env` therefore reaches
        // `args`, `cwd`, the URL and the bearer ladder but NOT `env` values. In production both are
        // `process.env` and there is no divergence; in a test the two seams are simply different,
        // and giving `resolve_command_secret` an `EnvFn` is a `secrets.rs` change this unit does
        // not own.
        let real_home = std::env::var("HOME").unwrap_or_default();
        let mut entry = stdio_entry(ENV_TO_STDERR, &[]);
        entry.env = Some(record(&[("K", "${HOME}")]));
        let error = builder()
            .connect_stdio(&request("interp", entry))
            .await
            .expect_err("the fixture exits 3");
        assert!(
            error.to_string().ends_with(&format!("({real_home})")),
            "expected the interpolated home {real_home}: {error}"
        );

        // `!cmd` runs a shell command and the child sees its trimmed stdout.
        let mut entry = stdio_entry(ENV_TO_STDERR, &[]);
        entry.env = Some(record(&[("K", "!echo hunter2")]));
        let error = builder()
            .connect_stdio(&request("command", entry))
            .await
            .expect_err("the fixture exits 3");
        assert!(
            error.to_string().ends_with("(hunter2)"),
            "expected the command's stdout: {error}"
        );

        // `!!X` consumes exactly ONE `!` and interpolates the rest — so the child sees `!` followed
        // by the expanded home, and nothing is executed.
        let mut entry = stdio_entry(ENV_TO_STDERR, &[]);
        entry.env = Some(record(&[("K", "!!${HOME}")]));
        let error = builder()
            .connect_stdio(&request("escaped", entry))
            .await
            .expect_err("the fixture exits 3");
        assert!(
            error.to_string().ends_with(&format!("(!{real_home})")),
            "expected one `!` consumed and the rest interpolated: {error}"
        );
    }

    /// The five `!command` failure strings are user-visible verbatim, and the failure happens
    /// **before** a child exists — `resolveEnv` runs inside `new StdioClientTransport({...})`, which
    /// is upstream's step 7.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failing_env_command_surfaces_upstreams_sentence_and_spawns_nothing() {
        let mut entry = stdio_entry("exit 0", &[]);
        entry.env = Some(record(&[("K", "!exit 1")]));
        let error = builder()
            .connect_stdio(&request("x", entry))
            .await
            .expect_err("a non-zero secret command fails the connect");
        assert_eq!(
            error.to_string(),
            "Failed to resolve MCP server \"x\" stdio env \"K\": command exited with code 1"
        );
    }

    /// `args = (definition.args ?? []).map(interpolateEnvVars)` and
    /// `cwd = resolveConfigPath(definition.cwd) ?? this.defaultCwd`, read back out of the live
    /// server's `instructions`. This is the only test in the file that completes a real MCP
    /// handshake, so it is also the proof that [`ConnectionBuilder::connect_stdio`] produces a
    /// usable connection rather than merely a spawned process.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn args_are_interpolated_and_cwd_follows_resolve_config_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut entry = stdio_entry(TINY_MCP, &["${TOKEN}", "$env:TOKEN", "{env:TOKEN}", "${NOPE}"]);
        entry.env = Some(record(&[(
            "PV",
            rmcp::model::ProtocolVersion::LATEST.as_str(),
        )]));
        entry.cwd = Some(temp.path().to_string_lossy().into_owned());

        let connection = builder()
            .connect_stdio(&request("live", entry))
            .await
            .expect("the fixture speaks MCP");
        let info = connection
            .peer()
            .peer_info()
            .expect("initialize completed, so peer info exists");
        let instructions = info
            .instructions
            .clone()
            .expect("the fixture puts cwd|args in instructions");
        let (cwd, args) = instructions.split_once('|').expect("cwd|args");

        // `temp.path()` can be a symlink on macOS (`/var` → `/private/var`); compare canonically.
        assert_eq!(
            std::fs::canonicalize(cwd).expect("cwd exists"),
            std::fs::canonicalize(temp.path()).expect("tempdir exists"),
        );
        // All THREE placeholder forms expand, and a missing variable becomes the empty string.
        assert_eq!(args, "s3cret s3cret s3cret ");

        connection.close().await.expect("close");
    }

    // ── MCP-119 · §3.9 ─────────────────────────────────────────────────────────────

    /// A real stdio MCP server whose every behaviour is an argument, so one fixture covers the
    /// capability gate, pagination and all three per-list failure policies.
    ///
    /// `$1` is a log file it appends **every method it receives** to, one per line. That log is the
    /// only way to assert the half of the capability gate that matters — that an ungated
    /// `resources/list` is not merely discarded but never sent — and it is read from the server's
    /// side of the pipe, so nothing on the client side can fake it.
    ///
    /// `$2` is the `capabilities` object echoed back from `initialize`; `$3`/`$4`/`$5` select the
    /// tools/prompts/resources behaviour. Free of `${`, `$env:` and `{env:` for the same reason
    /// [`TINY_MCP`] is: `args` (which includes the script) go through `interpolateEnvVars`.
    const DISCOVERY_MCP: &str = r#"
log="$1"
caps="$2"
tools="$3"
prompts="$4"
resources="$5"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
  if [ -n "$method" ]; then printf '%s\n' "$method" >> "$log"; fi
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":%s,"serverInfo":{"name":"fixture","version":"1"},"instructions":"the fixture speaks"}}\n' "$id" "$PV" "$caps"
      ;;
    *'"method":"tools/list"'*)
      case "$tools" in
        fail) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32603,"message":"tools are unavailable"}}\n' "$id" ;;
        paged)
          case "$line" in
            *'"cursor":"page2"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"second","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
            *) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"first","inputSchema":{"type":"object"}}],"nextCursor":"page2"}}\n' "$id" ;;
          esac ;;
        *) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"echo back","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
      esac
      ;;
    *'"method":"prompts/list"'*)
      case "$prompts" in
        fail) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32603,"message":"prompts exploded"}}\n' "$id" ;;
        *) printf '{"jsonrpc":"2.0","id":%s,"result":{"prompts":[{"name":"greet"}]}}\n' "$id" ;;
      esac
      ;;
    *'"method":"resources/list"'*)
      case "$resources" in
        fail) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32603,"message":"resources exploded"}}\n' "$id" ;;
        *) printf '{"jsonrpc":"2.0","id":%s,"result":{"resources":[{"name":"doc","uri":"file:///doc"}]}}\n' "$id" ;;
      esac
      ;;
    *'"method":"notifications/'*) : ;;
    *)
      if [ -n "$id" ]; then printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"; fi
      ;;
  esac
done
"#;

    /// One [`DISCOVERY_MCP`] entry, plus the log path the fixture records its wire traffic in.
    fn discovery_entry(
        log: &std::path::Path,
        capabilities: &str,
        tools: &str,
        prompts: &str,
        resources: &str,
    ) -> ServerEntry {
        let log = log.to_string_lossy().into_owned();
        let mut entry = stdio_entry(
            DISCOVERY_MCP,
            &[&log, capabilities, tools, prompts, resources],
        );
        entry.env = Some(record(&[("PV", rmcp::model::ProtocolVersion::LATEST.as_str())]));
        entry
    }

    /// Every method the fixture saw, in order.
    fn wire_log(log: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The whole of `createConnection`'s post-handshake block against a tools-only server: the
    /// catalog is fetched, `instructions` rides back on the record, and the two capabilities the
    /// server did **not** advertise are never asked about.
    ///
    /// The last clause is why the fixture keeps a wire log. `resources == []` is satisfied equally
    /// by a gate that works and by one that asks and throws the answer away, and only one of those
    /// is what upstream does — an unsolicited `resources/list` is a request some servers answer with
    /// an error the user then sees.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_tools_only_server_is_listed_once_and_never_asked_for_the_rest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("wire.log");
        let entry = discovery_entry(&log, r#"{"tools":{}}"#, "ok", "ok", "ok");

        let created = builder()
            .create_connection(request("live", entry))
            .await
            .expect("a tools-only server connects");

        assert_eq!(created.status, ConnectionStatus::Connected);
        let names: Vec<&str> = created
            .discovery
            .tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect();
        assert_eq!(names, vec!["echo"], "`tools/list` reached the record");
        assert_eq!(
            created.discovery.instructions.as_deref(),
            Some("the fixture speaks"),
            "`client.getInstructions?.()` is read off the handshake result"
        );
        assert!(created.discovery.resources.is_empty());
        assert!(created.discovery.prompts.is_empty());
        assert!(!created.discovery.prompt_discovery_failed);

        let methods = wire_log(&log);
        assert!(methods.iter().any(|method| method == "tools/list"), "{methods:?}");
        assert!(
            !methods.iter().any(|method| method == "resources/list"),
            "the resources capability was absent, so NO request may be sent: {methods:?}"
        );
        assert!(
            !methods.iter().any(|method| method == "prompts/list"),
            "the prompts capability was absent, so NO request may be sent: {methods:?}"
        );

        created.resource.close().await.expect("close");
    }

    /// `do { … } while (cursor)` — a second page is fetched and both pages reach the record.
    ///
    /// rmcp's `Peer::list_all_tools` owns the loop, so what this pins is that discovery uses the
    /// paginating helper rather than the single-shot `list_tools`. The fixture answers the first
    /// call with a `nextCursor` and the second (identified by the cursor on the wire) without one,
    /// so a port that read only `result.tools` would return `["first"]` and fail here.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_paginated_tool_list_is_walked_to_the_last_cursor() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("wire.log");
        let entry = discovery_entry(&log, r#"{"tools":{}}"#, "paged", "ok", "ok");

        let created = builder()
            .create_connection(request("live", entry))
            .await
            .expect("connect");

        let names: Vec<&str> = created
            .discovery
            .tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect();
        assert_eq!(names, vec!["first", "second"], "both pages, in order");
        assert_eq!(
            wire_log(&log).iter().filter(|method| *method == "tools/list").count(),
            2,
            "one request per page"
        );

        created.resource.close().await.expect("close");
    }

    /// `fetchAllPrompts`' catch: an advertised `prompts` capability whose list throws yields
    /// `prompts: []` **and** `promptDiscoveryFailed: true`, and the connection still succeeds.
    ///
    /// The flag is the whole reason the payload carries one. `init.ts:340` publishes the prompt map
    /// as live only when it is false, so collapsing "the server has no prompts" into "we could not
    /// ask" would cache an empty prompt surface from one transient failure.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_advertised_prompt_list_that_throws_is_recorded_as_failed_not_empty() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("wire.log");
        let entry = discovery_entry(&log, r#"{"tools":{},"prompts":{}}"#, "ok", "fail", "ok");

        let created = builder()
            .create_connection(request("live", entry))
            .await
            .expect("a prompts/list failure does not fail the connect");

        assert_eq!(created.status, ConnectionStatus::Connected);
        assert!(created.discovery.prompts.is_empty());
        assert!(
            created.discovery.prompt_discovery_failed,
            "the capability was advertised and the list threw"
        );
        // The tools list is untouched by the sibling's failure — which is what `join!` buys over
        // `try_join!`, whose short-circuit would have cancelled it.
        assert_eq!(created.discovery.tools.len(), 1);
        assert!(wire_log(&log).iter().any(|method| method == "prompts/list"));

        created.resource.close().await.expect("close");
    }

    /// `fetchAllResources`' catch: an advertised `resources` capability whose list throws is
    /// **silently** `[]` — no flag, no log, no failure. The asymmetry with prompts is upstream's,
    /// and it is deliberate: nothing downstream needs to tell "no resources" from "could not ask".
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_advertised_resource_list_that_throws_is_swallowed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("wire.log");
        let entry = discovery_entry(&log, r#"{"tools":{},"resources":{}}"#, "ok", "ok", "fail");

        let created = builder()
            .create_connection(request("live", entry))
            .await
            .expect("a resources/list failure does not fail the connect");

        assert_eq!(created.status, ConnectionStatus::Connected);
        assert!(created.discovery.resources.is_empty());
        assert!(!created.discovery.prompt_discovery_failed, "prompts are a different list");
        assert_eq!(created.discovery.tools.len(), 1);
        assert!(wire_log(&log).iter().any(|method| method == "resources/list"));

        created.resource.close().await.expect("close");
    }

    /// `fetchAllTools` has no catch: a `tools/list` that throws fails the **connect**, and the
    /// failure carries the §3.3 step-8 treatment every other `createConnection` failure gets.
    ///
    /// This is also the arm that gives [`McpError::SetupFailed`] its upstream producer — a
    /// discovery failure whose subsequent `close` also fails is what the aggregate is for.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_tool_list_that_throws_fails_the_connect() {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = temp.path().join("wire.log");
        let entry = discovery_entry(&log, r#"{"tools":{}}"#, "fail", "ok", "ok");

        let outcome = builder().create_connection(request("live", entry)).await;
        let Err(error) = outcome else {
            panic!("a failing `tools/list` must fail the connect, not connect with no tools");
        };
        assert!(
            error.to_string().contains("tools are unavailable"),
            "the server's own message reaches the user: {error}"
        );
    }

    /// `debug: true` ⇒ stderr is **inherited**, so there is no tail and therefore no `(...)` suffix.
    /// The pair with the `debug: false` cases above is what pins the polarity — rmcp's default is
    /// `inherit`, the opposite of what a reader of `stderr: definition.debug ? "inherit" : "pipe"`
    /// expects to have to write.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn debug_mode_inherits_stderr_and_produces_no_suffix() {
        let mut entry = stdio_entry(ENV_TO_STDERR, &[]);
        entry.env = Some(record(&[("K", "diagnostic")]));
        entry.debug = Some(true);
        let error = builder()
            .connect_stdio(&request("noisy", entry))
            .await
            .expect_err("the fixture exits 3");
        assert!(
            !error.to_string().contains("(diagnostic)"),
            "debug mode has no tail to append: {error}"
        );
    }

    /// `if (definition.pluginDataDir) mkdirSync(definition.pluginDataDir, { recursive: true })` —
    /// §3.3 step 5, and it happens **before** the spawn so a plugin's first write cannot race its
    /// own server.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_plugin_data_directory_exists_before_the_child_does() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nested = temp.path().join("plugin/data/dir");
        let mut entry = stdio_entry("test -d \"$D\" && printf ready >&2; exit 3", &[]);
        entry.plugin_data_dir = Some(nested.to_string_lossy().into_owned());
        entry.env = Some(record(&[("D", nested.to_string_lossy().into_owned().as_str())]));

        let error = builder()
            .connect_stdio(&request("plugin", entry))
            .await
            .expect_err("the fixture exits 3");
        assert!(nested.is_dir(), "the directory must exist after the connect");
        assert!(
            error.to_string().ends_with("(ready)"),
            "the CHILD must have seen it, not just the test: {error}"
        );
    }

    /// `createClient` runs `resolveVersionNegotiation` **first**, so an invalid `protocolVersion`
    /// throws before a process exists. A port that spawned first would leak one child per
    /// misconfigured server.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_invalid_protocol_version_throws_before_anything_is_spawned() {
        let mut entry = stdio_entry("printf spawned >&2; exit 3", &[]);
        entry.protocol_version = Some(crate::config::ProtocolVersionSetting::Other(
            serde_json::from_str::<crate::config::RawJson>("\"2025-06-18\"").expect("raw"),
        ));
        let error = builder()
            .connect_stdio(&request("pinned", entry))
            .await
            .expect_err("an unknown revision fails the connect");
        assert_eq!(error.to_string(), "Invalid MCP protocolVersion: 2025-06-18");
        // No child ran, so no stderr tail was appended.
        assert!(!error.to_string().contains("spawned"));
    }

    // ── an HTTP fixture ───────────────────────────────────────────────────────────────────────
    //
    // A hand-rolled HTTP/1.1 responder rather than a framework: the assertions are about exact
    // bytes on the wire (which `Authorization` header arrived, how many attempts were made), and a
    // framework would normalise precisely the things under test. Every response carries
    // `Connection: close`, so one request is one accepted socket and the request log is ordered.

    /// One request as the server saw it.
    #[derive(Debug, Clone)]
    struct Recorded {
        method: String,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl Recorded {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        }

        fn all(&self, name: &str) -> Vec<&str> {
            self.headers
                .iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
                .collect()
        }
    }

    /// How many `initialize` POSTs to answer with 401 before answering with a real handshake.
    /// `usize::MAX` is the permanent-401 server.
    struct HttpFixture {
        url: String,
        log: Arc<std::sync::Mutex<Vec<Recorded>>>,
        _task: tokio::task::JoinHandle<()>,
    }

    /// What the fixture does, beyond the 401 count. Every field defaults to the shape the original
    /// fixture hardcoded, so an existing call site reads the same.
    #[derive(Clone)]
    struct FixtureOptions {
        /// How many `initialize` POSTs to answer with 401 before answering with a real handshake.
        unauthorized_initializes: usize,
        /// Emit `WWW-Authenticate` on those 401s. **`false` is the bare 401** — the shape rmcp
        /// never turns into an `AuthRequiredError`, and the one no test covered before.
        challenge: bool,
        /// Answer those 401s with `Content-Type: application/json` and a parseable JSON-RPC error
        /// body instead of `Content-Length: 0`. With `challenge: false` this is the shape rmcp
        /// collapses into `Ok(StreamableHttpPostResponse::Json(..))`
        /// (`reqwest/streamable_http_client.rs:287-290`), so no transport error is constructed and
        /// nothing `unauthorized_challenge` walks can see it. With `challenge: true` rmcp's
        /// `:212-226` arm claims it first and the ladder is reached today — which is what makes this
        /// field an ablation rather than a restatement.
        json_rpc_body: bool,
        /// Emit `mcp-session-id` on the handshake response. `false` is a legal stateless
        /// streamable-HTTP server, and the case `has_session_id` used to answer `true` for.
        session_id: bool,
        /// Read the `initialize` POST and then never answer it, holding the socket open. The
        /// wedged-server case `requestTimeoutMs` exists for.
        stall_initialize: bool,
        cancel_before: Option<(usize, CancelToken)>,
        /// The bearer token this server accepts. `Some(token)` makes **the credential the only
        /// gate**: every request that does not carry exactly `Bearer <token>` is answered 401,
        /// whichever request it is and however many have gone before.
        ///
        /// This is deliberately not the `unauthorized_initializes` counter. A fixture that counts
        /// attempts answers 200 to a request carrying no credential at all, which would let a
        /// store-backed provider that always returns `None` pass a "connects on the first attempt"
        /// test. Gating on the token is what makes those tests honest.
        require_bearer: Option<&'static str>,
    }

    impl Default for FixtureOptions {
        fn default() -> Self {
            Self {
                unauthorized_initializes: 0,
                challenge: true,
                json_rpc_body: false,
                session_id: true,
                stall_initialize: false,
                cancel_before: None,
                require_bearer: None,
            }
        }
    }

    impl HttpFixture {
        async fn start(unauthorized_initializes: usize) -> Self {
            Self::start_with(FixtureOptions {
                unauthorized_initializes,
                ..FixtureOptions::default()
            })
            .await
        }

        /// As [`Self::start`], but the fixture cancels `token` **before writing** the response to
        /// the `nth` request (1-based). Writing after the cancel would be a race — the client could
        /// finish reading the response first — and the whole point of the arm-3 test is that the
        /// abort is already visible when the attempt settles.
        async fn start_cancelling_before_response(
            unauthorized_initializes: usize,
            nth: usize,
            token: CancelToken,
        ) -> Self {
            Self::start_with(FixtureOptions {
                unauthorized_initializes,
                cancel_before: Some((nth, token)),
                ..FixtureOptions::default()
            })
            .await
        }

        async fn start_with(options: FixtureOptions) -> Self {
            let FixtureOptions {
                unauthorized_initializes,
                challenge,
                json_rpc_body,
                session_id,
                stall_initialize,
                cancel_before,
                require_bearer,
            } = options;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind loopback");
            let addr = listener.local_addr().expect("addr");
            let log: Arc<std::sync::Mutex<Vec<Recorded>>> =
                Arc::new(std::sync::Mutex::new(Vec::new()));
            let sink = Arc::clone(&log);
            let task = tokio::spawn(async move {
                let mut initializes = 0_usize;
                let mut seen = 0_usize;
                // Sockets the fixture deliberately never answers. Held rather than dropped: a drop
                // would close the connection and the client would see EOF, which is a *different*
                // failure from a server that stays connected and stays silent.
                let mut held: Vec<tokio::net::TcpStream> = Vec::new();
                loop {
                    let Ok((mut socket, _)) = listener.accept().await else {
                        return;
                    };
                    let Some(recorded) = read_request(&mut socket).await else {
                        continue;
                    };
                    let is_initialize = recorded.body.contains("\"method\":\"initialize\"");
                    if is_initialize {
                        initializes += 1;
                    }
                    seen += 1;
                    if let Some((nth, token)) = cancel_before.as_ref()
                        && seen == *nth
                    {
                        token.cancel();
                    }
                    sink.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(recorded.clone());

                    // The credential gate. `None` — every existing call site — is "no gate", so the
                    // counter arm below reads exactly as it did before.
                    let authorized = match require_bearer {
                        None => true,
                        Some(token) => recorded.headers.iter().any(|(key, value)| {
                            key.eq_ignore_ascii_case("authorization")
                                && value.trim() == format!("Bearer {token}")
                        }),
                    };

                    let response = if recorded.method == "GET" || recorded.method == "DELETE" {
                        // No server-initiated stream and no session teardown: both are optional.
                        "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                    } else if is_initialize && stall_initialize {
                        // Read, recorded, never answered — and the socket is held open by keeping
                        // it in scope. This is the wedged server: the TCP connect succeeded, so
                        // nothing below the MCP layer fails, and only `requestTimeoutMs` can end it.
                        held.push(socket);
                        continue;
                    } else if !authorized || (is_initialize && initializes <= unauthorized_initializes)
                    {
                        let challenge_header = if challenge {
                            "WWW-Authenticate: Bearer realm=\"mcp\", resource_metadata=\"https://example.invalid/.well-known\"\r\n"
                        } else {
                            // The BARE 401. rmcp builds `AuthRequiredError` only when the header is
                            // present, so this is the shape that used to fall out as a hard connect
                            // error instead of reaching the OAuth ladder.
                            ""
                        };
                        if json_rpc_body {
                            // The id is ECHOED, so rmcp's `expect_response`
                            // (`service/client.rs:191-204`) takes the `JsonRpcError` arm at `:194`
                            // rather than `UncorrelatedErrorResponse` at `:200`. Both fail today,
                            // but only the echoing shape is what a real server produces, and the
                            // ablation has to fail for the right reason.
                            let id = json_id(&recorded.body);
                            let payload = format!(
                                "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":-32001,\"message\":\"Unauthorized\"}}}}"
                            );
                            format!(
                                "HTTP/1.1 401 Unauthorized\r\n{challenge_header}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                                payload.len()
                            )
                        } else {
                            format!("HTTP/1.1 401 Unauthorized\r\n{challenge_header}Content-Length: 0\r\nConnection: close\r\n\r\n")
                        }
                    } else if is_initialize {
                        let id = json_id(&recorded.body);
                        let payload = format!(
                            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"protocolVersion\":\"{}\",\"capabilities\":{{}},\"serverInfo\":{{\"name\":\"fixture\",\"version\":\"1\"}}}}}}",
                            rmcp::model::ProtocolVersion::LATEST.as_str()
                        );
                        let session_header = if session_id {
                            "mcp-session-id: fixture-session\r\n"
                        } else {
                            // A stateless streamable-HTTP server. Legal, and rmcp's
                            // `allow_stateless` default accepts it.
                            ""
                        };
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{session_header}Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                            payload.len()
                        )
                    } else if recorded.body.contains("\"method\":\"tools/list\"") {
                        // MCP-119 made discovery part of every successful connect, so a fixture
                        // that completes a handshake must answer this or the connect fails with
                        // `UnexpectedResponse` — `{}` does not deserialize into a `ListToolsResult`.
                        // An EMPTY catalog is the right answer here: this fixture advertises
                        // `capabilities: {}`, so `tools/list` is the only list discovery sends, and
                        // these tests are about the transport and the OAuth ladder, not the
                        // inventory.
                        let id = json_id(&recorded.body);
                        let payload =
                            format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"tools\":[]}}}}");
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                            payload.len()
                        )
                    } else if recorded.body.contains("\"id\":") {
                        let id = json_id(&recorded.body);
                        let payload = format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{}}}}");
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                            payload.len()
                        )
                    } else {
                        // A notification. 202 with no body is what the spec asks for.
                        "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                    };
                    use tokio::io::AsyncWriteExt as _;
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                }
            });
            Self {
                url: format!("http://{addr}/mcp"),
                log,
                _task: task,
            }
        }

        fn requests(&self) -> Vec<Recorded> {
            self.log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn initializes(&self) -> usize {
            self.requests()
                .iter()
                .filter(|request| request.body.contains("\"method\":\"initialize\""))
                .count()
        }
    }

    fn json_id(body: &str) -> String {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|value| value.get("id").cloned())
            .map_or_else(|| "0".to_string(), |id| id.to_string())
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Recorded> {
        use tokio::io::AsyncReadExt as _;
        let mut raw = Vec::new();
        let mut buffer = [0_u8; 4096];
        let head_end = loop {
            if let Some(index) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
            let read = socket.read(&mut buffer).await.ok()?;
            if read == 0 {
                return None;
            }
            raw.extend_from_slice(buffer.get(..read)?);
        };
        let head = String::from_utf8_lossy(raw.get(..head_end)?).into_owned();
        let mut lines = head.lines();
        let method = lines
            .next()?
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_string(), value.trim().to_string()))
            })
            .collect();
        let length: usize = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.parse().ok())
            .unwrap_or(0);
        while raw.len() < head_end + length {
            let read = socket.read(&mut buffer).await.ok()?;
            if read == 0 {
                break;
            }
            raw.extend_from_slice(buffer.get(..read)?);
        }
        let body = String::from_utf8_lossy(raw.get(head_end..)?).into_owned();
        Some(Recorded {
            method,
            headers,
            body,
        })
    }

    /// A [`HttpAuthProvider`] that counts what the ladder asked it for. The count IS the assertion
    /// for "no keychain read happened before the first 401".
    #[derive(Debug, Default)]
    struct CountingAuth {
        token: Option<String>,
        calls: std::sync::Mutex<Vec<Option<String>>>,
        invalidations: std::sync::Mutex<Vec<String>>,
    }

    impl CountingAuth {
        fn with_token(token: &str) -> Arc<Self> {
            Arc::new(Self {
                token: Some(token.to_string()),
                ..Self::default()
            })
        }

        fn empty() -> Arc<Self> {
            Arc::new(Self::default())
        }

        fn calls(&self) -> Vec<Option<String>> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn invalidations(&self) -> Vec<String> {
            self.invalidations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl HttpAuthProvider for CountingAuth {
        fn authorize<'a>(
            &'a self,
            _server: &'a str,
            _url: &'a str,
            challenge: Option<&'a str>,
        ) -> BoxFuture<'a, McpResult<Option<String>>> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(challenge.map(str::to_string));
            let token = self.token.clone();
            Box::pin(async move { Ok(token) })
        }

        fn invalidate_auth_entry_cache(&self, server: &str) {
            self.invalidations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(server.to_string());
        }
    }

    fn http_entry(url: &str) -> ServerEntry {
        ServerEntry {
            url: Some(url.to_string()),
            ..ServerEntry::default()
        }
    }

    // ── MCP-109 · the transport is really constructed ─────────────────────────────────────────

    /// The streamable-HTTP transport connects, carries `Mcp-Session-Id` forward, and the connection
    /// reports having one — which is `session-recovery.ts`'s `hadSessionId` gate (MCP-134).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_streamable_http_server_connects_and_the_session_id_is_visible() {
        let fixture = HttpFixture::start(0).await;
        let connection = builder()
            .connect_http_client(&request("http", http_entry(&fixture.url)))
            .await
            .expect("the fixture completes the handshake");
        assert_eq!(connection.status, ConnectionStatus::Connected);
        assert!(
            connection.resource.has_session_id(),
            "streamable HTTP is the transport that has a session id"
        );
        assert_eq!(fixture.initializes(), 1, "one attempt, one initialize");
        connection.resource.close().await.expect("close");
    }

    /// MCP-134's other answer: a **stateless** streamable-HTTP server sends no `Mcp-Session-Id`, and
    /// `has_session_id` must be `false` for it.
    ///
    /// This is the ablation that the previous test could not perform. `has_session_id` was a
    /// hardcoded `true`, and the fixture always emitted `mcp-session-id`, so the test above passed
    /// identically with the field read from the wire and with it read from nothing. Consumed in
    /// production at `lifecycle.rs:1090` → `should_reconnect_after_refresh`, whose upstream first
    /// line is `if (!hadSessionId) return false` — so with the constant, a plain 404 from a
    /// stateless server was classified as a terminated session on every health tick and drove a
    /// reconnect + rediscovery cycle upstream never performs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stateless_http_server_reports_no_session_id() {
        let fixture = HttpFixture::start_with(FixtureOptions {
            session_id: false,
            ..FixtureOptions::default()
        })
        .await;
        let connection = builder()
            .connect_http_client(&request("stateless", http_entry(&fixture.url)))
            .await
            .expect("a stateless streamable-HTTP server is a legal server and connects");
        assert_eq!(connection.status, ConnectionStatus::Connected);
        assert!(
            !connection.resource.has_session_id(),
            "no `Mcp-Session-Id` on the wire means no session id, and `hadSessionId` closes the \
             session-recovery gate"
        );
        connection.resource.close().await.expect("close");
    }

    /// A **bare** 401 — 401 with no `WWW-Authenticate` — reaches the OAuth ladder.
    ///
    /// rmcp only builds `AuthRequiredError` when the challenge header is present, so this shape used
    /// to leave `unauthorized_challenge` answering `None` and the connect failing at arm 7 with
    /// `unexpected server response: HTTP 401 Unauthorized` — the user never offered `/mcp-auth`.
    /// Upstream's `isUnauthorizedHttpError` is status-only and reaches `needs-auth` here.
    /// `HttpFixture` always emitted the header before this test existed, which is why nothing caught
    /// it.
    ///
    /// WHICH ARM IT NOW PROVES: the assertions are unchanged, but the path underneath them is not.
    /// This 401 lands on the `initialize` POST, which [`UnauthorizedProbe`] owns, so it is typed as
    /// [`StreamableHttpError::AuthRequired`] with an empty header and claimed by
    /// [`unauthorized_challenge`]'s `AuthRequiredError` downcast — **not** by [`bare_unauthorized`]'s
    /// `HTTP 401 ` prefix test, which no longer sees the handshake POST at all. That prefix arm is
    /// pinned instead by `the_401_predicate_still_refuses_every_other_status`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_bare_401_with_no_challenge_still_reaches_the_oauth_ladder() {
        let fixture = HttpFixture::start_with(FixtureOptions {
            unauthorized_initializes: usize::MAX,
            challenge: false,
            ..FixtureOptions::default()
        })
        .await;
        let auth = CountingAuth::empty();
        let connection = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("bare", http_entry(&fixture.url)))
            .await
            .expect("a bare 401 is `needs-auth`, not a connect failure");

        assert_eq!(connection.status, ConnectionStatus::NeedsAuth);
        assert_eq!(fixture.initializes(), 2, "implicit-deferred promotes and retries once");
        assert_eq!(auth.invalidations(), vec!["bare".to_string()]);
        // The retry carried an EMPTY challenge, because there genuinely was none — that is what
        // `Some("")` means, and the arm the old doc claimed existed but nothing could produce.
        assert_eq!(
            auth.calls(),
            vec![Some(String::new())],
            "the promoted attempt asks the provider with an empty challenge"
        );
    }

    /// A 403 must NOT be widened along with the bare 401. `InsufficientScopeError` is rmcp's
    /// 403-with-challenge and upstream's predicate is 401-only, so a scope denial stays a hard
    /// error. Pinned here because the bare-401 widening is the one change that could have taken
    /// 403 with it.
    #[test]
    fn the_401_predicate_still_refuses_every_other_status() {
        for status in ["HTTP 403 Forbidden: nope", "HTTP 500 Internal Server Error: ", "HTTP 404 Not Found: "] {
            let error: rmcp::transport::streamable_http_client::StreamableHttpError<
                reqwest::Error,
            > = rmcp::transport::streamable_http_client::StreamableHttpError::UnexpectedServerResponse(
                std::borrow::Cow::Owned(status.to_string()),
            );
            assert!(!bare_unauthorized(&error), "{status} is not a 401");
        }
        let unauthorized: rmcp::transport::streamable_http_client::StreamableHttpError<
            reqwest::Error,
        > = rmcp::transport::streamable_http_client::StreamableHttpError::UnexpectedServerResponse(
            std::borrow::Cow::Borrowed("HTTP 401 Unauthorized: "),
        );
        assert!(bare_unauthorized(&unauthorized));
    }

    /// The OTHER 401 rmcp does not type: 401 + `Content-Type: application/json` + a parseable
    /// JSON-RPC error body. rmcp applies its JSON-RPC-error shortcut to every non-success status
    /// (`reqwest/streamable_http_client.rs:278-290`), not just 400 the way the pinned TS SDK does
    /// (`index.mjs:5374-5381`), so this used to arrive at the ladder as
    /// `ClientInitializeError::JsonRpcError` — a shape `unauthorized_challenge` answers `None` for
    /// and `bare_unauthorized` is never even called on.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_401_with_a_json_rpc_body_still_reaches_the_oauth_ladder() {
        let fixture = HttpFixture::start_with(FixtureOptions {
            unauthorized_initializes: usize::MAX,
            challenge: false,
            json_rpc_body: true,
            ..FixtureOptions::default()
        })
        .await;
        let auth = CountingAuth::empty();
        let connection = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("jsonrpc401", http_entry(&fixture.url)))
            .await
            .expect("a 401 is `needs-auth` whatever body it carries");

        assert_eq!(connection.status, ConnectionStatus::NeedsAuth);
        assert_eq!(fixture.initializes(), 2, "implicit-deferred promotes and retries once");
        assert_eq!(auth.invalidations(), vec!["jsonrpc401".to_string()]);
        assert_eq!(auth.calls(), vec![Some(String::new())], "no header, so an empty challenge");
    }

    /// An OAuth token and a **configured** `Authorization` header must not both go on the wire.
    ///
    /// rmcp carries the bearer in a separate `auth_header` channel from the custom-header map and
    /// both channels APPEND, so parity with upstream's single `Headers` object has to be restored at
    /// every producer. MEASURED before the fix as `["Bearer from-store", "Static abc123"]` — the
    /// credential the config did not name was additionally disclosed to the server. Upstream's
    /// `_commonHeaders` spreads `requestInit.headers` over the provider's header, so the configured
    /// one wins.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_oauth_token_never_joins_a_configured_authorization_header() {
        let fixture = HttpFixture::start(0).await;
        let mut entry = http_entry(&fixture.url);
        entry.auth = Some(crate::config::AuthMode::Named(crate::config::AuthKind::Oauth));
        entry.headers = Some(record(&[("Authorization", "Static abc123")]));
        let auth = CountingAuth::with_token("from-store");

        let connection = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("both", entry))
            .await
            .expect("connects");
        connection.resource.close().await.expect("close");

        let first = fixture.requests();
        let first = first.first().expect("one request");
        assert_eq!(
            first.all("authorization"),
            vec!["Static abc123"],
            "exactly one value, and the configured header wins"
        );
        assert!(
            !auth.calls().is_empty(),
            "the OAuth store is still consulted — `supportsOAuth` is true even with custom headers"
        );
    }

    /// The stdio pre-flight's `!command` env resolution must not hold the async worker that carries
    /// the connect future.
    ///
    /// `resolve_command_secret` is a `std::process::Command` spawn polled with
    /// `std::thread::sleep`, bounded by a 10-second timeout and cancellable by nothing. Run inline
    /// it parks a tokio worker for its whole duration **inside the manager's single-flight connect
    /// future**, during which `close`/`close_all`'s abort cannot preempt the attempt.
    ///
    /// Measured on a ONE-worker runtime, which is what makes the assertion mean something: the
    /// 200 ms timer can only fire on time if the worker is free while the 1-second command runs.
    /// Inline, the timer fires late — at roughly the command's own duration.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_slow_env_command_does_not_hold_the_worker_carrying_the_connect() {
        let mut entry = stdio_entry("exec cat", &[]);
        entry.env = Some(record(&[("SLOW", "!sleep 1; echo value")]));
        let started = std::time::Instant::now();
        let outcome = tokio::time::timeout(
            Duration::from_millis(200),
            builder().connect_stdio(&request("slow-env", entry)),
        )
        .await;
        assert!(outcome.is_err(), "the connect is still in flight at 200 ms");
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(700),
            "the timer fired {elapsed:?} in, so the worker was blocked by the env command rather \
             than free to run the timer"
        );
    }

    /// `requestTimeoutMs` bounds the **handshake**, on the stdio arm.
    ///
    /// The child accepts its pipes and never answers `initialize`. Before
    /// [`connect_client_bounded`], `CreateConnection::request_options` was built by the manager and
    /// read by nobody, so this connect — and with it the manager's per-name single-flight slot —
    /// parked until someone called `close`. MEASURED as still hanging after 6 s with
    /// `requestTimeoutMs: 300`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_wedged_stdio_server_fails_its_handshake_at_request_timeout_ms() {
        let mut entry = stdio_entry("exec sleep 60", &[]);
        entry.request_timeout_ms = Some(300.0);
        let started = std::time::Instant::now();
        let error = builder()
            .connect_stdio(&timed_request("wedged", entry))
            .await
            .expect_err("a server that never answers `initialize` must not hang the connect");
        assert!(
            error.to_string().contains(HANDSHAKE_TIMED_OUT),
            "expected `{HANDSHAKE_TIMED_OUT}`, got {error}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the budget, not the test harness, is what ended this"
        );
    }

    /// The same bound on the HTTP arm: the fixture reads `initialize` and holds the socket open
    /// without answering, so nothing below the MCP layer fails and only the budget can end it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_wedged_http_server_fails_its_handshake_at_request_timeout_ms() {
        let fixture = HttpFixture::start_with(FixtureOptions {
            stall_initialize: true,
            ..FixtureOptions::default()
        })
        .await;
        let mut entry = http_entry(&fixture.url);
        entry.request_timeout_ms = Some(300.0);
        let started = std::time::Instant::now();
        let error = builder()
            .connect_http_client(&timed_request("wedged-http", entry))
            .await
            .expect_err("a silent HTTP server must not hang the connect");
        assert!(
            error.to_string().contains(HANDSHAKE_TIMED_OUT),
            "expected `{HANDSHAKE_TIMED_OUT}`, got {error}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    // ── MCP-114 · §3.4 steps 1–6 ──────────────────────────────────────────────────────────────

    /// `resolveServerUrl`'s missing-variable throw, with its singular/plural rule, raised **before**
    /// any socket is opened.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_url_with_a_missing_variable_fails_before_any_request() {
        let error = builder()
            .connect_http_client(&request("u", http_entry("https://x.example/${NOPE}")))
            .await
            .expect_err("a missing variable is a hard failure");
        assert_eq!(
            error.to_string(),
            "Missing environment variable in MCP server URL: NOPE"
        );

        let error = builder()
            .connect_http_client(&request(
                "u",
                http_entry("https://x.example/${NOPE}/${ALSONOPE}"),
            ))
            .await
            .expect_err("two missing variables pluralise");
        assert_eq!(
            error.to_string(),
            "Missing environment variables in MCP server URL: NOPE, ALSONOPE"
        );
    }

    /// Every header shape of §3.4 steps 2–6, read off the wire: a `!command` header runs, a `!!`
    /// header is an escaped literal, a plain header interpolates, and `auth: "bearer"` becomes
    /// exactly one `Authorization: Bearer <token>`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resolved_headers_and_the_bearer_token_reach_the_wire() {
        let fixture = HttpFixture::start(0).await;
        let mut entry = http_entry(&fixture.url);
        // `x-plain` interpolates against the REAL process environment, not the builder's seam —
        // see the residual named in `the_child_environment_is_resolved_exactly_as_resolve_env_
        // specifies`: `resolve_command_secrets_record` takes no `EnvFn`. The bearer ladder DOES
        // take the seam (`resolve_bearer_token`), which is why the two lines below read different
        // environments for the same syntax.
        let real_home = std::env::var("HOME").unwrap_or_default();
        entry.headers = Some(record(&[
            ("x-command", "!echo hunter2"),
            ("x-escaped", "!!literal"),
            ("x-plain", "${HOME}"),
        ]));
        entry.auth = Some(crate::config::AuthMode::Named(crate::config::AuthKind::Bearer));
        entry.bearer_token = Some("${TOKEN}".to_string());

        let connection = builder()
            .connect_http_client(&request("hdr", entry))
            .await
            .expect("connects");
        connection.resource.close().await.expect("close");

        let first = fixture.requests().first().cloned().expect("one request");
        assert_eq!(first.header("x-command"), Some("hunter2"));
        assert_eq!(first.header("x-escaped"), Some("!literal"));
        assert_eq!(first.header("x-plain"), Some(real_home.as_str()));
        assert_eq!(
            first.all("authorization"),
            vec!["Bearer s3cret"],
            "exactly one Authorization header, and it carries the bearer token"
        );
    }

    /// The header-injection guard: a command whose output contains a newline is refused with
    /// upstream's exact sentence, and nothing is sent.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_newline_bearing_command_secret_is_refused_with_upstreams_sentence() {
        let fixture = HttpFixture::start(0).await;
        let mut entry = http_entry(&fixture.url);
        entry.headers = Some(record(&[("x-bad", "!printf 'a\\nb'")]));

        let error = builder()
            .connect_http_client(&request("inject", entry))
            .await
            .expect_err("a newline cannot go on the wire");
        assert_eq!(
            error.to_string(),
            "Failed to resolve MCP server \"inject\" HTTP command secret: command returned an invalid header value"
        );
        assert!(fixture.requests().is_empty(), "nothing was sent");
    }

    // ── MCP-115 · the implicit/explicit ladder ────────────────────────────────────────────────

    /// The headline property of §3.4: with `auth` unset, **the credential store is not touched
    /// until the server answers 401 once**, and the retry after that challenge is on the same
    /// transport kind.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn implicit_oauth_defers_the_provider_until_the_first_401() {
        let fixture = HttpFixture::start(1).await;
        let auth = CountingAuth::with_token("from-store");
        let connection = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("implicit", http_entry(&fixture.url)))
            .await
            .expect("the second attempt succeeds");

        assert_eq!(connection.status, ConnectionStatus::Connected);
        assert_eq!(fixture.initializes(), 2, "exactly two attempts");
        // ONE provider construction, and it happened only after the challenge arrived — carrying
        // that challenge, which is the `WWW-Authenticate` value the 401 sent.
        let calls = auth.calls();
        assert_eq!(calls.len(), 1, "one provider construction");
        assert_eq!(
            calls.first().and_then(Clone::clone).as_deref(),
            Some(
                "Bearer realm=\"mcp\", resource_metadata=\"https://example.invalid/.well-known\""
            ),
            "the challenge is carried into the provider"
        );
        // The retried attempt carried the token; the first did not.
        let initializes: Vec<Recorded> = fixture
            .requests()
            .into_iter()
            .filter(|request| request.body.contains("\"method\":\"initialize\""))
            .collect();
        assert_eq!(initializes.first().and_then(|r| r.header("authorization")), None);
        assert_eq!(
            initializes.get(1).and_then(|r| r.header("authorization")),
            Some("Bearer from-store")
        );
        assert!(auth.invalidations().is_empty(), "a success invalidates nothing");
        connection.resource.close().await.expect("close");
    }

    /// With `auth` set explicitly the provider exists from the first attempt — the other half of
    /// the implicit/explicit split.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn explicit_oauth_reads_the_store_before_the_first_attempt() {
        let fixture = HttpFixture::start(0).await;
        let auth = CountingAuth::with_token("eager");
        let mut entry = http_entry(&fixture.url);
        entry.auth = Some(crate::config::AuthMode::Named(crate::config::AuthKind::Oauth));

        let connection = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("explicit", entry))
            .await
            .expect("connects on the first attempt");
        assert_eq!(fixture.initializes(), 1);
        assert_eq!(auth.calls(), vec![None], "constructed before any 401");
        assert_eq!(
            fixture
                .requests()
                .first()
                .and_then(|r| r.header("authorization")),
            Some("Bearer eager")
        );
        connection.resource.close().await.expect("close");
    }

    /// A permanent 401 yields `needs-auth` rather than an error, and
    /// `invalidateAuthEntryCache(name)` runs **at most once per episode** (MCP-116).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_permanent_401_becomes_needs_auth_and_invalidates_exactly_once() {
        let fixture = HttpFixture::start(usize::MAX).await;
        let auth = CountingAuth::empty();
        let connection = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("locked", http_entry(&fixture.url)))
            .await
            .expect("needs-auth is not an error");

        assert_eq!(connection.status, ConnectionStatus::NeedsAuth);
        assert!(connection.credentials_invalidated);
        assert_eq!(fixture.initializes(), 2, "one retry, then needs-auth");
        assert_eq!(auth.invalidations(), vec!["locked".to_string()]);

        // The carry-forward: a second connect that already knows the credential was discarded must
        // NOT discard it again.
        let mut second = request("locked", http_entry(&fixture.url));
        second.credentials_invalidated = true;
        let connection = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&second)
            .await
            .expect("needs-auth again");
        assert_eq!(connection.status, ConnectionStatus::NeedsAuth);
        assert_eq!(
            auth.invalidations(),
            vec!["locked".to_string()],
            "still exactly one invalidation across both episodes"
        );
    }

    /// `supportsOAuth` false ⇒ a 401 is a **hard error**, not `needs-auth`. `auth: false` is the
    /// shortest way to say that.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_401_against_a_non_oauth_server_is_a_hard_error() {
        let fixture = HttpFixture::start(usize::MAX).await;
        let mut entry = http_entry(&fixture.url);
        entry.auth = Some(crate::config::AuthMode::Disabled(false));
        let auth = CountingAuth::empty();

        let error = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("noauth", entry))
            .await
            .expect_err("no OAuth means a 401 is fatal");
        assert!(error.to_string().contains("noauth"), "{error}");
        assert_eq!(fixture.initializes(), 1, "no retry without OAuth");
        assert!(auth.calls().is_empty(), "the store was never consulted");
        assert!(auth.invalidations().is_empty());
    }

    /// Arm 3 of the ladder: a close racing a 401 is an **abort**, not a `needs-auth`. The abort
    /// check sits between the aggregate rethrow and the 401 arms and the order is the specification.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_abort_racing_a_401_wins_over_the_needs_auth_arm() {
        // Deterministic by construction: the fixture cancels the attempt token BEFORE it writes
        // the first 401, so by the time the attempt settles the abort is already visible. A
        // sleep-then-cancel here would be a race against two loopback round trips, and the wrong
        // side of it is a `needs-auth` that silently passes for the wrong reason.
        let token = CancelToken::new();
        let fixture =
            HttpFixture::start_cancelling_before_response(usize::MAX, 1, token.clone()).await;
        let mut connect = request("raced", http_entry(&fixture.url));
        connect.attempt = token;
        let auth = CountingAuth::empty();
        let error = builder()
            .with_auth_provider(Arc::clone(&auth) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&connect)
            .await
            .expect_err("an aborted connect is an error, never needs-auth");
        assert!(
            matches!(error, McpError::Aborted(_)),
            "expected an abort, got {error}"
        );
        assert!(auth.invalidations().is_empty(), "an abort invalidates nothing");
    }

    // ── MCP-115 · the STORE-BACKED provider — journey A, the returning user ───────────────────
    //
    // Everything above this line proves the ladder against `CountingAuth`, a hand-scripted double.
    // These prove the same ladder against [`StoredCredentialAuth`] over a real
    // [`crate::credentials::McpAuthStore`] — the type production installs — with the fixture's
    // **only** gate being the bearer token, so a provider that answered `None` could not pass.

    /// A memory-backed vault rooted in a fresh temp dir.
    ///
    /// `with_backends` rather than `new`: the keychain is a host dependency and a write to it would
    /// be a side effect on the developer's machine. The stub environment also pins the entry cache
    /// **on**, which the eviction test below depends on.
    fn stored_credential_store() -> (crate::credentials::McpAuthStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = crate::credentials::McpAuthStore::with_backends(
            Arc::new(crate::credentials::MemorySecretStore::new()),
            Arc::new(crate::credentials::MemorySecretStore::new()),
            McpDirs::new(dir.path().to_path_buf(), dir.path().to_path_buf()),
            AuthStorageOptions::with_base_dir(dir.path().join("mcp-oauth")),
            Arc::new(|_| None),
        );
        (store, dir)
    }

    /// A memory-backed vault whose backend fails every operation — the broken keychain.
    fn broken_credential_store() -> (crate::credentials::McpAuthStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = crate::credentials::McpAuthStore::with_backends(
            Arc::new(crate::credentials::MemorySecretStore::with_fault(
                crate::credentials::SimulatedFault::Unavailable,
            )),
            Arc::new(crate::credentials::MemorySecretStore::new()),
            McpDirs::new(dir.path().to_path_buf(), dir.path().to_path_buf()),
            AuthStorageOptions::with_base_dir(dir.path().join("mcp-oauth")),
            Arc::new(|_| None),
        );
        (store, dir)
    }

    fn now_secs() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs() as i64)
    }

    /// Put a credential in the vault through the v2.25.0 plaintext entry the store imports on its
    /// first read.
    ///
    /// This route needs no `rmcp` type to build a `StoredCredentials`, and `clientInfo.clientId` is
    /// mandatory — `translate_legacy_entry` drops tokens with no client id rather than fabricating
    /// one. `serverUrl` must match byte-for-byte: `get_auth_for_url` is fail-closed and compares
    /// with string equality, no normalization.
    fn seed_credential(
        store: &crate::credentials::McpAuthStore,
        server: &str,
        url: &str,
        token: &str,
        expires_at: i64,
    ) {
        let path = store.auth_entry_file_path(server);
        std::fs::create_dir_all(path.parent().expect("a server directory")).expect("mkdir");
        let body = serde_json::json!({
            "tokens": {
                "accessToken": token,
                "refreshToken": "fixture-refresh",
                "expiresAt": expires_at,
                "scope": "mcp",
            },
            "clientInfo": { "clientId": "fixture-client" },
            "serverUrl": url,
        });
        std::fs::write(&path, serde_json::to_vec(&body).expect("json")).expect("write");
    }

    /// The provider production builds, plus the runtime it must be shut down with.
    fn stored_provider(
        store: &crate::credentials::McpAuthStore,
    ) -> (Arc<StoredCredentialAuth>, Arc<OAuthRuntime>) {
        let runtime = crate::oauth::create_oauth_runtime(None);
        let provider = Arc::new(StoredCredentialAuth::new(
            store.clone(),
            Arc::clone(&runtime),
            CancelToken::new(),
            std::collections::HashSet::new(),
        ));
        (provider, runtime)
    }

    fn explicit_oauth_entry(url: &str) -> ServerEntry {
        ServerEntry {
            auth: Some(crate::config::AuthMode::Named(crate::config::AuthKind::Oauth)),
            ..http_entry(url)
        }
    }

    /// **Journey A.** A credential is already in the vault; the server connects on attempt one,
    /// with no 401, no retry and nothing opened.
    ///
    /// The fixture's only gate is the bearer token — it does not count attempts — so this cannot
    /// pass with a provider that answers `None`, which is exactly what `ConnectionBuilder::new`'s
    /// default does and what production shipped before [`StoredCredentialAuth`] was installed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stored_credential_connects_an_http_server_on_the_first_attempt() {
        let fixture = HttpFixture::start_with(FixtureOptions {
            require_bearer: Some("stored-token"),
            ..FixtureOptions::default()
        })
        .await;
        let (store, _dir) = stored_credential_store();
        seed_credential(
            &store,
            "returning",
            &fixture.url,
            "stored-token",
            now_secs() + 3600,
        );
        let (provider, runtime) = stored_provider(&store);

        // The whole factory seam, not just the ladder: `create_connection` also runs MCP-119's
        // discovery, so the assertions below cover the requests that follow the handshake — which
        // is where a token that authorized only the first POST would show up.
        let connection = builder()
            .with_auth_provider(Arc::clone(&provider) as Arc<dyn HttpAuthProvider>)
            .create_connection(request("returning", explicit_oauth_entry(&fixture.url)))
            .await
            .expect("a stored credential connects");

        assert_eq!(connection.status, ConnectionStatus::Connected);
        assert_eq!(
            fixture.initializes(),
            1,
            "explicit OAuth reads the store BEFORE the handshake, so there is no 401 and no retry"
        );
        let requests = fixture.requests();
        let handshake = requests.first().expect("the handshake was sent");
        assert_eq!(
            handshake.all("authorization"),
            vec!["Bearer stored-token"],
            "exactly one Authorization header, carrying the stored token"
        );
        assert!(
            requests
                .iter()
                .any(|recorded| recorded.body.contains("\"method\":\"tools/list\"")),
            "discovery ran, so the token authorized more than the handshake"
        );
        for recorded in &requests {
            assert_eq!(
                recorded.all("authorization"),
                vec!["Bearer stored-token"],
                "every request on the connection carried the credential: {}",
                recorded.method
            );
        }

        // The provider is printed by `ConnectionBuilder`'s own `Debug`; a token must never be in it.
        let rendered = format!("{provider:?}");
        assert!(!rendered.contains("stored-token"), "{rendered}");
        assert!(rendered.contains("StoredCredentialAuth"), "{rendered}");

        connection.resource.close().await.expect("close");
        crate::oauth::shutdown_oauth(&runtime).await;
    }

    /// The negative control for the test above: same fixture, same wiring, **no** seeded
    /// credential. `needs-auth`, and — because `auth: "oauth"` is the explicit arm — without a
    /// retry.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_empty_store_ends_an_explicit_oauth_server_at_needs_auth() {
        let fixture = HttpFixture::start_with(FixtureOptions {
            require_bearer: Some("stored-token"),
            ..FixtureOptions::default()
        })
        .await;
        let (store, _dir) = stored_credential_store();
        let (provider, runtime) = stored_provider(&store);

        let connection = builder()
            .with_auth_provider(Arc::clone(&provider) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("first-login", explicit_oauth_entry(&fixture.url)))
            .await
            .expect("needs-auth is not an error");

        assert_eq!(connection.status, ConnectionStatus::NeedsAuth);
        assert!(connection.credentials_invalidated);
        assert_eq!(fixture.initializes(), 1, "explicit ⇒ no retry");
        assert_eq!(
            fixture
                .requests()
                .first()
                .and_then(|recorded| recorded.header("authorization")),
            None,
            "an empty vault attaches nothing"
        );
        crate::oauth::shutdown_oauth(&runtime).await;
    }

    /// The implicit arm of journey A: with `auth` absent the vault is not touched until the server
    /// proves it needs to be, and the retry after that 401 carries the stored token. Two requests,
    /// still no prompt.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_implicit_oauth_server_retries_once_with_the_stored_token() {
        let fixture = HttpFixture::start_with(FixtureOptions {
            require_bearer: Some("stored-token"),
            ..FixtureOptions::default()
        })
        .await;
        let (store, _dir) = stored_credential_store();
        seed_credential(
            &store,
            "implicit",
            &fixture.url,
            "stored-token",
            now_secs() + 3600,
        );
        let (provider, runtime) = stored_provider(&store);

        let connection = builder()
            .with_auth_provider(Arc::clone(&provider) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("implicit", http_entry(&fixture.url)))
            .await
            .expect("the retry carries the stored token");

        assert_eq!(connection.status, ConnectionStatus::Connected);
        assert_eq!(fixture.initializes(), 2, "one 401, then one authorized attempt");
        let initializes: Vec<Recorded> = fixture
            .requests()
            .into_iter()
            .filter(|recorded| recorded.body.contains("\"method\":\"initialize\""))
            .collect();
        assert_eq!(
            initializes.first().and_then(|r| r.header("authorization")),
            None,
            "deferred: nothing was read before the 401"
        );
        assert_eq!(
            initializes.get(1).and_then(|r| r.header("authorization")),
            Some("Bearer stored-token")
        );
        connection.resource.close().await.expect("close");
        crate::oauth::shutdown_oauth(&runtime).await;
    }

    /// A broken keychain is **not** `needs-auth`. Answering `Ok(None)` here would make an
    /// unreadable vault indistinguishable from "you have never logged in": the user is sent to
    /// authenticate, the flow writes a credential that cannot be read back, and the loop never
    /// terminates. The connect fails, loudly, before a single byte is sent.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_broken_credential_store_fails_the_connect_rather_than_asking_for_a_login() {
        let fixture = HttpFixture::start_with(FixtureOptions {
            require_bearer: Some("stored-token"),
            ..FixtureOptions::default()
        })
        .await;
        let (store, _dir) = broken_credential_store();
        let (provider, runtime) = stored_provider(&store);

        let error = builder()
            .with_auth_provider(Arc::clone(&provider) as Arc<dyn HttpAuthProvider>)
            .connect_http_client(&request("broken", explicit_oauth_entry(&fixture.url)))
            .await
            .expect_err("an unreadable vault is an error, never needs-auth");

        assert!(
            error.is_credential_store_failure(),
            "expected a credential-store failure, got {error}"
        );
        assert_eq!(fixture.initializes(), 0, "nothing was sent");
        crate::oauth::shutdown_oauth(&runtime).await;
    }

    /// `authorize` hands `get_valid_token` **this generation's** runtime, never `None`.
    ///
    /// `get_runtime(None)` resurrects the module-level legacy runtime and re-inserts it into the
    /// process-global live set that only `shutdown_oauth` removes, so a `None` would leak one
    /// live-runtime id per connect and wedge the shared callback listener open for the life of the
    /// process. The observable difference is here: against a **shut-down** runtime the provider
    /// must abort, where a `None` would have quietly succeeded off the legacy one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn authorize_uses_the_generation_runtime_and_never_the_legacy_one() {
        let url = "https://fixture.invalid/mcp";
        let (store, _dir) = stored_credential_store();
        seed_credential(&store, "scoped", url, "stored-token", now_secs() + 3600);
        let (provider, runtime) = stored_provider(&store);

        assert_eq!(
            provider
                .authorize("scoped", url, None)
                .await
                .expect("a live runtime reads the vault"),
            Some("stored-token".to_string()),
            "the credential is reachable while the runtime is live"
        );

        crate::oauth::shutdown_oauth(&runtime).await;
        let error = provider
            .authorize("scoped", url, None)
            .await
            .expect_err("a dead runtime aborts rather than falling back");
        assert!(
            matches!(error, McpError::Aborted(_)),
            "expected an abort, got {error}"
        );
    }

    /// `invalidate_auth_entry_cache` forgets the cached entry so the next read reloads secure
    /// storage — including a cached **absence**, which is the shape that matters: the vault caches
    /// `None` as readily as `Some`, so without eviction a credential written after a failed connect
    /// would stay invisible for the life of the process.
    ///
    /// It is deliberately unconditional. The once-per-episode policy is
    /// [`ConnectionBuilder::connect_http_client`]'s `invalidated` flag (MCP-116); a second latch
    /// here would mean a rotated credential is never re-read.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn invalidate_auth_entry_cache_forgets_a_cached_absence() {
        let url = "https://fixture.invalid/mcp";
        let (store, _dir) = stored_credential_store();
        let (provider, runtime) = stored_provider(&store);

        assert_eq!(
            provider.authorize("rotating", url, None).await.expect("empty"),
            None,
            "nothing is stored yet"
        );

        seed_credential(&store, "rotating", url, "late-token", now_secs() + 3600);
        assert_eq!(
            provider.authorize("rotating", url, None).await.expect("cached"),
            None,
            "the cached absence still stands — this is what makes the eviction load-bearing"
        );

        provider.invalidate_auth_entry_cache("rotating");
        assert_eq!(
            provider
                .authorize("rotating", url, None)
                .await
                .expect("reloaded"),
            Some("late-token".to_string()),
            "the next read reached secure storage"
        );

        // Twice in a row, with no episode latch of its own.
        provider.invalidate_auth_entry_cache("rotating");
        provider.invalidate_auth_entry_cache("rotating");
        assert_eq!(
            provider
                .authorize("rotating", url, None)
                .await
                .expect("still readable"),
            Some("late-token".to_string())
        );
        crate::oauth::shutdown_oauth(&runtime).await;
    }

    /// A credential minted for one URL is not presented to another. The vault's binding is
    /// fail-closed and `authorize` neither widens nor works around it, so a server whose `url`
    /// changed lands at `needs-auth` and re-authenticates rather than leaking a token to a host it
    /// was never issued for.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_credential_bound_to_another_url_is_not_offered() {
        let (store, _dir) = stored_credential_store();
        seed_credential(
            &store,
            "moved",
            "https://old.invalid/mcp",
            "stored-token",
            now_secs() + 3600,
        );
        let (provider, runtime) = stored_provider(&store);

        assert_eq!(
            provider
                .authorize("moved", "https://new.invalid/mcp", None)
                .await
                .expect("a mismatch is not an error"),
            None
        );
        crate::oauth::shutdown_oauth(&runtime).await;
    }

    // ── MCP-115a · the per-request header command ─────────────────────────────────────────────

    /// The signing command runs on **every** outbound request, not once per connection, and its
    /// derived `Authorization` **replaces** the bearer one rather than joining it — upstream's
    /// `headers.set` semantics (`request-headers-command.ts:320`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_request_headers_command_signs_every_request_and_owns_authorization() {
        let fixture = HttpFixture::start(0).await;
        let mut entry = http_entry(&fixture.url);
        entry.auth = Some(crate::config::AuthMode::Named(crate::config::AuthKind::Bearer));
        entry.bearer_token = Some("static-bearer".to_string());
        entry.request_headers_command = Some(crate::config::HttpRequestHeadersCommand {
            command: Some("sh".to_string()),
            args: Some(vec![
                "-c".to_string(),
                "printf '{\"x-signature\":\"sig\",\"authorization\":\"Signature derived\"}'"
                    .to_string(),
            ]),
            ..crate::config::HttpRequestHeadersCommand::default()
        });

        let connection = builder()
            .connect_http_client(&request("signed", entry))
            .await
            .expect("connects");
        connection.resource.close().await.expect("close");

        let requests = fixture.requests();
        let posts: Vec<&Recorded> = requests
            .iter()
            .filter(|request| request.method == "POST")
            .collect();
        assert!(
            posts.len() >= 2,
            "initialize plus notifications/initialized at least: {}",
            posts.len()
        );
        for post in &posts {
            assert_eq!(post.header("x-signature"), Some("sig"));
            assert_eq!(
                post.all("authorization"),
                vec!["Signature derived"],
                "exactly one Authorization, and it is the DERIVED one"
            );
        }
    }

    /// A signing helper that fails aborts the request with upstream's sentence rather than sending
    /// an unsigned one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failing_signing_helper_aborts_the_request_instead_of_sending_it_unsigned() {
        let fixture = HttpFixture::start(0).await;
        let mut entry = http_entry(&fixture.url);
        entry.request_headers_command = Some(crate::config::HttpRequestHeadersCommand {
            command: Some("sh".to_string()),
            args: Some(vec!["-c".to_string(), "exit 7".to_string()]),
            ..crate::config::HttpRequestHeadersCommand::default()
        });

        let error = builder()
            .connect_http_client(&request("broken", entry))
            .await
            .expect_err("a failing helper fails the connect");
        assert!(
            error.to_string().contains("exited with code 7"),
            "expected the helper's own sentence: {error}"
        );
        assert!(
            fixture.requests().is_empty(),
            "nothing may go on the wire unsigned"
        );
    }


    // ── MCP-124 · does the taxonomy actually unblock `close`'s pending-connect rethrow? ────────

    /// A factory that parks until released, then fails with whatever it was given.
    #[derive(Debug)]
    struct ParkedFactory {
        gate: Arc<tokio::sync::Notify>,
        error: std::sync::Mutex<Option<McpError>>,
    }

    impl ConnectionFactory for ParkedFactory {
        fn create(&self, _request: CreateConnection) -> BoxFuture<'static, McpResult<NewConnection>> {
            let gate = Arc::clone(&self.gate);
            let error = self
                .error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            Box::pin(async move {
                gate.notified().await;
                Err(error.unwrap_or_else(|| McpError::other("no error was armed")))
            })
        }
    }

    /// `close`'s no-connection arm:
    /// ```text
    /// const pendingConnect = this.connectPromises.get(name);
    /// if (pendingConnect) { try { await pendingConnect } catch (error) {
    ///   if (this.containsCleanupFailure(error)) throw error; } }
    /// ```
    ///
    /// **This is the test the MCP-124 hand-off was about.** Before MCP-124 landed the aggregate
    /// variants, `McpError::is_cleanup_failure` matched only `RuntimeCleanupFailed | OAuthAggregate`
    /// — so a `createConnection` that failed with `MCP connection setup failed` was swallowed here
    /// and `close` reported success. The arm now fires, and it fires *structurally*: the
    /// discriminator is the variant, never the text.
    ///
    /// The pairing matters as much as the positive case: an ordinary connect failure — including one
    /// whose message literally reads `"MCP connection setup failed"` — must still be swallowed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn close_rethrows_a_pending_connects_setup_failure_and_swallows_everything_else() {
        async fn race_close_against(error: McpError) -> McpResult<()> {
            let gate = Arc::new(tokio::sync::Notify::new());
            let factory = Arc::new(ParkedFactory {
                gate: Arc::clone(&gate),
                error: std::sync::Mutex::new(Some(error)),
            });
            let manager = Arc::new(crate::server_manager::McpServerManager::with_factory(
                None, factory,
            ));
            let entry = ServerEntry {
                command: Some("true".to_string()),
                ..ServerEntry::default()
            };

            let connecting = {
                let manager = Arc::clone(&manager);
                let entry = entry.clone();
                tokio::spawn(async move { manager.connect("parked", &entry, None).await })
            };
            // Wait until the attempt is actually registered, so `close` takes the
            // no-connection-but-pending-connect branch rather than the do-nothing one.
            for _ in 0..200_u32 {
                if manager.is_connecting("parked") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert!(manager.is_connecting("parked"), "the connect must be in flight");

            let closing = {
                let manager = Arc::clone(&manager);
                tokio::spawn(async move { manager.close("parked").await })
            };
            tokio::time::sleep(Duration::from_millis(20)).await;
            gate.notify_waiters();

            let _ = connecting.await.expect("join connect");
            closing.await.expect("join close")
        }

        // The teardown failure: re-thrown.
        let outcome = race_close_against(McpError::SetupFailed(crate::errors::CleanupErrors::from(
            vec![
                McpError::other("connect ECONNREFUSED"),
                McpError::other("transport close failed"),
            ],
        )))
        .await;
        let error = outcome.expect_err("a setup failure must reach the closer");
        assert_eq!(
            error.to_string(),
            "connect ECONNREFUSED: transport close failed",
            "and it renders the way formatTerminalError renders it"
        );
        // The class survives the `ManagerError` → `McpError` boundary too, which is the half of
        // MCP-124 that `From<&ManagerError>` had to land: before it, this arrived as
        // `McpError::Other` carrying the same text and `is_cleanup_failure()` was `false`.
        assert!(
            matches!(error, McpError::SetupFailed(_)),
            "expected `SetupFailed` at the public boundary, got {error:?}"
        );
        assert!(error.is_cleanup_failure());

        // The ordinary connect failure: swallowed.
        race_close_against(McpError::other("connect ECONNREFUSED"))
            .await
            .expect("an ordinary connect failure during a close is expected and swallowed");

        // THE DOCUMENTED DIVERGENCE, at the one call site it changes: a server that puts the
        // aggregate's own text in an error message satisfies upstream's
        // `/cleanup failed|setup failed/` and is re-thrown there; here it is swallowed.
        race_close_against(McpError::Server {
            server: "parked".to_string(),
            message: "MCP connection setup failed".to_string(),
        })
        .await
        .expect("a text-only impostor is not a cleanup failure");
    }

    /// `SetupFailed`'s one producer that exists **today**, exercised.
    ///
    /// The doc on `create_connection` used to say the variant had "no producer on this path". It
    /// does have one — narrow, but real: `post_handshake`'s abort check is a genuine post-handshake
    /// step, and when a `close` racing a settled handshake trips it *and* the `resource.close()`
    /// after it also fails, the catch wraps both into `MCP connection setup failed`. Discovery
    /// (MCP-119) is upstream's producer and has not landed; that is a different sentence from
    /// "none exists", and the difference is what this test holds in place.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_abort_whose_own_cleanup_fails_is_a_setup_failure() {
        #[derive(Debug)]
        struct FailingClose;
        impl ConnectionResource for FailingClose {
            fn close(&self) -> futures::future::BoxFuture<'_, McpResult<()>> {
                Box::pin(async { Err(McpError::other("transport close failed")) })
            }
        }

        let mut connect = request("raced", stdio_entry("exec cat", &[]));
        // The close that arrived a microsecond after the handshake settled.
        connect.attempt = CancelToken::new();
        connect.attempt.cancel();

        // `NewConnection` is not `Debug`, so the Ok arm is destructured rather than `expect_err`'d.
        let Err(error) = builder()
            .post_handshake(&connect, Arc::new(FailingClose) as Arc<dyn ConnectionResource>, false)
            .await
        else {
            panic!("an aborted post-handshake step is a failure");
        };
        assert!(
            matches!(error, McpError::SetupFailed(_)),
            "expected `SetupFailed`, got {error:?}"
        );
        assert!(error.is_cleanup_failure(), "and `close_inner` must be able to see it");

        // The pairing: the SAME abort with a cleanup that SUCCEEDS is the abort, not a setup
        // failure. That is what makes the aggregate mean "the teardown failed too".
        #[derive(Debug)]
        struct CleanClose;
        impl ConnectionResource for CleanClose {
            fn close(&self) -> futures::future::BoxFuture<'_, McpResult<()>> {
                Box::pin(async { Ok(()) })
            }
        }
        let Err(error) = builder()
            .post_handshake(&connect, Arc::new(CleanClose) as Arc<dyn ConnectionResource>, false)
            .await
        else {
            panic!("still an abort");
        };
        assert!(matches!(error, McpError::Aborted(_)), "got {error:?}");
    }

    /// The factory is installable: `McpServerManager::with_factory(cwd, Arc::new(builder))` replaces
    /// `UnbuiltConnectionFactory` and a real server connects through the manager's own state
    /// machine. Without this the ladder above is only reachable from its own tests.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_builder_is_a_usable_connection_factory_for_the_manager() {
        let fixture = HttpFixture::start(0).await;
        let manager = Arc::new(crate::server_manager::McpServerManager::with_factory(
            None,
            Arc::new(builder()),
        ));
        let connection = manager
            .connect("live", &http_entry(&fixture.url), None)
            .await
            .expect("the manager drives the builder");
        assert_eq!(connection.status(), ConnectionStatus::Connected);
        assert!(connection.resource().has_session_id());
        manager.close("live").await.expect("close");
    }


    /// A failed handshake must not leak the child, and the *mechanism* is worth pinning because it
    /// is not the obvious one: nothing in this port calls `close()` on that path.
    /// `serve_client_with_ct_inner` drops the transport, and `ChildWithCleanup::drop`
    /// (`rmcp-3.1.4/src/transport/child_process.rs:45-57`) spawns a `kill()`. If a future rmcp
    /// version moves the child out of that guard before its first await — which is exactly what
    /// `graceful_shutdown` already does — this test is what notices.
    ///
    /// The fixture ignores stdin closure and never answers `initialize`, so only the kill can end
    /// it. Linux-only because it reads `/proc`, the same gate `server_manager`'s child-process
    /// tests use.
    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failed_handshake_leaves_no_child_behind() {
        fn alive(pid: u32) -> bool {
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                return false;
            };
            // `pid (comm) state …`; `comm` may contain spaces and parentheses, so the state is the
            // first field after the LAST `)`. A killed-but-unreaped child is `Z` and is NOT alive.
            stat.rsplit_once(')').is_some_and(|(_, rest)| {
                rest.trim_start().chars().next().is_some_and(|state| state != 'Z')
            })
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let pid_file = temp.path().join("pid");
        // Writes its pid, then blocks forever on a fifo-less read that stdin closure cannot end —
        // `sleep` ignores stdin entirely, which is the whole point.
        let script = "printf '%s' \"$$\" > \"$P\"; exec sleep 60";
        let mut entry = stdio_entry(script, &[]);
        entry.env = Some(record(&[("P", pid_file.to_string_lossy().into_owned().as_str())]));

        let connect = request("hung", entry);
        let token = connect.attempt.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            token.cancel();
        });
        let error = builder()
            .connect_stdio(&connect)
            .await
            .expect_err("a server that never answers `initialize` cannot connect");
        handle.await.expect("join");
        assert!(
            matches!(error, McpError::Server { .. }),
            "expected a connect failure, got {error}"
        );

        let pid: u32 = std::fs::read_to_string(&pid_file)
            .expect("the child wrote its pid")
            .trim()
            .parse()
            .expect("a pid");
        // The kill is spawned as a detached task, so it is not synchronous with the error.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline && alive(pid) {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(!alive(pid), "the child survived a failed handshake (pid {pid})");
    }

}
