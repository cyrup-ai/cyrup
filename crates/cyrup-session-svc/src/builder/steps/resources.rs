//! Steps 4a, 4b and 5 — the extension tier and the resource discovery it feeds.
//!
//! One module because the three are a single ordering constraint: the live host-services backend
//! must exist before the extension host (every wasm load is injected with it), the extension host
//! must exist before discovery (`resources_discover` contributes skill/prompt/theme paths into the
//! registry the pointers and system prompt are derived from), and discovery must run before the
//! disk-extension load (the package tier contributes extension roots).

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_ext::{ExtensionHost, HostConfig};
use cyrup_resources::{discover, DiscoveryConfig, ResourceOverrides, SkillPointer};

use super::{BuildCtx, ModelPick, ToolSurface};
use crate::builder::natives::{ext_mode, natives_to_load, SUBAGENT_CHILD_ENV};
use crate::builder::packages::{configured_packages_from_settings, load_installed_packages};
use crate::builder::{extension_discovery_roots, thinking_level_to_str, ExtensionFlagValue};
use crate::error::SessionServiceError;

/// The extension tier steps 4a + 4b stand up: the ONE `LiveHostServices` every load is injected
/// with, the host itself, pi's `hasUI` verdict, and the contained native load failures.
pub(in crate::builder) struct ExtStack {
    pub(in crate::builder) host_services: Arc<crate::host_services::LiveHostServices>,
    pub(in crate::builder) host: Arc<ExtensionHost>,
    pub(in crate::builder) has_ui: bool,
    /// EXT-S01 — per-extension `init` failures, contained here and folded into
    /// `startup_diagnostics.extensions` by [`discover_resources`].
    pub(in crate::builder) native_load_errors: Vec<crate::services::ExtensionLoadDiagnostic>,
}

