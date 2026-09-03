//! The `NativeExtension` impl and the construction gate — `index.ts` (13a; MCP-001, MCP-008,
//! MCP-009, MCP-011, MCP-013, MCP-014).
//!
//! Upstream, `index.ts`'s `createMcpAdapter(options)` clones the programmatic config at factory
//! time and again per call, returns `mcpAdapter(pi)`, and `export default createMcpAdapter()` is
//! what pi's loader invokes; `package.json` declares
//! `"pi": {"extensions": ["./index.ts"], "skills": ["./skills"]}`. Here that is
//! [`mcp_extension_for_env`] plus `SessionFactory::with_native_extension` at the three session-build
//! arms of `crates/cyrup/src/main.rs`.
//!
//! # Why the state lives on the struct
//!
//! In pi, `lifecycleGeneration`, `currentOwner`, `currentOAuthRuntime`, `state`, `initPromise`,
//! `registeredDirectTools`, `fallbackDeactivatedTools`, `registeredPromptCommands`,
//! `proxyToolRegistered`, `proxyToolDescription` and `directToolsFrozen` are **closure variables of
//! a factory that runs once per process**. cyrup re-runs `init()` on the *same*
//! `Arc<dyn NativeExtension>` for every session build, so those variables become fields here and
//! survive re-`init` exactly as upstream's closure variables survive `session_start`.
//!
//! One consequence must be handled explicitly (MCP-014): because the *registry* is fresh on every
//! build but the fingerprint map is not, `init()` must register **every** tool while still updating
//! the fingerprints. The fingerprint diff (MCP-036) suppresses re-registration only *within* a
//! session, never across an `init()`.
//!
//! # The division of labour across the four entry points
//!
//! * **`init()` performs registration only** — read the config and the cache, register the tools,
//!   commands, renderers and the flag, subscribe, spawn the pre-warm task. **No teardown of a
//!   previous generation.**
//! * **`on_event(SessionShutdown)` is the only teardown point**, and it is where the metadata flush
//!   lives.
//! * **`on_event(SessionStart)` is the generation bump** and builds the new runtime.
//! * **`on_event(Input)` gates the turn** on one keep-alive convergence pass — `index.ts:489-511`,
//!   added upstream by `48799fa` after 13a was written. It owns no state and bumps nothing; it only
//!   spends time, so that the turn about to be built sees a current tool catalog.
//!
//! Putting teardown in `init()` would kill generation N's MCP children before N's own shutdown
//! flush ran — and if the build then failed, generation N would stay live with a torn-down MCP
//! runtime and no path back.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use cyrup_core::ExtensionId;
use cyrup_ext::native::{HostCtx, InitApi, NativeExtension};
use cyrup_ext::{ExtError, HookOutcome, HostEvent};
use futures::future::{BoxFuture, FutureExt, Shared};
use indexmap::IndexMap;

use crate::config::McpConfig;
use crate::dirs::McpDirs;
use crate::errors::McpError;
use crate::owner::McpRuntimeOwner;
use crate::registration::{RegisteredSurface, ToolDispatch};
use crate::state::McpState;

/// The literal, stable extension id every registration, log and diagnostic surface refers to.
pub const EXTENSION_ID: &str = "mcp";

/// One in-flight `initializeMcp`, memoised so a second `SessionStart` within the same generation
/// joins it rather than starting a rival build.
///
/// Held as `Arc<InitTask>` so `startInitialization`'s third staleness check — *"a second
/// initialization has superseded this one within the same generation"* — is an **identity**
/// comparison (`Arc::ptr_eq`), not a value comparison. Dropping that check silently permits a
/// double commit within one generation (MCP-011).
pub type InitTask = Shared<BoxFuture<'static, Result<Arc<McpState>, Arc<McpError>>>>;

/// `startInitialization(..., "stale_session_start")` (`index.ts:405`) — the
/// [`crate::lifecycle::shutdown_state`] reason a build that lost its generation is torn down under.
///
/// A sibling of [`crate::lifecycle::SESSION_RESTART_STATE_REASON`] and
/// [`crate::lifecycle::SESSION_SHUTDOWN_STATE_REASON`], and it lives here rather than beside them
/// because it names a state this file — not `lifecycle.rs` — is the only producer of: a runtime that
/// was built and then never committed. `shutdown_state` takes `&'static str`, so the literal has to
/// be a const somewhere.
pub const STALE_SESSION_START_STATE_REASON: &str = "stale_session_start";

/// The MCP adapter as one native extension.
pub struct McpExtension {
    id: ExtensionId,
    /// `<agent_dir>` + the session cwd, resolved by the binary before construction. `init()`
    /// carries no `HostCtx`, so both must be threaded in explicitly — the same reason
    /// `cyrup_ext_subagents::extension::SubagentsExtension` captures its `cwd` at construction.
    dirs: McpDirs,
    /// `createMcpAdapter(options).config` — a caller-supplied configuration that replaces
    /// discovery. Cloned per use, so a caller cannot mutate a live runtime through it.
    programmatic_config: Option<McpConfig>,
    /// The live capability backend, stashed by [`NativeExtension::set_host_services`], which the
    /// builder calls **before** `init`. That ordering is what makes an `init`-spawned background
    /// task legitimate: it already holds the backend and observes the later manager / UI / inject
    /// attachments through the `Arc`'s interior mutability.
    host_services: Arc<OnceLock<Arc<dyn cyrup_ext::host::HostServices>>>,
    /// `lifecycleGeneration`. Bumped on every `SessionStart` and every `SessionShutdown`; every
    /// asynchronous continuation re-checks it before writing anywhere.
    generation: AtomicU64,
    /// `currentOwner` — the live generation's ownership token.
    owner: Mutex<Option<Arc<McpRuntimeOwner>>>,
    /// `currentOAuthRuntime` — the live generation's OAuth flow registry.
    ///
    /// Held here, and not merely inside [`McpState`], because it must outlive a build that never
    /// commits: [`crate::oauth::create_oauth_runtime`] inserts the runtime's id into a
    /// **process-global** live-runtime set and only [`crate::oauth::shutdown_oauth`] removes it and
    /// drops the shared loopback listener. A generation whose `initializeMcp` rejected has no state
    /// to reach the runtime through, so without this slot every failed or superseded session start
    /// would leak one live-runtime id for the life of the process.
    ///
    /// It is also what makes [`crate::lifecycle::PreviousGeneration::oauth`] reachable from the two
    /// session handlers — the field that snapshot type has always been shaped for.
    oauth_runtime: Mutex<Option<Arc<crate::state::OAuthRuntime>>>,
    /// `state` — the committed runtime, or `None` between generations.
    state: Mutex<Option<Arc<McpState>>>,
    /// `initPromise` — the in-flight build, if any.
    init_task: Mutex<Option<Arc<InitTask>>>,
    /// `registeredDirectTools` — tool name to fingerprint, surviving re-`init` (MCP-036).
    registered_direct_tools: Mutex<IndexMap<String, String>>,
    /// `registeredPromptCommands` — the prompt-command dedup set, surviving re-`init`.
    registered_prompt_commands: Mutex<IndexMap<String, String>>,
    /// `fallbackDeactivatedTools` — tools removed through the `setActiveTools` fallback because
    /// cyrup, like upstream's own documented no-`unregisterTool` branch, cannot unregister.
    fallback_deactivated_tools: Mutex<Vec<String>>,
    /// `proxyToolDescription` — the last description the proxy tool was registered with, so an
    /// unchanged description does not re-register and invalidate the prompt cache.
    proxy_tool_description: Mutex<Option<String>>,
    /// `directToolsFrozen` — set once `settings.freezeDirectTools` has taken effect after the
    /// initial sync. Once frozen, reconnects never rebuild the system prompt.
    direct_tools_frozen: AtomicBool,
    /// The executor slot every tool registered by the CURRENT `init` pass reads
    /// ([`RegisteredSurface::dispatch`]). `init` mints a fresh [`ToolDispatch`] per pass, hands it
    /// to every `DirectTool` / `ProxyTool` it registers, and stashes the same `Arc` here — without
    /// which the runtime has no reachable handle to install [`crate::registration::McpToolDispatch`]
    /// into and every registered MCP tool would answer `MCP not initialized` forever (MCP-214).
    dispatch: Mutex<Option<Arc<ToolDispatch>>>,
    /// The post-`init` registration handle (HA-1 / MCP-037), stashed the same way
    /// [`Self::set_host_services`] stashes its backend. `None` on a host that never bound one
    /// (a test harness constructing the extension directly), which every caller treats as
    /// "the surface is whatever `init` registered" — the pre-HA-1 behaviour, degraded gracefully.
    late_registrar: Mutex<Option<Arc<dyn cyrup_ext::LateRegistrar>>>,
    /// A `Weak` to this extension, bound by [`Self::into_arc`] — which is construction itself, not
    /// a step a caller can forget. Every `Arc<McpExtension>` in the tree comes through it.
    ///
    /// The `onToolMetadataUpdated` listener lives in [`McpState`], which this extension owns, so
    /// the listener cannot hold a STRONG handle without cycling. It cannot take one from its
    /// caller either: every construction site coerces to `Arc<dyn NativeExtension>` in the same
    /// expression that builds the `Arc`, and every holder downstream — including
    /// `crates/cyrup/src/main.rs` — has only the trait object, which does not downcast back.
    /// Binding the `Weak` at construction is what makes [`Self::install_surface_sync`] callable.
    self_weak: OnceLock<std::sync::Weak<McpExtension>>,
    /// This generation's [`crate::proxy::ProxyCtx`], built over the one production
    /// [`crate::proxy::ProxyEnv`] by [`Self::install_runtime_env`] and read by the dispatcher
    /// (MCP-214). `None` until the commit tail installs it, because the env holds the committed
    /// state and so cannot exist before the commit.
    proxy_ctx: Mutex<Option<Arc<crate::proxy::ProxyCtx>>>,
    /// `resolveMcpToolRenderOptions(settings)`, resolved **once, at load**, from the same early
    /// config `init` registered the surface from (MCP-238).
    ///
    /// Upstream resolves it at `installMcpAdapter` time and closes over the result, so
    /// `toolResultRendering` and `collapsedResultLines` are frozen for the session: a transcript
    /// whose rows changed shape halfway through because the user edited `mcp.json` would be a
    /// port defect, not a feature. Re-resolved on each `init` — i.e. once per generation — which is
    /// exactly upstream's "once per adapter install".
    render_options: Mutex<crate::renderers::McpToolRenderOptions>,
    /// The home directory the six-source ladder's three tool-agnostic global rungs
    /// (`~/.config/mcp/mcp.json`, `~/.agents/mcp.json`, `~/.agents/mcp/mcp.json`) and all seven
    /// import families resolve against. `None` ⇒ [`crate::config::home_dir`], which is production.
    ///
    /// This is a *field* for the same reason [`crate::config::ConfigContext::home`] is: edition 2024
    /// made `std::env::set_var` `unsafe` and std's own conclusion is that a multithreaded program
    /// must not call it at all, so `CYRUP_HOME` is unusable from an in-process test. Without this
    /// seam, `MCP-001`'s cyrup-it proof would read the developer's real `~/.config/mcp/mcp.json`
    /// and its result would be a property of the machine rather than of the port.
    home: Option<PathBuf>,
    /// The credential vault each generation authenticates through, forwarded to
    /// [`crate::runtime::InitializeOptions::auth_store`]. `None` — production — lets
    /// `initialize_mcp` build the real one.
    ///
    /// A field for exactly the reason [`Self::home`] is: the backend selector is an environment
    /// switch and `std::env::set_var` is unusable in-process. See
    /// [`crate::runtime::InitializeOptions::auth_store`] for what is unprovable without it.
    auth_store: Option<crate::credentials::McpAuthStore>,
    /// The browser launcher `/mcp-auth`'s flow hands the authorization URL to, when something other
    /// than [`crate::oauth::OpenerLauncher`] is wanted. `None` means the production default.
    ///
    /// [`crate::oauth::NoopLauncher`] has existed since the flow was written, documented "for
    /// headless hosts and tests", and nothing could reach it: `AuthenticateOptions::launcher`
    /// defaults to `OpenerLauncher` and no caller ever set the field. This is the seam that was
    /// missing, not the launcher.
    browser_launcher: Option<Arc<dyn crate::oauth::BrowserLauncher>>,
}

impl McpExtension {
    /// `syncToolSurface` (MCP-036 / MCP-043) — re-resolve the surface and register what CHANGED,
    /// from a live handler.
    ///
    /// Call after anything that can change the discovered surface: `/mcp reconnect`,
    /// `mcp({connect:"x"})`, a lazy first call that connects, a `tools/list_changed` notification.
    ///
    /// A no-op returning `false` when no registrar was bound (HA-1 absent, or a test harness), so
    /// every caller can call it unconditionally and get the pre-HA-1 behaviour rather than an
    /// error.
    ///
    /// Returns whether the DIRECT-TOOL set changed — `added + updated + deactivated`, which is
    /// upstream's own `changed` (`index.ts:257`). A pass that re-registered nothing but the proxy
    /// tool or a prompt command returns `false`: neither is a direct-tool change, and neither
    /// belongs in the notice. `false` is also the answer when no registrar was bound.
    ///
    /// The FINGERPRINT DIFF is the point, not an optimisation. Re-registering a tool with identical
    /// bytes still rewrites `Agent::set_tools` and the base system prompt, which invalidates the
    /// provider's prompt-cache prefix — so upstream compares `directToolFingerprint` and registers
    /// only on a difference, and `syncProxyTool` compares the description string for the same
    /// reason. Registering unconditionally here would be correct and expensive on every reconnect.
    pub fn sync_tool_surface(&self) -> bool {
        let Some(registrar) = self.late_registrar.lock().ok().and_then(|g| g.clone()) else {
            return false;
        };

        // `settings.freezeDirectTools`: once latched, a reconnect never rebuilds the surface and
        // only `mcp({connect})` rediscovers — upstream logs exactly that advice when it fires.
        if self.direct_tools_frozen() {
            return false;
        }

        // REUSE this generation's executor. `init` stashed it and whoever owns the generation
        // installs into it; a fresh one would be an empty `OnceLock` nothing can install, so every
        // tool this pass registers would answer `MCP not initialized` forever.
        //
        // `None` means `init` has not run this generation. Refuse rather than mint: minting IS the
        // defect, and a caller that sees `false` has lost nothing — the surface is still whatever
        // `init` will register when it runs.
        //
        // Checked HERE, with the other two cheap guards, and before the config load below: it is a
        // lock and a clone, while the load re-reads `mcp.json` and `mcp-cache.json` from disk.
        let Some(dispatch) = self.dispatch() else {
            tracing::debug!("MCP: no executor for this generation yet; surface sync skipped");
            return false;
        };

        // Re-read config AND cache from disk, exactly as `init` does: a reconnect that discovered
        // new tools wrote them to `mcp-cache.json`, and that file is the whole input to
        // `resolve_direct_tools`. Reusing the captured early config would resolve the surface the
        // session started with, which is the bug this method exists to fix.
        let config = self.programmatic_config.clone().unwrap_or_else(|| {
            self.config_context().load().config
        });

        // Seed the sink with what the model is CURRENTLY shown, so the pass registers only
        // differences. These three slots are the extension's memory of the last pass.
        let mut sink = crate::registration::LateSink {
            registrar,
            known_tools: self
                .registered_direct_tools
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default(),
            known_proxy: self.proxy_tool_description.lock().ok().and_then(|g| g.clone()),
            known_commands: self
                .registered_prompt_commands
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default(),
        };
        let surface: RegisteredSurface =
            crate::registration::register_surface(&mut sink, &self.dirs, &config, dispatch);

        // `syncDirectTools`' `(previous ? updated : added)` (`index.ts:229`), computed against the
        // PREVIOUS map — still `sink.known_tools`, because `register_surface` never touches it.
        //
        // Iterating `surface.tool_names` rather than every resolved spec is the point: that list
        // already holds ONLY what registered (`register_surface` pushes at `registration.rs:2868`,
        // after the `should_register_tool` gate at `:2865`, and `LateSink::should_register_tool` IS
        // `previous != fingerprint`), so a tool whose fingerprint did not change is neither added
        // nor updated. `PROXY_TOOL_NAME` is pushed onto the same list (`registration.rs:2892`) and
        // is never in `known_tools`, so it must be excluded — otherwise every proxy-description
        // change would read as an added tool.
        let (mut added, mut updated) = (0usize, 0usize);
        for name in surface
            .tool_names
            .iter()
            .filter(|name| name.as_str() != crate::registration::PROXY_TOOL_NAME)
        {
            if sink.known_tools.contains_key(name) {
                updated += 1;
            } else {
                added += 1;
            }
            // `index.ts:223-228` — the re-activation arm, PER TOOL and gated on this tool actually
            // having been in the fallback set. A tool that comes back must leave that set AND be
            // put back into the active list, or it stays invisible for the rest of the session.
            self.reactivate_tool(name);
        }
        // `index.ts:233-237` — every previously-registered name absent from the NEW resolution.
        // `surface.direct_tool_fingerprints` records EVERY resolved spec, registered or not
        // (`registration.rs:2862-2864`), which is exactly upstream's `nextNames` (`index.ts:212`).
        // Upstream's `registeredDirectTools.delete(toolName)` half of that loop is the adoption
        // below: it replaces the map wholesale, so a name absent from the new fingerprints is gone.
        let deactivated: Vec<String> = sink
            .known_tools
            .keys()
            .filter(|name| !surface.direct_tool_fingerprints.contains_key(*name))
            .cloned()
            .collect();
        self.deactivate_tools(&deactivated);

        // ADOPT the new surface, exactly as `init` does: these slots ARE the diff's input next
        // time, so a pass that registers and forgets would re-register everything on every call.
        if let Ok(mut slot) = self.registered_direct_tools.lock() {
            *slot = surface.direct_tool_fingerprints.clone();
        }
        if let Ok(mut slot) = self.registered_prompt_commands.lock() {
            *slot = surface
                .prompt_commands
                .iter()
                .map(|spec| (spec.command_name.clone(), spec.server_name.clone()))
                .collect();
        }
        if let Ok(mut slot) = self.proxy_tool_description.lock() {
            slot.clone_from(&surface.proxy_description);
        }
        // `self.dispatch` is deliberately NOT touched. This pass reused the generation's
        // executor rather than minting one, so there is nothing new to adopt — and replacing the
        // handle would discard the installed instance, which MCP-214 calls the ONLY handle to the
        // `Arc<ToolDispatch>` that this generation's tools closed over.

        // `index.ts:257-263` — the sum of all three, and a UI notification. Prompt commands and
        // the proxy description are deliberately NOT counted: upstream's `changed` is
        // `added + updated + deactivated` out of `syncDirectTools` alone.
        let removed = deactivated.len();
        let changed = added + updated + removed;
        if changed > 0
            && let Some(services) = self.host_services()
        {
            services.notify(
                &format!("MCP: direct tools refreshed (+{added}, ~{updated}, -{removed})"),
                cyrup_ext::NotifyKind::Info,
            );
        }
        changed > 0
    }

    /// `index.ts:186-203` `deactivateTools(toolNames)` — the `setActiveTools` fallback, the ONLY
    /// branch cyrup has.
    ///
    /// `ExtensionRegistry` has no `unregisterTool`, which lands this on upstream's own
    /// `unregisterTool === undefined` branch — a supported upstream configuration, so `unregistered`
    /// is always empty and `fallbackNames` is always `toolNames`. A deactivated MCP tool stops being
    /// callable but its name stays in the registry for the session, exactly as upstream behaves
    /// against a host without `unregisterTool`.
    fn deactivate_tools(&self, names: &[String]) {
        if names.is_empty() {
            return;
        }
        let Some(services) = self.host_services() else { return };
        let remove: std::collections::HashSet<&str> = names.iter().map(String::as_str).collect();
        // `getActiveToolsIfReady()` returning undefined (`index.ts:176-184`, the "Action methods
        // cannot be called during extension loading" arm) is `active_tools() == None`, and
        // upstream treats an EMPTY active list the same way (`index.ts:193`).
        let Some(active) = services.active_tools().filter(|active| !active.is_empty()) else {
            if let Ok(mut slot) = self.fallback_deactivated_tools.lock() {
                slot.extend(names.iter().cloned());
            }
            return;
        };
        let next: Vec<String> =
            active.iter().filter(|name| !remove.contains(name.as_str())).cloned().collect();
        // `if (nextActiveTools.length !== activeTools.length)` — the fallback set is recorded ONLY
        // on this branch too (`index.ts:198-201`).
        if next.len() != active.len() {
            if let Ok(mut slot) = self.fallback_deactivated_tools.lock() {
                slot.extend(names.iter().cloned());
            }
            services.set_active_tools(&next);
        }
    }

