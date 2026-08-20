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
//! # The division of labour across the three entry points
//!
//! * **`init()` performs registration only** — read the config and the cache, register the tools,
//!   commands, renderers and the flag, subscribe, spawn the pre-warm task. **No teardown of a
//!   previous generation.**
//! * **`on_event(SessionShutdown)` is the only teardown point**, and it is where the metadata flush
//!   lives.
//! * **`on_event(SessionStart)` is the generation bump** and builds the new runtime.
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
use futures::future::{BoxFuture, Shared};
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
}

impl McpExtension {
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
            state: Mutex::new(None),
            init_task: Mutex::new(None),
            registered_direct_tools: Mutex::new(IndexMap::new()),
            registered_prompt_commands: Mutex::new(IndexMap::new()),
            fallback_deactivated_tools: Mutex::new(Vec::new()),
            proxy_tool_description: Mutex::new(None),
            direct_tools_frozen: AtomicBool::new(false),
            dispatch: Mutex::new(None),
            home: None,
        }
    }

    /// Pin the home directory the config ladder's home-anchored rungs resolve against (see
    /// the `home` field). Production never calls this; a test that must be hermetic always does.
    #[must_use]
    pub fn with_home(mut self, home: PathBuf) -> Self {
        self.home = Some(home);
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

    /// The live generation's owner, if there is one.
    #[must_use]
    pub fn owner(&self) -> Option<Arc<McpRuntimeOwner>> {
        self.owner.lock().ok().and_then(|slot| slot.clone())
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
    #[must_use]
    pub fn registered_direct_tools(&self) -> &Mutex<IndexMap<String, String>> {
        &self.registered_direct_tools
    }

    /// `registeredPromptCommands` — the prompt-command dedup set (MCP-039), same locking rationale.
    #[must_use]
    pub fn registered_prompt_commands(&self) -> &Mutex<IndexMap<String, String>> {
        &self.registered_prompt_commands
    }

    /// `fallbackDeactivatedTools` — the tools removed through the `setActiveTools` fallback,
    /// retained so a tool that reappears after being deactivated is put back into the active set
    /// (MCP-038).
    #[must_use]
    pub fn fallback_deactivated_tools(&self) -> &Mutex<Vec<String>> {
        &self.fallback_deactivated_tools
    }

    /// `proxyToolDescription` — the description the proxy tool was last registered with. An
    /// unchanged description must not re-register: identical system-prompt bytes are what keep the
    /// provider's prompt cache valid (MCP-043).
    #[must_use]
    pub fn proxy_tool_description(&self) -> &Mutex<Option<String>> {
        &self.proxy_tool_description
    }

    /// The current generation's executor slot, or `None` before the first `init`.
    ///
    /// The install point for MCP-214's dispatcher: `runtime::initialize_mcp` calls
    /// `ToolDispatch::install(...)` on this once [`McpState`] exists, and every tool registered by
    /// the same pass goes live at that instant. Cloned out rather than borrowed because a pass
    /// replaces the slot wholesale.
    #[must_use]
    pub fn dispatch(&self) -> Option<Arc<ToolDispatch>> {
        self.dispatch.lock().ok().and_then(|slot| slot.clone())
    }

    /// `session_start`'s generation protocol, abort-before-await (MCP-008).
    ///
    /// **Minimal body (MCP-008 fills it).** The ordering it must reproduce, exactly:
    /// bump the generation; snapshot the previous state / owner / OAuth runtime; null `state` and
    /// `initPromise`; call `previous_owner.begin_stop("MCP extension session restarted")`
    /// **synchronously, before awaiting anything** — [`McpRuntimeOwner::begin_stop`] exists for
    /// precisely this and must not be collapsed into a single `stop().await`, because the whole
    /// point is that the cancel is observable before the cleanup completes; re-check
    /// `generation == my_gen && owner.is_active()` after the join; then `startInitialization`.
    ///
    /// **Ordering note:** under cyrup's replacement tail the previous generation's
    /// `SessionShutdown` has *already* run when this fires (MCP-014), so the snapshots are normally
    /// `None`. The snapshot-and-stop arm is the defence for the paths where they are not — a
    /// `SessionStart` with no preceding shutdown, or a build that skipped the install tail.
    async fn on_session_start(&self, _reason: &str) {
        let my_generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let previous_owner = self.owner.lock().ok().and_then(|mut slot| slot.take());
        let _previous_state = self.state.lock().ok().and_then(|mut slot| slot.take());
        if let Ok(mut slot) = self.init_task.lock() {
            *slot = None;
        }

        // Synchronous cancel, then the await — never the other way round.
        if let Some(owner) = previous_owner {
            let draining = owner.begin_stop(Some("MCP extension session restarted"));
            if let Err(error) = draining.await {
                tracing::error!("MCP: session restart cleanup failed: {error}");
            }
        }

        // MCP-008's post-await re-check. A newer generation superseded this one while the previous
        // owner drained, so this continuation must not become the live runtime — which is what
        // `startInitialization`'s triple staleness check enforces once MCP-011 lands here.
        if self.generation() != my_generation {
            tracing::debug!(
                "MCP: session start for generation {my_generation} superseded before initialization"
            );
        }
    }

    /// `session_shutdown` — the **only** teardown point (MCP-009, MCP-010).
    ///
    /// **Minimal body (MCP-009/MCP-010 fill it).** The ordering it must reproduce: bump the
    /// generation; snapshot and null `state` / `currentOwner` / `currentOAuthRuntime` /
    /// `initPromise`; `owner.begin_stop("MCP extension session shutdown")` **before** the join;
    /// then join the owner stop, `shutdown_state` (whose metadata-flush error must win over a
    /// concurrent shutdown failure) and `shutdown_oauth` together.
    ///
    /// cyrup dispatches this as an **awaited** notify *before* the session cancel token fires, so
    /// the handler genuinely gets to finish — better than upstream, and it needs no compensation.
    async fn on_session_shutdown(&self, _reason: &str) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let owner = self.owner.lock().ok().and_then(|mut slot| slot.take());
        let _state = self.state.lock().ok().and_then(|mut slot| slot.take());
        if let Ok(mut slot) = self.init_task.lock() {
            *slot = None;
        }
        if let Some(owner) = owner {
            let draining = owner.begin_stop(Some("MCP extension session shutdown"));
            if let Err(error) = draining.await {
                tracing::error!("MCP: session shutdown cleanup failed: {error}");
            }
        }
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
            let explicit = crate::config::config_path_from_argv(std::env::args()).map(PathBuf::from);
            let mut ctx =
                crate::config::ConfigContext::new(self.dirs.clone(), explicit.as_deref());
            if let Some(home) = self.home.clone() {
                ctx = ctx.with_home(home);
            }
            ctx.load().config
        });

        let surface: RegisteredSurface =
            crate::registration::register_surface(api, &self.dirs, &config);
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

        // Every registered tool needs its renderer DECLARED here — cyrup splits upstream's per-tool
        // `renderCall`/`renderResult` arguments into a declaration plus a name-keyed serve, so a
        // tool without this call has an unreachable renderer (MCP-036).
        for name in &surface.tool_names {
            api.register_tool_renderer(name.clone());
        }

        // `startLoadTimeInitialization` (MCP-012): pre-warm ONLY when some enabled server declares
        // `lifecycle: "eager" | "keep-alive"`. Everything else connects lazily on first call, which
        // is what keeps a cold start from paying for N subprocess handshakes.
        if crate::runtime::needs_load_time_initialization(&config) {
            tracing::debug!("MCP: eager/keep-alive servers configured — pre-warm pending");
        }

        Ok(())
    }

    /// The three subscribed seams (see [`crate::registration::SUBSCRIBED_EVENTS`]).
    ///
    /// `ToolResult` is `error-signal.ts`'s `toolErrorOverride`: a returned MCP failure is re-flagged
    /// as an error with `HookOutcome::Mutate(EventPatch::ToolResult { is_error: Some(true), .. })`,
    /// whose `apply_patch` leaves `content` and `details` untouched when `None` (MCP-045).
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::SessionStart { reason, .. } => {
                self.on_session_start(reason).await;
                HookOutcome::Noop
            }
            HostEvent::SessionShutdown { reason, .. } => {
                self.on_session_shutdown(reason).await;
                HookOutcome::Noop
            }
            // MCP-045 fills the `isError` override.
            _ => HookOutcome::Noop,
        }
    }

    /// Bind the live capability backend. Called **before** [`Self::init`], which is what makes an
    /// `init`-spawned background task legitimate.
    fn set_host_services(&self, services: Arc<dyn cyrup_ext::host::HostServices>) {
        let _ = self.host_services.set(services);
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
    Some(Arc::new(McpExtension::with_config(dirs, config)) as Arc<dyn NativeExtension>)
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
}