/// Steps 4a + 4b — the live host-services backend and the extension host over it.
pub(in crate::builder) async fn extension_stack(
    ctx: &BuildCtx,
    tools: &ToolSurface,
    model: &ModelPick,
    natives: Vec<Arc<dyn cyrup_ext::NativeExtension>>,
) -> Result<ExtStack, SessionServiceError> {
    let BuildCtx { cfg, cwd, provider, .. } = ctx;
    let bash_proc = &tools.bash_proc;
    let resolved_model = &model.resolved;
    let model_ref = &model.model_ref;
    let thinking = model.thinking;
    // 4a. the LIVE host-services backend (arch-08 §5.6) — built BEFORE the extension host so
    // the SAME instance is injected into every wasm load (auto-discovery here + an explicit
    // `AgentSession::load_wasm_extension`) AND stored on the session. A single instance is
    // load-bearing: a loaded guest's `control` capability routes to whichever `LiveHostServices`
    // was injected at load time, and `AgentSession::apply_pending_control` drains the one on
    // `services.host_services`; if these differ the guest's `control` op is silently lost. Seed the
    // active model + wire the command-tier control channel up front so guest reads/ops are live.
    // `bash_proc` (the local process ops) + `cwd` back the `exec` capability grant (Pi
    // `execCommand`, exec.ts:34-46): a granted extension execs argv (shell:false) through the
    // SAME process backend the `bash` seam uses, defaulting to the session cwd.
    let host_services = Arc::new(crate::host_services::LiveHostServices::new(
        provider.clone(),
        bash_proc.clone(),
        cwd.clone(),
    ));
    // Only when there IS one: a guest's `getModel()` reads `Option`-shaped state
    // (`host_services.rs`), matching pi's `ctx.model` being `undefined` on a modelless session.
    if let (Some(mr), Some(m)) = (model_ref.as_ref(), resolved_model.as_ref()) {
        host_services.update_model(
            mr.clone(),
            m.context_window,
            Some(thinking_level_to_str(thinking)),
        );
    }
    host_services.wire_control_channel();

    // 4b. extension host (cyrup-ext) — built BEFORE resource discovery so the
    // `resources_discover` aggregate (extendResourcesFromExtensions, Pi agent-session.ts:2112)
    // can merge extension-contributed skill/prompt/theme paths into the registry the skill
    // pointers + system prompt are then derived from.
    let (mode, has_ui) = ext_mode(cfg.app_mode);
    let host_config = HostConfig { mode, has_ui, cwd: cwd.clone() };
    // With `wasm-host`, spin up the Wasmtime engine so live wasm extensions can be loaded with
    // `LiveHostServices` injected (the seam below); otherwise a native-only host (the default).
    #[cfg(feature = "wasm-host")]
    let host = ExtensionHost::with_wasm(host_config)?;
    #[cfg(not(feature = "wasm-host"))]
    let host = ExtensionHost::new(host_config);
    // Attach the live `getActiveTools` source BEFORE any tool set is materialized, so every tool
    // `active_tools` hands back is wrapped for `addedToolNames` derivation (Pi
    // `wrapRegisteredTool`, extensions/wrapper.ts:17-36). The source reads the SAME
    // `DynamicToolState` `setActiveTools` mutates, so a tool that widens the active set during its
    // own `execute` is observed by the wrapper's "after" snapshot. It reads `None` until
    // `attach_dynamic_tools` runs further down, which is correct: nothing executes before then.
    host.set_active_tool_source(host_services.clone());
    // PERM-011 half B: hand the backend the host's ONE `SharedBus`, so a NATIVE extension's
    // `HostServices::emit_event` lands in the same queue a WASM guest's `bus.emit` does and is
    // fanned out by the same drain. Done here because the ordering is forced — the host takes
    // this backend as an argument, so the bus cannot exist at `LiveHostServices::new` — and
    // BEFORE the native load loop below, so no extension can emit into an unattached bus.
    host_services.attach_event_bus(Arc::clone(host.bus()));
    // P-1 (reconciliation §2 item 1): late-bind the session's OWN `host_services` into every
    // native built-in — the SAME `LiveHostServices` the WASM path gets via `discover_and_load`
    // below — so a native extension can reach the live session id/file, dialogs, and
    // message-injection from a background task OUTSIDE any `HostCtx`. `load_native_with_services`
    // calls `NativeExtension::set_host_services` before `init`; the manager / ui sink / inject sink
    // attach later (steps 6/10 + the mode entry point) and the captured `Arc` observes them.
    let native_services: Arc<dyn cyrup_ext::host::HostServices> = host_services.clone();
    // EXT-S01: CONTAIN a native extension's load/`init` failure. This loop used to propagate the
    // first error with a bare `?`, so one built-in (permission-system, intercom, subagents)
    // failing `init` took the ENTIRE session down — no session at all, and the remaining natives
    // were never even attempted. Pi records a per-extension load failure and keeps building
    // (`LoadExtensionsResult.errors`, surfaced as `Failed to load extension "<path>": <err>`,
    // main.ts:735-738). Collected here and folded into `startup_diagnostics.extensions` below
    // (the same channel the wasm/disk path at step 4c already uses), so a contained failure
    // reaches BOTH the `[Extension issues]` startup panel AND — because it is marked `fatal` —
    // `AgentSessionRuntime::diagnostics()`, where the bin reports it on stderr and exits 1 in
    // every mode (Pi main.ts:843-849). Containment is per-extension, NOT forgiveness: Pi keeps
    // building past the failure and then refuses to run. The natives are cyrup's security
    // built-ins (permission-system, intercom), so anything short of a non-zero exit would turn
    // a failed permission gate into a fail-OPEN session.
    let mut native_load_errors: Vec<crate::services::ExtensionLoadDiagnostic> = Vec::new();
    // SEAM-071: `--no-extensions` gates the AMBIENT natives too. It used to gate only the
    // WASM/disk discovery roots (`extension_discovery_roots`), while this loop loaded every
    // native unconditionally — so `cyrup --no-extensions` still started an intercom broker,
    // which is how the suite accumulated 13 immortal broker processes per run. Upstream, the
    // analogs of these three are installed packages in `resolvedPaths.extensions`, and
    // `noExtensions` reduces THAT tier to the explicit `-e` paths alone
    // (`resource-loader.ts:451-452`, `:555-557` @v0.83.0). It does not touch pi's inline
    // `extensionFactories` tier, and neither does this — see `native_survives_no_extensions`.
    let is_subagent_child = std::env::var_os(SUBAGENT_CHILD_ENV).is_some();
    for ext in natives_to_load(natives, cfg.no_extensions, is_subagent_child) {
        let id = ext.id();
        if let Err(e) = host.load_native_with_services(ext, native_services.clone()).await {
            tracing::error!(extension = %id, error = %e, "native extension failed to load");
            native_load_errors.push(crate::services::ExtensionLoadDiagnostic {
                // A native built-in has no on-disk path; its id is the display key the panel
                // shows (Pi's per-extension diagnostics are keyed by the loader's path).
                path: PathBuf::from(id.as_str()),
                error: e.to_string(),
                fatal: true,
            });
        }
    }
    let ext_host = Arc::new(host);

    Ok(ExtStack { host_services, host: ext_host, has_ui, native_load_errors })
}

