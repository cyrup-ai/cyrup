//! Process and configuration bootstrap: everything that has to be set up **before** a session,
//! a provider or a runtime exists.
//!
//! These are the phases pi runs between `main()`'s first line and `createAgentSessionRuntime`:
//! stderr logging, the bootstrap HTTP proxy, directory resolution, the startup settings manager and
//! its diagnostics, one-time migrations' companion first-time-setup wizard, the `models.json` load,
//! and the two halves of the runtime model-catalog overlay.
//!
//! Each function here is a self-contained phase. **Their ORDER is not encoded here** — it is
//! load-bearing pi parity (PROV-047 above every egress path, the `sessionDir` tier chain after the
//! startup manager, DRIFT-007's two phases straddling the `--list-models` exit) and it stays
//! visible as one readable sequence in `main.rs`, where each call site carries the pi `main.ts`
//! line it corresponds to.

use std::sync::Arc;

use cyrup_config::{
    AuthStore, CliConfigOverrides, ConfigDirs, EnvVars, ModelFile, SettingsManager, SettingsScope,
    SettingsStore,
};
use cyrup_session_svc::{AppMode, SessionConfig};
use cyrup_tui::{StdinTerminalProbe, UiTheme};

use crate::cli::Cli;
use crate::diagnostics::Diagnostic;
use crate::session_resolve::is_fresh_target;
use crate::startup::file_settings_store;

/// PROV-047 — the BOOTSTRAP `httpProxy` install, Pi main.ts:536-538 @v0.83.0:
///
/// ```ts
/// const bootstrapSettingsManager = SettingsManager.create(cwd, agentDir, { projectTrusted: false });
/// applyHttpProxySettings(bootstrapSettingsManager.getGlobalSettings().httpProxy);
/// configureHttpDispatcher();
/// ```
///
/// It sits HERE — above the package/config subcommand pre-dispatch (pi `handlePackageCommand`,
/// main.ts:541), above the credential-print pre-dispatch (`runCredentialPrintCommand`, :557) and
/// above `parseArgs` (:562) — because every one of those can egress before a session exists:
/// `cyrup auth check` / `print-bearer-token` REFRESHES an expired OAuth credential by default
/// (`credential_print.rs`), and `restore_model_catalog` / the catalog revalidation and update
/// check all run upstream of `SessionBuilder::build`. Until this call landed, the only
/// `configure_http_proxy` in the process was the one in `cyrup-session-svc/src/builder.rs:1462`
/// (pi's SECOND call, main.ts:801), so a user whose network requires `httpProxy` got a working
/// chat and a silently-direct login — the exact split PROV-047 names.
///
/// `projectTrusted: false` is pi's, verbatim, and is why the store is loaded untrusted here; the
/// key is `GLOBAL_ONLY` besides (CFG-057). The accessor installs the SETTING alone — pi's `??=`
/// gives an ambient `HTTP_PROXY`/`HTTPS_PROXY` precedence, and that precedence lives in the
/// resolver (`node_http_proxy::get_proxy_env`, which consults `configured_http_proxy` only after
/// all four ambient lookups miss). CFG-060 deleted the `EnvVars` argument this call used to pass
/// as `EnvVars::default()`: the accessor's env fallback was dead here and would have inverted the
/// precedence for any caller that passed a real environment.
///
/// pi's paired `configureHttpDispatcher()` with no argument installs `DEFAULT_HTTP_IDLE_TIMEOUT_MS`
/// — which is already the initial value of cyrup's process-global
/// (`cyrup_provider::stream::sse::HTTP_IDLE_TIMEOUT_MS`), so there is no second call to make: the
/// settings-driven `configure_http_idle_timeout` at builder.rs:1475 is pi's `:802`.
pub fn install_bootstrap_http_proxy() {
    let env = EnvVars::from_process();
    if let Ok(dirs) = ConfigDirs::resolve(&CliConfigOverrides::default(), &env) {
        let bootstrap = SettingsManager::load(file_settings_store(&dirs), false);
        cyrup_provider::configure_http_proxy(bootstrap.effective().http_proxy());
    }
}