    /// `index.ts:223-228` — a tool re-registered after having been deactivated leaves the fallback
    /// set and is appended to the active list.
    ///
    /// The `delete` returning true is the gate: a tool that was never deactivated must not cause a
    /// `setActiveTools` write, because that call rewrites the agent's tool array and the base
    /// system prompt and so invalidates the provider's prompt-cache prefix.
    fn reactivate_tool(&self, name: &str) {
        let removed = self
            .fallback_deactivated_tools
            .lock()
            .map(|mut slot| {
                let before = slot.len();
                slot.retain(|entry| entry != name);
                slot.len() != before
            })
            .unwrap_or(false);
        if !removed {
            return;
        }
        let Some(services) = self.host_services() else { return };
        let Some(active) = services.active_tools() else { return };
        if !active.iter().any(|entry| entry == name) {
            let mut next = active;
            next.push(name.to_string());
            services.set_active_tools(&next);
        }
    }

    /// Construct the adapter for an already-resolved `<agent_dir>` and session `cwd`.
    #[must_use]
    pub fn new(dirs: McpDirs) -> Self {
        Self::with_config(dirs, None)
    }

    /// As [`Self::new`], with `createMcpAdapter({config})`'s programmatic configuration.
    #[must_use]
    pub fn with_config(dirs: McpDirs, programmatic_config: Option<McpConfig>) -> Self {
        Self {
            id: ExtensionId::from(EXTENSION_ID),
            dirs,
            programmatic_config,
            host_services: Arc::new(OnceLock::new()),
            generation: AtomicU64::new(0),
            owner: Mutex::new(None),
            oauth_runtime: Mutex::new(None),
            state: Mutex::new(None),
            init_task: Mutex::new(None),
            registered_direct_tools: Mutex::new(IndexMap::new()),
            registered_prompt_commands: Mutex::new(IndexMap::new()),
            fallback_deactivated_tools: Mutex::new(Vec::new()),
            proxy_tool_description: Mutex::new(None),
            direct_tools_frozen: AtomicBool::new(false),
            dispatch: Mutex::new(None),
            late_registrar: Mutex::new(None),
            self_weak: OnceLock::new(),
            proxy_ctx: Mutex::new(None),
            render_options: Mutex::new(crate::renderers::McpToolRenderOptions::default()),
            home: None,
            auth_store: None,
            browser_launcher: None,
        }
    }

    /// Wrap into the `Arc` an extension is used as, binding the self-handle in the same step.
    ///
    /// The ONLY supported way to build an `Arc<McpExtension>`. `self_weak` cannot be bound
    /// after the fact from outside this crate — it is private — and binding it is what makes
    /// [`Self::install_surface_sync`] able to install the `onToolMetadataUpdated` listener at all.
    ///
    /// Folding it into construction is deliberate rather than convenient. `mcp_extension_for_env`
    /// and the `cyrup-it` harness each built their own `Arc`, the harness's doc comment asserted
    /// the two were identical, and once the binding was added to one of them they silently were
    /// not — the harness kept compiling and its tests kept passing, on the unbound branch. A
    /// public setter would have fixed that instance and left the next constructor free to forget.
    /// One constructor cannot diverge.
    pub fn into_arc(self) -> Arc<Self> {
        let ext = Arc::new(self);
        // Infallible: `ext` was created on the line above, so nothing else holds the `OnceLock`.
        let _ = ext.self_weak.set(Arc::downgrade(&ext));
        ext
    }

    /// Pin the home directory the config ladder's home-anchored rungs resolve against (see
    /// the `home` field). Production never calls this; a test that must be hermetic always does.
    #[must_use]
    pub fn with_home(mut self, home: PathBuf) -> Self {
        self.home = Some(home);
        self
    }

    /// Pin the credential vault every generation this extension starts authenticates through (see
    /// the `auth_store` field). Production never calls this; a test that drives an OAuth HTTP
    /// server always does, because the host keychain is neither available nor writable in CI.
    #[must_use]
    pub fn with_auth_store(mut self, store: crate::credentials::McpAuthStore) -> Self {
        self.auth_store = Some(store);
        self
    }

    /// Pin the browser launcher `/mcp-auth` hands the authorization URL to (see the
    /// `browser_launcher` field). Production never calls this and keeps
    /// [`crate::oauth::OpenerLauncher`]; a test that drives a login always does, because a real
    /// `opener::open` against a fixture's `/authorize` opens a stray tab on whoever runs the suite.
    #[must_use]
    pub fn with_browser_launcher(
        mut self,
        launcher: Arc<dyn crate::oauth::BrowserLauncher>,
    ) -> Self {
        self.browser_launcher = Some(launcher);
        self
    }

    /// The resolved directory layout this extension reads and writes.
    #[must_use]
    pub fn dirs(&self) -> &McpDirs {
        &self.dirs
    }

    /// The live capability backend, once the builder has bound it. `None` in a default host, an SDK
    /// embedding or a headless build — every consumer degrades rather than failing.
    #[must_use]
    pub fn host_services(&self) -> Option<Arc<dyn cyrup_ext::host::HostServices>> {
        self.host_services.get().cloned()
    }

    /// The current lifecycle generation. Every asynchronous continuation compares against the value
    /// it captured before its first await.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// The committed runtime, if this generation has one.
    #[must_use]
    pub fn state(&self) -> Option<Arc<McpState>> {
        self.state.lock().ok().and_then(|slot| slot.clone())
    }

    /// The config context this generation reads and writes through — `--mcp-config` from argv, the
    /// resolved [`crate::dirs::McpDirs`], and the pinned test home.
    ///
    /// Built per call rather than memoised because `config_path_from_argv` reads the process
    /// arguments and `load()` re-reads both files from disk, which is the property
    /// `install_surface_sync` depends on: a `/mcp disable` that rewrote the file must be visible to
    /// the next read without invalidating a cache.
    ///
    /// Returns the **context**, not the loaded config, because the two existing callers want
    /// `.load().config` while `/mcp disable` and `cyrup mcp init` want the context's own writers.
    #[must_use]
    pub(crate) fn config_context(&self) -> crate::config::ConfigContext {
        let explicit = crate::config::config_path_from_argv(std::env::args()).map(PathBuf::from);
        let mut ctx = crate::config::ConfigContext::new(self.dirs.clone(), explicit.as_deref());
        if let Some(home) = self.home.clone() {
            ctx = ctx.with_home(home);
        }
        ctx
    }

    /// `$HOME` as this extension resolved it, for a helper that rebuilds the config ladder from
    /// owned inputs rather than borrowing the extension (see `crate::panel_host::SetupCallbacks`).
    #[must_use]
    pub(crate) fn home(&self) -> Option<&PathBuf> {
        self.home.as_ref()
    }

    /// A `Weak` handle to this extension, for a callback object that must outlive the call that
    /// built it.
    ///
    /// `None` when the extension was not built through [`Self::into_arc`] — the in-crate unit tests
    /// hold the value directly rather than through an `Arc`, and with no self handle a callback
    /// could not call back. Every panel entry point degrades to its no-overlay branch on `None`
    /// rather than opening a half-wired panel.
    ///
    /// **Weak, never strong.** The callbacks are handed to a panel that the extension's own command
    /// arm is blocked on; a strong handle would make the extension keep itself alive for as long as
    /// any callback object survived. This is the same reason [`Self::install_surface_sync`] takes a
    /// `Weak`.
    #[must_use]
    pub(crate) fn self_handle(&self) -> Option<std::sync::Weak<McpExtension>> {
        self.self_weak.get().cloned()
    }

    /// The live generation's owner, if there is one.
    #[must_use]
    pub fn owner(&self) -> Option<Arc<McpRuntimeOwner>> {
        self.owner.lock().ok().and_then(|slot| slot.clone())
    }

    /// `initPromise` — the in-flight build, if one has not yet settled.
    ///
    /// The join point for anything that must wait on a runtime that is still coming up: clone the
    /// inner [`InitTask`] out of the `Arc` and await it under a bound (see [`Self::on_input`], which
    /// does exactly that). Joining is not the same as owning — the returned handle carries no
    /// authority to commit, and the commit tail's identity check
    /// ([`Self::init_task_is`]) is what keeps a joiner from being mistaken for the builder.
    #[must_use]
    pub fn init_task(&self) -> Option<Arc<InitTask>> {
        self.init_task.lock().ok().and_then(|slot| slot.clone())
    }

    /// `initPromise === promise` — `startInitialization`'s **third** staleness check.
    ///
    /// An `Arc::ptr_eq` and never a value comparison: [`InitTask`] is a `Shared` future, which has
    /// no equality and no stable id of its own, and the whole point of the check is identity. Two
    /// `SessionStart`s inside one generation each memoise their own build; without this the second
    /// build's commit and the first build's commit would both pass the other two checks and both
    /// write `state`.
    fn init_task_is(&self, task: &Arc<InitTask>) -> bool {
        self.init_task()
            .is_some_and(|current| Arc::ptr_eq(&current, task))
    }

    /// MCP-015's snapshot: every context-derived value `initializeMcp` can need, read
    /// **synchronously** from the dispatch ctx before this generation's first await.
    ///
    /// `cwd` deliberately comes from [`Self::dirs`] and **not** from `ctx.cwd`. The same directory
    /// reaches the extension twice — `mcp_extension_for_env` builds [`McpDirs`] from it at
    /// construction, and the session builder independently builds the `HostCtx` from its own copy —
    /// and the two are consumed by different halves of one system: `snapshot.cwd` becomes the MCP
    /// child processes' working directory, while `self.dirs` resolves the config ladder and the
    /// metadata cache. If they ever diverge, servers spawn in one directory while the config and
    /// cache that describe them are read from another. One source; if a future host makes the ctx
    /// authoritative, the fix is to rebuild [`McpDirs`] from it, not to mix the two.
    ///
    /// `initial_signal` is `None` **by construction, permanently**: [`NativeExtension::on_event`]
    /// receives no cancellation token — the dispatcher races the handler against the session token
    /// rather than handing it in — so there is no producer to read one from. `crate::abort::combine`
    /// degrades that to a clone of the owner's own token with no forwarder task, which is the
    /// documented and correct degradation.
    fn context_snapshot(&self, ctx: &HostCtx) -> crate::runtime::ContextSnapshot {
        crate::runtime::ContextSnapshot {
            // The same expression `init` and `sync_tool_surface` resolve the config from. Read
            // unconditionally: `initialize_mcp` consults it only on the arm where no programmatic
            // config replaced discovery, so gating it would change nothing but the reading.
            config_path: crate::config::config_path_from_argv(std::env::args()).map(PathBuf::from),
            cwd: self.dirs.cwd().to_path_buf(),
            has_ui: ctx.has_ui,
            mode: mode_str(ctx.mode).to_string(),
            initial_signal: None,
            services: self.host_services(),
        }
    }

    /// The frozen-at-load tool-result render options (MCP-238). A poisoned slot degrades to the
    /// compact default rather than refusing to draw a row.
    #[must_use]
    pub fn render_options(&self) -> crate::renderers::McpToolRenderOptions {
        self.render_options
            .lock()
            .map(|slot| *slot)
            .unwrap_or_default()
    }

    /// Whether `settings.freezeDirectTools` has taken effect (MCP-036).
    #[must_use]
    pub fn direct_tools_frozen(&self) -> bool {
        self.direct_tools_frozen.load(Ordering::Acquire)
    }

    /// Latch `directToolsFrozen`. Once set, a reconnect never rebuilds the system prompt and only
    /// `mcp({ connect: "server" })` rediscovers — upstream logs exactly that advice when it fires.
    pub fn freeze_direct_tools(&self) {
        self.direct_tools_frozen.store(true, Ordering::Release);
    }

    /// `registeredDirectTools` — tool name to fingerprint. Exposed as the lock itself because the
    /// diff (MCP-036) reads and writes it as one critical section: comparing a fingerprint and then
    /// re-registering under a different lock acquisition would let two syncs interleave.
    ///
    /// **The field is production-live; this accessor is not.** The sync pass reads
    /// `self.registered_direct_tools` directly (seeded into `LateSink::known_tools` by
    /// [`Self::sync_tool_surface`] at `extension.rs:259`, written back at `:314` and `:1947`), so
    /// nothing in the shipped path needs a getter. It stays `pub` because it is the only way an
    /// out-of-crate observer can see the map at all — `cyrup-it`'s MCP suite asserts on it
    /// (`crates/cyrup-it/tests/mcp/live_tool_call.rs:945`), and that crate cannot touch the field.
    #[must_use]
    pub fn registered_direct_tools(&self) -> &Mutex<IndexMap<String, String>> {
        &self.registered_direct_tools
    }

    /// `registeredPromptCommands` — the prompt-command dedup set (MCP-039), same locking rationale.
    ///
    /// **The field is production-live; this accessor is not.** The sync pass reads and writes
    /// `self.registered_prompt_commands` directly (`extension.rs:265`, `:317`, `:1950`); this getter
    /// exists for the crate's tests.
    #[must_use]
    pub fn registered_prompt_commands(&self) -> &Mutex<IndexMap<String, String>> {
        &self.registered_prompt_commands
    }

    /// `fallbackDeactivatedTools` — the tools removed through the `setActiveTools` fallback,
    /// retained so a tool that reappears after being deactivated is put back into the active set
    /// (MCP-038).
    ///
    /// **The field is production-live; this accessor is not.** The `setActiveTools` fallback reads
    /// and rewrites `self.fallback_deactivated_tools` directly (`extension.rs:366`, `:376`); this
    /// getter exists for the crate's tests.
    #[must_use]
    pub fn fallback_deactivated_tools(&self) -> &Mutex<Vec<String>> {
        &self.fallback_deactivated_tools
    }

    /// `proxyToolDescription` — the description the proxy tool was last registered with. An
    /// unchanged description must not re-register: identical system-prompt bytes are what keep the
    /// provider's prompt cache valid (MCP-043).
    ///
    /// **The field is production-live; this accessor is not.** The re-registration guard runs off
    /// `self.proxy_tool_description` directly — read into `LateSink::known_proxy` at
    /// `extension.rs:263`, written back at `:324` and `:1957` — never off the registered
    /// [`crate::proxy::McpTool`]. This getter exists for the crate's tests.
    #[must_use]
    pub fn proxy_tool_description(&self) -> &Mutex<Option<String>> {
        &self.proxy_tool_description
    }

    /// The current generation's executor slot, or `None` before the first `init`.
    ///
    /// The install point for MCP-214's dispatcher: the commit tail
    /// ([`Self::commit_initialization`]) calls `ToolDispatch::install(...)` on this once the
    /// generation's [`McpState`] has been committed, and every tool registered by the same `init`
    /// pass goes live at that instant. Cloned out rather than borrowed because a pass replaces the
    /// slot wholesale.
    ///
    /// It cannot be `crate::runtime::initialize_mcp`'s job, tempting as the position is: that
    /// function's inputs are the owner, the dirs, the snapshot and the options — it holds no handle
    /// to this extension and so has nothing to install into.
    #[must_use]
    pub fn dispatch(&self) -> Option<Arc<ToolDispatch>> {
        self.dispatch.lock().ok().and_then(|slot| slot.clone())
    }

    /// Wire `onToolMetadataUpdated` -> `syncToolSurface` (HA-1's production caller).
    ///
    /// This is `startInitialization`'s `nextState.onToolMetadataUpdated = …` — installed by the
    /// commit tail ([`Self::commit_initialization`]) after the state commits, so a hook firing
    /// mid-build cannot observe a half-installed surface.
    ///
    /// **The two guards upstream's closure has and this one does not**, named rather than left for
    /// the next reader to find: `if (state !== nextState || !owner.isActive()) return;`. Both are
    /// safe to omit *here* and only here, because [`Self::sync_tool_surface`] re-reads `mcp.json`
    /// and `mcp-cache.json` from disk and diffs against the **extension's** current fingerprint
    /// maps rather than against anything the notifying state holds. A late notification from
    /// generation N arriving during N+1 therefore re-syncs N+1's surface — correctly — instead of
    /// republishing N's. The `Weak` upgrade covers the process-teardown case the owner check would.
    ///
    /// Upstream's chain is `manager.setMetadataListChangedListener(...)` ->
    /// `onToolMetadataUpdated` -> `syncToolSurface` -> `pi.registerTool`. Every link now exists on
    /// the cyrup side; this call is the one that closes it. `execute_connect` already fires the
    /// notification (`proxy.rs:2881`, `notify_tool_metadata_updated(server, "proxy-connect")`), so
    /// once this listener is installed a mid-session connect surfaces its tools at the next turn
    /// boundary without a restart.
    ///
    /// Weak on the extension so the listener — owned by the state, which the extension owns —
    /// cannot form a cycle that keeps either alive.
    pub fn install_surface_sync(&self, state: &Arc<McpState>) {
        let Some(weak) = self.self_weak.get().cloned() else {
            // Built without `into_arc` — the in-crate unit tests hold the value directly rather
            // than as an `Arc`. Nothing to install: with no self handle the listener could not
            // call back.
            tracing::debug!("MCP: no self handle bound; surface sync listener not installed");
            return;
        };
        state.set_tool_metadata_listener(Some(Arc::new(move |server: &str, reason: &str| {
            let Some(ext) = weak.upgrade() else { return };
            if ext.sync_tool_surface() {
                tracing::debug!(server, reason, "MCP: tool surface re-synced after metadata update");
            }
        })));
    }

    /// Build this generation's [`crate::proxy::ProxyCtx`] over the one production
    /// [`crate::proxy::ProxyEnv`] and stash it where the dispatcher (MCP-214) can find it.
    ///
    /// Called from the commit tail, exactly where [`Self::install_surface_sync`] is, and for the
    /// same reason: the env holds the committed state, so it cannot exist before the commit.
    /// Wave 2's MCP-011 commit tail calls the two together.
    pub fn install_runtime_env(&self, state: &Arc<McpState>) {
        let Some(weak) = self.self_weak.get().cloned() else {
            // Built without `into_arc` — the in-crate unit tests hold the value directly. With no
            // self handle the env's `sync_tool_surface` could not call back, so install nothing
            // rather than a half-wired context.
            tracing::debug!("MCP: no self handle bound; runtime env not installed");
            return;
        };
        let env = Arc::new(crate::live::RuntimeEnv::new(
            Arc::clone(state),
            self.dirs.clone(),
            weak,
        ));
        let ctx = Arc::new(crate::proxy::ProxyCtx::new(
            Arc::clone(state),
            env as Arc<dyn crate::proxy::ProxyEnv>,
        ));
        if let Ok(mut slot) = self.proxy_ctx.lock() {
            *slot = Some(ctx);
        }
    }

    /// This generation's proxy context, for the dispatcher (MCP-214).
    #[must_use]
    pub fn proxy_ctx(&self) -> Option<Arc<crate::proxy::ProxyCtx>> {
        self.proxy_ctx.lock().ok().and_then(|slot| slot.clone())
    }