/// What step 5 hands back: the merged resource registry, the startup diagnostics accumulated across
/// discovery + both extension tiers + the model files, the two model-catalog layers, the guest
/// provider sink, and the read-gated skill pointers the system prompt is derived from.
pub(in crate::builder) struct Resources {
    pub(in crate::builder) resources: Arc<cyrup_resources::ResourceRegistry>,
    pub(in crate::builder) startup_diagnostics: crate::services::StartupDiagnostics,
    pub(in crate::builder) model_config: Arc<cyrup_config::ModelFile>,
    pub(in crate::builder) catalog_overlay: Option<Arc<cyrup_provider::CatalogOverlay>>,
    pub(in crate::builder) guest_providers: Arc<crate::guest_providers::GuestProviderRegistry>,
    pub(in crate::builder) skills: Vec<SkillPointer>,
}

/// Step 5 — resources discovery (cyrup-resources), the disk-extension load, and the model-catalog
/// layers.
pub(in crate::builder) async fn discover_resources(
    ctx: &BuildCtx,
    ext: &ExtStack,
    tools: &ToolSurface,
    skills_override: Option<crate::builder::SkillsOverrideFn>,
) -> Result<Resources, SessionServiceError> {
    let BuildCtx { cfg, cwd, cancel, settings, .. } = ctx;
    let trusted = ctx.trusted;
    let ext_host = &ext.host;
    let read_available = tools.read_available;
    // 5. resources discovery (cyrup-resources) — RUN FIRST (before disk-extension load) so
    // the package-declared extension dirs discovery collects (`registry.ext_crate_paths`) can be
    // folded into the extension discovery roots below, matching Pi's `resolve()` producing
    // `resolvedPaths.extensions` (the package tier) which is then merged into the loaded
    // extension set (resource-loader.ts:379,403-407). Discovery is a pure fs pass with no
    // dependency on the not-yet-loaded disk extensions; the extension-*contributed* resources are
    // folded in AFTER the load via `aggregate_resources` (unchanged, below).
    let mut disc = DiscoveryConfig::new(cwd.clone(), cfg.agent_dir.clone());
    // R6: plumb the user-tier cross-tool `~/.agents` base (Pi `getHomeDir()/.agents`,
    // package-manager.ts:2286,217) so cyrup-resources loads `~/.agents/skills` (user scope) and
    // dedups the project `.agents/skills` ancestor walk against it.
    disc.user_agents_dir = Some(cfg.home.join(".agents"));
    disc.trusted_project = trusted;
    // C1 (gap-07 #1 / gap-13 C1): read the on-disk install registry back into discovery so an
    // installed package's skills/prompts/themes actually load into the assembled session. Pi's
    // `PackageManager.resolve()` re-reads `projectSettings.packages`/`globalSettings.packages`
    // from the settings store on EVERY call (package-manager.ts:880-897), so an installed package
    // is structurally impossible to forget; cyrup persists installs to a SEPARATE file-backed
    // `packages.json` store, so the builder must take the explicit read step the bin's `install`
    // write mirrors (`PackageStore::new(dirs.package_dir, Some(dirs.cwd))`, subcommands.rs:396).
    // `project_root` + `package_global_dir` are the SAME store roots `install` writes to, so
    // `installed_dir` resolves each record's working tree at the exact on-disk path `install`
    // created (Global at `<package_dir>/packages/<id>`, Project at `<cwd>/.cyrup/packages/<id>`).
    disc.project_root = Some(cwd.clone());
    disc.package_global_dir = cfg.package_dir.clone();
    disc.installed = load_installed_packages(&cfg.package_dir, cwd);
    disc.enable_skills = !cfg.no_skills;
    disc.enable_prompts = !cfg.no_prompt_templates;
    disc.enable_themes = !cfg.no_themes;
    // Settings-tier resource overrides (cross-layer wiring; Pi `package-manager.ts:2265-2278`):
    // the `skills`/`prompts`/`themes` settings lists are enable/disable patterns over the
    // auto-discovered loose resources. The layered `SettingsManager` exposes the per-layer split
    // (Pi `globalSettings`/`projectSettings`, settings-manager.ts:455-470), so global-scope
    // discovery is gated by the GLOBAL layer's lists and project-scope by the PROJECT layer's —
    // not the merged effective view (which would let a project list silently widen the global
    // scope, or vice-versa). Empty lists — the default — preserve "discover everything".
    //
    // The SAME arrays also carry Pi's positive (plain-path) listings, which `resolveLocalEntries`
    // LOADS at the settings tier (package-manager.ts:905-931, :2255-2276) — including the
    // `extensions` array, the first member of Pi's `RESOURCE_TYPES` (:194). cyrup had shipped the
    // filter half only for `extensions`, so a settings-declared extension root was inert (CFG-004).
    disc.global_overrides = ResourceOverrides {
        skills: settings.global().skill_paths(),
        prompts: settings.global().prompt_template_paths(),
        themes: settings.global().theme_paths(),
        extensions: settings.global().extension_paths(),
    };
    disc.project_overrides = ResourceOverrides {
        skills: settings.project().skill_paths(),
        prompts: settings.project().prompt_template_paths(),
        themes: settings.project().theme_paths(),
        extensions: settings.project().extension_paths(),
    };
    // CFG-003: `settings.packages` is Pi's ONLY package channel — `PackageManager.resolve()`
    // re-collects `projectSettings.packages` then `globalSettings.packages` on every call and
    // resolves each entry to a working tree (package-manager.ts:891-901). cyrup read only its own
    // `packages.json` install registry, so a package DECLARED in settings contributed nothing.
    // Project entries are pushed first so they win the shared package precedence rank (:887-893).
    let (configured_packages, package_errors) = configured_packages_from_settings(settings, cwd, &cfg.agent_dir);
    disc.configured_packages = configured_packages;
    // CFG-003: pi's session path calls `packageManager.resolve()` with NO `onMissing`
    // (resource-loader.ts:403 and :549 @v0.83.0), so a declared git package with no working tree
    // is CLONED during assembly unless `isOfflineModeEnabled()` (package-manager.ts:1260-1271).
    // The flag is threaded rather than read here because `cyrup-resources` performs no env
    // lookups and this crate has no `NetworkPolicy`; the bin resolves `--offline`/`CYRUP_OFFLINE`
    // /`PI_OFFLINE` and sets it (`cyrup/src/main.rs`).
    disc.install_missing_packages = cfg.install_missing_packages;
    let report = discover(&disc, cancel.token()).await?;
    // TUI-006: the discovery pass's structured diagnostics (shadowed same-name skills, a
    // configured path that does not exist, a malformed frontmatter) used to be dropped on the
    // floor here. Pi shows them at startup even under `quietStartup`
    // (`showDiagnosticsWhenQuiet: true`, interactive-mode.ts:1769), so they now travel on
    // `AgentSessionServices::startup_diagnostics` for the front-end to render.
    let mut startup_diagnostics = crate::services::StartupDiagnostics {
        resources: report.diagnostics.clone(),
        // EXT-S01: the native built-ins that failed to load at step 4b, contained above.
        extensions: ext.native_load_errors.clone(),
        ..Default::default()
    };
    // A malformed `packages` entry never takes the settings document (or the session) down; it
    // is reported alongside the discovery diagnostics.
    for message in package_errors {
        startup_diagnostics.resources.push(cyrup_resources::ResourceDiagnostic::error(
            cyrup_resources::ResourceKind::Package,
            cfg.agent_dir.join("settings.json"),
            message,
        ));
    }

    // CFG-002: `<agent_dir>/models.json` — the user's custom-provider / custom-model file. Pi
    // loads it ONCE per runtime (`ModelConfig.load(join(getAgentDir(),"models.json"))`,
    // model-runtime.ts:137-139) and composes it over the built-in provider catalogs
    // (`composeModelProvider`, provider-composer.ts:411-437). cyrup had the reader
    // (`load_models_file`) and the path (`ConfigDirs::models_path`) but NO production caller, so
    // the entire custom-provider surface was dead. A malformed file is reported and skipped —
    // never fatal, never a panic (Pi keeps an empty snapshot + one error string,
    // model-config.ts:248-271).
    let (model_file, model_file_error) =
        cyrup_config::load_models_file_reporting(&cfg.agent_dir.join("models.json"));
    startup_diagnostics.models.extend(model_file_error);
    // The persisted pi.dev catalog overlay (DRIFT-007), loaded from disk ONLY. This is the
    // cache-only restore Pi performs at `agent-session-services.ts:180`
    // (`refresh({ allowNetwork: false })`): a session build must never block on a network call,
    // and an offline run must still see the catalogs it saw last time. A refresh that ADDS to
    // this cache is the running mode's fire-and-forget job (Pi `main.ts:863-866`).
    let catalog_overlay = load_persisted_catalog_overlay(&cfg.agent_dir).await;
    // Surface composition errors (a provider block Pi would `throw` on) once, at startup, rather
    // than on every catalog read.
    {
        let base = cyrup_provider::default_models(cyrup_provider::CreateModelsOptions {
            credentials: None,
            auth_context: None,
            catalog_overlay: catalog_overlay.clone(),
        })
        .get_models(None);
        let (_, errors) = model_file.compose(&base);
        startup_diagnostics.models.extend(errors);
    }
    let model_config = Arc::new(model_file);

    // Resolve the on-disk extension discovery roots from `--extension`/`--no-extensions` (Pi
    // `resourceLoaderOptions.additionalExtensionPaths`/`noExtensions`, main.ts:660,664), then
    // fold in the package-declared extension dirs discovery just collected (gap-07 #2: Pi merges
    // the package tier's `resolvedPaths.extensions` into the loaded set, resource-loader.ts:
    // 379,403-407 `mergePaths(cliEnabledExtensions, enabledExtensions)`). `configured` is the
    // pre-trust configured-extension tier — the same shape package extension dirs enter — so
    // appending them here makes an installed package's extension load alongside the
    // project/global/CLI roots. The live wasm *instantiation* of each discovered extension runs
    // only under the `wasm-host` feature (the Wasmtime engine + the `wasm32-wasip2` guest
    // toolchain — the gated arch-08b live-wasm tail, residual ledger §09 #13). Native built-ins
    // are already loaded above.
    let mut ext_roots = extension_discovery_roots(cfg);
    // SEAM-071, second half: the package tier is pi's `enabledExtensions`, the exact operand
    // `noExtensions` drops (`extensionPaths = this.noExtensions ? cliEnabledExtensions :
    // this.mergePaths(cliEnabledExtensions, enabledExtensions)`, resource-loader.ts:451-452
    // @v0.83.0). Appending it into `configured` unconditionally re-admitted every installed
    // package's extension through the one tier `--no-extensions` is defined to keep, so the flag
    // silently did not mean what it says. `cfg.extra_extension_paths` — the real `-e` tier — is
    // still merged by `extension_discovery_roots` either way.
    if !cfg.no_extensions {
        ext_roots.configured.extend(report.registry.ext_crate_paths.iter().cloned());
    }
    #[cfg(feature = "wasm-host")]
    {
        // Inject the session's OWN `host_services` (built at 4a) so a disk-discovered guest's
        // `control` capability reaches the same queue `apply_pending_control` drains.
        let host_services_for_load: Arc<dyn cyrup_ext::host::HostServices> = ext.host_services.clone();
        // The per-path `errors` (Pi `LoadExtensionsResult.errors` → "Failed to load extension"
        // diagnostics, main.ts:679-682) are retained on `startup_diagnostics` so the TUI can
        // render Pi's `[Extension issues]` block (TUI-006) instead of dropping them here. Each
        // carries its `fatal` flag through unchanged, so a genuine load fault also reaches the
        // bin's exit-1 checkpoint while the project-trust skip does not (`LoadError::fatal`).
        let load_result =
            ext_host.discover_and_load(&ext_roots, trusted, host_services_for_load).await;
        startup_diagnostics.extensions.extend(load_result.errors.iter().map(|e| {
            crate::services::ExtensionLoadDiagnostic {
                path: e.path.clone(),
                error: e.error.clone(),
                fatal: e.fatal,
            }
        }));
    }
    #[cfg(not(feature = "wasm-host"))]
    let _ = &ext_roots;

    // Apply the CLI-captured extension flag overrides now that every loaded extension's
    // `registerFlag` has run (Pi runs `applyExtensionFlagValues` inside
    // `createAgentSessionServices`, agent-session-services.ts:167 — AFTER the extensions load).
    // Without this step the 1:1-ported CLI capture (`cfg.extension_flag_values`, from the bin's
    // `partition_extension_flags` / Pi `unknownFlags`) is dropped one call short of the
    // guest-visible `getFlag` (gap-08 §5.6). The ext-host resolves each value against the
    // registered flag's declared type and stores it in the shared flag store `getFlag` consults.
    if !cfg.extension_flag_values.is_empty() {
        let overrides: Vec<(String, cyrup_ext::ExtensionFlagOverride)> = cfg
            .extension_flag_values
            .iter()
            .map(|(name, v)| {
                let ov = match v {
                    ExtensionFlagValue::Bool(b) => cyrup_ext::ExtensionFlagOverride::Bool(*b),
                    ExtensionFlagValue::Str(s) => {
                        cyrup_ext::ExtensionFlagOverride::Str(s.clone())
                    }
                };
                (name.clone(), ov)
            })
            .collect();
        // SEAM-S01: the reconciliation diagnostics — `Unknown option(s): --foo` and
        // `Extension flag "--foo" requires a value` (Pi agent-session-services.ts:98-125) — are
        // retained here. They used to be `continue`d away inside the ext-host, so a mistyped
        // `--flag` produced no message and no non-zero exit. Pi merges them into
        // `services.diagnostics` (:182), which becomes `runtime.diagnostics` and is reported +
        // `process.exit(1)`-ed at main.ts:843-848.
        startup_diagnostics.flags.extend(ext_host.apply_extension_flag_values(&overrides)?);
    }

    // Bind the shared model-registry sink and FLUSH any provider registrations queued while native
    // + disk extensions loaded (Pi `runner.bindCore` pending-flush, runner.ts:345-362). The SAME
    // `Arc` is the `ext_host` sink (future `registerProvider`s upsert live) and the session's read
    // view (its catalog is UNIONed into the model registry, and its provider installed on select).
    let guest_providers = Arc::new(crate::guest_providers::GuestProviderRegistry::new());
    ext_host.registry().bind_model_registry(guest_providers.clone())?;
    // extendResourcesFromExtensions("startup") (Pi agent-session.ts:2109-2135): fold every
    // `resources_discover` handler's contributed skill/prompt/theme paths into the registry
    // BEFORE the skill pointers + system prompt are derived. An empty aggregate (no handlers, or
    // nothing contributed) leaves the discovered registry untouched (Pi's early returns at
    // :2118/:2124).
    let resources = {
        let agg = ext_host.aggregate_resources(&cancel.token()).await;
        // Fold BOTH the extension-contributed paths AND the explicit CLI `--skill`/
        // `--prompt-template`/`--theme` paths (Pi `additionalSkillPaths` et al.) into the
        // discovered registry before skill-pointer + system-prompt derivation. An empty aggregate
        // (no handlers, no CLI paths) leaves the discovered registry untouched.
        // The aggregate now attributes each path to its extension (gap-08 #15); for registry
        // discovery we take the path strings in concatenated load order.
        let mut skill_paths: Vec<PathBuf> =
            agg.skill_paths.iter().map(|p| PathBuf::from(&p.path)).collect();
        let mut prompt_paths: Vec<PathBuf> =
            agg.prompt_paths.iter().map(|p| PathBuf::from(&p.path)).collect();
        let mut theme_paths: Vec<PathBuf> =
            agg.theme_paths.iter().map(|p| PathBuf::from(&p.path)).collect();
        skill_paths.extend(cfg.extra_skill_paths.iter().cloned());
        prompt_paths.extend(cfg.extra_prompt_paths.iter().cloned());
        theme_paths.extend(cfg.extra_theme_paths.iter().cloned());
        if skill_paths.is_empty() && prompt_paths.is_empty() && theme_paths.is_empty() {
            report.registry
        } else {
            let extra =
                cyrup_resources::DiscoveredPaths { skill_paths, prompt_paths, theme_paths };
            report.registry.extend(&extra)
        }
    };
    let resources = Arc::new(resources);
    // Read-gated skill pointers (R-06-010): only when the `read` tool is available.
    let mut skills: Vec<SkillPointer> = if read_available && !cfg.no_skills {
        resources.skills.winners().map(|s| s.pointer()).collect()
    } else {
        Vec::new()
    };
    // Synthetic-skill injection (Pi `skillsOverride`, resource-loader.ts:630): transform the
    // discovered pointer set before it feeds the context snapshot + system prompt. Applied to the
    // (possibly-empty) base so an embedder can inject skills discovery found none of; the emit is
    // still `read`-gated downstream (skills_inject.rs), matching Pi.
    if let Some(f) = skills_override {
        skills = f(skills);
    }

    Ok(Resources {
        resources,
        startup_diagnostics,
        model_config,
        catalog_overlay,
        guest_providers,
        skills,
    })
}