/// Resolve the config directories and the CLI override set they were resolved from (CLI > env >
/// default; the only place the environment is read). `--session-dir`, `--offline`, `--api-key` and
/// `--model(s)` thread through [`CliConfigOverrides`].
///
/// The overrides are returned alongside the dirs because two later phases need them verbatim: the
/// DRIFT-007 catalog refresh and the startup update check both resolve a
/// [`cyrup_config::policy::NetworkPolicy`] from `(settings, env, overrides)`.
pub fn resolve_dirs(cli: &Cli, env: &EnvVars) -> anyhow::Result<(CliConfigOverrides, ConfigDirs)> {
    use anyhow::Context;
    let overrides = CliConfigOverrides {
        session_dir: cli.session_dir.clone(),
        offline: cli.offline || env.offline,
        trust_override: cli.trust_override(),
        model: cli.model.clone(),
        models: cli.models.clone(),
        api_key: cli.api_key.clone(),
        ..Default::default()
    };
    let dirs = ConfigDirs::resolve(&overrides, env).context("resolving config directories")?;
    Ok((overrides, dirs))
}

/// Pi's `startupSettingsManager` (main.ts:610-611), created after the migrations and used for
/// exactly two things: surfacing settings load/parse errors as warnings, and the `sessionDir`
/// lookup. One manager, both jobs — as upstream. The caller reports the diagnostics.
///
/// `project_trusted: false` is cyrup's standing pre-trust posture (R-07-002). Pi's startup manager
/// defaults to `projectTrusted: true` (settings-manager.ts:320), so an UNTRUSTED project's
/// `.cyrup/settings.json` cannot relocate the session dir under cyrup; the global
/// `<agent_dir>/settings.json` tier — the documented one — behaves exactly as upstream.
pub fn load_startup_settings(dirs: &ConfigDirs) -> (SettingsManager, Vec<Diagnostic>) {
    let mut mgr = SettingsManager::load(file_settings_store(dirs), false);
    let diagnostics = collect_settings_diagnostics(&mut mgr, "startup session lookup");
    (mgr, diagnostics)
}

/// Drain settings load/parse errors into warning diagnostics (Pi `collectSettingsDiagnostics`,
/// main.ts:77-85): `(<context>, <scope> settings) <message>`. Takes the caller's manager rather than
/// building a throwaway one, because Pi passes the *same* `startupSettingsManager` it then queries
/// for `sessionDir` (main.ts:610-611, 629) — draining a second, independent manager's errors would
/// leave the live one still holding them.
fn collect_settings_diagnostics(
    mgr: &mut cyrup_config::SettingsManager,
    context: &str,
) -> Vec<Diagnostic> {
    mgr.drain_load_errors()
        .into_iter()
        .map(|e| {
            let scope = match e.scope {
                SettingsScope::Global => "global",
                SettingsScope::Project => "project",
            };
            Diagnostic::warning(format!("({context}, {scope} settings) {}", e.message))
        })
        .collect()
}