    /// `startInitialization(ctx, owner, oauthRuntime, generation, staleReason)` (`index.ts:292`) —
    /// build this generation's runtime, memoise the build, and drive its commit (MCP-011).
    ///
    /// **It spawns; it must never await the build inline, and that is structural rather than a
    /// preference.** `cyrup_ext::dispatch`'s `DEFAULT_INVOKE_BUDGET` wraps every native `on_event`
    /// in a 5-second `tokio::time::timeout` and, on expiry, **drops the handler future** and lets
    /// the action proceed; the native path additionally drops it the moment the session cancel
    /// token fires. [`crate::runtime::initialize_mcp`] connects subprocesses and performs their
    /// `initialize` / `tools/list` handshakes, which on a cold start routinely outlives that budget.
    /// An awaiting `on_session_start` would therefore have its handshakes cancelled mid-flight,
    /// intermittently and per machine, with the session continuing as if nothing happened and every
    /// MCP tool answering `MCP not initialized` for the rest of it. Upstream spawns for the same
    /// reason: `const initialization = startInitialization(...)` is not awaited on the ordinary
    /// path (`index.ts:405`).
    ///
    /// The returned handle is the memo, not the result: it is what [`Self::on_input`] and the
    /// dispatcher's init gate join so that a caller arriving mid-build waits for *this* build
    /// instead of starting a rival one.
    ///
    /// Returns `None` — having spawned nothing — when this extension was built without
    /// [`Self::into_arc`], the same degradation [`Self::install_surface_sync`] takes: with no self
    /// handle there is nothing to commit the build *into*.
    fn start_initialization(
        &self,
        owner: Arc<McpRuntimeOwner>,
        oauth_runtime: Arc<crate::state::OAuthRuntime>,
        snapshot: crate::runtime::ContextSnapshot,
        dispatch_ctx: HostCtx,
        generation: u64,
        stale_reason: &'static str,
    ) -> Option<Arc<InitTask>> {
        let Some(strong) = self.self_weak.get().and_then(std::sync::Weak::upgrade) else {
            tracing::debug!("MCP: no self handle bound; session initialization not started");
            return None;
        };

        // `owner.addCleanup(() => cleanupMaterializedBinaryResources(owner.signal))`
        // (`index.ts:293`). Registered FIRST so it runs LAST under `begin_stop`'s LIFO drain, which
        // is the order `runtime.rs`'s cleanup list documents: the graceful shutdown and the OAuth
        // teardown that `initialize_mcp` registers must both have run before the materialized
        // resource directories are removed out from under them.
        owner.add_cleanup(Box::new(|| {
            Box::pin(async {
                if let Err(error) = crate::renderers::MaterializedResources::global().cleanup() {
                    tracing::debug!("MCP: materialized-resource cleanup failed: {error}");
                }
                Ok(())
            })
        }));

        // `const promise = initializeMcp(...); initPromise = promise;` (`index.ts:294-302`).
        //
        // `oauth_runtime: Some(...)` is deliberate and is upstream's own shape: it makes
        // `owns_oauth_runtime` false, so `initialize_mcp` does NOT register the `shutdown_oauth`
        // cleanup and the three explicit teardown sites — `on_session_start`,
        // `on_session_shutdown` and the failure tail below — own it instead. That is what lets a
        // build which never commits still have its runtime shut down.
        let task: InitTask = {
            let owner = Arc::clone(&owner);
            let dirs = self.dirs.clone();
            let options = crate::runtime::InitializeOptions {
                programmatic_config: self.programmatic_config.clone(),
                oauth_runtime: Some(Arc::clone(&oauth_runtime)),
                auth_store: self.auth_store.clone(),
            };
            async move {
                crate::runtime::initialize_mcp(owner, dirs, snapshot, options)
                    .await
                    // `Shared` requires a `Clone` output and `McpError` is not; the `Arc` is what
                    // lets a second joiner see the same failure rather than a poisoned memo.
                    .map_err(Arc::new)
            }
            .boxed()
            .shared()
        };
        let handle = Arc::new(task);
        if let Ok(mut slot) = self.init_task.lock() {
            *slot = Some(Arc::clone(&handle));
        }

        // The `.then` / `.catch` chain. A `Shared` makes no progress unless something polls it, so
        // this task is not an optimisation — it is the only driver. Removing it and relying on
        // `on_input` to poll would mean the runtime starts building when the user first types.
        let driver = Arc::clone(&handle);
        tokio::spawn(async move {
            match (*driver).clone().await {
                Ok(next_state) => {
                    Self::commit_initialization(
                        strong,
                        next_state,
                        &owner,
                        &dispatch_ctx,
                        generation,
                        &driver,
                        stale_reason,
                    )
                    .await;
                }
                Err(error) => {
                    strong
                        .fail_initialization(&error, &owner, &oauth_runtime, generation, &driver)
                        .await;
                }
            }
        });

        Some(handle)
    }

    /// `startInitialization`'s `.then` tail (`index.ts:304-330`) — the commit.
    ///
    /// Everything the deliverable needs happens here: this is where the built runtime becomes *the*
    /// runtime, where the surface listener and the proxy context are installed, and where the model
    /// is shown the tools the servers reported.
    ///
    /// An associated function rather than a method because `self: &Arc<Self>` is not a stable
    /// receiver and the dispatcher install needs a `Weak` derived from the very `Arc` being
    /// committed into.
    async fn commit_initialization(
        ext: Arc<Self>,
        next_state: Arc<McpState>,
        owner: &Arc<McpRuntimeOwner>,
        dispatch_ctx: &HostCtx,
        generation: u64,
        task: &Arc<InitTask>,
        stale_reason: &'static str,
    ) {
        // 1 — the TRIPLE staleness check (`index.ts:305`), all three `||`-joined and checked before
        // any write. Each catches a different way this build can have been superseded while it ran:
        // the owner was stopped; a newer generation started; a second build inside this same
        // generation replaced the memo. On any of them the new state is **shut down instead of
        // committed** — leaking it would leave a live server manager, its children and its
        // lifecycle timers running with nothing able to reach them.
        if !owner.is_active() || ext.generation() != generation || !ext.init_task_is(task) {
            if let Err(error) = crate::lifecycle::shutdown_state(
                Some(next_state),
                stale_reason,
                crate::live::metadata_flush(ext.dirs.clone()),
            )
            .await
            {
                tracing::error!("MCP: failed to clean stale initialization state: {error}");
            }
            return;
        }

        // 2 — `state = nextState`.
        if let Ok(mut slot) = ext.state.lock() {
            *slot = Some(Arc::clone(&next_state));
        }

        // 2b — MCP-471's producer, and it has to be here rather than only in `on_event`'s tail.
        // That tail records the ctx `if let Some(state) = self.state()`, which on the FIRST
        // `SessionStart` is `None`: the build is spawned, so the handler has long returned by the
        // time the state exists. Without this line the generation's first consent dialog would run
        // with `with_human_wait(None)` and forfeit its P-3 budget forgiveness.
        next_state.set_human_wait_ctx(dispatch_ctx);

        // 2c — MCP-334: bind `hasPendingAuth` to THIS generation's OAuth runtime.
        //
        // `initialize_mcp` step 7 builds the lifecycle manager before the state exists, so it can
        // only pass a placeholder — and the placeholder answers `false` for every server, which
        // leaves `lifecycle.ts:177` and `:229`'s two "skip the reconnect while OAuth is pending"
        // gates permanently open. The 30-second keep-alive check would then connect a server whose
        // login is still in flight: the window is the one journey B sits in longest — between
        // `execute_auth_complete` closing the connection so the new token is used and the retry
        // connecting — and a tick landing there races that retry, spends another 401 and files a
        // connect failure that puts the server into the 60-second backoff.
        //
        // `None` for the base directory is upstream's own argument
        // (`new McpLifecycleManager(manager, name => hasPendingAuth(name, undefined, oauthRuntime))`,
        // `init.ts:143`): the scan is by server name, which is what the lifecycle manager knows.
        //
        // Installed HERE, from the committed state, rather than at construction, because
        // `state.oauth_runtime` is the runtime this generation actually authenticates through — a
        // predicate bound earlier could only have captured a runtime that a replacement might have
        // superseded.
        {
            let runtime = Arc::clone(&next_state.oauth_runtime);
            next_state.lifecycle.set_pending_auth_check(Arc::new(move |server: &str| {
                crate::oauth::has_pending_auth_sync(&runtime, server, None)
            }));
        }

        // 3 — the proxy context, BEFORE the dispatcher is installed. Reversed, a tool call landing
        // in the window between the two would find an installed dispatcher whose `proxy_ctx()` is
        // still `None` and answer `not_initialized` once, non-deterministically.
        ext.install_runtime_env(&next_state);

        // 4 — MCP-214's executor, into the slot every tool this generation's `init` registered
        // closed over. Without it `DirectTool::execute` and `ProxyTool::execute` both take their
        // `None` arm and answer `MCP not initialized` for the life of the generation.
        //
        // `install` is a `OnceLock::set` and a second call is a no-op, which needs no
        // special-casing precisely because [`crate::dispatch::McpDispatch`] holds a `Weak` and
        // re-reads `proxy_ctx()` at call time: an instance installed by one generation serves the
        // next one correctly. An instance that had captured this `ProxyCtx` would route the next
        // generation's calls into this generation's dead state.
        if let Some(slot) = ext.dispatch() {
            slot.install(Arc::new(crate::dispatch::McpDispatch::new(Arc::downgrade(&ext))));
        }

        // 5 — `nextState.onToolMetadataUpdated = …` (`index.ts:307-316`): the listener that turns a
        // later `tools/list_changed`, reconnect or `mcp({connect})` into a re-registered surface.
        ext.install_surface_sync(&next_state);

        // 6 — `syncPromptCommands(); syncToolSurface(ctx)` (`index.ts:316-317`). One call covers
        // both: `register_surface` registers prompt commands and tools through the same sink.
        //
        // THIS is the step that puts the connected servers' tools in front of the model. Landing
        // step 2 without it commits a runtime nobody can see.
        ext.sync_tool_surface();

        // 7 — `updateStatusBar(nextState)` (`index.ts:318`).
        crate::live::update_status_bar(&next_state);

        // 8 — `initPromise = null` (`index.ts:319`), and AFTER the sync rather than before: a
        // caller arriving during the sync window should join a settled build, not be told there is
        // no build to join.
        if let Ok(mut slot) = ext.init_task.lock() {
            *slot = None;
        }

        // 9 — `freezeDirectTools` (`index.ts:320-323`). Read from the config the runtime was
        // actually built from, which is the same document `init` registered the early surface from.
        if next_state
            .config
            .settings
            .as_ref()
            .is_some_and(crate::config::McpSettings::freeze_direct_tools)
        {
            ext.freeze_direct_tools();
            tracing::info!(
                "MCP: direct tools frozen after initial sync — reconnects won't rebuild the \
                 system prompt; use mcp({{ connect: \"server\" }}) to rediscover"
            );
        }
    }

    /// `startInitialization`'s `.catch` tail (`index.ts:331-349`) — a build that rejected.
    ///
    /// The guards are upstream's, in upstream's order, and the two differences from the commit
    /// tail's triple check are both deliberate:
    ///
    /// * it is a **two**-clause generation check, not three — a build whose memo was replaced but
    ///   whose generation is still live has still failed and still deserves the log line;
    /// * the memo check admits the `initPromise === null` case, which is how a build that committed
    ///   and then cleared the slot can still report a *later* failure.
    async fn fail_initialization(
        &self,
        error: &Arc<McpError>,
        owner: &Arc<McpRuntimeOwner>,
        oauth_runtime: &Arc<crate::state::OAuthRuntime>,
        generation: u64,
        task: &Arc<InitTask>,
    ) {
        // `if (!owner.isActive() || generation !== lifecycleGeneration) return;`
        if !owner.is_active() || self.generation() != generation {
            return;
        }
        // `if (initPromise !== promise && initPromise !== null) return;`
        if !self.init_task_is(task) && self.init_task().is_some() {
            return;
        }

        tracing::error!("MCP initialization failed: {error}");
        if let Ok(mut slot) = self.init_task.lock() {
            *slot = None;
        }

        // `if (state) return;` — a live prior state means the session is still usable and must not
        // be torn down because a *replacement* build failed.
        if self.state().is_some() {
            return;
        }

        // `await Promise.all([owner.stop(...), shutdownOAuth(oauthRuntime)])`. `begin_stop` rather
        // than a collapsed `stop().await` so the cancel is observable at call time, and the OAuth
        // runtime is torn down here because `start_initialization` handed `initialize_mcp` a
        // runtime it does not own — nothing else will remove its id from the process-global live
        // set.
        let stop = owner.begin_stop(Some("MCP initialization failed"));
        let oauth = crate::oauth::shutdown_oauth(oauth_runtime);
        let (stop, ()) = futures::future::join(stop, oauth).await;
        if let Err(error) = stop {
            tracing::error!("MCP: failed to clean rejected initialization: {error}");
        }
    }

    /// `session_start`'s generation protocol, abort-before-await, then the build (MCP-008/MCP-011).
    ///
    /// The ordering, and every step of it is load-bearing:
    ///
    /// 1. bump the generation, and snapshot the previous state / owner / OAuth runtime out of their
    ///    slots in the same breath;
    /// 2. publish the NEW owner and OAuth runtime **before** the drain await, so a call arriving
    ///    mid-drain fences against the generation that is starting rather than the one that is
    ///    ending;
    /// 3. take MCP-015's context snapshot — synchronously, before the first await;
    /// 4. cancel the previous generation synchronously and then join its drain
    ///    ([`crate::lifecycle::shutdown_previous_generation`], which is `begin_stop` +
    ///    `shutdown_state` + `shutdown_oauth`; the cancel happens at call time, not at first poll,
    ///    which is why that function is a plain `fn`);
    /// 5. re-check the generation and the owner after the join, and **return** if superseded;
    /// 6. `startInitialization`, which spawns.
    ///
    /// **Ordering note:** under cyrup's replacement tail the previous generation's
    /// `SessionShutdown` has *already* run when this fires (MCP-014), so the snapshots are normally
    /// `None`. The snapshot-and-stop arm is the defence for the paths where they are not — a
    /// `SessionStart` with no preceding shutdown, or a build that skipped the install tail.
    async fn on_session_start(&self, reason: &str, ctx: &HostCtx) {
        let my_generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;

        let previous = crate::lifecycle::PreviousGeneration {
            state: self.state.lock().ok().and_then(|mut slot| slot.take()),
            owner: self.owner.lock().ok().and_then(|mut slot| slot.take()),
            oauth: self.oauth_runtime.lock().ok().and_then(|mut slot| slot.take()),
        };
        if let Ok(mut slot) = self.init_task.lock() {
            *slot = None;
        }
        // The env holds a STRONG `Arc<McpState>`; leaving it set would keep the dead generation's
        // manager, lifecycle and child processes reachable from the dispatcher.
        if let Ok(mut slot) = self.proxy_ctx.lock() {
            *slot = None;
        }

        let owner = Arc::new(McpRuntimeOwner::new());
        let oauth_runtime = crate::oauth::create_oauth_runtime(Some(&owner.token()));
        if let Ok(mut slot) = self.owner.lock() {
            *slot = Some(Arc::clone(&owner));
        }
        if let Ok(mut slot) = self.oauth_runtime.lock() {
            *slot = Some(Arc::clone(&oauth_runtime));
        }

        // MCP-015 — everything ctx-derived is captured HERE, before the first await, and the one
        // owned clone is what crosses into the spawned build.
        let snapshot = self.context_snapshot(ctx);
        let dispatch_ctx = ctx.clone();

        crate::lifecycle::shutdown_previous_generation(
            previous,
            crate::lifecycle::SESSION_RESTART_STOP_REASON,
            crate::lifecycle::SESSION_RESTART_STATE_REASON,
            "MCP: failed to shut down previous session state",
            crate::live::metadata_flush(self.dirs.clone()),
        )
        .await;

        // The post-await re-check. A newer generation superseded this one while the previous owner
        // drained, so this continuation must not become the live runtime.
        if self.generation() != my_generation || !owner.is_active() {
            tracing::debug!(
                "MCP: session start ({reason}) for generation {my_generation} superseded before \
                 initialization"
            );
            return;
        }

        let _ = self.start_initialization(
            owner,
            oauth_runtime,
            snapshot,
            dispatch_ctx,
            my_generation,
            STALE_SESSION_START_STATE_REASON,
        );
    }

    /// `pi.on("input", …)` (`index.ts:489-511`) — converge the keep-alive servers **before** the
    /// submission becomes a turn.
    ///
    /// Upstream `48799fa` (#374, "converge stale keep-alive tool catalogs"). The 30-second health
    /// check already reconnects a dead keep-alive server and re-lists a drifted one, but a user who
    /// submits 200 ms after a remote server rotated its session gets a turn built on the *previous*
    /// catalog — the model is offered tools that no longer exist and misses ones that now do. This
    /// handler closes that window by making the submission itself await one convergence pass.
    ///
    /// Four properties are the port, and none is decoration:
    ///
    /// 1. **The owner is captured once, before the first await** (`const inputOwner = currentOwner`,
    ///    `index.ts:490`) and re-checked after (`:503`). A session replacement mid-pass must not let
    ///    a dead generation's convergence gate the live generation's turn.
    /// 2. **A build in flight is waited for, bounded** by [`crate::proxy::INIT_WAIT_TIMEOUT_MS`]
    ///    (`index.ts:493-499`) — the same gate `mcp()` dispatch uses. A timeout `return`s rather
    ///    than failing: an input must never be swallowed because MCP was slow to start.
    /// 3. **`ensureConverged` is awaited, not spawned** (`:506`). Fire-and-forget would reintroduce
    ///    the very race the commit fixes, and the single-flight slot inside
    ///    [`McpLifecycleManager::ensure_converged`][crate::lifecycle::McpLifecycleManager::ensure_converged]
    ///    is what keeps a burst of submissions from launching rival passes.
    /// 4. **Every failure is swallowed at `debug`** (`:507-511`), and an abort is not even logged.
    ///    A keep-alive server that cannot come back must degrade the turn, never block it.
    ///
    /// cyrup dispatches `Input` as an awaited emission before the turn is built
    /// (`cyrup-session-svc`'s `emit_input_event`), so awaiting here has upstream's exact effect.
    ///
    /// **One host difference, deliberately not compensated.** `cyrup_ext::dispatch`'s
    /// `DEFAULT_INVOKE_BUDGET` caps every native handler at 5 s and, on expiry, skips it and lets
    /// the action proceed; upstream's `await` is unbounded. The degradation is in the same
    /// direction as upstream's own `catch` — the turn continues against a possibly-stale catalog
    /// rather than hanging — and a pass is bounded near that anyway, because `refreshTools` carries
    /// `KEEP_ALIVE_REFRESH_TIMEOUT_MS = 5_000` per server across
    /// [`KEEP_ALIVE_CHECK_CONCURRENCY`][crate::lifecycle::KEEP_ALIVE_CHECK_CONCURRENCY] workers.
    /// Buying back the unbounded wait would mean holding the input thread past the budget, which is
    /// a worse failure than the one it fixes.
    async fn on_input(&self) {
        // `const inputOwner = currentOwner; if (!inputOwner?.isActive()) return;`
        let Some(owner) = self.owner().filter(|owner| owner.is_active()) else {
            return;
        };

        // `if (!state && initPromise) { try { await awaitWithTimeout(...) } catch { return } }`
        if self.state().is_none()
            && let Some(task) = self.init_task.lock().ok().and_then(|slot| slot.clone())
        {
            let waited = tokio::time::timeout(
                std::time::Duration::from_millis(crate::proxy::INIT_WAIT_TIMEOUT_MS),
                (*task).clone(),
            )
            .await;
            // Upstream's bare `catch { return }` covers BOTH arms: the timeout and a rejected
            // build. Neither is this handler's to report — `startInitialization` already did.
            if !matches!(waited, Ok(Ok(_))) {
                return;
            }
        }

        // `const inputState = state; if (!inputState || !inputOwner.isActive()) return;` — the
        // post-await re-read, and the owner re-check that goes with it.
        let Some(state) = self.state() else { return };
        if !owner.is_active() {
            return;
        }

        if let Err(error) = state.lifecycle.ensure_converged(owner.token()).await {
            // `if (!isAbortError(error, inputOwner.signal))` — a cancelled pass is the expected
            // shape of a session ending mid-submission, not a fault worth a line.
            if !crate::abort::is_abort_error(&error, Some(&owner.token())) {
                tracing::debug!("MCP: keep-alive convergence failed before input: {error}");
            }
        }
    }