/// Load `<agent_dir>/models-store.json` as a model-catalog overlay, WITHOUT any network access
/// (DRIFT-007).
///
/// Infallible and disk-only. A missing/corrupt cache, an overlay no newer than the compiled-in
/// catalogs (the post-upgrade case, pi #7016), or an entry that mislabels its provider all yield
/// `None`, which is byte-identical to the pre-DRIFT-007 behavior. It can never remove a built-in
/// model, so a session built from a broken cache is never worse off than one built from none.
async fn load_persisted_catalog_overlay(
    agent_dir: &std::path::Path,
) -> Option<Arc<cyrup_provider::CatalogOverlay>> {
    let store: Arc<dyn cyrup_provider::ModelsStore> = Arc::new(
        cyrup_config::models_store::FileModelsStore::new(
            agent_dir.join(cyrup_config::models_store::MODELS_STORE_FILE_NAME),
        ),
    );
    let catalog = cyrup_provider::RemoteCatalog::new(store)
        .with_local_generated_at(cyrup_provider::builtin_model_data_generated_at());
    let ids: Vec<String> = cyrup_provider::all_providers()
        .iter()
        .map(|p| p.id().as_str().to_string())
        .collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let overlay = catalog.load_overlay(&refs).await;
    (!overlay.is_empty()).then(|| Arc::new(overlay))
}