/// Experimental first-time setup — Pi main.ts:615-617 (`:663-664` @v0.84.1).
///
/// pi's condition is `appMode === "interactive" && !parsed.help && parsed.listModels === undefined
/// && shouldRunFirstTimeSetup()`. `!parsed.help` needs no conjunct — `main.rs` prints help and
/// returns upstream of this gate — but `list_models` does: `resolve_app_mode` answers `Interactive`
/// for `cyrup --list-models gpt` on a TTY and the listing exit is DOWNSTREAM, so without it the
/// wizard would mount on a command pi answers with a model list.
///
/// `detected` is pi's own detection (`detectTerminalThemeForAuto({ ui, timeoutMs: 100 })`,
/// startup-ui.ts:180) — the 100 ms bound is pi's. The theme is the detected polarity rather than
/// `UiTheme::default()`: pi's `createStartupTui` resolves the theme *setting* first
/// (startup-ui.ts:77-84), and on a first run there is no `settings.json` by definition (the gate's
/// own fourth clause), so what it resolves to is exactly this.
///
/// Pi's `showFirstTimeSetup` returns void; a cancel at either step persists nothing, and a
/// persistence failure is propagated rather than swallowed.
pub async fn maybe_run_first_time_setup(
    mode: AppMode,
    cli: &Cli,
    dirs: &ConfigDirs,
    env: &EnvVars,
    settings: &mut SettingsManager,
) -> anyhow::Result<bool> {
    if mode != AppMode::Interactive
        || cli.list_models.is_some()
        || !crate::startup::should_run_first_time_setup(
            &dirs.settings_path(),
            env.agent_dir.is_some(),
        )
    {
        return Ok(false);
    }
    let detected = cyrup_tui::detect_terminal_theme_for_auto(
        &StdinTerminalProbe,
        std::time::Duration::from_millis(100),
        &std::env::var("COLORFGBG").unwrap_or_default(),
    );
    let theme = if detected == cyrup_tui::TerminalTheme::Light {
        UiTheme::light()
    } else {
        UiTheme::dark()
    };
    let _ = crate::startup::run_first_time_setup(&theme, settings, detected).await?;
    Ok(true)
}

/// `<agent_dir>/models.json` — the user's custom-provider / custom-model file (CFG-002).
///
/// Pi loads it ONCE per runtime (`ModelConfig.load(join(getAgentDir(),"models.json"))`,
/// model-runtime.ts:137-139) and every provider/model resolution reads the registry composed from
/// it (`rebuildProviders`, :225-231). It must be loaded before `--list-models` and before provider
/// selection, or a declared provider is unlistable and unlaunchable. A load/parse failure is loud (a
/// returned warning) but never fatal: the file degrades to empty and the built-in registry stands
/// (Pi keeps an empty snapshot + one error string, model-config.ts:251).
///
/// The per-provider composition failures are Pi's `compositionErrors` map (model-runtime.ts:104):
/// the offending block is dropped, its built-ins survive, and the rest of the file applies.
pub fn load_models_json(dirs: &ConfigDirs) -> (Arc<ModelFile>, Vec<Diagnostic>) {
    let (file, load_error) = cyrup_config::load_models_file_reporting(&dirs.models_path());
    let mut warnings: Vec<Diagnostic> = load_error.into_iter().map(Diagnostic::warning).collect();
    warnings.extend(
        crate::provider::models_json_composition_errors(&file)
            .into_iter()
            .map(Diagnostic::warning),
    );
    (Arc::new(file), warnings)
}

/// Runtime model-catalog overlay, phase 2 (DRIFT-007) — Pi's post-init `void modelRuntime.refresh()`
/// (main.ts:863-866 / interactive-mode.ts `run()`): a DETACHED revalidation of the catalogs restored
/// by [`crate::provider::restore_model_catalog`], gated on the [`cyrup_config::policy::NetworkPolicy`]
/// allowing outbound traffic. Nothing here is awaited, so startup is never blocked, and every failure
/// mode leaves the compiled-in catalogs exactly as they are.
///
/// MODE-GATED, matching Pi's two — and only two — trigger sites: `main.ts:864` guards on
/// `appMode === "rpc"` by name and interactive fires its own inside `InteractiveMode.run()`.
/// Creation itself never fetches upstream (`allowModelNetwork: false` at `main.ts:158` and
/// `package-manager-cli.ts:401`, consumed at `model-runtime.ts:163`), so `cyrup -p "…"` and
/// `--mode json` issue no catalog request either — the scripted/CI path stays offline and reads the
/// disk-restored overlay only. `mode_refreshes_catalogs` also re-checks inside the spawn, so this
/// outer guard is an optimization (it skips the settings/auth reads) rather than the gate itself.
///
/// Pi refreshes ONLY providers whose credential resolves (`models.ts:296`); without that a bare
/// start would fan out one request per built-in provider.
pub fn maybe_spawn_catalog_refresh(
    mode: AppMode,
    dirs: &ConfigDirs,
    env: &EnvVars,
    overrides: &CliConfigOverrides,
    settings_store: &Arc<dyn SettingsStore>,
) {
    if !crate::provider::mode_refreshes_catalogs(mode) {
        return;
    }
    let startup_settings = SettingsManager::load(settings_store.clone(), false);
    let policy =
        cyrup_config::policy::NetworkPolicy::resolve(startup_settings.effective(), env, overrides);
    let auth = AuthStore::at(dirs.agent_dir.join("auth.json"));
    let configured: Vec<String> = cyrup_provider::all_providers()
        .iter()
        .filter(|p| auth.has_auth(p.id(), None))
        .map(|p| p.id().as_str().to_string())
        .collect();
    crate::provider::spawn_model_catalog_refresh(dirs, policy, mode, configured);
}