    /// `session_shutdown` — the **only** teardown point (MCP-009, MCP-010).
    ///
    /// The ordering it reproduces: bump the generation; snapshot and null `state` / `currentOwner`
    /// / `currentOAuthRuntime` / `initPromise` (and this generation's proxy context, which holds a
    /// strong handle to the state); `owner.begin_stop("MCP extension session shutdown")`
    /// **synchronously, before** the join; then join the owner stop, `shutdown_state` — whose
    /// metadata-flush error must win over a concurrent shutdown failure — and `shutdown_oauth`
    /// together. [`crate::lifecycle::shutdown_previous_generation`] is all of that.
    ///
    /// **This is where the metadata flush the crate docs promise actually happens.** The flush is
    /// `shutdown_state`'s synchronous prefix, and reaching it requires handing that function the
    /// generation's state and a real [`crate::live::metadata_flush`] — which is why the handler
    /// routes through the joined teardown rather than stopping the owner alone.
    ///
    /// cyrup dispatches this as an **awaited** notify *before* the session cancel token fires, so
    /// the handler genuinely gets to finish — better than upstream, and it needs no compensation.
    async fn on_session_shutdown(&self, reason: &str) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let previous = crate::lifecycle::PreviousGeneration {
            state: self.state.lock().ok().and_then(|mut slot| slot.take()),
            owner: self.owner.lock().ok().and_then(|mut slot| slot.take()),
            oauth: self.oauth_runtime.lock().ok().and_then(|mut slot| slot.take()),
        };
        if let Ok(mut slot) = self.init_task.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.proxy_ctx.lock() {
            *slot = None;
        }
        tracing::debug!("MCP: session shutdown ({reason})");
        crate::lifecycle::shutdown_previous_generation(
            previous,
            crate::lifecycle::SESSION_SHUTDOWN_STOP_REASON,
            crate::lifecycle::SESSION_SHUTDOWN_STATE_REASON,
            "MCP: session shutdown cleanup failed",
            crate::live::metadata_flush(self.dirs.clone()),
        )
        .await;
    }

    // =============================================================================================
    // MCP-334 — `/mcp-auth`, the interactive first login
    //
    // `index.ts:622-670` (the command handler) over `commands.ts:245-333` (`authenticateServer`),
    // with `reconnectServer` (`commands.ts:150-215`) supplied by the already-ported
    // [`crate::proxy::execute_connect`].
    //
    // # Why this is journey B's missing link rather than a convenience
    //
    // A first connect against an OAuth HTTP server ends at `needs-auth`, and
    // `crate::runtime::initialize_mcp`'s startup pass records the byte-exact line
    // `"OAuth authentication required. Run /mcp-auth {name}."`. Until this handler existed,
    // `NativeExtension::execute_command` was left on its default arm, which answers
    // `Err(ExtError::Component("native extension has no handler for command `mcp-auth`"))` — so the
    // one instruction the runtime gives a user with no stored credential was an error message. The
    // credential store therefore had no *human* route into it at all: the only working first login
    // was the model issuing `mcp({action:"auth-start"})` and `mcp({action:"auth-complete"})` on the
    // user's behalf.
    // =============================================================================================

    /// `pi.registerCommand("mcp-auth", { handler })` (`index.ts:623-669`).
    ///
    /// The ordering is upstream's and each step is a distinct refusal:
    ///
    /// 1. capture the owner and build the **fenced** services handle
    ///    (`createOwnedUi(ctx.ui, commandOwner)`), so a session replacement mid-flow cannot paint
    ///    into the session that replaced it;
    /// 2. `if (!serverName && !commandCtx.hasUI) return;` — a bare `/mcp-auth` in a headless session
    ///    has no server to act on and no way to ask for one;
    /// 3. join the in-flight build (`await initPromise`) — a user who types `/mcp-auth linear`
    ///    during startup must not be told "MCP not initialized";
    /// 4. a bare `/mcp-auth` picks a server (see [`Self::pick_oauth_server`]);
    /// 5. [`Self::authenticate_server`];
    /// 6. on success only, reconnect so the stored token is actually used
    ///    ([`Self::reconnect_after_auth`]).
    ///
    /// # The two output channels, and why the level decides which one is used
    ///
    /// [`NativeExtension::execute_command`]'s `Ok(Some(text))` return is surfaced by the session as
    /// an **Info** notification and nothing else; it cannot carry a level. Every refusal below has
    /// a level upstream (`error` for a missing server, `warning` for a disabled one), so when a UI
    /// is attached this handler notifies at that level itself and answers `Ok(None)` — the
    /// convention `NativeExtension::execute_command`'s own documentation prescribes. With no UI the
    /// message rides the return channel instead, which is strictly more than upstream does (it
    /// drops the text when `ctx.ui` is undefined) and cannot double-print, because the two arms are
    /// exclusive.
    async fn on_mcp_auth_command(
        &self,
        args: &str,
        ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        // 1 — `const commandOwner = currentOwner;` and `createOwnedUi(ctx.ui, commandOwner)`.
        // Captured ONCE, before the first await, for the reason `on_input` documents: a handler
        // that re-reads the slot after an await fences against the generation that replaced it.
        let owner = self.owner();
        let ui = self.command_services(ctx, owner.as_ref());

        let server_name = args.trim();
        // 2 — `if (!serverName && !commandCtx.hasUI) return;`
        if server_name.is_empty() && ui.is_none() {
            return Ok(None);
        }

        // 3 — `if (!state && initPromise) { … } if (!state) { notify("MCP not initialized") }`.
        let state = match self.await_committed_state().await {
            Ok(state) => state,
            Err(message) => return surface(ui.as_ref(), message, cyrup_ext::NotifyKind::Error),
        };
        // `commandOwner?.throwIfInactive()` after the await — a build that finished under a
        // superseded generation must not be acted on.
        if owner.as_ref().is_some_and(|owner| !owner.is_active()) {
            return Ok(None);
        }

        // 4 — the bare form, which RETURNS rather than falling through to step 5. The panel it
        // opens has already authenticated and reconnected through its own callbacks; steps 5 and 6
        // below are the named form's, and running them after the panel would start a second flow
        // against a server that just finished one.
        if server_name.is_empty() {
            return match self.pick_oauth_server(&state, ctx, ui.as_ref()).await {
                Picked::Refused(outcome) => surface(ui.as_ref(), outcome.message, outcome.kind),
                Picked::Handled => Ok(None),
            };
        }
        let target = server_name.to_string();

        // 5 — `authenticateServer(serverName, state.config, commandCtx, commandCtx.signal, …)`.
        let authenticated = self.authenticate_server(&state, &target, ctx).await;
        if !authenticated.ok {
            return surface(ui.as_ref(), authenticated.message, authenticated.kind);
        }

        // 6 — `if (result.ok) { commandOwner?.throwIfInactive(); await reconnectServer(...) }`.
        //
        // The success line is surfaced BEFORE the reconnect, as upstream surfaces it (it notifies
        // inside `authenticateServer`, `commands.ts:317-319`). The ordering is worth keeping: the
        // reconnect re-runs the handshake and `tools/list`, so a user shown nothing until that
        // finishes cannot tell a slow server from a wedged command.
        if let Some(services) = ui.as_ref() {
            notify(services, &authenticated.message, authenticated.kind);
        }
        if owner.as_ref().is_some_and(|owner| !owner.is_active()) {
            // The credential IS stored; only the retry is skipped, and by the session replacement
            // that will connect the server on its own startup pass.
            return Ok(ui.is_none().then_some(authenticated.message));
        }
        let reconnected = self.reconnect_after_auth(&target).await;
        match ui.as_ref() {
            Some(services) => {
                notify(services, &reconnected.message, reconnected.kind);
                Ok(None)
            }
            // No UI: both lines ride the one return channel rather than being dropped.
            None => Ok(Some(join_lines(&authenticated.message, &reconnected.message))),
        }
    }

    /// `createOwnedUi(ctx.ui, commandOwner)` (`index.ts:629-631`) — the command's own fenced
    /// services handle.
    ///
    /// `None` is upstream's `commandHasUI ? … : undefined`: with no renderer attached there is
    /// nothing to notify through, and every caller then routes its text to the command's return
    /// channel instead.
    ///
    /// This deliberately does NOT read [`McpState::ui`]: that handle is built by
    /// `crate::runtime::initialize_mcp` and so does not exist before the build commits, while this
    /// handler must be able to report `MCP initialization failed` — a message that exists precisely
    /// because there is no state. With no owner (a session that has not started a generation) the
    /// raw backend is used unfenced, which is the same degradation `McpState::dialog` takes.
    pub(crate) fn command_services(
        &self,
        ctx: &HostCtx,
        owner: Option<&Arc<McpRuntimeOwner>>,
    ) -> Option<Arc<dyn cyrup_ext::host::HostServices>> {
        if !ctx.has_ui {
            return None;
        }
        let services = self.host_services()?;
        match owner {
            Some(owner) => Some(Arc::new(crate::owner::OwnedServices::new(
                services,
                Arc::clone(owner),
            ))),
            None => Some(services),
        }
    }

    /// `if (!state && initPromise) { state = await initPromise } if (!state) …` (`index.ts:640-655`).
    ///
    /// Awaited **unbounded**, unlike [`Self::on_input`]'s bounded join, and the asymmetry is the
    /// port: `cyrup_ext::dispatch`'s invocation budget wraps `on_event` only —
    /// `cyrup_ext::facade::execute_native_command` races the handler against the session cancel
    /// token and nothing else — so a command may legitimately outlive a cold start. It must: an
    /// OAuth round trip waits on a human in a browser, which is orders of magnitude longer than any
    /// budget this crate could pick.
    pub(crate) async fn await_committed_state(&self) -> Result<Arc<McpState>, String> {
        if let Some(state) = self.state() {
            return Ok(state);
        }
        let Some(task) = self.init_task() else {
            return Err("MCP not initialized".to_string());
        };
        match (*task).clone().await {
            Ok(state) => Ok(state),
            Err(error) => Err(format!("MCP initialization failed: {error}")),
        }
    }

    /// `openMcpAuthPanel(state, pi, commandCtx, earlyConfigPath)` (`commands.ts:605-652`) — the
    /// bare `/mcp-auth`'s server choice.
    ///
    /// The three refusals are upstream's, byte for byte and in upstream's order: the programmatic
    /// config hint (`index.ts:659`), the empty-candidate warning (`commands.ts:627`), and the
    /// no-terminal-overlay notice ([`crate::ui::auth_panel_unavailable_message`],
    /// `commands.ts:613`).
    ///
    /// # The `canRenderPanel` probe is made twice, on purpose
    ///
    /// Upstream gates on `hasUI && mode === "tui"` and then calls `ctx.ui.custom`. Here the mode
    /// test and the host's own capability answer are separate facts: [`ExtMode`](cyrup_ext::native::ExtMode) says what kind of
    /// session this is, and `HostServices::open_overlay` says whether anything took the overlay.
    /// A host can report [`ExtMode::Tui`](cyrup_ext::native::ExtMode::Tui) and still decline — nothing guarantees a renderer is
    /// attached — so both are checked and both raise the same refusal. Checking only the second
    /// would work but would open, and immediately tear down, a panel in a mode upstream never
    /// offers it in.
    ///
    /// # There is deliberately no `!has_ui` guard here
    ///
    /// The headless case never reaches this function: [`Self::command_services`] answers `None` when
    /// `!ctx.has_ui`, and the only caller bails on `server_name.is_empty() && ui.is_none()` before
    /// the pick. So a guard here would be unreachable — it was added once, on the belief that a
    /// headless `/mcp-auth` was emitting a notice upstream swallows, and removed again when the
    /// call path showed the caller had always returned silently.
    ///
    /// # The panel authenticates; this function does not hand a name back
    ///
    /// Under [`crate::ui::PanelOptions::auth_only`] the panel runs the flow itself through
    /// [`crate::ui::McpPanelCallbacks::authenticate`] — the same `authenticate_server` the named
    /// form calls. So both the authenticated and the dismissed outcome map to
    /// [`Picked::Handled`]: by the time the overlay closes the work is done, and handing a picked
    /// server back (a variant this enum deliberately lacks) would make the caller run a second
    /// flow against a server that just finished one. `openMcpAuthPanel` returns `{ configChanged: false }` unconditionally for the
    /// same reason.
    async fn pick_oauth_server(
        &self,
        state: &Arc<McpState>,
        ctx: &HostCtx,
        ui: Option<&Arc<dyn cyrup_ext::host::HostServices>>,
    ) -> Picked {
        if self.programmatic_config.is_some() {
            return Picked::Refused(AuthCommandOutcome::failed(
                PROGRAMMATIC_CONFIG_AUTH_HINT.to_string(),
                cyrup_ext::NotifyKind::Info,
            ));
        }
        // `Object.entries(config.mcpServers).filter(([, d]) => !isServerDisabled(d) && supportsOAuth(d))`
        let candidates: Vec<String> = state
            .config
            .mcp_servers
            .iter()
            .filter(|(_, definition)| {
                !definition.is_disabled() && crate::oauth::supports_oauth(definition)
            })
            .map(|(name, _)| name.clone())
            .collect();
        if candidates.is_empty() {
            return Picked::Refused(AuthCommandOutcome::failed(
                NO_OAUTH_CAPABLE_SERVERS.to_string(),
                cyrup_ext::NotifyKind::Warning,
            ));
        }
        // `!canRenderPanel(ctx)` — `hasUI && mode === "tui"`. A UI that exists but cannot paint a
        // terminal overlay gets the refusal that names the form needing none.
        let refused = || {
            Picked::Refused(AuthCommandOutcome::failed(
                crate::ui::auth_panel_unavailable_message(mode_str(ctx.mode)),
                cyrup_ext::NotifyKind::Info,
            ))
        };
        if ctx.mode != cyrup_ext::native::ExtMode::Tui {
            return refused();
        }
        // The caller's handle, NOT a fresh `command_services(ctx, self.owner())`. Rebuilding it
        // here would re-read the owner slot *after* the `await_committed_state` the caller already
        // went through, fencing the panel against whatever generation is current now instead of
        // the one this command started under — the bug `on_input` documents and the reason
        // `on_mcp_auth_command` captures `ui` before its first await.
        let (Some(ui), Some(weak)) = (ui, self.self_handle()) else {
            return refused();
        };

        let mut diagnostics = Vec::new();
        let provenance = self.config_context().server_provenance(&mut diagnostics);
        let cache = crate::dirs::load_metadata_cache(&self.dirs.metadata_cache());
        let callbacks: Arc<dyn crate::ui::McpPanelCallbacks> =
            Arc::new(crate::panel_host::PanelCallbacks::new(
                weak,
                Arc::clone(state),
                ctx.clone(),
                self.dirs.clone(),
            ));
        let model = crate::ui::McpPanelModel::new(
            &state.config,
            cache,
            &provenance,
            Arc::clone(&callbacks),
            crate::ui::PanelOptions {
                // The panel's OWN notice, which names the keystrokes the panel actually has. The
                // select dialog's question does not carry over — it promised no keybindings.
                notice_lines: vec![crate::ui::AUTH_PANEL_NOTICE.to_string()],
                auth_only: true,
                keys: crate::ui::PanelKeys::from_agent_dir(self.dirs.agent_dir()),
                server_hash: None,
            },
        );
        if crate::ui::open_mcp_panel(
            ui.as_ref(),
            model,
            callbacks,
            tokio::runtime::Handle::current(),
        )
        .is_none()
        {
            // No host took the overlay. Same situation as the mode guard above, same refusal.
            return refused();
        }
        // Authenticated or dismissed, the panel has already done whatever was going to happen.
        Picked::Handled
    }

    /// `authenticateServer(serverName, config, ctx, signal, runtime)` (`commands.ts:245-333`).
    ///
    /// Every guard, message and level below is upstream's, in upstream's order. The one addition is
    /// the abort arm: `commands.ts:328` rethrows an abort out of the `catch`, which here would only
    /// become a `command:mcp-auth: …` error notice for a cancellation the user asked for, so it
    /// answers a silent outcome instead.
    ///
    /// # What makes the URL reach the user
    ///
    /// `options.on_authorization_url` (see [`Self::authorization_url_hook`]). Without it
    /// `crate::oauth::authenticate` falls back to `tracing::info!`, which in a TUI session is
    /// invisible — so the command would open a browser (or fail to) and then sit there with nothing
    /// on screen. That fallback is right for the model-facing
    /// `mcp({action:"auth-start"})` route, which returns the URL in the tool result; it is wrong
    /// for a human at a prompt, and installing the hook is the difference.
    ///
    /// # What is deliberately NOT installed: `on_authorization_input`
    ///
    /// Upstream also passes `onAuthorizationInput`, a confirm-then-input prompt that lets a user on
    /// a remote machine paste the callback URL back. **One** thing is missing for it here, and it is
    /// outside this unit: `cyrup_ext::HostServices::input` is a blocking round trip whose only
    /// cancellation lever is `DialogOptions::signal_id`, a guest-side registry id a native caller
    /// cannot mint — and [`crate::owner::McpDialog::input`] passes
    /// `DialogOptions::default()`, so the token is never set. The hook's
    /// contract is that its [`cyrup_core::CancelToken`] dismisses the prompt the moment the loopback
    /// callback wins the race, and a prompt that cannot be dismissed would be left on screen after
    /// the flow had already completed. Installing it therefore needs
    /// [`crate::owner::McpDialog::input`] to take a `CancelToken` first.
    ///
    /// The second blocker this note used to carry — that `McpDialog` had no `input` verb at all — is
    /// **gone**: MCP-471's dialog arms added it. Only the cancellation lever remains.
    ///
    /// Leaving the hook `None` is the documented behaviour — the
    /// flow simply awaits the loopback callback — and the notice this handler prints names the
    /// reachable manual route (`mcp({action:"auth-complete"})`, `crate::proxy::execute_auth_complete`)
    /// for the remote case.
    pub(crate) async fn authenticate_server(
        &self,
        state: &Arc<McpState>,
        server_name: &str,
        ctx: &HostCtx,
    ) -> AuthCommandOutcome {
        // `const ui = ctx.hasUI ? ctx.ui : undefined; if (!ui) return { ok: false, … }`. The state's
        // handle is the fenced one the flow will notify through, so its absence is the same refusal.
        let Some(ui) = ctx.has_ui.then(|| state.ui.clone()).flatten() else {
            return AuthCommandOutcome::failed(
                AUTH_REQUIRES_INTERACTIVE.to_string(),
                cyrup_ext::NotifyKind::Error,
            );
        };
        let Some(definition) = state.config.mcp_servers.get(server_name).cloned() else {
            return AuthCommandOutcome::failed(
                format!("Server \"{server_name}\" not found in config"),
                cyrup_ext::NotifyKind::Error,
            );
        };
        if definition.is_disabled() {
            return AuthCommandOutcome::failed(
                format!(
                    "Server \"{server_name}\" is disabled. Run /mcp enable {server_name}, then /reload."
                ),
                cyrup_ext::NotifyKind::Warning,
            );
        }
        if !crate::oauth::supports_oauth(&definition) {
            // CYRUP-DELTA: upstream keeps TWO strings here — `msg_not_oauth` returns one line and
            // the notify shows two (`commands.ts:268-274`). cyrup carries the two-line form for
            // both. The returned message reaches the caller only to be notified or surfaced in an
            // outcome, so the one-line variant had no reader; a second literal that nothing
            // distinguishes is the parallel-vocabulary defect `#91` swept. Recorded rather than
            // silently collapsed: if a surface ever needs the short form, this is where it splits.
            return AuthCommandOutcome::failed(
                format!(
                    "Server \"{server_name}\" does not use OAuth authentication.\nSet \"auth\": \"oauth\" or omit auth for auto-detection."
                ),
                cyrup_ext::NotifyKind::Error,
            );
        }

        // `resolveServerUrl(definition)` is INSIDE upstream's `try`, so a missing `${VAR}` lands in
        // the catch as `Failed to authenticate …` rather than as the no-URL message.
        let server_url = match crate::credentials::resolve_server_url(
            definition.url.as_deref(),
            &crate::credentials::process_env(),
        ) {
            Err(error) => {
                return AuthCommandOutcome::failed(
                    failed_to_authenticate(server_name, &error.to_string()),
                    cyrup_ext::NotifyKind::Error,
                );
            }
            Ok(url) => url.filter(|url| !url.is_empty()),
        };
        let Some(server_url) = server_url else {
            return AuthCommandOutcome::failed(
                format!(
                    "Server \"{server_name}\" has no URL configured (OAuth requires HTTP transport)"
                ),
                cyrup_ext::NotifyKind::Error,
            );
        };

        // `signal ??= ctx.signal` — the owner's token is the command's abort, and it is what
        // `shutdown_oauth` cancels on a session replacement.
        let cancel = self
            .owner()
            .map_or_else(cyrup_core::CancelToken::new, |owner| owner.token());

        // `ui.setStatus("mcp-auth", `Authenticating ${serverName}...`)`.
        cyrup_ext::host::HostServices::set_status(
            &*ui,
            AUTH_STATUS_KEY,
            Some(&format!("Authenticating {server_name}...")),
        );

        let mut options = state.auth_options(&self.dirs, &cancel);
        options.on_authorization_url =
            Some(Self::authorization_url_hook(server_name, Arc::clone(&ui)));
        // Production leaves this alone and keeps `OpenerLauncher`. The URL is surfaced by the hook
        // above BEFORE the handoff either way, so a launcher that does nothing loses nothing.
        if let Some(launcher) = &self.browser_launcher {
            options.launcher = Arc::clone(launcher);
        }
        let result =
            crate::oauth::authenticate(server_name, &server_url, Some(&definition), &options).await;

        // `finally { if (!signal?.aborted) ui.setStatus("mcp-auth", undefined); }` — an aborted
        // command leaves the status alone because the generation that owns the status bar is gone.
        if !cancel.is_cancelled() {
            cyrup_ext::host::HostServices::set_status(&*ui, AUTH_STATUS_KEY, None);
        }

        match result {
            Ok(crate::oauth::AuthStatus::Authenticated) => AuthCommandOutcome::ok(format!(
                "OAuth authentication successful for \"{server_name}\"."
            )),
            Ok(_) => AuthCommandOutcome::failed(
                format!("OAuth authentication failed for \"{server_name}\"."),
                cyrup_ext::NotifyKind::Error,
            ),
            Err(error) if crate::abort::is_abort_error(&error, Some(&cancel)) => {
                AuthCommandOutcome::silent()
            }
            Err(error) => AuthCommandOutcome::failed(
                failed_to_authenticate(server_name, &error.to_string()),
                cyrup_ext::NotifyKind::Error,
            ),
        }
    }

    /// `options.onAuthorizationUrl` (`commands.ts:290-297`) — surface the authorization URL
    /// **before** the browser is opened.
    ///
    /// The flow calls this and only then hands the URL to the launcher
    /// (`crate::oauth::authenticate_inner`, "always surface the URL first"), so a user whose
    /// machine has no browser — or whose browser cannot reach the loopback listener — is never
    /// stranded looking at a blank screen.
    ///
    /// The handle is [`McpState::ui`], the generation's **fenced** services handle: once the owner
    /// stops, `notify` is inert, so a URL produced by a superseded generation's flow cannot paint
    /// into the session that replaced it.
    ///
    /// Nothing secret crosses here. An authorization URL is designed to be read aloud — it carries
    /// the client id, the PKCE **challenge** (never the verifier) and the CSRF state, and the code
    /// it eventually returns arrives on the loopback listener, not through this text.
    fn authorization_url_hook(
        server_name: &str,
        ui: Arc<crate::owner::OwnedServices>,
    ) -> crate::oauth::AuthorizationUrlHook {
        let server_name = server_name.to_string();
        Arc::new(move |authorization_url: String| {
            let server_name = server_name.clone();
            let ui = Arc::clone(&ui);
            Box::pin(async move {
                cyrup_ext::host::HostServices::notify(
                    &*ui,
                    &authorization_url_notice(&server_name, &authorization_url),
                    cyrup_ext::NotifyKind::Info,
                );
                Ok(())
            })
        })
    }

    /// `reconnectServer(state, ctx, name)` (`commands.ts:150-215`) — close, connect, and commit the
    /// metadata, so the credential the flow just stored is the one the connection uses.
    ///
    /// **Delegated, not re-derived.** [`crate::proxy::execute_connect`] is that function's ported
    /// body: it closes a live connection, connects, and runs the eight-step metadata commit in
    /// upstream's order — store metadata, prompts iff discovery succeeded, instructions
    /// set-or-**delete**, cache write, `notifyToolMetadataUpdated`, keep-alive mark, clear failure,
    /// status bar. That notification is what drives `install_surface_sync` ->
    /// [`Self::sync_tool_surface`], so the newly authorized server's tools reach the model without
    /// a restart — the last step of journey B. A second hand-written reconnect here would fork that
    /// commit order.
    ///
    /// A `needs-auth` result after a successful authentication is not a contradiction and is
    /// reported as upstream reports it (`commands.ts:178`): the token was stored but the server
    /// still refused it, which is the one outcome that must not be silently rendered as success.
    async fn reconnect_after_auth(&self, server_name: &str) -> AuthCommandOutcome {
        let Some(ctx) = self.proxy_ctx() else {
            return AuthCommandOutcome::failed(
                format!(
                    "OAuth credentials were stored for \"{server_name}\", but MCP is not initialized; the connection was not retried."
                ),
                cyrup_ext::NotifyKind::Warning,
            );
        };
        self.reconnect_one(&ctx, server_name).await
    }

    /// One connect-and-report step — `reconnectServer`'s try/catch (`commands.ts:169-221`) without
    /// its caller-specific "not initialized" refusal.
    ///
    /// Extracted from [`Self::reconnect_after_auth`] so `/mcp reconnect`
    /// ([`McpExtension::arm_reconnect`](crate::McpExtension)) reuses it instead of standing up a
    /// second copy of the eight-step commit. The two callers differ only in the message they give
    /// when there is no [`crate::proxy::ProxyCtx`] — the auth path names the credential it just
    /// stored, the command path does not — so that guard stays with each caller and this function
    /// takes the context already in hand.
    pub(crate) async fn reconnect_one(
        &self,
        ctx: &Arc<crate::proxy::ProxyCtx>,
        server_name: &str,
    ) -> AuthCommandOutcome {
        let cancel = self
            .owner()
            .map_or_else(cyrup_core::CancelToken::new, |owner| owner.token());
        let connected = crate::proxy::execute_connect(ctx, server_name, &cancel).await;
        let result = match connected {
            Ok(result) => result,
            Err(error) => {
                return AuthCommandOutcome::failed(
                    format!("Failed to connect to \"{server_name}\": {error}"),
                    cyrup_ext::NotifyKind::Error,
                );
            }
        };
        match ctx.env.get_connection(server_name) {
            Some(crate::proxy::ConnectionStatus::Connected) => {
                let tools = ctx.state.tool_metadata.lock().map_or(0, |metadata| {
                    metadata.get(server_name).map_or(0, Vec::len)
                });
                let resources = ctx.state.resource_counts.lock().map_or(0, |counts| {
                    counts.get(server_name).copied().unwrap_or(0)
                });
                AuthCommandOutcome::ok(format!(
                    "MCP: Reconnected to {server_name} ({tools} tools, {resources} resources)"
                ))
            }
            Some(crate::proxy::ConnectionStatus::NeedsAuth) => AuthCommandOutcome::failed(
                format!("MCP: {server_name} requires OAuth. Run /mcp-auth {server_name} first."),
                cyrup_ext::NotifyKind::Warning,
            ),
            _ => AuthCommandOutcome::failed(
                connect_failure_message(&result)
                    .unwrap_or_else(|| format!("Failed to connect to \"{server_name}\".")),
                cyrup_ext::NotifyKind::Warning,
            ),
        }
    }
}

// =================================================================================================
// MCP-334 — the `/mcp-auth` handler's vocabulary
// =================================================================================================

/// `ui.setStatus("mcp-auth", …)`'s key (`commands.ts:286`, `:332`). The key is what pairs the
/// `Some(text)` that raises the status line with the `None` that clears it.
const AUTH_STATUS_KEY: &str = "mcp-auth";

/// `commands.ts:254` — `/mcp-auth` in a session with no renderer.
///
/// The authorization-code grant needs a browser and a human; `settings.autoAuth` plus a
/// `client_credentials` grant is the headless route, and it runs from the tool layer
/// (`crate::proxy::attempt_auto_auth`), not from a slash command.
const AUTH_REQUIRES_INTERACTIVE: &str = "OAuth authentication requires an interactive session.";

/// `index.ts:659` — a bare `/mcp-auth` against a programmatic (in-memory SDK) config.
///
/// The picker resolves servers from the on-disk configuration; a caller who supplied the config
/// object has no such document, so the only workable form is the one that names its target.
const PROGRAMMATIC_CONFIG_AUTH_HINT: &str =
    "Use /mcp-auth <server> to authenticate a server from the in-memory SDK config.";

/// `commands.ts:627` — the bare `/mcp-auth` with nothing to offer.
const NO_OAUTH_CAPABLE_SERVERS: &str = "No OAuth-capable MCP servers are configured.";

/// One `/mcp-auth` step's result — upstream's `McpAuthResult` (`commands.ts:243`) plus the
/// notification level, which upstream carries implicitly by calling `ui.notify` at the site.
///
/// The level cannot be recovered from `ok` alone: a disabled server and a missing one are both
/// failures but upstream reports the first at `warning` and the second at `error`, and a handler
/// that flattened them would either shout about a routine misconfiguration or whisper a real fault.
pub(crate) struct AuthCommandOutcome {
    /// `result.ok` — whether the step succeeded. The `/mcp-auth` handler reconnects on `true` only.
    pub(crate) ok: bool,
    /// The user-facing text. **Empty means say nothing** — the abort arm, where the user cancelled
    /// and a notice would be noise.
    pub(crate) message: String,
    /// The level [`notify`] uses when a UI is attached.
    pub(crate) kind: cyrup_ext::NotifyKind,
}

impl AuthCommandOutcome {
    /// A success, reported at `info`.
    fn ok(message: String) -> Self {
        Self { ok: true, message, kind: cyrup_ext::NotifyKind::Info }
    }

    /// A failure at the level upstream reports it.
    fn failed(message: String, kind: cyrup_ext::NotifyKind) -> Self {
        Self { ok: false, message, kind }
    }

    /// A failure with nothing to say — the user's own cancellation.
    fn silent() -> Self {
        Self { ok: false, message: String::new(), kind: cyrup_ext::NotifyKind::Info }
    }
}

/// What the bare `/mcp-auth` resolved to.
///
/// **There is no `Server(String)` variant, and its absence is the point.** The bare form opens the
/// `auth_only` panel, which runs the OAuth flow *and* the post-auth reconnect itself through
/// [`crate::ui::McpPanelCallbacks`] — the same `authenticate_server` and `reconnect_one` the named
/// form calls, reached from inside the overlay. So the bare form never hands a server name back for
/// the caller to act on: by the time the overlay closes there is nothing left to do.
/// `openMcpAuthPanel` likewise returns `{ configChanged: false }` and no name.
enum Picked {
    /// One of `openMcpAuthPanel`'s three early returns, already phrased.
    Refused(AuthCommandOutcome),
    /// The panel ran. Whether the user authenticated or dismissed it, the work is finished and the
    /// panel has already said whatever there was to say on screen.
    Handled,
}

/// `commands.ts:325` — the `catch` arm's wording, with the message sanitized.
///
/// The interpolated text can come from a remote authorization server (an error body, a metadata
/// document) and lands in a terminal, so it goes through [`crate::ui::sanitize_terminal_text`]
/// exactly as `logoutServer` sanitizes its own (`commands.ts:355`). An escape sequence smuggled
/// through an OAuth error would otherwise repaint the user's screen.
fn failed_to_authenticate(server_name: &str, message: &str) -> String {
    format!(
        "Failed to authenticate \"{server_name}\": {}",
        crate::ui::sanitize_terminal_text(message)
    )
}
/// `terminalHyperlink(label, url)` (`commands.ts:27-29`) — OSC 8 with both halves sanitized FIRST.
///
/// The order is load-bearing and cannot be reversed: [`crate::ui::sanitize_terminal_text`] opens
/// with `strip_osc_sequences`, so sanitizing the finished sequence would erase the link entirely.
/// Sanitize the parts, then build the escape.
fn terminal_hyperlink(label: &str, url: &str) -> String {
    format!(
        "\u{1b}]8;;{}\u{1b}\\{}\u{1b}]8;;\u{1b}\\",
        crate::ui::sanitize_terminal_text(url),
        crate::ui::sanitize_terminal_text(label),
    )
}

/// `options.onAuthorizationUrl`'s notice (`commands.ts:291-296`).
///
/// # The one adapted sentence, and why
///
/// Upstream's second paragraph ends *"copy the full localhost URL from the browser address bar and
/// paste it into Pi"*, which is an instruction about upstream's `onAuthorizationInput` prompt.
/// `McpExtension::authenticate_server` documents why that prompt is not installed here, so
/// promising it would send a remote user looking for a dialog that never opens. The sentence
/// instead names the route that **is** wired end to end — `mcp({action:"auth-complete"})`, whose
/// own instructions (`crate::proxy::format_manual_auth_instructions`) use this same call shape —
/// so the two texts tell a user the same thing.
///
/// The URL is sanitized for the same reason [`failed_to_authenticate`]'s message is: it is built
/// from a remote authorization server's metadata and is about to be printed to a terminal. That
/// sanitization happens INSIDE [`terminal_hyperlink`] — see its doc for why the order cannot be
/// reversed.
///
/// Upstream wraps the URL as `terminalHyperlink(authorizationUrl, authorizationUrl)`
/// (`commands.ts:292`) — label and target both the URL — so a terminal that understands OSC 8 makes
/// it clickable and one that does not renders the label, which is the same URL. The plain-text
/// reading is identical either way.
fn authorization_url_notice(server_name: &str, authorization_url: &str) -> String {
    let url = terminal_hyperlink(authorization_url, authorization_url);
    format!(
        "Open this URL to authenticate {server_name}:\n\n{url}\n\nAfter approving, authentication \
         completes automatically if the browser can reach its localhost callback. On a remote \
         machine, copy the full redirected localhost URL from the address bar and send it back \
         with:\nmcp({{ action: \"auth-complete\", server: \"{server_name}\", args: {{ redirectUrl: \
         \"PASTE_REDIRECT_URL_HERE\" }} }})"
    )
}

/// The `message` [`crate::proxy::execute_connect`] puts in `details` on every failure arm
/// (`proxy/auth.rs`'s `auth_required`, `aborted` and `connect_failed` branches all insert it).
///
/// Read from `details` rather than from `content` because the details map is the structured half —
/// the text block is a rendered sentence that already wraps the same string.
fn connect_failure_message(result: &cyrup_core::ToolResult) -> Option<String> {
    result
        .details
        .as_ref()?
        .get("message")?
        .as_str()
        .map(str::to_string)
}

/// Notify through a services handle at `kind`, skipping an empty message.
///
/// The skip is [`AuthCommandOutcome::silent`]'s contract: a blank notification is a visible empty
/// row in the transcript, which is worse than the silence the abort arm asked for.
fn notify(services: &Arc<dyn cyrup_ext::host::HostServices>, message: &str, kind: cyrup_ext::NotifyKind) {
    if message.is_empty() {
        return;
    }
    services.notify(message, kind);
}

/// Deliver one `/mcp-auth` message on whichever channel the session has.
///
/// With a services handle the message is notified **at its own level** and the command answers
/// `Ok(None)`; with none it rides `execute_command`'s return channel, which the session surfaces as
/// `Info`. The two are exclusive, so nothing is ever printed twice. See
/// [`McpExtension::on_mcp_auth_command`] for why the level is what decides.
fn surface(
    services: Option<&Arc<dyn cyrup_ext::host::HostServices>>,
    message: String,
    kind: cyrup_ext::NotifyKind,
) -> Result<Option<String>, ExtError> {
    match services {
        Some(services) => {
            notify(services, &message, kind);
            Ok(None)
        }
        None if message.is_empty() => Ok(None),
        None => Ok(Some(message)),
    }
}

/// Join two `/mcp-auth` lines for the no-UI return channel, dropping an empty half.
fn join_lines(first: &str, second: &str) -> String {
    match (first.is_empty(), second.is_empty()) {
        (true, _) => second.to_string(),
        (_, true) => first.to_string(),
        _ => format!("{first}\n{second}"),
    }
}

/// [`cyrup_ext::native::ExtMode`] as `ctx.mode`'s string, for
/// [`crate::runtime::ContextSnapshot::mode`].
///
/// Only `"tui"` is load-bearing: [`crate::runtime::ContextSnapshot::is_tui_mode`] is
/// `has_ui && mode == "tui"`, and that is what gates URL elicitation. The other three exist so a
/// diagnostic reading the snapshot sees the mode the session is actually in.
pub(crate) const fn mode_str(mode: cyrup_ext::native::ExtMode) -> &'static str {
    match mode {
        cyrup_ext::native::ExtMode::Tui => "tui",
        cyrup_ext::native::ExtMode::Rpc => "rpc",
        cyrup_ext::native::ExtMode::Json => "json",
        cyrup_ext::native::ExtMode::Print => "print",
    }
}