/// Default-launch model (Pi `findInitialModel`, model-resolver.ts:527-607): when NEITHER
/// `--provider` nor `--model` (nor a `--models` scope) is given, cyrup must launch on a REAL
/// configured provider — the saved settings default, else a configured provider's curated default —
/// instead of stopping at the zero-model `UnconfiguredProvider` that `select_provider` yields for
/// the no-flag case (there is no provider prefix to key off).
///
/// Returns the `(provider_id, model_pattern)` the caller re-runs `select_provider` with, or `None`
/// when nothing is configured — in which case the empty catalog stands: `resolve_model` then yields
/// `model: None` + `modelFallbackMessage` (pi sdk.ts:216-218), which the interactive TUI shows as a
/// banner and the non-interactive modes turn into pi's `main.ts:852-855` exit.
///
/// Only for a FRESH session — a resumed/continued session keeps its own restored model.
///
/// Pi `hasConfiguredAuth`: the model's provider has a stored credential / known env var (e.g.
/// `TOGETHER_API_KEY`) — the same `auth.json`-backed `AuthStore` the session builds — **or** a
/// `models.json` block of its own that carries a configured `apiKey` (CFG-022). Pi's
/// `configuredProviders` set is filled by running `checkAuth` over every COMPOSED provider
/// (model-runtime.ts:372-374), so a user-declared provider counts with an empty `auth.json`; without
/// the second tier a fresh custom-provider-only install filtered its own provider out of step 4 and
/// dead-ended on the empty `unconfigured` catalog instead.
pub fn resolve_default_launch_model(
    cli: &Cli,
    dirs: &ConfigDirs,
    config: &SessionConfig,
    models_json: &Arc<ModelFile>,
    settings_store: &Arc<dyn SettingsStore>,
) -> Option<(String, String)> {
    if cli.provider.is_some()
        || cli.model.is_some()
        || !cli.models.is_empty()
        || !is_fresh_target(&config.target)
    {
        return None;
    }
    let auth = AuthStore::at(dirs.agent_dir.join("auth.json"));
    let auth_models_json = models_json.clone();
    let has_configured_auth = move |m: &cyrup_provider::Model| {
        cyrup_config::provider_is_configured(&auth, &auth_models_json, &m.provider, None)
    };
    // Saved settings default `(provider, model)` (Pi step 3), read from the same file store.
    let settings = SettingsManager::load(settings_store.clone(), false);
    let eff = settings.effective();
    let default_provider = eff.default_provider();
    let default_model = eff.default_model();
    crate::provider::default_launch_model(
        default_provider.as_deref(),
        default_model.as_deref(),
        &has_configured_auth,
        models_json,
    )
}

/// Initialise `tracing` to **stderr**, honouring `RUST_LOG`. Off by default; `--verbose` raises the
/// floor to `debug`. Idempotent and never fatal.
pub fn init_tracing(verbose: bool) {
    use tracing_subscriber::{EnvFilter, fmt};
    let default = if verbose { "debug" } else { "warn" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