impl std::fmt::Debug for McpExtension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpExtension")
            .field("id", &self.id)
            .field("generation", &self.generation())
            .field("agent_dir", &self.dirs.agent_dir())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl NativeExtension for McpExtension {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    /// **Ambient — `true`, and it is load-bearing.**
    ///
    /// `pi-mcp-adapter` is an *installed package* upstream, living in the PATH tier that
    /// `noExtensions` collapses (`resource-loader.ts:451-452`: `const extensionPaths =
    /// this.noExtensions ? cliEnabledExtensions : this.mergePaths(...)`). cyrup compiles in what pi
    /// installs, and `native_survives_no_extensions` consults exactly this method to implement
    /// `--no-extensions`. Returning `false` — the default, meaning "the embedder named me
    /// explicitly", which is pi's INLINE factory tier — would make `--no-extensions` mean something
    /// different in the two products.
    fn is_ambient(&self) -> bool {
        true
    }

    // `decides_project_trust` is deliberately NOT overridden, and the default `false` is the whole
    // point (MCP-001). A native that opts into the pre-trust bootstrap pass has its `init` run
    // **twice on the very same object** — cyrup has no re-instantiation for a native, unlike pi,
    // whose module cache holds factories and builds a fresh `Extension` per pass. This `init` is not
    // idempotent in that sense: it spawns the eager/keep-alive pre-warm task, so a second pass would
    // start a second pre-warm against the same servers. The adapter has no `project_trust` vote to
    // cast, so it has nothing to gain from the pass either.

    /// The registration window — and **it must never return `Err`**.
    ///
    /// A native extension's failing `init()` is marked a fatal startup diagnostic by the session
    /// builder, and every mode arm turns that into `dispose(); exit 1`. Upstream's
    /// `installMcpAdapter` cannot fail: every disk read it performs is defensive. So a malformed
    /// `mcp.json` or `mcp-cache.json` degrades to an empty surface here, never to an `Err`
    /// (MCP-003).
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        let config = self.programmatic_config.clone().unwrap_or_else(|| {
            self.config_context().load().config
        });

        // A NEW generation gets a NEW executor: this pass mints fresh tool objects, and the
        // dispatch they read is installed once this generation's `McpState` exists.
        let surface: RegisteredSurface = crate::registration::register_surface(
            api,
            &self.dirs,
            &config,
            Arc::new(crate::registration::ToolDispatch::default()),
        );
        tracing::debug!(
            "MCP: registered {} tool(s) and {} command(s) from disk",
            surface.tool_names.len(),
            surface.command_names.len()
        );

        // ADOPT the surface. A pass that registers tools and then drops what it registered is not
        // idempotent in cyrup's sense: `init` re-runs on the SAME object for every session
        // generation (the crate docs' ordering inversion, MCP-014), so these three slots are the
        // extension's memory of what the model was last shown. `registeredDirectTools` is what
        // MCP-036's diff compares a fingerprint against, `proxyToolDescription` is what MCP-043
        // compares to avoid re-registering identical bytes — the check that keeps the provider's
        // prompt-cache prefix valid across a reconnect — and `registeredPromptCommands` is
        // MCP-039's dedup set. Replaced wholesale rather than merged: a pass re-reads the config and
        // the cache from disk, so its surface is the whole truth for this generation.
        //
        // `dispatch` is the load-bearing one. It is the ONLY handle to the `Arc<ToolDispatch>` that
        // this pass's tools closed over; letting it fall out of scope here would strand every tool
        // registered above on the `MCP not initialized` arm with no way to ever bind them.
        if let Ok(mut slot) = self.registered_direct_tools.lock() {
            *slot = surface.direct_tool_fingerprints.clone();
        }
        if let Ok(mut slot) = self.registered_prompt_commands.lock() {
            *slot = surface
                .prompt_commands
                .iter()
                .map(|spec| (spec.command_name.clone(), spec.server_name.clone()))
                .collect();
        }
        if let Ok(mut slot) = self.proxy_tool_description.lock() {
            slot.clone_from(&surface.proxy_description);
        }
        if let Ok(mut slot) = self.dispatch.lock() {
            *slot = Some(Arc::clone(&surface.dispatch));
        }
        // MCP-238 — resolved from the SAME early config the surface was registered from, and never
        // re-read after this point.
        if let Ok(mut slot) = self.render_options.lock() {
            *slot = config.settings.as_ref().map_or_else(
                crate::renderers::McpToolRenderOptions::default,
                crate::renderers::resolve_mcp_tool_render_options,
            );
        }

        // Renderers are DECLARED inside `register_surface`, beside each registration (MCP-036).
        // They used to be declared here, in a second loop over `surface.tool_names` — which worked
        // only because `init` was the sole caller. HA-1 gave the same pass a second, late caller
        // with no such loop, so a declaration that lives outside the pass exists on one path and
        // not the other.

        // `startLoadTimeInitialization` (MCP-012): pre-warm ONLY when some enabled server declares
        // `lifecycle: "eager" | "keep-alive"`. Everything else connects lazily on first call, which
        // is what keeps a cold start from paying for N subprocess handshakes.
        if crate::runtime::needs_load_time_initialization(&config) {
            tracing::debug!("MCP: eager/keep-alive servers configured — pre-warm pending");
        }

        Ok(())
    }

    /// The four subscribed seams (see [`crate::registration::SUBSCRIBED_EVENTS`]).
    ///
    /// `ToolResult` is `error-signal.ts`'s `toolErrorOverride`: a returned MCP failure is re-flagged
    /// as an error with `HookOutcome::Mutate(EventPatch::ToolResult { is_error: Some(true), .. })`,
    /// whose `apply_patch` leaves `content` and `details` untouched when `None` (MCP-045).
    ///
    /// `Input` is [`Self::on_input`] — `index.ts:489-511`'s pre-turn convergence. It returns
    /// [`HookOutcome::Noop`] because upstream's handler returns nothing: it gates the turn by
    /// *taking time*, and never transforms the submission's text or images.
    async fn on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        let outcome = match ev {
            HostEvent::SessionStart { reason, .. } => {
                self.on_session_start(reason, ctx).await;
                HookOutcome::Noop
            }
            HostEvent::Input { .. } => {
                self.on_input().await;
                HookOutcome::Noop
            }
            HostEvent::SessionShutdown { reason, .. } => {
                self.on_session_shutdown(reason).await;
                HookOutcome::Noop
            }
            // `error-signal.ts:13-21` `toolErrorOverride` — re-flag EXACTLY two `details.error`
            // codes (MCP-045). A failed MCP tool call is RETURNED, not thrown, so without this the
            // agent records it as a success (`error-signal.ts:4-6`).
            //
            // `auth_required` is deliberately NOT one of them (`error-signal.ts:10-11`): it is a
            // prompt to run `/mcp-auth`, not a tool failure, and flagging it would make the model
            // retry instead of authenticate.
            //
            // The `is_error: false` guard is cyrup-only and observably identical to upstream:
            // `toolErrorOverride` returns `{isError: true}` for an already-flagged result too, and
            // `apply_patch` would then write `true` over `true`.
            //
            // The two-code list itself lives in ONE place — `McpErrorCode::is_tool_error_override`
            // (`proxy/error_vocab.rs`), reached here through `tool_error_override`, which is the
            // direct port of `toolErrorOverride(details)`. Inlining the `matches!` here forked the
            // vocabulary: `error_vocab`'s copy is the one the code-table test pins, so a divergence
            // would have been invisible to it.
            HostEvent::ToolResult { details: Some(details), is_error: false, .. }
                if crate::proxy::error_vocab::tool_error_override(Some(details)) == Some(true) =>
            {
                // `content` / `details` / `usage` stay `None` so `apply_patch`
                // (`contract.rs:96-113`) leaves them untouched — that is the whole reason the patch
                // is four `Option`s rather than a replacement.
                HookOutcome::Mutate(cyrup_ext::EventPatch::ToolResult {
                    content: None,
                    details: None,
                    is_error: Some(true),
                    usage: None,
                    terminate: None,
                })
            }
            _ => HookOutcome::Noop,
        };
        // MCP-471 — record the dispatch context so a consent dialog opened LATER, from a path that
        // carries no `HostCtx` of its own (`Tool::execute` for the gateway and the direct tools,
        // `ClientHandler::create_message` for sampling), can still take the `#[must_use]`
        // `begin_human_wait` guard. `HumanWaitGate::begin` is private to `cyrup-ext` and reachable
        // only through a ctx, so this is the only producer there can be.
        //
        // AFTER the match, and it covers every dispatch EXCEPT the first `SessionStart` — which is
        // exactly why `commit_initialization` records the ctx itself. `on_session_start` spawns the
        // build rather than awaiting it (the 5-second dispatch budget drops a handler that outlives
        // it), so on that first event there is no committed state here to record onto and this
        // block is a no-op. Every later dispatch refreshes the slot through this line. See
        // `McpState::human_wait_ctx` for why storing a ctx past its own dispatch is sound (the gate
        // is one shared `Arc` per native handle, and the fields that go stale are never read
        // through this slot).
        if let Some(state) = self.state() {
            state.set_human_wait_ctx(ctx);
        }
        outcome
    }

    /// Service the slash commands this extension registers.
    ///
    /// `/mcp-auth` (MCP-334) and `/mcp` (`crate::commands`) are both routed. The per-server prompt
    /// commands keep the trait's default answer — `native extension has no handler for command …` —
    /// because their dispatcher is MCP-039 and is not ported; reproducing that error verbatim here
    /// is what keeps overriding this method from changing their behaviour.
    ///
    /// **The name is matched on its base.** `cyrup_ext`'s facade disambiguates two extensions
    /// registering the same command into `mcp-auth:1` / `mcp-auth:2` (SEAM-048, pi's
    /// `resolveRegisteredCommands`) and dispatches with the REGISTERED name, so an exact comparison
    /// against the constant would leave this extension unable to service its own command whenever
    /// something else registered the name first.
    async fn execute_command(
        &self,
        name: &str,
        args: &str,
        ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        let base = name.split(':').next().unwrap_or(name);
        if base == crate::registration::MCP_AUTH_COMMAND {
            return self.on_mcp_auth_command(args, ctx).await;
        }
        if base == crate::registration::MCP_COMMAND {
            return self.on_mcp_command(args, ctx).await;
        }
        Err(ExtError::Component(format!("native extension has no handler for command `{name}`")))
    }

    /// `renderCall` for every name [`crate::registration::register_surface`] declared a renderer
    /// for — the gateway tool and each direct tool (MCP-237, MCP-241, MCP-243).
    ///
    /// [`crate::renderers::render_call`] splits on
    /// [`crate::registration::PROXY_TOOL_NAME`]: the gateway gets
    /// `formatMcpProxyToolCallLines`' seven branches, everything else gets
    /// `formatMcpDirectToolCallLines`.
    /// MCP-041 — `/mcp`'s dynamic argument completions.
    ///
    /// This is the seam the TUI already drives: `App::refresh_extension_completions` calls
    /// `ExtensionHost::command_completions`, which routes the native tier **first** and lands here.
    /// Opting in with `InitApi::add_autocomplete` (done at registration) is what puts `/mcp` in the
    /// front-end's table; without it this method is never asked.
    async fn argument_completions(
        &self,
        name: &str,
        prefix: &str,
    ) -> Result<Vec<String>, ExtError> {
        Ok(crate::commands::argument_completions(self, name, prefix))
    }

    fn render_call(&self, key: &str, call: &serde_json::Value) -> Option<serde_json::Value> {
        crate::renderers::render_call(key, call, self.render_options())
    }

    /// `renderResult` (MCP-239, MCP-240, MCP-242).
    ///
    /// `toolsExpanded` is read from [`cyrup_ext::host::HostServices`] here rather than inside the
    /// renderer, which is what keeps [`crate::renderers::render_result`] a pure, synchronous
    /// projection — the host calls it on the UI's event path and an `async` renderer would stall
    /// the frame. A headless build has no services and collapses, which is the correct default.
    fn render_result(&self, key: &str, result: &serde_json::Value) -> Option<serde_json::Value> {
        crate::renderers::render_result(
            key,
            result,
            self.render_options(),
            self.host_services()
                .is_some_and(|services| services.tools_expanded()),
        )
    }

    /// Bind the live capability backend. Called **before** [`Self::init`], which is what makes an
    /// `init`-spawned background task legitimate.
    fn set_host_services(&self, services: Arc<dyn cyrup_ext::host::HostServices>) {
        let _ = self.host_services.set(services);
    }

    /// Bind the post-`init` registration handle (HA-1 / MCP-037). Also called before
    /// [`Self::init`], and for the same reason: `init` may pre-warm an eager server, and the
    /// connection that completes must be able to surface the tools it discovered.
    fn set_late_registrar(&self, registrar: Arc<dyn cyrup_ext::LateRegistrar>) {
        if let Ok(mut slot) = self.late_registrar.lock() {
            *slot = Some(registrar);
        }
    }
}

/// The construction gate — `cyrup_mcp::mcp_extension_for_env(...)`, mirroring
/// `cyrup_ext_subagents::extension::subagent_extension_for_env`.
///
/// **This gate does not gate.** It returns `Some` unconditionally, and that is the port, not an
/// oversight: upstream `pi-mcp-adapter` is an installed package present in **every session of every
/// mode**, and switching it off is `--no-extensions`' job — which reaches it through
/// [`NativeExtension::is_ambient`], not through this function. The `Option` return exists so the
/// call sites in `crates/cyrup/src/main.rs` read identically to their three siblings, and so a
/// future gate (an opt-out env var, a child-mode carve-out) has somewhere to live that the callers
/// already handle.
///
/// Note in particular that this does **not** return `None` inside a subagent child. A child re-execs
/// `cyrup` in Print/Json mode and resolves its `mcp:` tool selectors against the *parent's*
/// `mcp-cache.json`; `MCP_DIRECT_TOOLS` then pins which servers it needs, and MCP-013's blocking
/// wait at `SessionStart` is what makes them present on the child's very first turn.
#[must_use]
pub fn mcp_extension_for_env(
    agent_dir: &Path,
    config: Option<McpConfig>,
    cwd: PathBuf,
) -> Option<Arc<dyn NativeExtension>> {
    let dirs = McpDirs::new(agent_dir.to_path_buf(), cwd);
    // `into_arc` — never a bare `Arc::new` — because it is what binds the self-handle the metadata
    // listener needs, and this coercion to `Arc<dyn NativeExtension>` is one-way.
    Some(McpExtension::with_config(dirs, config).into_arc() as Arc<dyn NativeExtension>)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn extension() -> McpExtension {
        McpExtension::new(McpDirs::new(
            PathBuf::from("/nonexistent/agent"),
            PathBuf::from("/w"),
        ))
    }

    #[tokio::test]
    async fn init_never_fails_and_registers_the_flag_and_commands() {
        let ext = extension();
        let mut api = InitApi::new();
        assert!(ext.init(&mut api).await.is_ok());
    }

    /// The renderer seam: `init` must *declare* a renderer for every tool it registers AND the
    /// extension must actually *serve* one, or every MCP row falls back to the host's own framing.
    /// Cut 2 landed `renderers.rs`; this pins that it is reachable through the trait.
    #[tokio::test]
    async fn the_registered_renderers_are_served_not_just_declared() {
        use serde_json::json;

        let ext = extension();
        let mut api = InitApi::new();
        ext.init(&mut api).await.unwrap();

        // The gateway tool's call row — `formatMcpProxyToolCallLines`' `tool @ server` head.
        let proxy = ext
            .render_call(
                crate::registration::PROXY_TOOL_NAME,
                &json!({ "tool": "list_issues", "server": "linear", "args": {} }),
            )
            .expect("the gateway tool must render its own call row");
        assert!(
            proxy.to_string().contains("mcp call list_issues @ linear"),
            "{proxy}"
        );

        // Any other name takes the direct-tool formatter, which leads with the prefixed name.
        let direct = ext
            .render_call("linear_list_issues", &json!({ "state": "open" }))
            .expect("a direct tool must render its own call row");
        assert!(direct.to_string().contains("linear_list_issues"), "{direct}");

        // The result side renders for both, and a headless build (no `HostServices`) collapses.
        assert!(
            ext.render_result(
                crate::registration::PROXY_TOOL_NAME,
                &json!({ "content": [{ "type": "text", "text": "ok" }] })
            )
            .is_some()
        );

        // MCP-238: the options are resolved once at `init`, from that generation's config.
        assert_eq!(
            ext.render_options().result_rendering,
            crate::config::ToolResultRendering::Compact,
            "no `mcp.json` on disk means the compact default"
        );
    }

    #[tokio::test]
    async fn init_survives_a_malformed_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mcp.json"), "{{{").unwrap();
        let ext = McpExtension::new(McpDirs::new(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        ));
        let mut api = InitApi::new();
        assert!(ext.init(&mut api).await.is_ok(), "a stray `{{{{{{` must not crash cyrup");
    }

    /// `init` must ADOPT what it registered, not drop it. `dispatch` is the one that would be an
    /// unrecoverable loss: it is the only handle to the `Arc<ToolDispatch>` every tool registered by
    /// this pass closed over, so without it MCP-214 has nowhere to install and every MCP tool call
    /// answers `MCP not initialized` for the life of the generation.
    #[tokio::test]
    async fn init_adopts_the_surface_it_registered() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mcp.json"),
            r#"{"mcpServers":{"adopted":{"command":"/nonexistent/never","disabled":true}}}"#,
        )
        .unwrap();
        let ext = McpExtension::new(McpDirs::new(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        ))
        .with_home(dir.path().to_path_buf());

        assert!(ext.dispatch().is_none(), "nothing is adopted before the first pass");

        let mut api = InitApi::new();
        assert!(ext.init(&mut api).await.is_ok());

        let dispatch = ext.dispatch().expect("the pass's executor slot survives `init`");
        assert!(!dispatch.is_installed(), "…un-installed, which is MCP-214's job");
        // The cold-cache surface is proxy-only, so the description slot is seeded and the
        // direct-tool fingerprint map is empty — both are what the next pass diffs against.
        let description = ext
            .proxy_tool_description()
            .lock()
            .unwrap()
            .clone()
            .expect("MCP-043's identity check needs the bytes it last registered");
        assert!(
            description.contains("adopted"),
            "the seeded description is the one built from THIS config: {description}"
        );
        assert!(ext.registered_direct_tools().lock().unwrap().is_empty());
        assert!(ext.registered_prompt_commands().lock().unwrap().is_empty());

        // A second pass replaces the slots wholesale rather than accumulating — `init` re-reads
        // config and cache from disk, so its surface is the whole truth for that generation.
        let mut api = InitApi::new();
        assert!(ext.init(&mut api).await.is_ok());
        assert!(ext.dispatch().is_some());
        assert!(ext.registered_direct_tools().lock().unwrap().is_empty());
    }

    #[test]
    fn the_adapter_is_ambient_and_does_not_decide_project_trust() {
        let ext = extension();
        assert!(ext.is_ambient(), "--no-extensions must switch the adapter off");
        assert!(
            !ext.decides_project_trust(),
            "opting in would run this non-idempotent init twice on the same object"
        );
    }

    #[test]
    fn the_gate_attaches_in_every_session() {
        assert!(
            mcp_extension_for_env(Path::new("/a"), None, PathBuf::from("/w")).is_some(),
            "upstream is an installed package present in every session of every mode"
        );
    }

    #[tokio::test]
    async fn session_shutdown_bumps_the_generation_and_clears_the_slots() {
        let ext = extension();
        assert_eq!(ext.generation(), 0);
        ext.on_session_shutdown("quit").await;
        assert_eq!(ext.generation(), 1);
        assert!(ext.state().is_none() && ext.owner().is_none());
    }

    // ── `pi.on("input")` — pre-turn keep-alive convergence (`index.ts:489-511`, `48799fa`) ────

    /// A submission with a state and a live owner must actually drive one convergence pass.
    ///
    /// The observation point is the **reconnect-failure callback**: with the manager seam still
    /// unbound (`ManagerSupervisor`'s MCP-100 bodies), a keep-alive server has no connection and
    /// `connect` fails, so `reportConnectionFailure` fires exactly once per pass. That the callback
    /// ran at all is the proof the handler reached `ensureConverged` — which is the whole of the
    /// fix, and the half that a missing [`crate::registration::SUBSCRIBED_EVENTS`] entry or a
    /// spawned-instead-of-awaited call would silently drop.
    #[tokio::test]
    async fn an_input_converges_the_keep_alive_servers_before_the_turn() {
        let owner = Arc::new(McpRuntimeOwner::new());
        let state = input_state(Arc::clone(&owner));
        state.lifecycle.register_server(
            "linear",
            crate::config::ServerEntry {
                lifecycle: Some(crate::config::ServerLifecycle::KeepAlive),
                url: Some("https://mcp.linear.app/sse".to_string()),
                ..Default::default()
            },
            None,
        );
        state.lifecycle.mark_keep_alive("linear");

        let converged = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&converged);
        state
            .lifecycle
            .set_reconnect_failure_callback(Arc::new(move |name, _error| {
                assert_eq!(name, "linear");
                flag.store(true, Ordering::Release);
            }));

        let ext = extension();
        *ext.owner.lock().unwrap() = Some(Arc::clone(&owner));
        *ext.state.lock().unwrap() = Some(Arc::clone(&state));

        assert!(matches!(
            ext.on_event(&input_event(), &event_ctx()).await,
            HookOutcome::Noop
        ));
        assert!(
            converged.load(Ordering::Acquire),
            "the submission must await one convergence pass, not merely schedule one"
        );
    }

    /// The two guards that make the handler safe on a dead or unbuilt generation
    /// (`index.ts:490-491`, `:502`). Neither may panic, and neither may converge.
    #[tokio::test]
    async fn an_input_with_no_owner_or_a_stopped_owner_converges_nothing() {
        let ext = extension();
        // `if (!inputOwner?.isActive()) return` with `currentOwner === undefined`.
        assert!(matches!(
            ext.on_event(&input_event(), &event_ctx()).await,
            HookOutcome::Noop
        ));

        let owner = Arc::new(McpRuntimeOwner::new());
        let state = input_state(Arc::clone(&owner));
        state.lifecycle.register_server(
            "linear",
            crate::config::ServerEntry {
                lifecycle: Some(crate::config::ServerLifecycle::KeepAlive),
                ..Default::default()
            },
            None,
        );
        state.lifecycle.mark_keep_alive("linear");
        let converged = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&converged);
        state
            .lifecycle
            .set_reconnect_failure_callback(Arc::new(move |_name, _error| {
                flag.store(true, Ordering::Release);
            }));

        *ext.owner.lock().unwrap() = Some(Arc::clone(&owner));
        *ext.state.lock().unwrap() = Some(state);
        owner.begin_stop(Some("session replaced")).await.unwrap();

        let _ = ext.on_event(&input_event(), &event_ctx()).await;
        assert!(
            !converged.load(Ordering::Acquire),
            "a replaced generation must not gate the live session's turn"
        );
    }

    /// MCP-471's producer half: every dispatch records its ctx on the committed state, so a consent
    /// dialog opened later from a ctx-less path (`Tool::execute`, `ClientHandler::create_message`)
    /// can still take the P-3 budget-forgiveness guard.
    ///
    /// Recorded **after** the handler runs, because on a `SessionStart` there is no state to record
    /// onto until the build has returned — hence the "no state, no record, no panic" arm first.
    #[tokio::test]
    async fn every_dispatch_records_its_ctx_for_a_later_consent_dialog() {
        let ext = extension();
        // No committed state yet: the record is skipped, and the dispatch is unaffected.
        assert!(matches!(ext.on_event(&input_event(), &event_ctx()).await, HookOutcome::Noop));

        let owner = Arc::new(McpRuntimeOwner::new());
        let state = input_state(Arc::clone(&owner));
        *ext.owner.lock().unwrap() = Some(Arc::clone(&owner));
        *ext.state.lock().unwrap() = Some(Arc::clone(&state));
        assert!(
            state.human_wait_ctx.lock().unwrap().is_none(),
            "nothing recorded before the first dispatch"
        );

        let ctx = event_ctx();
        let _ = ext.on_event(&input_event(), &ctx).await;

        let recorded = state.human_wait_ctx.lock().unwrap().clone().expect("the ctx is recorded");
        assert!(
            Arc::ptr_eq(&recorded.human_wait_gate(), &ctx.human_wait_gate()),
            "the recorded clone must carry the SAME `HumanWaitGate` the dispatcher polls, or the \
             guard suspends a budget nobody is watching"
        );
    }

    /// The subscription is half the fix: `on_input` is unreachable without it.
    #[test]
    fn the_input_seam_is_subscribed() {
        assert!(
            crate::registration::SUBSCRIBED_EVENTS.contains(&cyrup_ext::EventKind::Input),
            "`index.ts:489` is a fourth `pi.on` registration, not a variant of the other three"
        );
    }

    fn input_event() -> HostEvent {
        HostEvent::Input {
            text: "ship it".to_string(),
            images: Vec::new(),
            source: cyrup_ext::InputEventSource::Interactive,
            streaming_behavior: None,
        }
    }

    fn event_ctx() -> HostCtx {
        HostCtx::event(cyrup_ext::native::ExtMode::Tui, true, PathBuf::from("/w"))
    }

    // --- MCP-036 / MCP-038: the removal half ---------------------------------------------------

    /// A `HostServices` that answers `getActiveTools` from a slot and records every
    /// `setActiveTools` write and every notice — the three seams the removal half drives. Not
    /// `RecordingServices`: that backend leaves `active_tools` / `set_active_tools` on their trait
    /// defaults (`None` / no-op), which is precisely the arm under test.
    #[derive(Default)]
    struct ToolSetServices {
        active: Mutex<Option<Vec<String>>>,
        writes: Mutex<Vec<Vec<String>>>,
        notices: Mutex<Vec<String>>,
    }

    impl ToolSetServices {
        fn with_active(names: &[&str]) -> Arc<Self> {
            let services = Arc::new(Self::default());
            *services.active.lock().unwrap() =
                Some(names.iter().map(|name| (*name).to_string()).collect());
            services
        }

        fn writes(&self) -> Vec<Vec<String>> {
            self.writes.lock().unwrap().clone()
        }

        fn notices(&self) -> Vec<String> {
            self.notices.lock().unwrap().clone()
        }
    }

    impl cyrup_ext::host::HostServices for ToolSetServices {
        fn notify(&self, message: &str, _kind: cyrup_ext::NotifyKind) {
            self.notices.lock().unwrap().push(message.to_string());
        }
        fn active_tools(&self) -> Option<Vec<String>> {
            self.active.lock().unwrap().clone()
        }
        fn set_active_tools(&self, names: &[String]) {
            *self.active.lock().unwrap() = Some(names.to_vec());
            self.writes.lock().unwrap().push(names.to_vec());
        }
    }

    /// The HA-1 registration handle, recording what a late pass registered.
    #[derive(Default)]
    struct RecordingRegistrar {
        tools: Mutex<Vec<String>>,
    }

    impl RecordingRegistrar {
        fn tools(&self) -> Vec<String> {
            self.tools.lock().unwrap().clone()
        }
    }

    impl cyrup_ext::LateRegistrar for RecordingRegistrar {
        fn register_tool(&self, tool: Arc<dyn cyrup_core::Tool>) -> Result<(), ExtError> {
            self.tools.lock().unwrap().push(tool.name().to_string());
            Ok(())
        }
        fn register_command(
            &self,
            _name: String,
            _desc: cyrup_ext::CommandDescriptor,
        ) -> Result<(), ExtError> {
            Ok(())
        }
        fn register_tool_renderer(&self, _tool_name: String) -> Result<(), ExtError> {
            Ok(())
        }
        fn owner(&self) -> ExtensionId {
            ExtensionId::from(EXTENSION_ID)
        }
    }

    fn fallback_names(ext: &McpExtension) -> Vec<String> {
        ext.fallback_deactivated_tools().lock().unwrap().clone()
    }

    fn bind_services(ext: &McpExtension, services: &Arc<ToolSetServices>) {
        ext.set_host_services(
            Arc::clone(services) as Arc<dyn cyrup_ext::host::HostServices>
        );
    }

    /// `index.ts:194-202` — the `setActiveTools` fallback, the only branch cyrup has.
    #[test]
    fn a_removed_tool_leaves_the_active_set_and_is_recorded_as_deactivated() {
        let ext = extension();
        let services = ToolSetServices::with_active(&["read", "srv_gone"]);
        bind_services(&ext, &services);

        ext.deactivate_tools(&["srv_gone".to_string()]);

        assert_eq!(services.writes(), vec![vec!["read".to_string()]]);
        assert_eq!(fallback_names(&ext), vec!["srv_gone".to_string()]);
    }

    /// `if (nextActiveTools.length !== activeTools.length)` (`index.ts:198`): a name the host never
    /// had active must not cost a `setActiveTools` write — that call rewrites the agent's tool array
    /// and the base system prompt — and must not enter the fallback set either.
    #[test]
    fn a_tool_that_is_not_in_the_active_set_writes_nothing() {
        let ext = extension();
        let services = ToolSetServices::with_active(&["read"]);
        bind_services(&ext, &services);

        ext.deactivate_tools(&["srv_gone".to_string()]);

        assert!(services.writes().is_empty());
        assert!(fallback_names(&ext).is_empty());
    }

    /// `getActiveToolsIfReady()` returning undefined, and the empty-list arm beside it
    /// (`index.ts:176-184`, `:193-196`): record the fallback, write nothing.
    #[test]
    fn no_active_tool_list_records_the_fallback_without_a_write() {
        let ext = extension();
        let services = Arc::new(ToolSetServices::default());
        bind_services(&ext, &services);

        ext.deactivate_tools(&["srv_gone".to_string()]);

        assert!(services.writes().is_empty());
        assert_eq!(fallback_names(&ext), vec!["srv_gone".to_string()]);
    }

    /// `index.ts:223-228` — `fallbackDeactivatedTools.delete(name)` returning true IS the gate, and
    /// a tool that comes back is appended to the active list rather than staying invisible.
    #[test]
    fn reactivation_is_gated_on_the_fallback_set() {
        let ext = extension();
        let services = ToolSetServices::with_active(&["read", "srv_back"]);
        bind_services(&ext, &services);

        // Never deactivated: the delete returns false, so nothing is written.
        ext.reactivate_tool("srv_back");
        assert!(services.writes().is_empty());

        ext.deactivate_tools(&["srv_back".to_string()]);
        ext.reactivate_tool("srv_back");

        assert_eq!(
            services.writes(),
            vec![
                vec!["read".to_string()],
                vec!["read".to_string(), "srv_back".to_string()],
            ]
        );
        assert!(fallback_names(&ext).is_empty());
    }

    fn late_pass_extension() -> McpExtension {
        McpExtension::with_config(
            McpDirs::new(PathBuf::from("/nonexistent/agent"), PathBuf::from("/w")),
            Some(McpConfig::default()),
        )
    }

    /// `PROXY_TOOL_NAME` rides on `surface.tool_names` (`registration.rs:2892`) and is never in
    /// `known_tools`, so counting the list unfiltered would report an `added` tool — and a UI notice
    /// — on every proxy-description change. A pass that registered nothing but the proxy is not a
    /// direct-tool change.
    #[tokio::test]
    async fn the_proxy_tool_is_not_counted_as_an_added_direct_tool() {
        let ext = late_pass_extension();
        let mut api = InitApi::new();
        ext.init(&mut api).await.unwrap();

        let registrar = Arc::new(RecordingRegistrar::default());
        ext.set_late_registrar(Arc::clone(&registrar) as Arc<dyn cyrup_ext::LateRegistrar>);
        let services = ToolSetServices::with_active(&["read"]);
        bind_services(&ext, &services);
        // `should_register_proxy` compares against this slot, so clearing it is what puts
        // `PROXY_TOOL_NAME` back onto `surface.tool_names` for this pass.
        *ext.proxy_tool_description().lock().unwrap() = None;

        assert!(!ext.sync_tool_surface());
        assert_eq!(registrar.tools(), vec![crate::registration::PROXY_TOOL_NAME.to_string()]);
        assert!(services.notices().is_empty());
    }

    /// `index.ts:233-237` + `:257-263` — a previously-registered tool absent from the new
    /// resolution is deactivated, dropped from `registeredDirectTools` by the adoption, and counted
    /// in the one notice.
    #[tokio::test]
    async fn a_vanished_direct_tool_is_deactivated_dropped_and_notified() {
        let ext = late_pass_extension();
        let mut api = InitApi::new();
        ext.init(&mut api).await.unwrap();

        ext.set_late_registrar(
            Arc::new(RecordingRegistrar::default()) as Arc<dyn cyrup_ext::LateRegistrar>
        );
        let services = ToolSetServices::with_active(&["read", "srv_ghost"]);
        bind_services(&ext, &services);
        // The model is currently shown a tool the config and cache no longer resolve.
        ext.registered_direct_tools()
            .lock()
            .unwrap()
            .insert("srv_ghost".to_string(), "fingerprint".to_string());

        assert!(ext.sync_tool_surface());

        assert_eq!(services.writes(), vec![vec!["read".to_string()]]);
        assert_eq!(
            services.notices(),
            vec!["MCP: direct tools refreshed (+0, ~0, -1)".to_string()]
        );
        assert_eq!(fallback_names(&ext), vec!["srv_ghost".to_string()]);
        // Upstream's `registeredDirectTools.delete(toolName)` is cyrup's wholesale adoption.
        assert!(ext.registered_direct_tools().lock().unwrap().is_empty());
    }

    // ── `startInitialization` and the commit tail (`index.ts:292-350`, MCP-011) ───────────────

    fn session_start_event() -> HostEvent {
        HostEvent::SessionStart { reason: "startup".to_string(), previous_session_file: None }
    }

    /// A memoised build that is already settled, so the commit tail can be driven directly.
    fn settled_task(state: &Arc<McpState>) -> Arc<InitTask> {
        let state = Arc::clone(state);
        Arc::new(async move { Ok(state) }.boxed().shared())
    }

    /// **The wiring proof.** A `SessionStart` must reach [`crate::runtime::initialize_mcp`] and
    /// commit what it built — which before MCP-011 it did not: `on_session_start` bumped the
    /// generation, tore the previous one down and returned, so `init_task` was written to `None` at
    /// two sites and to `Some` at none, and no configured server ever connected in production.
    ///
    /// The build is **spawned**, so the handler returns before it settles and the assertions poll.
    /// That is the point, not a wrinkle: the native dispatch budget drops a handler future that
    /// outlives it, and `initialize_mcp` connects subprocesses.
    #[tokio::test]
    async fn a_session_start_builds_the_runtime_and_runs_the_commit_tail() {
        let dir = tempfile::tempdir().unwrap();
        // A programmatic config, so the build is hermetic: `initialize_mcp` skips discovery
        // entirely and cannot reach the developer's real `~/.config/mcp/mcp.json`.
        let ext = McpExtension::with_config(
            McpDirs::new(dir.path().to_path_buf(), dir.path().to_path_buf()),
            Some(McpConfig::default()),
        )
        .with_home(dir.path().to_path_buf())
        .into_arc();

        let mut api = InitApi::new();
        ext.init(&mut api).await.unwrap();
        assert!(ext.state().is_none() && ext.proxy_ctx().is_none() && ext.owner().is_none());

        assert!(matches!(
            ext.on_event(&session_start_event(), &event_ctx()).await,
            HookOutcome::Noop
        ));
        assert_eq!(ext.generation(), 1, "the handler bumps the generation");
        // Published BEFORE the drain await, so a call arriving mid-start fences against the
        // generation that is starting.
        let owner = ext.owner().expect("the new generation's owner is published synchronously");
        assert!(owner.is_active());

        let state = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                if let Some(state) = ext.state() {
                    return state;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the spawned build must settle and COMMIT — check `init_task` was memoised");

        // The commit tail's two installers, both of which had zero call sites before this unit.
        assert!(
            ext.proxy_ctx().is_some(),
            "`install_runtime_env` must run, or the dispatcher answers `not_initialized` forever"
        );
        assert!(
            state.on_tool_metadata_updated.lock().unwrap().is_some(),
            "`install_surface_sync` must run, or a later connect never reaches the model"
        );
        assert!(
            ext.dispatch().is_some_and(|slot| slot.is_installed()),
            "MCP-214's executor must be installed, or every registered MCP tool answers \
             `MCP not initialized` for the life of the generation"
        );
        // `initPromise = null`, and only after the surface sync.
        assert!(ext.init_task().is_none(), "the memo is cleared once the build has committed");
        // MCP-471's producer for the first `SessionStart`: `on_event`'s tail cannot cover it,
        // because the state did not exist when that tail ran.
        assert!(
            state.human_wait_ctx.lock().unwrap().is_some(),
            "the commit tail records the dispatch ctx the first turn's consent dialog needs"
        );

        // The symmetric teardown takes all four slots, including the OAuth runtime — without which
        // every session would leak one process-global live-runtime id.
        ext.on_session_shutdown("quit").await;
        assert_eq!(ext.generation(), 2);
        assert!(ext.state().is_none() && ext.owner().is_none() && ext.proxy_ctx().is_none());
        assert!(ext.oauth_runtime.lock().unwrap().is_none());
        assert!(!owner.is_active(), "the generation's owner is stopped, not merely dropped");
    }

    /// A real stdio MCP server, as an `sh` script — the same fixture runtime `runtime.rs`'s
    /// `TINY_MCP` and `server_manager.rs`'s child-process tests use, so it adds no host dependency.
    ///
    /// `$1` is the protocol version to echo back (passed positionally rather than through `env`,
    /// because a stdio server's `env` REPLACES the child's environment and `sh` needs `PATH`), and
    /// `$2` is a marker file it touches on start, so "a child process really ran" is checkable from
    /// the outside.
    const LIVE_MCP: &str = r#"
: > "$2"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}}\n' "$id" "$1"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"echo back","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}}\n' "$id"
      ;;
    *'"method":"notifications/'*) : ;;
    *)
      if [ -n "$id" ]; then printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"; fi
      ;;
  esac
done
"#;

    /// **A server listed in `mcp.json` connects because a session started.** A real child process
    /// is spawned, completes the MCP `initialize` handshake, and the generation's state records the
    /// live connection.
    ///
    /// Every link this unit added is on the path and none can be faked: `on_session_start` must
    /// build a [`crate::runtime::ContextSnapshot`] and call [`McpExtension::start_initialization`];
    /// `start_initialization` must memoise the build **and spawn its driver** (a `Shared` nobody
    /// polls never runs); and [`McpExtension::commit_initialization`] must survive the staleness
    /// check and publish the state. Before this unit, [`crate::runtime::initialize_mcp`] had no
    /// production caller at all and this test could not have been written.
    ///
    /// # What this test deliberately does NOT assert, and where that IS asserted
    ///
    /// That the discovered catalog reaches the MODEL. It does — discovery (MCP-119) issues
    /// `tools/list` from `post_handshake` and `initialize_mcp`'s metadata build records the result
    /// — but "reaches the model" is a claim about a live session's tool array, and this test has an
    /// `McpExtension` and a child process, not an assembled `AgentSession`. Asserting it from here
    /// would mean asserting on `state.tool_metadata`, which is one hop short of the thing that
    /// matters and would pass just as happily with the surface sync broken.
    ///
    /// `crates/cyrup-it/tests/mcp/live_tool_call.rs` is where it is asserted, against a real
    /// session: `the_live_surface_carries_the_servers_discovered_catalog` takes the same fixture
    /// server's `tools/list` answer through `state.tool_metadata["fixture"]` (`name ==
    /// "fixture_echo"`, `original_name == "echo"`, the server's own description and schema) and out
    /// into the tool array the agent hands the provider, and
    /// `a_model_issued_direct_tool_call_returns_the_servers_own_result` calls it and reads
    /// `echoed:pong` back off the child.
    ///
    /// Multi-threaded on purpose: there is a real child process, a real stdio transport and a
    /// stderr pump behind this.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_session_start_connects_a_configured_server() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("fixture-started");
        let config: McpConfig = serde_json::from_value(serde_json::json!({
            "mcpServers": {
                "fixture": {
                    "command": "sh",
                    "args": [
                        "-c",
                        LIVE_MCP,
                        "sh",
                        rmcp::model::ProtocolVersion::LATEST.as_str(),
                        marker.to_string_lossy(),
                    ],
                }
            }
        }))
        .unwrap();

        let ext = McpExtension::with_config(
            McpDirs::new(dir.path().to_path_buf(), dir.path().to_path_buf()),
            Some(config),
        )
        .with_home(dir.path().to_path_buf())
        .into_arc();
        let mut api = InitApi::new();
        ext.init(&mut api).await.unwrap();

        // The whole of the production trigger: one `SessionStart`.
        let _ = ext.on_event(&session_start_event(), &event_ctx()).await;

        // `mcp-cache.json` does not exist in this tempdir, so `initialize_mcp` sets
        // `bootstrap_all` and the startup pass connects every enabled server once. The connection
        // map is written by that pass, which runs inside the SPAWNED build — so the wait is real
        // and not a formality.
        let state = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if let Some(state) = ext.state()
                    && state.manager.get_connection("fixture").is_some_and(|connection| {
                        connection.status() == crate::lifecycle::ConnectionStatus::Connected
                    })
                {
                    return state;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("a configured server must connect when a session starts");

        // Three independent facts, because each catches a different break.
        assert!(marker.exists(), "a real child process ran");
        assert!(
            state.server_instructions.lock().is_ok(),
            "the generation's state is the one the connect ran against"
        );
        // §9's cold-cache bootstrap ran, which is what set `bootstrap_all` and made this an
        // unconditional startup connect rather than a lazy one.
        assert!(
            dir.path().join("mcp-cache.json").exists(),
            "the cold-cache bootstrap writes an empty cache before the startup pass"
        );
        // The commit tail ran on the state that owns this connection.
        assert!(ext.proxy_ctx().is_some());
        assert!(state.on_tool_metadata_updated.lock().unwrap().is_some());

        // Teardown drains the child through the owner's cleanup stack.
        ext.on_session_shutdown("quit").await;
        assert!(ext.state().is_none());
        assert!(
            state.manager.get_connection("fixture").is_none(),
            "`shutdown_previous_generation` really drains the generation's children — the \
             graceful shutdown closes the connection and removes it from the map"
        );
    }

    /// The triple staleness check (`index.ts:305`), one clause at a time. Every clause must SHUT
    /// THE NEW STATE DOWN rather than commit it — a leaked state keeps a live server manager, its
    /// children and its lifecycle timers running with nothing able to reach them.
    #[tokio::test]
    async fn a_stale_build_is_shut_down_instead_of_committed() {
        let ext = extension().into_arc();

        // Clause 2 — a newer generation started while the build ran.
        let owner = Arc::new(McpRuntimeOwner::new());
        let state = input_state(Arc::clone(&owner));
        let task = settled_task(&state);
        *ext.init_task.lock().unwrap() = Some(Arc::clone(&task));
        McpExtension::commit_initialization(
            Arc::clone(&ext),
            Arc::clone(&state),
            &owner,
            &event_ctx(),
            ext.generation() + 1,
            &task,
            STALE_SESSION_START_STATE_REASON,
        )
        .await;
        assert!(ext.state().is_none() && ext.proxy_ctx().is_none());
        assert!(!owner.is_active(), "the orphaned runtime is torn down, not leaked");

        // Clause 3 — a second build inside the SAME generation superseded the memo. This is the
        // clause an `Arc::ptr_eq` exists for: the two `Shared`s have equal *values* and different
        // identities, and without the check both commits would land.
        let owner = Arc::new(McpRuntimeOwner::new());
        let state = input_state(Arc::clone(&owner));
        let mine = settled_task(&state);
        let theirs = settled_task(&state);
        assert!(!Arc::ptr_eq(&mine, &theirs));
        *ext.init_task.lock().unwrap() = Some(theirs);
        McpExtension::commit_initialization(
            Arc::clone(&ext),
            Arc::clone(&state),
            &owner,
            &event_ctx(),
            ext.generation(),
            &mine,
            STALE_SESSION_START_STATE_REASON,
        )
        .await;
        assert!(ext.state().is_none(), "the superseded build must not commit");

        // Clause 1 — the owner was stopped while the build ran.
        let owner = Arc::new(McpRuntimeOwner::new());
        let state = input_state(Arc::clone(&owner));
        let task = settled_task(&state);
        *ext.init_task.lock().unwrap() = Some(Arc::clone(&task));
        owner.begin_stop(Some("session replaced")).await.unwrap();
        McpExtension::commit_initialization(
            Arc::clone(&ext),
            state,
            &owner,
            &event_ctx(),
            ext.generation(),
            &task,
            STALE_SESSION_START_STATE_REASON,
        )
        .await;
        assert!(ext.state().is_none() && ext.proxy_ctx().is_none());

        // And the live case commits, so the three assertions above are about staleness rather than
        // about the tail never working.
        let owner = Arc::new(McpRuntimeOwner::new());
        let state = input_state(Arc::clone(&owner));
        let task = settled_task(&state);
        *ext.init_task.lock().unwrap() = Some(Arc::clone(&task));
        McpExtension::commit_initialization(
            Arc::clone(&ext),
            Arc::clone(&state),
            &owner,
            &event_ctx(),
            ext.generation(),
            &task,
            STALE_SESSION_START_STATE_REASON,
        )
        .await;
        assert!(ext.state().is_some() && ext.proxy_ctx().is_some());
        assert!(state.on_tool_metadata_updated.lock().unwrap().is_some());
    }

    /// A build that rejects must report once and tear its own generation down — but must NOT tear
    /// down a *live* prior state, because a session that already has a runtime is still usable.
    #[tokio::test]
    async fn a_rejected_build_tears_down_only_its_own_generation() {
        let ext = extension().into_arc();
        let error = Arc::new(McpError::other("no"));

        // With a live state, `if (state) return;` — the owner survives.
        let owner = Arc::new(McpRuntimeOwner::new());
        let state = input_state(Arc::clone(&owner));
        let oauth = crate::oauth::create_oauth_runtime(None);
        let task = settled_task(&state);
        *ext.state.lock().unwrap() = Some(Arc::clone(&state));
        *ext.init_task.lock().unwrap() = Some(Arc::clone(&task));
        ext.fail_initialization(&error, &owner, &oauth, ext.generation(), &task).await;
        assert!(owner.is_active(), "a replacement build's failure must not kill the live session");
        assert!(ext.init_task().is_none(), "`initPromise = null` still runs");

        // With no state, the generation is stopped and its OAuth runtime shut down.
        *ext.state.lock().unwrap() = None;
        let task = settled_task(&state);
        *ext.init_task.lock().unwrap() = Some(Arc::clone(&task));
        ext.fail_initialization(&error, &owner, &oauth, ext.generation(), &task).await;
        assert!(!owner.is_active());

        // A superseded generation reports nothing and touches nothing.
        let owner = Arc::new(McpRuntimeOwner::new());
        let task = settled_task(&state);
        *ext.init_task.lock().unwrap() = Some(Arc::clone(&task));
        ext.fail_initialization(&error, &owner, &oauth, ext.generation() + 1, &task).await;
        assert!(owner.is_active());
        assert!(ext.init_task().is_some(), "a stale failure must not clear the live memo");
    }

    /// `ContextSnapshot`'s `cwd` is the extension's own, never the ctx's: the servers' working
    /// directory and the directory their config and metadata cache are read from must be one value.
    #[test]
    fn the_context_snapshot_takes_its_cwd_from_the_dirs_not_the_ctx() {
        let ext = extension();
        let snapshot = ext.context_snapshot(&HostCtx::event(
            cyrup_ext::native::ExtMode::Print,
            false,
            PathBuf::from("/somewhere/else"),
        ));
        assert_eq!(snapshot.cwd, PathBuf::from("/w"));
        assert_eq!(snapshot.mode, "print");
        assert!(!snapshot.has_ui && !snapshot.is_tui_mode());
        assert!(
            snapshot.initial_signal.is_none(),
            "`on_event` carries no cancellation token; there is no producer to read one from"
        );

        let tui = ext.context_snapshot(&event_ctx());
        assert!(tui.has_ui && tui.is_tui_mode(), "URL elicitation is gated on exactly this");
    }


    // ---------------------------------------------------------------------------------------
    // MCP-334 — `/mcp-auth`
    // ---------------------------------------------------------------------------------------

    fn command_ctx(has_ui: bool) -> HostCtx {
        HostCtx::command(cyrup_ext::native::ExtMode::Tui, has_ui, PathBuf::from("/w"))
    }

    /// An `McpState` carrying `config` and, optionally, a fenced services handle — the two things
    /// every `authenticateServer` guard reads.
    fn auth_state(config: McpConfig, ui: bool) -> Arc<McpState> {
        use futures::FutureExt;

        let owner = Arc::new(McpRuntimeOwner::new());
        let manager = Arc::new(crate::state::McpServerManager::default());
        let lifecycle = Arc::new(crate::lifecycle::McpLifecycleManager::new(
            Arc::clone(&manager),
            Arc::new(|_| false),
        ));
        let services = ui.then(|| {
            Arc::new(crate::owner::OwnedServices::new(
                Arc::new(ToolSetServices::default()) as Arc<dyn cyrup_ext::host::HostServices>,
                Arc::clone(&owner),
            ))
        });
        Arc::new(McpState::new(crate::state::McpStateParts {
            owner,
            manager,
            lifecycle,
            config,
            programmatic_config: None,
            oauth_runtime: crate::oauth::create_oauth_runtime(None),
            auth_storage_options: crate::credentials::AuthStorageOptions::default(),
            ui: services,
            open_browser: Arc::new(|_| async { Ok(()) }.boxed()),
            send_message: Arc::new(|_| {}),
        }))
    }

    fn config_with(servers: &[(&str, crate::config::ServerEntry)]) -> McpConfig {
        let mut config = McpConfig::default();
        for (name, entry) in servers {
            config.mcp_servers.insert((*name).to_string(), entry.clone());
        }
        config
    }

    /// The refusal a [`Picked`] carries, with the other arm folded into a *distinguishable* failure
    /// rather than a `panic!` — this crate denies `clippy::panic` in test code too, and an
    /// assertion that names the arm it got is more diagnostic than an unwind anyway.
    fn refusal(picked: Picked) -> AuthCommandOutcome {
        match picked {
            Picked::Refused(outcome) => outcome,
            Picked::Handled => AuthCommandOutcome::failed(
                "expected a refusal, but the panel ran".to_string(),
                cyrup_ext::NotifyKind::Error,
            ),
        }
    }

    fn oauth_http(url: &str) -> crate::config::ServerEntry {
        crate::config::ServerEntry {
            url: Some(url.to_string()),
            auth: Some(crate::config::AuthMode::Named(crate::config::AuthKind::Oauth)),
            ..Default::default()
        }
    }

    /// Overriding [`NativeExtension::execute_command`] must not change what an unowned name answers.
    /// `/mcp` and `/mcp-auth` are both owned now; the prompt commands still belong to MCP-039, and a
    /// silent `Ok(None)` for one would turn "not ported" into "ran and said nothing".
    ///
    /// The name here is deliberately one this extension never registers — the point is the
    /// *fallback*, not any particular command, and pinning it against an owned name would only
    /// re-assert the routing the two arms above already prove.
    #[tokio::test]
    async fn an_unowned_command_still_reports_that_there_is_no_handler() {
        let ext = extension();
        let error = ext
            .execute_command("mcp-not-a-command", "", &command_ctx(true))
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            ExtError::Component(
                "native extension has no handler for command `mcp-not-a-command`".to_string()
            )
            .to_string()
        );
    }

    /// `/mcp` is routed now, and its no-UI path is silent by construction: `showStatus` returns
    /// early when `!hasUI`, so the default arm has nothing to notify and nothing to return.
    #[tokio::test]
    async fn mcp_routes_on_the_base_name_and_answers_through_notifications() {
        let ext = extension();
        // No state and no init task: the prologue notifies "MCP not initialized" and returns.
        // What matters here is that it is no longer an `ExtError` — the command is owned.
        let answer = ext
            .execute_command("mcp", "status", &command_ctx(false))
            .await
            .expect("`/mcp` is routed, not rejected");
        assert_eq!(answer, None, "`/mcp` speaks through notifications, never the return channel");
    }

    /// SEAM-048 for `/mcp`, the same disambiguation `/mcp-auth` is pinned for below: a second
    /// extension registering `mcp` pushes this one to `mcp:2`, and the base-name split is what keeps
    /// the adapter able to service its own command.
    #[tokio::test]
    async fn mcp_routes_on_the_disambiguated_name_too() {
        let ext = extension();
        let answer = ext
            .execute_command("mcp:2", "status", &command_ctx(false))
            .await
            .expect("the base name is what routes");
        assert_eq!(answer, None);
    }

    /// SEAM-048's disambiguated form — a second extension registering `mcp-auth` pushes this one's
    /// registration to `mcp-auth:2`, and `execute_native_command` dispatches with the REGISTERED
    /// name. Matching the constant exactly would leave the adapter unable to service its own
    /// command, and the failure would only appear on a machine with a colliding extension.
    ///
    /// The assertion doubles as the no-UI return-channel pin: with no renderer the message rides
    /// `Ok(Some(...))` rather than being dropped the way upstream drops it.
    #[tokio::test]
    async fn mcp_auth_routes_on_the_base_name_and_answers_on_the_return_channel() {
        let ext = extension();
        let answer = ext
            .execute_command("mcp-auth:2", "linear", &command_ctx(false))
            .await
            .unwrap();
        assert_eq!(answer, Some("MCP not initialized".to_string()));
    }

    /// `if (!serverName && !commandCtx.hasUI) return;` (`index.ts:636`) — a bare `/mcp-auth` in a
    /// session with no renderer has no server to act on and no way to ask for one.
    /// A headless `/mcp-auth` with no argument returns `Ok(None)` **and notifies nothing**.
    ///
    /// The second half is asserted rather than inferred, and that is the point of this test. A
    /// `!has_ui` guard was once added inside `pick_oauth_server` on the belief that this path was
    /// emitting a notice upstream swallows; it was unreachable, because `command_services` answers
    /// `None` when `!has_ui` and the caller bails before the pick. Reasoning about reachability is
    /// what got that wrong, so the silence is pinned here — at the layer that actually decides it —
    /// through a services double that records every `notify`.
    #[tokio::test]
    async fn a_bare_mcp_auth_without_a_renderer_says_nothing() {
        let ext = extension();
        let services = Arc::new(ToolSetServices::default());
        bind_services(&ext, &services);

        assert_eq!(
            ext.execute_command("mcp-auth", "   ", &command_ctx(false)).await.unwrap(),
            None,
            "the return channel stays empty"
        );
        assert!(
            services.notices().is_empty(),
            "and nothing reaches the user: {:?}",
            services.notices()
        );
    }

    /// `commands.ts:254` — the authorization-code grant needs a browser and a human.
    #[tokio::test]
    async fn authenticate_server_refuses_without_an_interactive_session() {
        let ext = extension();
        let state = auth_state(config_with(&[("s", oauth_http("https://s/mcp"))]), false);
        let outcome = ext.authenticate_server(&state, "s", &command_ctx(false)).await;
        assert!(!outcome.ok);
        assert_eq!(outcome.message, AUTH_REQUIRES_INTERACTIVE);
    }

    /// The three configuration refusals, byte-exact and at upstream's own levels
    /// (`commands.ts:257-276`). The levels are the reason [`AuthCommandOutcome`] carries one: a
    /// disabled server is a `warning` and a missing one an `error`, and flattening them would
    /// either shout about a routine toggle or whisper a real fault.
    #[tokio::test]
    async fn authenticate_server_reproduces_upstreams_three_configuration_refusals() {
        let ext = extension();
        let disabled = crate::config::ServerEntry {
            disabled: Some(true),
            ..oauth_http("https://d/mcp")
        };
        let stdio = crate::config::ServerEntry {
            command: Some("echo".to_string()),
            ..Default::default()
        };
        let state = auth_state(
            config_with(&[("disabled", disabled), ("stdio", stdio)]),
            true,
        );
        let ctx = command_ctx(true);

        let missing = ext.authenticate_server(&state, "gone", &ctx).await;
        assert!(!missing.ok);
        assert_eq!(missing.message, "Server \"gone\" not found in config");
        assert_eq!(missing.kind, cyrup_ext::NotifyKind::Error);

        let off = ext.authenticate_server(&state, "disabled", &ctx).await;
        assert!(!off.ok);
        assert_eq!(
            off.message,
            "Server \"disabled\" is disabled. Run /mcp enable disabled, then /reload."
        );
        assert_eq!(off.kind, cyrup_ext::NotifyKind::Warning);

        let not_oauth = ext.authenticate_server(&state, "stdio", &ctx).await;
        assert!(!not_oauth.ok);
        assert_eq!(
            not_oauth.message,
            "Server \"stdio\" does not use OAuth authentication.\nSet \"auth\": \"oauth\" or omit auth for auto-detection."
        );
        assert_eq!(not_oauth.kind, cyrup_ext::NotifyKind::Error);
    }

    /// `resolveServerUrl` is INSIDE upstream's `try` (`commands.ts:279`), so a URL naming a variable
    /// the environment does not define reports as `Failed to authenticate …` rather than as the
    /// no-URL message — and it must never reach the network to find that out.
    #[tokio::test]
    async fn an_unresolvable_url_fails_before_any_network_work() {
        let ext = extension();
        let state = auth_state(
            config_with(&[(
                "s",
                oauth_http("https://${CYRUP_MCP_A_VARIABLE_NOTHING_DEFINES}/mcp"),
            )]),
            true,
        );
        let outcome = ext.authenticate_server(&state, "s", &command_ctx(true)).await;
        assert!(!outcome.ok);
        assert!(
            outcome.message.starts_with("Failed to authenticate \"s\": "),
            "{}",
            outcome.message
        );
        assert!(outcome.message.contains("CYRUP_MCP_A_VARIABLE_NOTHING_DEFINES"));
    }

    /// The bare form's three early returns (`commands.ts:611-628`, `index.ts:659`).
    #[tokio::test]
    async fn the_bare_form_refuses_a_programmatic_config_and_an_empty_candidate_set() {
        let programmatic = McpExtension::with_config(
            McpDirs::new(PathBuf::from("/nonexistent/agent"), PathBuf::from("/w")),
            Some(McpConfig::default()),
        );
        let state = auth_state(config_with(&[("s", oauth_http("https://s/mcp"))]), true);
        let ctx = command_ctx(true);
        // A real handle for the first two cases, so their refusals are shown to precede the
        // handle check rather than depending on it.
        let services: Arc<dyn cyrup_ext::host::HostServices> =
            Arc::new(ToolSetServices::default());
        let outcome =
            refusal(programmatic.pick_oauth_server(&state, &ctx, Some(&services)).await);
        assert_eq!(outcome.message, PROGRAMMATIC_CONFIG_AUTH_HINT);
        assert_eq!(outcome.kind, cyrup_ext::NotifyKind::Info);

        // A disabled OAuth server and an enabled non-OAuth one are both invisible to the picker.
        let ext = extension();
        let hidden = config_with(&[
            (
                "off",
                crate::config::ServerEntry { disabled: Some(true), ..oauth_http("https://off/mcp") },
            ),
            (
                "stdio",
                crate::config::ServerEntry {
                    command: Some("echo".to_string()),
                    ..Default::default()
                },
            ),
        ]);
        let outcome =
            refusal(ext.pick_oauth_server(&auth_state(hidden, true), &ctx, Some(&services)).await);
        assert_eq!(outcome.message, NO_OAUTH_CAPABLE_SERVERS);
        assert_eq!(outcome.kind, cyrup_ext::NotifyKind::Warning);

        // With candidates but no fenced handle to open the panel through, the refusal names the
        // form that needs no overlay.
        let outcome = refusal(
            ext.pick_oauth_server(
                &auth_state(config_with(&[("s", oauth_http("https://s/mcp"))]), false),
                &ctx,
                None,
            )
            .await,
        );
        assert_eq!(outcome.message, crate::ui::auth_panel_unavailable_message("tui"));
    }

    /// The notice a user actually sees is the whole reason `on_authorization_url` is installed: the
    /// flow's own fallback is `tracing::info!`, which in a TUI is invisible.
    #[test]
    fn the_authorization_url_notice_carries_the_url_and_the_manual_route() {
        let notice = authorization_url_notice(
            "linear",
            "https://auth.example.com/authorize?state=abc&code_challenge=xyz",
        );
        assert!(notice.starts_with("Open this URL to authenticate linear:\n\n"));
        assert!(notice.contains("https://auth.example.com/authorize?state=abc&code_challenge=xyz"));
        assert!(
            notice.contains("mcp({ action: \"auth-complete\", server: \"linear\""),
            "the remote fallback must name the route that is actually wired: {notice}"
        );
    }

    /// Both interpolated strings reach a terminal and both are built from a remote server's bytes,
    /// so both are sanitized — an OSC-8 payload in an error body must not repaint the screen.
    #[test]
    fn remote_text_is_sanitized_before_it_reaches_the_terminal() {
        let hostile = "https://as/authorize\u{1b}]8;;https://evil.invalid\u{7}";
        let notice = authorization_url_notice("s", hostile);

        // The security property, unchanged: an OSC-8 sequence injected by the authorization server
        // must not survive into the terminal. `terminal_hyperlink` sanitizes both halves BEFORE
        // building its own escape, so the smuggled target is stripped rather than re-emitted.
        assert!(!notice.contains("evil.invalid"), "{notice}");
        assert!(!notice.contains('\u{7}'), "the injected BEL is stripped: {notice}");

        // What DID change (MCP-390): the notice now carries exactly ONE OSC-8 hyperlink of its own,
        // wrapping the sanitized URL — upstream's `terminalHyperlink(url, url)`. The blanket
        // "contains no ESC" assertion this test used to make is incompatible with emitting a link
        // at all, so it is replaced by the two properties that actually matter: no injected link
        // survives, and the only escape present is the one we built.
        assert_eq!(
            notice.matches("\u{1b}]8;;").count(),
            2,
            "an OSC-8 link is an opener and a closer, and nothing else: {notice}"
        );
        assert!(notice.contains("https://as/authorize"), "{notice}");

        let failure = failed_to_authenticate("s", "boom\u{1b}[31m\u{7}");
        assert_eq!(failure, "Failed to authenticate \"s\": boom");
    }

    /// `execute_connect`'s failure arms all carry `details.message`; the reconnect report reads it
    /// from there rather than re-parsing the rendered text block.
    #[test]
    fn a_connect_failure_message_is_read_from_the_details_map() {
        let mut result = cyrup_core::ToolResult::default();
        assert_eq!(connect_failure_message(&result), None);
        result.details = Some(serde_json::json!({"mode": "connect", "message": "no route to host"}));
        assert_eq!(connect_failure_message(&result), Some("no route to host".to_string()));
    }

    /// [`AuthCommandOutcome::silent`]'s contract, on both output channels: an aborted `/mcp-auth`
    /// prints nothing at all rather than an empty notification row.
    #[test]
    fn a_silent_outcome_prints_on_neither_channel() {
        let silent = AuthCommandOutcome::silent();
        assert!(!silent.ok);
        assert_eq!(surface(None, silent.message, silent.kind).unwrap(), None);
        assert_eq!(join_lines("", ""), "");
        assert_eq!(join_lines("first", ""), "first");
        assert_eq!(join_lines("", "second"), "second");
        assert_eq!(join_lines("first", "second"), "first\nsecond");
    }

    fn input_state(owner: Arc<McpRuntimeOwner>) -> Arc<McpState> {
        use futures::FutureExt;

        let manager = Arc::new(crate::state::McpServerManager::default());
        let lifecycle = Arc::new(crate::lifecycle::McpLifecycleManager::new(
            Arc::clone(&manager),
            Arc::new(|_| false),
        ));
        Arc::new(McpState::new(crate::state::McpStateParts {
            owner,
            manager,
            lifecycle,
            config: McpConfig::default(),
            programmatic_config: None,
            oauth_runtime: crate::oauth::create_oauth_runtime(None),
            auth_storage_options: crate::credentials::AuthStorageOptions::default(),
            ui: None,
            open_browser: Arc::new(|_| async { Ok(()) }.boxed()),
            send_message: Arc::new(|_| {}),
        }))
    }
}
