//! cyrup — the CLI binary (arch-11 §2.4). The sole `anyhow` boundary and the only binary in the
//! workspace.
//!
//! Thin by design: peek for a package/config subcommand (Pi dispatches these before arg parsing,
//! main.ts:486), else parse args, initialise tracing to **stderr** (so stdout stays clean for
//! PRINT/JSON/RPC), probe the TTYs, resolve config directories, map the CLI to a `SessionConfig`,
//! select a provider, build the one `AgentSession` seam over a FILE-BACKED settings store, install
//! signal handling, then dispatch to the resolved runtime mode. All reusable logic lives in the
//! `cyrup` library so it is testable without a TTY; only the interactive `CrosstermBackend` wiring
//! stays here (it needs a real terminal and is not unit-tested).

use std::io::{self, IsTerminal};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use cyrup::session_resolve::{Outcome, SessionFlags, SessionRef, resolve_session_target};
use cyrup::{
    AppMode, Cli, Diagnostic, DiagnosticLevel, Inputs, apply_arg_leniency, build_inputs,
    file_settings_store, format_no_models_available_message, initial_input, migrations,
    normalize_short_aliases, partition_extension_flags, render_help, resolve_app_mode,
    run_json_dispatch, run_print_dispatch, run_rpc_dispatch, select_provider,
    should_take_over_stdout, spawn_abort_on_signal, subcommands, timings,
};
use cyrup_config::{
    AuthStore, CliConfigOverrides, ConfigDirs, DefaultProjectTrust, EnvVars, Settings,
    SettingsManager, SettingsScope,
};
use cyrup_resources::theme::ThemeWatcher;
use cyrup_sdk::core::CancelToken;
use cyrup_session_svc::{
    AgentSession, AgentSessionRuntime, InputSource, ScopedModel, SessionConfig,
    SessionFactory, SessionInfo, SessionLayout, SessionServiceError, SessionTarget, SessionsRoot,
    UserInput, list_all, list_in_dir,
};
use cyrup_tui::{App, StdinTerminalProbe, ThemeController, UiTheme, crossterm_input_stream};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(err) => {
            // The single anyhow boundary: report the full cause chain to stderr, exit non-zero.
            eprintln!("cyrup: {err:#}");
            ExitCode::from(1)
        }
    }
}

async fn run() -> anyhow::Result<i32> {
    // Process identity (Pi `cli.ts:12-13`: `process.title = APP_NAME` + `PI_CODING_AGENT=true`) is
    // NOT replicated here: `process.title` has no std API, and `std::env::set_var` is `unsafe` under
    // edition 2024 (the env is not thread-safe to mutate once the runtime has spawned threads). The
    // bin is `unsafe`-free by policy, so this cosmetic identity marker is gated as a hard-language
    // limit (residual ledger §cyrup #20) rather than introducing `unsafe`.

    let mut timings = timings::Timings::new();

    // Pi rewrites its multi-char short aliases in its hand-rolled parser; clap cannot express them as
    // native shorts, so normalize them up front (`-nt` ⇒ `--no-tools`, …).
    let raw: Vec<String> = normalize_short_aliases(std::env::args());
    let argv: Vec<String> = raw.iter().skip(1).cloned().collect();

    // Internal `__subagent-runner --config <path>` pre-dispatch (arch-SA §2.2/§6.5; func-SA §1.1):
    // hop 2 of the SubAgents extension's mandated background-execution mechanism. This is an
    // internal-only subcommand, never advertised to users (not in `--help`, not in
    // `subcommands::SUBCOMMANDS`) — it MUST be recognized and dispatched before ANY user-facing arg
    // leniency/clap parsing (and before the package/config subcommand pre-dispatch below, which has
    // no knowledge of it and would otherwise fall through to ordinary clap parsing, misinterpreting
    // `--config <path>` against the user-facing `Cli` surface). `raw` (not `argv`) is passed since
    // `subagent_runner_cmd::is_selected` expects the binary name at index 0, matching
    // `std::env::args()`'s own shape.
    if cyrup::subagent_runner_cmd::is_selected(&raw) {
        return Ok(cyrup::subagent_runner_cmd::dispatch(&raw).await);
    }

    // Internal `__intercom-broker` pre-dispatch (spec/extensions/cyrup-intercom-port.md §7.3): the
    // hidden subcommand the per-session intercom extension re-execs `current_exe()` into to stand up
    // the standalone broker PROCESS (a Unix-socket hub). Recognized and dispatched here, before any
    // user-facing arg leniency/clap parsing, exactly like `__subagent-runner` above (the broker's own
    // `--config`-free argv must never reach the user-facing `Cli` surface).
    if cyrup::intercom_broker_cmd::is_selected(&raw) {
        return Ok(cyrup::intercom_broker_cmd::dispatch().await);
    }

    // Package/config subcommand pre-dispatch (Pi main.ts:486, before arg parsing). Resolve dirs with
    // no CLI overrides for the subcommand's package/project roots.
    if subcommands::first_subcommand(&argv).is_some() {
        let env = EnvVars::from_process();
        let dirs = ConfigDirs::resolve(&CliConfigOverrides::default(), &env)
            .context("resolving config directories")?;
        let trust_override = subcommands::trust_override(&argv);
        if let Some(code) = subcommands::dispatch(&argv, &dirs, trust_override).await? {
            return Ok(code);
        }
    }

    // `cyrup auth print-api-key|print-bearer-token` pre-dispatch (Pi
    // `if (await runCredentialPrintCommand(args)) return;`, main.ts:557-559 — immediately after the
    // config/package block and BEFORE `parseArgs`). Without it `auth` is not a known verb, so the
    // tokens survive arg leniency as bare positionals and become a chat PROMPT: no credential, no
    // error, an agent session started and tokens burned on an auth subcommand.
    if let Some(code) = cyrup::credential_print::dispatch(&argv).await {
        return Ok(code);
    }

    // Pi-faithful arg leniency (args.ts:80-82,131-139,202-203) BEFORE clap: a bad `--mode` is
    // silently dropped, a bad `--thinking` warns + drops, and an unknown single-dash option becomes a
    // Pi `Unknown option` error (exit 1) rather than a clap usage error (exit 2).
    let (lenient_argv, parse_diagnostics) = apply_arg_leniency(&argv);

    // Capture unknown `--flag[=val]` as extension flags before clap (Pi args.ts:188-201), then parse
    // the cleaned argv and stitch the captured flags back onto the struct.
    let (clean_argv, extension_flags) = partition_extension_flags(&lenient_argv);
    let mut clap_argv = vec![raw.first().cloned().unwrap_or_else(|| "cyrup".to_string())];
    clap_argv.extend(clean_argv);
    let mut cli = Cli::parse_from(&clap_argv);
    cli.extension_flags = extension_flags;
    // Trim each comma-split segment of `--models`/`--tools`/`--exclude-tools` and drop empty
    // tool/exclude-tool names, matching Pi's post-split normalization (args.ts:114,120-129). clap's
    // `value_delimiter = ','` splits but never trims, so `--tools "read, grep"` would otherwise keep
    // `" grep"` and silently drop the tool. Run before any consumer reads these Vecs.
    cli.normalize_list_flags();
    init_tracing(cli.verbose);
    timings.mark("parseArgs");

    // Report parse diagnostics (Pi main.ts:504-512): warnings + errors to stderr, any error exits 1.
    report_diagnostics(&parse_diagnostics);
    if parse_diagnostics
        .iter()
        .any(|d| d.level == DiagnosticLevel::Error)
    {
        return Ok(1);
    }

    // Rich `--help` body (Pi printHelp, args.ts:212). Loaded-extension flags are the outer extension
    // tier; the bin injects an empty set today (the injection point is preserved 1:1).
    if cli.help {
        print!("{}", render_help(&[]));
        return Ok(0);
    }

    // Conflicting-session-flag diagnostics (Pi `validateForkFlags`/`validateSessionIdFlags`).
    if let Err(msg) = cli.validate_session_flags() {
        anyhow::bail!("{msg}");
    }

    let stdin_tty = io::stdin().is_terminal();
    let stdout_tty = io::stdout().is_terminal();
    let mode = resolve_app_mode(&cli, stdin_tty, stdout_tty);

    // Stdout takeover (Pi main.ts:535-537): for a non-interactive run that is not a plain-metadata
    // command, install the guard so every *incidental* stdout write between here and the protocol
    // stream — a `runMigrations` notice, `createSessionManager`'s cross-project "Session found in
    // different project" hint — is rerouted to stderr (via `emit_stray_line`) instead of corrupting
    // the PRINT/JSON/RPC stream on stdout. The protocol writers keep writing to real stdout (their
    // injected `io::stdout()` sink is the analog of Pi's `writeRawStdout`).
    if should_take_over_stdout(&cli, mode) {
        cyrup::output_guard::take_over_stdout();
    }

    // `@file` is unsupported in RPC mode (Pi main.ts:540-543).
    if mode == AppMode::Rpc && !split_file_args(&cli).is_empty() {
        anyhow::bail!("@file arguments are not supported in RPC mode");
    }

    // Resolve directories (CLI > env > default; the only place env is read). `--session-dir`,
    // `--offline`, `--api-key`, `--model(s)` thread through `CliConfigOverrides`.
    let env = EnvVars::from_process();
    let overrides = CliConfigOverrides {
        session_dir: cli.session_dir.clone(),
        offline: cli.offline || env.offline,
        trust_override: cli.trust_override(),
        model: cli.model.clone(),
        models: cli.models.clone(),
        api_key: cli.api_key.clone(),
        ..Default::default()
    };
    let dirs = ConfigDirs::resolve(&overrides, &env).context("resolving config directories")?;

    // One-time startup migrations (Pi `runMigrations(cwd)`, main.ts:549): legacy auth/session/tools
    // moves + extension-system deprecation warnings.
    let migration = migrations::run_migrations(&dirs);
    timings.mark("runMigrations");

    // Pi's `startupSettingsManager` (main.ts:610-611), created after the migrations and used for
    // exactly two things: surfacing settings load/parse errors as warnings
    // (`collectSettingsDiagnostics(startupSettingsManager, "startup session lookup")`), and the
    // `sessionDir` lookup immediately below. One manager, both jobs — as upstream.
    //
    // `project_trusted: false` is cyrup's standing pre-trust posture (R-07-002; the same value this
    // site already used, and the one every other startup-phase manager here uses). Pi's startup
    // manager defaults to `projectTrusted: true` (settings-manager.ts:320), so an UNTRUSTED
    // project's `.cyrup/settings.json` cannot relocate the session dir under cyrup; the global
    // `<agent_dir>/settings.json` tier — the documented one — behaves exactly as upstream.
    let mut startup_settings =
        SettingsManager::load(file_settings_store(&dirs), Settings::new(), false);
    report_diagnostics(&collect_settings_diagnostics(
        &mut startup_settings,
        "startup session lookup",
    ));

    // `sessionDir` tier 3 (Pi main.ts:625-630): CLI `--session-dir` > `$CYRUP_SESSION_DIR` >
    // `startupSettingsManager.getSessionDir()` (settings-manager.ts:670-673). `ConfigDirs::resolve`
    // folded in the first two tiers; the settings tier has to be applied out here because the
    // settings file lives under the `agent_dir` that `resolve` itself computes — the same reason Pi
    // builds its startup manager only after the dirs exist. A settings-derived dir counts as
    // EXPLICIT: Pi hands it to `createSessionManager(parsed, cwd, sessionDir, …)` (main.ts:630)
    // through the same argument slot as `--session-dir`, so it is used literally rather than
    // cwd-encoded, and `session_list_layout`/`Cli::to_session_config` below key off that flag.
    let dirs = cyrup::apply_settings_session_dir(dirs, &startup_settings);

    // First-time-setup gate (Pi main.ts:557 / startup-ui.ts:115). Faithfully `false` for the cyrup
    // rebrand (not the official distribution), so the wizard is never invoked; the call-site exists
    // so the gate is real (the wizard UI itself is the ext-UI dialog host, an outer layer).
    if mode == AppMode::Interactive
        && cyrup::should_run_first_time_setup(&dirs.settings_path(), env.agent_dir.is_some())
    {
        // The interactive first-time-setup wizard is the ext-UI dialog host (outer layer); the
        // predicate above is the closeable gate.
    }

    // `--name` must be non-empty after trim (Pi main.ts:586-592).
    let session_name = cli.validated_name().map_err(|m| anyhow::anyhow!("{m}"))?;

    // `--api-key` requires a resolvable model spec (Pi main.ts:701-710): without any of
    // `--model`/`--provider`/`--models` there is no provider to attach the key to.
    if cli.api_key.is_some()
        && cli.model.is_none()
        && cli.provider.is_none()
        && cli.models.is_empty()
    {
        anyhow::bail!(
            "--api-key requires a model to be specified via --model, --provider/--model, or --models"
        );
    }

    // Standalone actions that run and exit (Pi `--export`/`--list-models`, main.ts:520,470).
    if let Some(export) = &cli.export {
        return export_session_html(export, cli.positionals.first().map(String::as_str)).await;
    }

    // `<agent_dir>/models.json` — the user's custom-provider / custom-model file (CFG-002). Pi loads
    // it ONCE per runtime (`ModelConfig.load(join(getAgentDir(),"models.json"))`,
    // model-runtime.ts:137-139) and every provider/model resolution reads the registry composed from
    // it (`rebuildProviders`, :225-231). It must be loaded HERE, before `--list-models` and before
    // provider selection, or a declared provider is unlistable and unlaunchable. A load/parse
    // failure is loud (a stderr warning) but never fatal: the file degrades to empty and the
    // built-in registry stands (Pi keeps an empty snapshot + one error string, model-config.ts:251).
    let models_json = {
        let (file, load_error) = cyrup_config::load_models_file_reporting(&dirs.models_path());
        let mut warnings: Vec<Diagnostic> = load_error.into_iter().map(Diagnostic::warning).collect();
        // Per-provider composition failures (Pi's `compositionErrors` map, model-runtime.ts:104):
        // the offending block is dropped, its built-ins survive, and the rest of the file applies.
        warnings.extend(
            cyrup::provider::models_json_composition_errors(&file)
                .into_iter()
                .map(Diagnostic::warning),
        );
        if !warnings.is_empty() {
            report_diagnostics(&warnings);
        }
        Arc::new(file)
    };

    // Runtime model-catalog overlay, phase 1 (DRIFT-007) — Pi's cache-only restore
    // `await modelRuntime.refresh({ allowNetwork: false })` (agent-session-services.ts:180), which
    // upstream performs inside `createAgentSessionRuntime` (main.ts:793). Disk only: it reads
    // `<agent_dir>/models-store.json` and installs it as the overlay, with NO network I/O.
    //
    // It sits HERE, beside the `models.json` load and above the `--list-models` exit, because that
    // is where Pi has it: the listing exit is main.ts:816, downstream of runtime creation, so
    // `pi --list-models` renders the persisted pi.dev overlay. Phase 2 (the network revalidation)
    // stays downstream at its own site, matching Pi's post-listing trigger at main.ts:863-866 — a
    // listing run therefore shows the cache and issues no request.
    cyrup::provider::restore_model_catalog(&dirs).await;

    // `--list-models` enumerates the FULL multi-provider registry (Pi `modelRegistry.getAvailable()`,
    // list-models.ts:35) — independent of `--provider`/`--model`, and resolved BEFORE provider
    // selection (so a `--provider <unknown>` does not gate the listing, matching Pi).
    if let Some(search) = &cli.list_models {
        return list_models(&cyrup::provider::all_available_models(&models_json), search);
    }
    let mut provider = select_provider(
        cli.provider.as_deref(),
        cli.model.as_deref(),
        cli.api_key.as_deref(),
        &models_json,
    )?;

    // Unknown-model diagnostic (Pi `resolveCliModel`, main.ts:377-378 / model-resolver.ts:494-500):
    // a `--model` on a *known* provider whose id is not in the catalog warns (the build still proceeds
    // with a custom-id model). An *unknown provider* already errored in `select_provider` above.
    if let Some(warning) = cyrup::unknown_model_warning(
        cli.provider.as_deref(),
        cli.model.as_deref(),
        &cyrup::provider::all_available_models(&models_json),
    ) {
        report_diagnostics(&[Diagnostic::warning(warning)]);
    }

    // Map CLI → SessionConfig. The diagnostics half is Pi's `resolvePromptInput` warning channel
    // (resource-loader.ts:60-63): a `--system-prompt`/`--append-system-prompt` token that names an
    // EXISTING but unreadable file warns and falls back to being used as literal text — never fatal.
    let (mut config, prompt_diagnostics) = cli.to_session_config_with_diagnostics(&dirs, mode);
    report_diagnostics(&prompt_diagnostics);

    // Non-interactive session-resolution depth (Pi `createSessionManager`, main.ts:254-350): a
    // `--session`/`--fork` partial-UUID prefix match, a global cross-project search, a
    // `--session-id` create-if-missing-by-exact-id, the plain-stdin fork-into-cwd confirm, and the
    // non-interactive missing-session-cwd guard. Engaged only when a session ref is supplied — the
    // bare `New`/`Continue` target from `to_session_config` stands otherwise (no needless listing).
    if (cli.fork.is_some() || cli.session.is_some() || cli.session_id.is_some())
        && let Some(code) = resolve_session(&cli, &dirs, mode, &mut config)?
    {
        return Ok(code);
    }

    // Pre-launch startup-UI orchestration (Pi `cli/startup-ui.ts` + `cli/session-picker.ts` +
    // `cli/project-trust.ts`): the `--resume` picker and the interactive project-trust prompt run over
    // the cyrup-tui selectors BEFORE the runtime is built. Interactive-only (each needs a real TTY);
    // the one-shot/RPC live path is untouched. Returns `Some(0)` when the user cancels the picker.
    if mode == AppMode::Interactive
        && let Some(code) = resolve_startup_ui(&cli, &dirs, mode, &mut config)?
    {
        return Ok(code);
    }

    let deprecation_warnings = migration.deprecation_warnings.clone();
    let settings_store = file_settings_store(&dirs);

    // Runtime model-catalog overlay, phase 2 (DRIFT-007) — Pi's post-init `void modelRuntime.refresh()`
    // (main.ts:863-866 / interactive-mode.ts `run()`): a DETACHED revalidation of the catalogs
    // restored above, gated on the NetworkPolicy allowing outbound traffic. Nothing here is awaited,
    // so startup is never blocked, and every failure mode leaves the compiled-in catalogs exactly as
    // they are today. Like Pi's, this site is downstream of the `--list-models` exit, so a listing
    // run renders the cache and issues no request.
    //
    // MODE-GATED, matching Pi's two — and only two — trigger sites: `main.ts:864` guards on
    // `appMode === "rpc"` by name and interactive fires its own inside `InteractiveMode.run()`.
    // Creation itself never fetches upstream (`allowModelNetwork: false` at `main.ts:158` and
    // `package-manager-cli.ts:401`, consumed at `model-runtime.ts:163`), so `cyrup -p "…"` and
    // `--mode json` must issue no catalog request either — the scripted/CI path stays offline and
    // reads the disk-restored overlay only. `mode_refreshes_catalogs` also re-checks inside the
    // spawn, so this outer guard is an optimization (it skips the settings/auth reads) rather than
    // the gate itself.
    if cyrup::provider::mode_refreshes_catalogs(mode) {
        let startup_settings = SettingsManager::load(settings_store.clone(), Settings::new(), false);
        let policy = cyrup_config::policy::NetworkPolicy::resolve(
            startup_settings.effective(),
            &env,
            &overrides,
        );
        // Pi refreshes ONLY providers whose credential resolves (`models.ts:296`); without that a
        // bare start would fan out one request per built-in provider.
        let auth = AuthStore::at(dirs.agent_dir.join("auth.json"));
        let configured: Vec<String> = cyrup_provider::all_providers()
            .iter()
            .filter(|p| auth.has_auth(p.id(), None))
            .map(|p| p.id().as_str().to_string())
            .collect();
        cyrup::provider::spawn_model_catalog_refresh(&dirs, policy, mode, configured);
    }

    let cancel = CancelToken::new();

    // Default-launch model (Pi `findInitialModel`, model-resolver.ts:527-607): when NEITHER
    // `--provider` nor `--model` (nor a `--models` scope) is given, cyrup must launch on a REAL
    // configured provider — the saved settings default, else a configured provider's curated default
    // — instead of always falling back to the offline scripted faux provider. The `select_provider`
    // call above yields faux for this no-flag case (there is no provider prefix to key off); here we
    // upgrade it to the resolved default provider/model when one is configured, and set the
    // corresponding `model_pattern` so the builder launches on that exact model (footer shows e.g.
    // `together/moonshotai/Kimi-K2.6`). Only for a FRESH session — a resumed/continued session keeps
    // its own restored model. When NOTHING is configured this is a no-op and faux stays the fallback.
    if cli.provider.is_none()
        && cli.model.is_none()
        && cli.models.is_empty()
        && is_fresh_target(&config.target)
    {
        // Pi `hasConfiguredAuth`: the model's provider has a stored credential / known env var
        // (e.g. `TOGETHER_API_KEY`) — the same `auth.json`-backed `AuthStore` the session builds —
        // **or** a `models.json` block of its own that carries a configured `apiKey` (CFG-022). Pi's
        // `configuredProviders` set is filled by running `checkAuth` over every COMPOSED provider
        // (model-runtime.ts:372-374), so a user-declared provider counts with an empty `auth.json`;
        // without the second tier a fresh custom-provider-only install filtered its own provider out
        // of step 4 and launched on the offline faux provider instead.
        let auth = AuthStore::at(dirs.agent_dir.join("auth.json"));
        let auth_models_json = models_json.clone();
        let has_configured_auth = move |m: &cyrup_provider::Model| {
            cyrup_config::provider_is_configured(&auth, &auth_models_json, &m.provider, None)
        };
        // Saved settings default `(provider, model)` (Pi step 3), read from the same file store.
        let settings = SettingsManager::load(settings_store.clone(), Settings::new(), false);
        let eff = settings.effective();
        let default_provider = eff.default_provider();
        let default_model = eff.default_model();
        if let Some((launch_provider, launch_pattern)) = cyrup::provider::default_launch_model(
            default_provider.as_deref(),
            default_model.as_deref(),
            &has_configured_auth,
            &models_json,
        ) {
            provider = select_provider(Some(&launch_provider), None, None, &models_json)?;
            config.model_pattern = Some(launch_pattern);
        }
    }

    // `--api-key` installs a RUNTIME credential on the same credential store the session's model
    // runtime reads (Pi `modelRuntime.setRuntimeApiKey(sessionOptions.model.provider, parsed.apiKey)`,
    // main.ts:764 → model-runtime.ts:400-418, whose store is `RuntimeCredentials` wrapping
    // `AuthStorage`). cyrup handed the key only to `select_provider`'s throwaway
    // `InMemoryCredentialStore`, so the session's `AuthStore` — the one behind `hasConfiguredAuth`,
    // `getProviderAuthStatus` and `/logout`'s `listCredentials()` — never saw it. Building the store
    // here (instead of letting `SessionBuilder` default it) is what lets the key be installed on it.
    //
    // The default-launch block above cannot have swapped `provider` out from under this: `--api-key`
    // is rejected earlier unless one of `--model`/`--provider`/`--models` is present, and that block
    // runs only when all three are absent.
    let auth_store = Arc::new(AuthStore::at(dirs.agent_dir.join("auth.json")));
    if let Some(api_key) = cli.api_key.as_deref() {
        auth_store.set_runtime_api_key(provider.id().clone(), api_key.to_string());
    }

    // Interactive mode drives the **multi-session** `AgentSessionRuntime` (arch-11 §3.4) so the
    // session-swap commands rebuild the active session in place and the TUI re-binds to it. The
    // one-shot/RPC modes keep the single fixed `AgentSession` seam unchanged.
    if let AppMode::Interactive = mode {
        // `PI_STARTUP_BENCHMARK` only supports interactive mode (Pi main.ts:800-804) — here it is
        // satisfied (interactive). In the one-shot/RPC arms below it is an error.
        let target = config.target.clone();
        let fresh = is_fresh_target(&target);
        let session_cwd = config.cwd.clone();
        let mut factory_builder = SessionFactory::new(provider, config)
            .settings_store(settings_store.clone())
            .auth(auth_store.clone())
            .provider_resolver(Arc::new(cyrup::provider::BuiltinProviderResolver::new(models_json.clone())));
        // SubAgents opt-in gate (default OFF, mirrors the two sibling companions) composed with the T6
        // child-mode gate (Pi `extension/index.ts:243-245` + `extension/fanout-child.ts:131`): a plain
        // TOP-LEVEL session attaches the orchestrator surface ONLY when opted in (`is_installed`:
        // `CYRUP_SUBAGENTS` truthy, or a `subagents/config.json` at user/project scope). A child
        // re-execs with `CYRUP_SUBAGENT_CHILD=1`; a plain child registers nothing (returns `None`),
        // while a fanout-authorized child (`CYRUP_SUBAGENT_FANOUT_CHILD=1`) gets a restricted,
        // mutation-blocked tool REGARDLESS of `is_installed`. `subagent_extension_for_env` encodes
        // that composed decision.
        // Intercom companion (spec/extensions/cyrup-intercom-port.md): the out-of-band supervisor
        // coordination bridge. Built FIRST (concrete) so its broker-backed delivery/clarify seam
        // channels can be handed to the SubAgents extension via `with_channels` (the port doc §8.4
        // item 1 / P5 handoff — CLOSING R-SA-037/119/120/123/124/125). Child-mode gated — a subagent
        // child with orchestrator metadata always attaches so `contact_supervisor` registers; a plain
        // session attaches only when opted in (`_concrete` returns `None` otherwise, no broker).
        let intercom_ext = cyrup_intercom::intercom_extension_for_env_concrete(
            dirs.agent_dir.clone(),
            session_cwd.clone(),
        )
        .map_err(|e| anyhow::anyhow!("building intercom extension: {e}"))?;
        // SubAgents opt-in gate composed with the T6 child-mode gate (see above): a plain top-level
        // session attaches only when opted in (`is_installed`); a plain child registers nothing; a
        // fanout-authorized child gets the restricted tool regardless. When intercom is attached this
        // session, thread its real channels in (else keep the NoTransport/NoOp degrade defaults,
        // R-SA-020).
        let subagent_ext = match &intercom_ext {
            Some(ic) => cyrup_ext_subagents::extension::subagent_extension_for_env_with_channels(
                &dirs.agent_dir,
                cyrup::subagent_config::load_subagent_extension_config(&dirs.agent_dir),
                session_cwd.clone(),
                ic.delivery_channel(),
                ic.clarify_channel(),
                ic.steer_channel(),
            ),
            None => cyrup_ext_subagents::extension::subagent_extension_for_env(
                &dirs.agent_dir,
                cyrup::subagent_config::load_subagent_extension_config(&dirs.agent_dir),
                session_cwd.clone(),
            ),
        };
        // Attach subagents first (matching prior load order), then the intercom extension itself.
        if let Some(ext) = subagent_ext {
            factory_builder = factory_builder.with_native_extension(ext);
        }
        // SUBA-S01 (pi `pi-args.ts:13`, which loads `subagent-prompt-runtime.ts` into the child
        // as its OWN extension): a plain subagent child attaches NO subagents extension —
        // `subagent_extension_for_env` returns `None` for it by design — so the child-side
        // `structured_output` tool cannot come from that gate. This one is independent: it
        // builds only when the parent passed both structured-output env vars, i.e. only for a
        // step that actually declared an `outputSchema`. Every other process attaches nothing.
        if let Some(runtime) = cyrup_ext_subagents::prompt_runtime::prompt_runtime_extension_for_env() {
            factory_builder = factory_builder.with_native_extension(runtime);
        }
        if let Some(ic) = intercom_ext {
            factory_builder = factory_builder.with_native_extension(ic);
        }
        // Permission system (port doc §4): the opt-in allow/ask/deny gate over tool calls, attached
        // via the SAME `.with_native_extension(...)` seam. `permission_extension_for_env` selects the
        // role by the `CYRUP_SUBAGENT_CHILD` signal — a subagent child loads the gate with the
        // child→parent ask-FORWARDING channel, this (root) session loads it with the in-session dialog
        // + the forwarding watcher — and returns `None` when the gate is not installed (DI-5) OR
        // when an installed gate is switched off by `"enabled": false` in its `config.json`
        // (v0.8.0 `index.ts:1473-1477`, the master switch that early-returns before registration).
        if let Some(ext) = cyrup_permission_system::permission_extension_for_env(
            dirs.agent_dir.clone(),
            session_cwd,
        ) {
            factory_builder = factory_builder.with_native_extension(ext);
        }
        let factory = Arc::new(factory_builder);
        let runtime = AgentSessionRuntime::create(factory, target)
            .await
            .context("building agent session runtime")?;
        // Pi main.ts:843-848 — report the runtime's build diagnostics and exit 1 on any error
        // (today: the extension-flag reconciliation errors, SEAM-S01).
        if report_runtime_diagnostics(&runtime).await {
            runtime.dispose().await;
            return Ok(1);
        }
        let session = runtime.session().await;
        apply_post_build(&session, session_name.as_deref(), &cli, fresh).await;
        // Migrated-credential notice (Pi `InteractiveMode` startup warning, interactive-mode.ts:797):
        // when `runMigrations` moved any provider credential into `auth.json`, name them.
        if !migration.migrated_auth_providers.is_empty() {
            eprintln!(
                "Warning: Migrated credentials to auth.json: {}",
                migration.migrated_auth_providers.join(", ")
            );
        }
        // Show extension-system deprecation warnings in interactive mode (Pi main.ts:781).
        let warnings = migrations::format_deprecation_warnings(&deprecation_warnings);
        if !warnings.is_empty() {
            eprint!("{warnings}");
        }
        let _signals = spawn_abort_on_signal(session.clone(), cancel.clone());
        // `PI_STARTUP_BENCHMARK` interactive run path (Pi main.ts:819-835): init the TUI, let stdin
        // drain terminal query replies for ~150ms, stop, then print timings — never the event loop.
        if timings::startup_benchmark_enabled() {
            run_interactive_benchmark().await?;
            timings.mark("interactiveMode.init");
            timings.print();
            return Ok(0);
        }
        timings.print();
        // Pi `prepareInitialMessage(parsed, settingsManager.getImageAutoResize(), stdinContent)`
        // (main.ts:828-832): the `images.autoResize` setting decides whether an `@image.png`
        // positional is downsampled to 2000px or inlined at full resolution.
        let auto_resize_images = session.services().settings.effective().image_auto_resize();
        let inputs = build_inputs(&cli, &dirs.cwd, auto_resize_images).await?;
        // The startup package-update check (Pi `interactive-mode.ts:850-856`): DETACHED, gated on
        // `NetworkPolicy::allow_update_check()` (`--offline` / `CYRUP_OFFLINE` /
        // `CYRUP_SKIP_VERSION_CHECK`), and delivered to the run loop over a channel so nothing here
        // is awaited before the first frame. `None` when the gate declines — see
        // `cyrup::update_check` for why only the PACKAGE half of Pi's pair is ported.
        let update_policy = cyrup_config::policy::NetworkPolicy::resolve(
            session.services().settings.effective(),
            &env,
            &overrides,
        );
        let package_updates = cyrup::update_check::spawn_package_update_check(
            dirs.package_dir.clone(),
            Some(dirs.cwd.clone()),
            update_policy,
        );
        let interactive = run_interactive(
            runtime.clone(),
            session.clone(),
            inputs,
            cli.verbose,
            cancel,
            package_updates,
        )
        .await;
        // Quit is a normal exit here too: Pi disposes the runtime on every host teardown path
        // (agent-session-runtime.ts:397-404), emitting `session_shutdown{reason:"quit"}` so
        // extensions can flush/deregister. Runs even when the TUI loop errored out.
        runtime.dispose().await;
        // …and then, on a clean quit, the ONE line Pi prints after disposing
        // (interactive-mode.ts:3594-3597): the exact invocation that returns here. Under an explicit
        // `--session-dir` this is the only surfaced route back to the session — the picker a bare
        // relaunch offers only ever lists the session's own directory.
        if interactive.is_ok() {
            print_resume_hint(&dirs, &session).await;
        }
        interactive?;
        return Ok(0);
    }

    // `PI_STARTUP_BENCHMARK` is interactive-only (Pi main.ts:800-804).
    if timings::startup_benchmark_enabled() {
        anyhow::bail!("PI_STARTUP_BENCHMARK only supports interactive mode");
    }

    match mode {
        AppMode::Rpc => {
            let target = config.target.clone();
            let fresh = is_fresh_target(&target);
            let session_cwd = config.cwd.clone();
            let mut factory_builder = SessionFactory::new(provider, config)
                .settings_store(settings_store.clone())
                .auth(auth_store.clone())
                .provider_resolver(Arc::new(cyrup::provider::BuiltinProviderResolver::new(models_json.clone())));
            // Intercom companion: built FIRST (concrete) so its broker-backed delivery/clarify seam
            // channels can be handed to SubAgents via `with_channels` (P5 handoff, CLOSING R-SA-037/
            // 119/120/123/124/125). Child-mode gated; `_concrete` returns `None` for a plain session.
            let intercom_ext = cyrup_intercom::intercom_extension_for_env_concrete(
                dirs.agent_dir.clone(),
                session_cwd.clone(),
            )
            .map_err(|e| anyhow::anyhow!("building intercom extension: {e}"))?;
            // SubAgents opt-in gate + T6 child-mode gate (see the interactive arm above): a plain
            // top-level session attaches only when opted in (`is_installed`); a plain subagent child
            // registers nothing; a fanout-authorized child gets the restricted tool regardless. Thread
            // the intercom channels in when intercom is attached, else keep the NoTransport/NoOp
            // degrade defaults.
            let subagent_ext = match &intercom_ext {
                Some(ic) => cyrup_ext_subagents::extension::subagent_extension_for_env_with_channels(
                    &dirs.agent_dir,
                    cyrup::subagent_config::load_subagent_extension_config(&dirs.agent_dir),
                    session_cwd.clone(),
                    ic.delivery_channel(),
                    ic.clarify_channel(),
                    ic.steer_channel(),
                ),
                None => cyrup_ext_subagents::extension::subagent_extension_for_env(
                    &dirs.agent_dir,
                    cyrup::subagent_config::load_subagent_extension_config(&dirs.agent_dir),
                    session_cwd.clone(),
                ),
            };
            if let Some(ext) = subagent_ext {
                factory_builder = factory_builder.with_native_extension(ext);
            }
            // SUBA-S01 (pi `pi-args.ts:13`, which loads `subagent-prompt-runtime.ts` into the child
            // as its OWN extension): a plain subagent child attaches NO subagents extension —
            // `subagent_extension_for_env` returns `None` for it by design — so the child-side
            // `structured_output` tool cannot come from that gate. This one is independent: it
            // builds only when the parent passed both structured-output env vars, i.e. only for a
            // step that actually declared an `outputSchema`. Every other process attaches nothing.
            if let Some(runtime) = cyrup_ext_subagents::prompt_runtime::prompt_runtime_extension_for_env() {
                factory_builder = factory_builder.with_native_extension(runtime);
            }
            if let Some(ic) = intercom_ext {
                factory_builder = factory_builder.with_native_extension(ic);
            }
            // Permission system (port doc §4): opt-in allow/ask/deny gate; same seam + child-gating.
            if let Some(ext) = cyrup_permission_system::permission_extension_for_env(
                dirs.agent_dir.clone(),
                session_cwd,
            ) {
                factory_builder = factory_builder.with_native_extension(ext);
            }
            let factory = Arc::new(factory_builder);
            let runtime = match AgentSessionRuntime::create(factory, target).await {
                Ok(r) => r,
                // Non-interactive no-models-available guard (Pi main.ts:795-798): print the provider
                // login guidance + exit 1 instead of a generic build error.
                Err(SessionServiceError::NoModels(_)) => return no_models_available(),
                Err(e) => {
                    return Err(anyhow::Error::new(e).context("building agent session runtime"));
                }
            };
            // Pi main.ts:843-848 (SEAM-S01) — same checkpoint, every mode.
            if report_runtime_diagnostics(&runtime).await {
                runtime.dispose().await;
                cyrup::output_guard::restore_stdout();
                return Ok(1);
            }
            let session = runtime.session().await;
            apply_post_build(&session, session_name.as_deref(), &cli, fresh).await;
            timings.print();
            let _signals = spawn_abort_on_signal(session, cancel.clone());
            let reader = tokio::io::BufReader::new(tokio::io::stdin());
            let mut writer = tokio::io::stdout();
            run_rpc_dispatch(&runtime, reader, &mut writer).await?;
            // Restore stdout at teardown (Pi `finally { restoreStdout() }`, main.ts:848).
            cyrup::output_guard::restore_stdout();
            Ok(0)
        }
        AppMode::Print | AppMode::Json => {
            // SEAM-006: print/json run on the RUNTIME host, exactly like interactive and RPC. Pi's
            // entry point is `runPrintMode(runtimeHost: AgentSessionRuntime, options)`
            // (print-mode.ts:32) — it has no bare-session host. Building a bare `AgentSession` here
            // left every loaded extension's `ctx.newSession()`/`ctx.fork()`/`ctx.switchSession()`/
            // `ctx.reload()` with nothing to act on (`SessionServiceError::NoRuntimeHost`, warned
            // and swallowed), and since this arm is what a spawned subagent child re-execs into,
            // EVERY subagent run inherited the missing host.
            let target = config.target.clone();
            let fresh = is_fresh_target(&config.target);
            let session_cwd = config.cwd.clone();
            let mut factory_builder = SessionFactory::new(provider, config)
                .settings_store(settings_store.clone())
                .auth(auth_store.clone())
                .provider_resolver(Arc::new(cyrup::provider::BuiltinProviderResolver::new(models_json.clone())));
            // Intercom companion: built FIRST (concrete) so its broker-backed delivery/clarify seam
            // channels can be handed to SubAgents via `with_channels` (P5 handoff, CLOSING R-SA-037/
            // 119/120/123/124/125). The one-shot print/json mode is exactly what a spawned subagent
            // child re-execs into, so the child branch of `_concrete` is what registers the child
            // surface there.
            let intercom_ext = cyrup_intercom::intercom_extension_for_env_concrete(
                dirs.agent_dir.clone(),
                session_cwd.clone(),
            )
            .map_err(|e| anyhow::anyhow!("building intercom extension: {e}"))?;
            // SubAgents opt-in gate + T6 child-mode gate (see the interactive arm above): a plain
            // top-level session attaches only when opted in (`is_installed`); a plain subagent child
            // registers nothing; a fanout-authorized child gets the restricted tool regardless. Thread
            // the intercom channels in when intercom is attached, else keep the NoTransport/NoOp
            // degrade defaults.
            let subagent_ext = match &intercom_ext {
                Some(ic) => cyrup_ext_subagents::extension::subagent_extension_for_env_with_channels(
                    &dirs.agent_dir,
                    cyrup::subagent_config::load_subagent_extension_config(&dirs.agent_dir),
                    session_cwd.clone(),
                    ic.delivery_channel(),
                    ic.clarify_channel(),
                    ic.steer_channel(),
                ),
                None => cyrup_ext_subagents::extension::subagent_extension_for_env(
                    &dirs.agent_dir,
                    cyrup::subagent_config::load_subagent_extension_config(&dirs.agent_dir),
                    session_cwd.clone(),
                ),
            };
            if let Some(ext) = subagent_ext {
                factory_builder = factory_builder.with_native_extension(ext);
            }
            // SUBA-S01 (pi `pi-args.ts:13`, which loads `subagent-prompt-runtime.ts` into the child
            // as its OWN extension): a plain subagent child attaches NO subagents extension —
            // `subagent_extension_for_env` returns `None` for it by design — so the child-side
            // `structured_output` tool cannot come from that gate. This one is independent: it
            // builds only when the parent passed both structured-output env vars, i.e. only for a
            // step that actually declared an `outputSchema`. Every other process attaches nothing.
            if let Some(runtime) = cyrup_ext_subagents::prompt_runtime::prompt_runtime_extension_for_env() {
                factory_builder = factory_builder.with_native_extension(runtime);
            }
            if let Some(ic) = intercom_ext {
                factory_builder = factory_builder.with_native_extension(ic);
            }
            // Permission system (port doc §4): opt-in allow/ask/deny gate; same seam + role selection.
            // The one-shot print/json mode is exactly what a spawned subagent child re-execs into, so
            // `permission_extension_for_env` loads the child→parent ask-FORWARDING channel here when
            // this is a `CYRUP_SUBAGENT_CHILD` and the gate is installed (P-4, forwarding.rs).
            if let Some(ext) = cyrup_permission_system::permission_extension_for_env(
                dirs.agent_dir.clone(),
                session_cwd,
            ) {
                factory_builder = factory_builder.with_native_extension(ext);
            }
            let factory = Arc::new(factory_builder);
            // `create_unannounced` also binds the self-handle (via `into_shared`) so the post-run
            // loop — auto-retry, post-run auto-compaction, queued continuations — fires for one-shot
            // print/json runs.
            //
            // SEAM-033: it does NOT announce `session_start`. Pi's `createAgentSessionRuntime`
            // doesn't either (agent-session-runtime.ts:414-432); the mode does, from
            // `rebindSession()` → `bindExtensions()` at print-mode.ts:119 → :73, which is reached
            // only after `main.ts` has applied `--name` (main.ts:650) and the scoped `--models`
            // (main.ts:742-750). `apply_post_build` below is cyrup's analog of both, so announcing
            // inside the constructor would show every `session_start` handler an unnamed, unscoped
            // session — and this arm is what a spawned subagent child re-execs into, so every
            // subagent run would inherit it. `run_print_dispatch`/`run_json_dispatch` announce.
            let runtime = match AgentSessionRuntime::create_unannounced(factory, target).await {
                Ok(r) => r,
                // Non-interactive no-models-available guard (Pi main.ts:795-798).
                Err(SessionServiceError::NoModels(_)) => return no_models_available(),
                Err(e) => {
                    return Err(anyhow::Error::new(e).context("building agent session runtime"));
                }
            };
            // Pi main.ts:843-848 (SEAM-S01) — same checkpoint, every mode.
            if report_runtime_diagnostics(&runtime).await {
                runtime.dispose().await;
                cyrup::output_guard::restore_stdout();
                return Ok(1);
            }
            let session = runtime.session().await;
            apply_post_build(&session, session_name.as_deref(), &cli, fresh).await;
            timings.print();
            // `settingsManager.getImageAutoResize()` for the `@file` image path (Pi main.ts:830),
            // read before `session` moves into the signal guard.
            let auto_resize_images = session.services().settings.effective().image_auto_resize();
            let _signals = spawn_abort_on_signal(session, cancel.clone());
            // NO prompt-required guard here: Pi has none. `buildInitialMessage` answers
            // `initialMessage: undefined` for a run with no stdin/`@file`/message
            // (initial-message.ts:36-42) and `runPrintMode` simply skips its send loops
            // (print-mode.ts:121-127), falling through to the terminal output block and returning 0.
            // The `ensure_prompt` bail that used to sit here inverted the exit code of every
            // prompt-less one-shot invocation — `cyrup -c -p`, `cyrup --session <id> --mode json` —
            // and suppressed JSON mode's session header entirely. See `run::turn_inputs`.
            let inputs = build_inputs(&cli, &dirs.cwd, auto_resize_images).await?;
            let mut out = io::stdout();
            let dispatch = if let AppMode::Json = mode {
                run_json_dispatch(&runtime, &inputs, &mut out).await
            } else {
                run_print_dispatch(&runtime, &inputs, &mut out).await
            };
            // Restore stdout at teardown (Pi `finally { restoreStdout() }`, main.ts:848).
            cyrup::output_guard::restore_stdout();
            dispatch
        }
        AppMode::Interactive => unreachable!("interactive mode is handled before this match"),
    }
}

/// Apply the per-run, post-build session knobs that have no `SessionConfig` slot: the trimmed
/// `--name` display name (Pi `appendSessionInfo`, main.ts:586) and the `--models` Ctrl+P scope (Pi
/// `resolveModelScope`/`scopedModels`, main.ts:685).
///
/// The scope patterns follow Pi's precedence `parsed.models ?? settingsManager.getEnabledModels()`
/// (main.ts:685): an explicit `--models` wins, otherwise the persisted `enabledModels` setting is the
/// fallback scope source. Matching itself is delegated to `cyrup-config`'s `minimatch`-faithful
/// resolver (see [`resolve_scoped_models`]), not a bespoke matcher.
///
/// `fresh` is whether this is a brand-new session (Pi `!hasExistingSession`, main.ts:394): a resumed
/// session keeps its own restored model, so the saved-default-in-scope active-model pick only fires
/// for a fresh session.
async fn apply_post_build(session: &AgentSession, name: Option<&str>, cli: &Cli, fresh: bool) {
    if let Some(name) = name {
        let _ = session.set_session_name(name).await;
    }
    // Pi `modelPatterns = parsed.models ?? settingsManager.getEnabledModels()` (main.ts:685): an
    // explicit `--models` wins; otherwise fall back to the persisted `enabledModels` setting.
    let patterns: Vec<String> = if cli.models.is_empty() {
        session.services().settings.effective().enabled_models().unwrap_or_default()
    } else {
        cli.models.clone()
    };
    if !patterns.is_empty() {
        let catalog = session.model_catalog();
        // Pi `resolveModelScope` prints EVERY diagnostic its `WithDiagnostics` sibling collected —
        // `console.warn(chalk.yellow(`Warning: ${diagnostic.message}`))`, model-resolver.ts:355-361 —
        // before returning the (possibly empty) scope, and does so on the live path at main.ts:741-743
        // for both `--models` and the `enabledModels` fallback. Without this a typo'd
        // `--models "anthropc/*"` scoped nothing with no output at all.
        report_diagnostics(&scope_diagnostics(&catalog, &patterns));
        let scoped = resolve_scoped_models(&catalog, &patterns);
        if !scoped.is_empty() {
            // The saved-default-in-scope active-model pick (Pi `buildSessionOptions`, main.ts:394-414):
            // when `--models` scopes the set and `--model` is omitted, the active model is the saved
            // default if it is in scope, else the first scoped model. Apply only on a fresh session.
            if cli.model.is_none() && fresh {
                let eff = session.services().settings.effective();
                if let Some(chosen) = pick_scoped_active_model(
                    &scoped,
                    eff.default_provider().as_deref(),
                    eff.default_model().as_deref(),
                ) {
                    let model = chosen.model.clone();
                    let thinking = chosen.thinking_level;
                    if session.set_model_resolved(model).await.is_ok() {
                        // Use the scoped model's thinking level only when `--thinking` was omitted
                        // (explicit `--thinking` takes precedence and is applied by the builder).
                        if cli.thinking.is_none()
                            && let Some(level) = thinking
                        {
                            let _ = session.set_thinking_level(level).await;
                        }
                    }
                }
            }
            session.set_scoped_models(scoped);
        }
    }
}

/// Whether a target starts a brand-new session (Pi `!hasExistingSession`): `New`/`CreateWithId` are
/// fresh; `Resume`/`Continue`/`Fork` carry (or restore) an existing transcript + model.
fn is_fresh_target(target: &SessionTarget) -> bool {
    matches!(target, SessionTarget::New | SessionTarget::CreateWithId(_))
}

/// The saved-default-in-scope active-model pick (Pi `buildSessionOptions`, main.ts:394-414): given the
/// resolved `--models` scope and the settings default `(provider, model)`, prefer the saved default
/// when it is a member of the scope (case-insensitive `provider`+`id` match, Pi `modelsAreEqual`),
/// else the first scoped model. `None` only when the scope is empty.
fn pick_scoped_active_model<'a>(
    scoped: &'a [ScopedModel],
    saved_provider: Option<&str>,
    saved_model: Option<&str>,
) -> Option<&'a ScopedModel> {
    let saved = match (saved_provider, saved_model) {
        (Some(provider), Some(model)) if !provider.is_empty() && !model.is_empty() => {
            scoped.iter().find(|sm| {
                sm.model.provider.as_str().eq_ignore_ascii_case(provider)
                    && sm.model.id.as_str().eq_ignore_ascii_case(model)
            })
        }
        _ => None,
    };
    saved.or_else(|| scoped.first())
}

/// Resolve the `--models`/`enabledModels` patterns to a [`ScopedModel`] set against the live catalog
/// (Pi `resolveModelScope`, model-resolver.ts:269-339): each pattern (optionally `:level`-suffixed)
/// selects the catalog models whose `provider/id` (or bare `id`) matches it; duplicates are
/// de-duplicated in first-seen order.
///
/// The matching is delegated to `cyrup-config`'s `ModelResolver::resolve_scope`, a byte-for-byte
/// `minimatch({ nocase: true })` port (13,877-case verified): a pattern containing `*`/`?`/`[` is
/// matched with real path-segment-aware globbing (`*` never crosses `/`, full `?`/`[...]`/`{a,b}`/
/// extglob support), and a non-glob pattern resolves to Pi's single best (alias-preferred,
/// `localeCompare`-tie-broken) model — exactly as `resolveModelScopeWithDiagnostics` does. This
/// replaces the prior bespoke `*`-only, non-path-segment-aware substring matcher in `cli.rs`, which
/// diverged from Pi (e.g. `anthropic*` wrongly matched every anthropic model; `[...]` classes were
/// unsupported). The resolver's `ScopedModel` is mapped onto the session-svc `ScopedModel` here.
fn resolve_scoped_models(
    catalog: &[cyrup_provider::Model],
    patterns: &[String],
) -> Vec<ScopedModel> {
    cyrup_config::ModelResolver::new(catalog)
        .resolve_scope(patterns)
        .into_iter()
        .map(|sm| ScopedModel {
            model: sm.model,
            thinking_level: sm.thinking_level,
        })
        .collect()
}

/// The scope diagnostics Pi emits alongside the resolved scope (Pi `ModelScopeDiagnostic` /
/// `resolveModelScopeWithDiagnostics`, model-resolver.ts:259-350), in Pi's order: for each pattern,
/// the `invalid-thinking-level` warning first (:330-332), then the `no-match` warning when the
/// pattern selected nothing (:311-318 for the glob arm, :334-341 for the non-glob arm).
///
/// `resolve_scope` returns only the matched set, so emptiness *per pattern* is the no-match test —
/// hence the one-element slice per iteration rather than a bulk call. That keeps the glob/non-glob
/// split, the `:level` suffix stripping and the `minimatch` semantics in the single ported
/// implementation instead of duplicating them here; the only cost is that the de-duplication Pi does
/// across patterns is irrelevant to emptiness anyway (a pattern whose every match was already seen
/// still matched, and still resolves non-empty on its own).
///
/// [CYRUP-DELTA] Pi's glob arm short-circuits on `findExactModelReferenceMatch(globPattern)` before
/// running `minimatch` (:308-314), so a literal model id that happens to contain `[` or `?` never
/// reaches the no-match branch. `resolve_scope` has no such short-circuit, so such an id would warn
/// here where Pi stays silent — no shipped catalog id contains a glob metacharacter, and closing it
/// belongs in `cyrup-config`'s resolver, not in the bin.
fn scope_diagnostics(catalog: &[cyrup_provider::Model], patterns: &[String]) -> Vec<Diagnostic> {
    let resolver = cyrup_config::ModelResolver::new(catalog);
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for pattern in patterns {
        // Pi pushes the `invalid-thinking-level` warning BEFORE the no-match check, and only for the
        // non-glob arm — the glob arm silently ignores an unrecognised `:suffix` and globs the whole
        // pattern (model-resolver.ts:288-297).
        if !is_glob_pattern(pattern)
            && let Some(message) = invalid_thinking_level_message(&resolver, pattern)
        {
            diagnostics.push(Diagnostic::warning(message));
        }
        if resolver.resolve_scope(std::slice::from_ref(pattern)).is_empty() {
            diagnostics.push(Diagnostic::warning(format!(
                "No models match pattern \"{pattern}\""
            )));
        }
    }
    diagnostics
}

/// Pi's glob test — a pattern is a glob iff it contains `*`, `?` or `[` (model-resolver.ts:286,
/// mirrored at `cyrup-config` model.rs:257).
fn is_glob_pattern(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?') || pattern.contains('[')
}

/// Pi's `invalid-thinking-level` message at Pi's exact wording — `Invalid thinking level "X" in
/// pattern "Y". Using default instead.` (`parseModelPattern`, model-resolver.ts:243).
///
/// `cyrup-config`'s [`cyrup_config::ModelResolver::parse_pattern`] detects the identical condition
/// but abbreviates the text to `invalid thinking level '<suffix>'` and drops the pattern (model.rs:
/// 205-212), because on the `--model` path that string is only ever appended to a caller-composed
/// sentence. So this replays `parseModelPattern`'s colon-stripping recursion (model-resolver.ts:
/// 196-246) to recover WHICH recursion level produced it, and formats Pi's sentence there:
///
/// * a valid `:level` suffix recurses on the prefix and *propagates* the inner warning (:218-226);
/// * an invalid suffix warns at THIS level and *overwrites* any inner warning, but only when the
///   prefix itself resolves to a model (:237-245) — otherwise the inner (model-less, warning-less)
///   result is returned verbatim.
///
/// Gated on the resolver reporting a warning at all, so a pattern that simply does not match
/// produces nothing here and falls through to the `no-match` diagnostic.
fn invalid_thinking_level_message(
    resolver: &cyrup_config::ModelResolver<'_>,
    pattern: &str,
) -> Option<String> {
    resolver.parse_pattern(pattern, false).warning?;
    // A warning implies the pattern did NOT match outright (an exact/partial hit returns early with
    // `warning: None`, model-resolver.ts:200-204), so a colon split did happen.
    let idx = pattern.rfind(':')?;
    let (prefix, rest) = pattern.split_at(idx);
    let suffix = rest.get(1..).unwrap_or("");
    if cyrup_config::parse_thinking_level(suffix).is_some() {
        // Valid level — the warning came from deeper in the recursion (:218-226).
        return invalid_thinking_level_message(resolver, prefix);
    }
    // Invalid suffix — Pi warns HERE iff the prefix resolves (:237-245).
    resolver.parse_pattern(prefix, false).model.map(|_| {
        format!("Invalid thinking level \"{suffix}\" in pattern \"{pattern}\". Using default instead.")
    })
}

/// Resolve the session target with Pi's full non-interactive depth (Pi `createSessionManager`,
/// main.ts:254-350) and write it onto `config`. Returns `Some(exit_code)` when the resolution itself
/// terminates the run (a not-found ref / id-collision → 1, a declined fork → 0); `None` when it set a
/// target to build. The session listings are scanned only here, behind the caller's ref-present guard.
fn resolve_session(
    cli: &Cli,
    dirs: &ConfigDirs,
    mode: AppMode,
    config: &mut SessionConfig,
) -> anyhow::Result<Option<i32>> {
    let flags = SessionFlags {
        fork: cli.fork.clone(),
        session: cli.session.clone(),
        session_id: cli.session_id.clone(),
        r#continue: cli.r#continue,
        resume: cli.resume,
        no_session: cli.no_session,
    };
    let (locals, globals) = gather_session_refs(dirs);
    let non_interactive = mode != AppMode::Interactive;
    let mut confirm = prompt_fork_confirm;
    let resolution = resolve_session_target(
        &flags,
        &dirs.cwd,
        &locals,
        &globals,
        non_interactive,
        &mut confirm,
    );
    // Pi prints these via `console.log` (stdout) / `console.error` (stderr) verbatim — no `Error:`
    // prefix (the messages are pre-composed, e.g. `No session found matching '<arg>'`). The
    // `console.log` lines route through the stdout guard: under a non-interactive takeover (Pi's
    // swapped `process.stdout.write`) they land on stderr so they cannot corrupt the JSON/RPC stream,
    // e.g. the cross-project "Session found in different project" hint (Pi main.ts:317).
    for line in &resolution.stdout {
        cyrup::output_guard::emit_stray_line(line);
    }
    for line in &resolution.stderr {
        eprintln!("{line}");
    }

    // Interactive missing-session-cwd Continue/Cancel prompt (Pi `promptForMissingSessionCwd`,
    // main.ts:575-580): a resumed session whose stored cwd is gone is offered a continuation against
    // the current cwd, or cancels to exit 0. The non-interactive arm already errored above.
    if let Some(issue) = resolution.missing_cwd {
        let theme = UiTheme::default();
        let body =
            cyrup::format_missing_session_cwd_prompt(&issue.session_cwd, &issue.fallback_cwd);
        return match cyrup::run_missing_cwd_prompt(&theme, &body, &issue.fallback_cwd)? {
            cyrup::MissingCwdChoice::Continue => {
                // Reopen the session against the chosen (fallback) cwd (Pi `SessionManager.open(
                // sessionFile, sessionDir, selectedCwd)`, main.ts:578).
                config.target = SessionTarget::Resume(issue.session_file);
                config.cwd_override = Some(issue.fallback_cwd);
                config.persist = !cli.no_session;
                Ok(None)
            }
            // Pi `if (!selectedCwd) process.exit(0)` (main.ts:576-577).
            cyrup::MissingCwdChoice::Cancel => Ok(Some(0)),
        };
    }

    Ok(match resolution.outcome {
        Some(Outcome::Build(target)) => {
            config.target = target;
            // Recompute persistence now the target may be Resume/Fork/CreateWithId (Pi: any explicit
            // session persists; `--no-session` forces ephemeral; interactive always persists).
            let explicit = !matches!(config.target, SessionTarget::New);
            config.persist = !cli.no_session && (explicit || mode == AppMode::Interactive);
            None
        }
        Some(Outcome::ExitOk) => Some(0),
        Some(Outcome::ExitErr) => Some(1),
        None => None,
    })
}

/// The pre-launch startup-UI orchestration (Pi `cli/startup-ui.ts` + `cli/session-picker.ts` +
/// `cli/project-trust.ts`): run the interactive `--resume` picker and the project-trust prompt over
/// the cyrup-tui selectors and feed their results back into `config` before the runtime is built.
/// Returns `Some(0)` when the resume picker is cancelled (Pi `No session selected` + exit 0), else
/// `None` (proceed). Interactive-only — the caller gates on the mode so the one-shot/RPC live path is
/// never touched. TTY-bound (it drives real terminals), so it is not unit-tested; the row/label/
/// decision builders it composes are unit-tested in [`cyrup::startup_ui`].
fn resolve_startup_ui(
    cli: &Cli,
    dirs: &ConfigDirs,
    mode: AppMode,
    config: &mut SessionConfig,
) -> anyhow::Result<Option<i32>> {
    let theme = UiTheme::default();

    // --resume (#1): mount the `SessionSelector` over the merged local+global session listing and
    // resume the chosen session (Pi `selectSession`, session-picker.ts:15-55). A bare `--resume`
    // mapped to `New` in `to_session_config`; the picker resolves the real target here.
    if cli.resume && matches!(config.target, SessionTarget::New) {
        let sessions = gather_session_infos(dirs);
        match cyrup::run_resume_picker(&theme, &sessions, None)? {
            cyrup::ResumeChoice::Selected(path) => {
                // Pi runs `getMissingSessionCwdIssue(sessionManager, cwd)` UNCONDITIONALLY after
                // `createSessionManager` — which handles `--resume` by returning the opened manager
                // (main.ts:321-332,573-585). So a `--resume`-selected session whose stored cwd is gone
                // must still get the interactive Continue/Cancel prompt, exactly as the
                // `--session`/`--session-id` open paths do via `resolve_session`. The picked session's
                // stored cwd comes from its `SessionInfo` listing (Pi `sessionManager.getCwd()`).
                let stored_cwd = sessions
                    .iter()
                    .find(|s| s.path == path)
                    .map(|s| s.cwd.clone())
                    .unwrap_or_default();
                if cyrup::session_cwd_is_missing(&stored_cwd) {
                    let body =
                        cyrup::format_missing_session_cwd_prompt(&stored_cwd, &dirs.cwd);
                    match cyrup::run_missing_cwd_prompt(&theme, &body, &dirs.cwd)? {
                        // Reopen the session against the current cwd (Pi `SessionManager.open(
                        // sessionFile, sessionDir, selectedCwd)`, main.ts:580).
                        cyrup::MissingCwdChoice::Continue => {
                            config.target = SessionTarget::Resume(path);
                            config.cwd_override = Some(dirs.cwd.clone());
                            config.persist = !cli.no_session;
                        }
                        // Pi `if (!selectedCwd) process.exit(0)` (main.ts:577-578).
                        cyrup::MissingCwdChoice::Cancel => return Ok(Some(0)),
                    }
                } else {
                    config.target = SessionTarget::Resume(path);
                    config.persist = !cli.no_session;
                }
            }
            // Pi `console.log(chalk.dim("No session selected")); process.exit(0)` (main.ts:329).
            cyrup::ResumeChoice::Cancelled => {
                println!("No session selected");
                return Ok(Some(0));
            }
        }
    }

    // Project trust (#3): when the resolved trust decision needs a prompt (trust-requiring project
    // resources, no `--approve`/`--no-approve`, no saved decision, default policy `prompt`), run the
    // `TrustSelector` and feed the chosen decision in as this run's trust override (the builder honors
    // an override directly, so no rebuild is needed). Cancelling proceeds untrusted (Pi `ui.select →
    // undefined`). Pi `createProjectTrustContext` (project-trust.ts:7-62) / `resolveProjectTrusted`
    // (main.ts:610-734).
    if config.trust_override.is_none() {
        let trust_store = cyrup_config::trust::TrustStore::new(dirs.agent_dir.join("trust.json"));
        let saved = trust_store.nearest(&dirs.cwd).ok().flatten();
        let default_trust = default_project_trust(dirs);
        let has_resources =
            cyrup::has_trust_requiring_project_resources(&dirs.cwd, &dirs.agent_dir);
        if cyrup::trust_needs_prompt(
            has_resources,
            None,
            saved.as_ref().map(|e| e.decision),
            default_trust,
            mode,
        ) {
            let options = cyrup_config::trust::trust_options(&dirs.cwd, false);
            if let Some(trusted) =
                cyrup::run_trust_prompt(&theme, &dirs.cwd, &options, &saved, &trust_store)?
            {
                config.trust_override = Some(trusted);
            }
        }
    }

    Ok(None)
}

/// The global-only `defaultProjectTrust` policy (Pi `getDefaultProjectTrust`), read from the file
/// settings store with the project scope untrusted (matching the startup settings manager).
fn default_project_trust(dirs: &ConfigDirs) -> DefaultProjectTrust {
    let mgr = SettingsManager::load(file_settings_store(dirs), Settings::new(), false);
    mgr.effective().default_project_trust()
}

/// The [`SessionLayout`] the `--resume` listing scans, mirroring Pi's per-call directory choice: an
/// explicit `--session-dir` is used LITERALLY, otherwise the cwd-encoded default applies
/// (`sessionDir ? normalizePath(sessionDir) : getDefaultSessionDir(cwd)`, session-manager.ts:1538).
/// This must agree with the write-side layout in `SessionServiceBuilder::build`, or a session written
/// under an explicit `--session-dir` would be listed at a different (doubly-nested) path
/// (gap-analysis 05, Finding 3).
fn session_list_layout(dirs: &ConfigDirs) -> SessionLayout {
    if dirs.session_dir_explicit {
        SessionLayout::literal(dirs.session_dir.clone(), dirs.cwd.clone())
    } else {
        SessionLayout::new(dirs.session_dir.clone(), dirs.cwd.clone())
    }
}

/// Pi's shared-directory cwd filter for the LOCAL listing:
/// `const filterCwd = sessionDir !== undefined && dir !== getDefaultSessionDirPath(cwd)`
/// (`SessionManager.list`, session-manager.ts:1639-1640), applied as
/// `.filter((session) => !filterCwd || sessionCwdMatches(session.cwd, resolvedCwd))` (:1641-1643).
/// A custom `--session-dir` may hold SEVERAL projects' sessions in one flat directory, so the local
/// listing must keep only this cwd's; the cwd-encoded default already isolates by cwd, so it never
/// filters — and an explicit dir that happens to BE the cwd-encoded default is likewise not filtered
/// (Pi compares the resolved paths, not just "was it explicit"). This is the same predicate the
/// CONTINUE path computes for `continue_recent_filtered` (`SessionServiceBuilder::build`,
/// builder.rs:576-583; Pi `continueRecent`, session-manager.ts:1558-1559).
fn session_list_cwd_filter(dirs: &ConfigDirs) -> Option<&Path> {
    if !dirs.session_dir_explicit {
        return None;
    }
    let default_dir = SessionLayout::new(dirs.agent_dir.join("sessions"), dirs.cwd.clone()).dir();
    (dirs.session_dir != default_dir).then_some(dirs.cwd.as_path())
}

/// Write Pi's exit hint — `To resume this session: cyrup [--session-dir DIR] --session ID` — on the
/// way out of interactive mode (`interactive-mode.ts:3594-3597`, using `formatResumeCommand`,
/// `:231-244`).
///
/// The gates (tty stdout, a persisted session, a session file that exists) live in
/// [`cyrup_tui::format_resume_command`]; this function's whole job is to resolve the four inputs off
/// the live session. `default_session_dir` is Pi's `getDefaultSessionDirPath(cwd)` — the SAME
/// cwd-encoded path [`session_list_cwd_filter`] compares against — so the `--session-dir` argument is
/// printed exactly when the session is not where a bare relaunch would look for it.
async fn print_resume_hint(dirs: &ConfigDirs, session: &AgentSession) {
    use std::io::Write;

    use cyrup_tui::crossterm::tty::IsTty;

    let session_file = session.session_file().await;
    let default_session_dir =
        SessionLayout::new(dirs.agent_dir.join("sessions"), dirs.cwd.clone()).dir();
    let target = cyrup_tui::ResumeTarget {
        session_id: session.session_id().as_str(),
        session_file: session_file.as_deref(),
        session_dir: session.session_dir(),
        default_session_dir: &default_session_dir,
    };
    let Some(command) = cyrup_tui::format_resume_command(&target, std::io::stdout().is_tty()) else {
        return;
    };
    let mut out = std::io::stdout();
    let _ = out.write_all(cyrup_tui::resume_hint_line(&command).as_bytes());
    let _ = out.flush();
}

/// The cross-project listing, mirroring Pi's TWO `SessionManager.listAll` overloads
/// (session-manager.ts:1653-1655). With a custom `sessionDir` it degenerates to
/// `listSessionsFromDir(customSessionDir)` — that ONE directory, newest-first, no cross-project walk
/// and (unlike `list`) no cwd filter, so the picker can still reach another project's session parked
/// in the shared dir (session-manager.ts:1660-1665). Without one it walks every project directory
/// under the sessions root (:1667+). Handing an explicit `--session-dir` to the root walk instead
/// would scan its SUBdirectories and return nothing for a flat shared dir.
fn list_global_sessions(dirs: &ConfigDirs) -> Vec<SessionInfo> {
    if dirs.session_dir_explicit {
        // Pi's `listAll(sessionDir)` overload — an unfiltered single-directory scan, i.e.
        // `cyrup_session::listing::list_all_in_dir`, which is `list_in_dir(dir, None, …)`.
        list_in_dir(&dirs.session_dir, None, None)
    } else {
        list_all(&SessionsRoot(dirs.session_dir.clone()))
    }
}

/// Scan the cwd's local session listing and the global cross-project listing into a merged
/// [`SessionInfo`] vector (locals first, globals de-duplicated by path) for the `--resume` picker (Pi
/// `selectSession`'s `current`/`all` `SessionsLoader`s, session-picker.ts:23-25 — fed by
/// `SessionManager.list(cwd, sessionDir, onProgress)` / `SessionManager.listAll(sessionDir,
/// onProgress)`, main.ts:372-373).
fn gather_session_infos(dirs: &ConfigDirs) -> Vec<SessionInfo> {
    let layout = session_list_layout(dirs);
    let mut sessions = list_in_dir(&layout.dir(), session_list_cwd_filter(dirs), None);
    for global in list_global_sessions(dirs) {
        if !sessions.iter().any(|s| s.path == global.path) {
            sessions.push(global);
        }
    }
    sessions
}

/// Scan the cwd's session listing and the global cross-project listing into [`SessionRef`]s (Pi
/// `SessionManager.list(cwd, sessionDir)` + `SessionManager.listAll(sessionDir)`, main.ts:218,227).
fn gather_session_refs(dirs: &ConfigDirs) -> (Vec<SessionRef>, Vec<SessionRef>) {
    let layout = session_list_layout(dirs);
    let locals: Vec<SessionRef> = list_in_dir(&layout.dir(), session_list_cwd_filter(dirs), None)
        .iter()
        .map(SessionRef::from)
        .collect();
    let globals: Vec<SessionRef> = list_global_sessions(dirs)
        .iter()
        .map(SessionRef::from)
        .collect();
    (locals, globals)
}

/// The plain-stdin fork-into-cwd confirmation (Pi `promptConfirm`, main.ts:191-203): a cooked-mode
/// `[y/N]` readline (NOT the TUI dialog host), run before any terminal takeover. Defaults to `no`.
///
/// The prompt itself routes through the stdout guard: Pi's `promptConfirm` writes it via readline to
/// `process.stdout`, which the stdout takeover redirects to stderr, so under a non-interactive
/// `--mode json`/`--mode rpc` run the `[y/N]` prompt lands on stderr and cannot corrupt the protocol
/// stream on stdout (the answer is still read from stdin).
fn prompt_fork_confirm() -> bool {
    cyrup::output_guard::emit_stray("Fork this session into current directory? [y/N] ");
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    let answer = line.trim().to_ascii_lowercase();
    answer == "y" || answer == "yes"
}

/// The `@file` references in the CLI positionals (Pi `parsed.fileArgs`). Used to reject `@file` in
/// RPC mode (main.ts:540).
fn split_file_args(cli: &Cli) -> Vec<String> {
    cyrup::split_positionals(&cli.positionals).0
}

/// `--export <file> [output.html]` (Pi `exportFromFile`, main.ts:520-531): read a session `.jsonl`,
/// render it to standalone HTML, write it, and exit. The optional second positional is the output
/// path (else `<input-stem>.html`). On success prints `Exported to: {path}`; on failure prints
/// `Error: {msg}` and exits 1 (Pi's exact messages).
async fn export_session_html(input: &std::path::Path, output: Option<&str>) -> anyhow::Result<i32> {
    let out_path = match output {
        Some(p) => std::path::PathBuf::from(p),
        None => input.with_extension("html"),
    };
    let result: anyhow::Result<()> = async {
        let jsonl = tokio::fs::read_to_string(input)
            .await
            .with_context(|| format!("reading session file {}", input.display()))?;
        let html = cyrup_session_svc::session_jsonl_to_html(&jsonl);
        tokio::fs::write(&out_path, html)
            .await
            .with_context(|| format!("writing HTML to {}", out_path.display()))?;
        Ok(())
    }
    .await;
    match result {
        Ok(()) => {
            println!("Exported to: {}", out_path.display());
            Ok(0)
        }
        Err(err) => {
            eprintln!("Error: {err}");
            Ok(1)
        }
    }
}

/// Humanise a token count (Pi `formatTokenCount`, list-models.ts:14-24): `200000` → `200K`,
/// `1000000` → `1M`, `1500000` → `1.5M`. Whole values drop the decimal.
fn format_token_count(count: u64) -> String {
    if count >= 1_000_000 {
        let millions = count as f64 / 1_000_000.0;
        if (millions.fract()).abs() < f64::EPSILON {
            format!("{}M", millions as u64)
        } else {
            format!("{millions:.1}M")
        }
    } else if count >= 1_000 {
        let thousands = count as f64 / 1_000.0;
        if (thousands.fract()).abs() < f64::EPSILON {
            format!("{}K", thousands as u64)
        } else {
            format!("{thousands:.1}K")
        }
    } else {
        count.to_string()
    }
}

/// Token-based fuzzy membership (Pi `fuzzyFilter` over `"{provider} {id}"`, list-models.ts:45): each
/// whitespace-separated token of `search` must be a case-insensitive subsequence of the haystack.
fn fuzzy_match(haystack: &str, search: &str) -> bool {
    let hay = haystack.to_ascii_lowercase();
    search.split_whitespace().all(|token| {
        let mut hay_chars = hay.chars();
        token
            .to_ascii_lowercase()
            .chars()
            .all(|c| hay_chars.any(|h| h == c))
    })
}

/// `--list-models [search]` (Pi `listModels`, list-models.ts:29-110): print the provider catalog as
/// an aligned `provider/model/context/max-out/thinking/images` table — token counts humanised, sorted
/// by provider then id, fuzzy-filtered by `search` — with Pi's `No models matching "x"` empty message.
fn list_models(models: &[cyrup_provider::Model], search: &str) -> anyhow::Result<i32> {
    use cyrup_provider::Modality;
    if models.is_empty() {
        // Pi `formatNoModelsAvailableMessage` (auth-guidance.ts:14) — the no-models guidance text.
        println!("{}", cyrup::format_no_models_available_message());
        return Ok(0);
    }

    let mut filtered: Vec<&cyrup_provider::Model> = if search.is_empty() {
        models.iter().collect()
    } else {
        models
            .iter()
            .filter(|m| {
                fuzzy_match(
                    &format!("{} {}", m.provider.as_str(), m.id.as_str()),
                    search,
                )
            })
            .collect()
    };
    if filtered.is_empty() {
        println!("No models matching \"{search}\"");
        return Ok(0);
    }

    // Sort by provider, then by model id (Pi list-models.ts:54-58).
    filtered.sort_by(|a, b| {
        a.provider
            .as_str()
            .cmp(b.provider.as_str())
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });

    struct Row {
        provider: String,
        model: String,
        context: String,
        max_out: String,
        thinking: String,
        images: String,
    }
    let rows: Vec<Row> = filtered
        .iter()
        .map(|m| Row {
            provider: m.provider.as_str().to_string(),
            model: m.id.as_str().to_string(),
            context: format_token_count(m.context_window),
            max_out: format_token_count(m.max_tokens),
            thinking: if m.reasoning { "yes" } else { "no" }.to_string(),
            images: if m.input.contains(&Modality::Image) {
                "yes"
            } else {
                "no"
            }
            .to_string(),
        })
        .collect();

    let hdr = (
        "provider", "model", "context", "max-out", "thinking", "images",
    );
    let w_provider = rows
        .iter()
        .map(|r| r.provider.len())
        .chain([hdr.0.len()])
        .max()
        .unwrap_or(0);
    let w_model = rows
        .iter()
        .map(|r| r.model.len())
        .chain([hdr.1.len()])
        .max()
        .unwrap_or(0);
    let w_context = rows
        .iter()
        .map(|r| r.context.len())
        .chain([hdr.2.len()])
        .max()
        .unwrap_or(0);
    let w_max = rows
        .iter()
        .map(|r| r.max_out.len())
        .chain([hdr.3.len()])
        .max()
        .unwrap_or(0);
    let w_think = rows
        .iter()
        .map(|r| r.thinking.len())
        .chain([hdr.4.len()])
        .max()
        .unwrap_or(0);
    let w_img = rows
        .iter()
        .map(|r| r.images.len())
        .chain([hdr.5.len()])
        .max()
        .unwrap_or(0);

    println!(
        "{:<w_provider$}  {:<w_model$}  {:<w_context$}  {:<w_max$}  {:<w_think$}  {:<w_img$}",
        hdr.0, hdr.1, hdr.2, hdr.3, hdr.4, hdr.5
    );
    for r in &rows {
        println!(
            "{:<w_provider$}  {:<w_model$}  {:<w_context$}  {:<w_max$}  {:<w_think$}  {:<w_img$}",
            r.provider, r.model, r.context, r.max_out, r.thinking, r.images
        );
    }
    Ok(0)
}

/// The terminal-query drain window for the startup benchmark (Pi `setTimeout(resolve, 150)`,
/// main.ts:826): the brief pause that lets the TUI's stdin handler consume the terminal's query
/// replies (Kitty keyboard protocol, device attributes, cell size) before the terminal is restored.
const BENCHMARK_DRAIN_MS: u64 = 150;

/// The `PI_STARTUP_BENCHMARK` interactive teardown (Pi main.ts:819-835): initialise the TUI over the
/// real terminal (Pi `interactiveMode.init()`), give the stdin handler [`BENCHMARK_DRAIN_MS`] to drain
/// the terminal's query replies, then stop + restore — measuring cold startup without running the
/// event loop. TTY-bound (it owns a real `CrosstermBackend`), so it is not unit-tested.
async fn run_interactive_benchmark() -> anyhow::Result<()> {
    let mut app = App::into_stdout(UiTheme::default()).context("initialising the terminal UI")?;
    app.detect_image_support();
    tokio::time::sleep(std::time::Duration::from_millis(BENCHMARK_DRAIN_MS)).await;
    let _ = app.restore();
    Ok(())
}

/// Assemble Pi's startup panel input from the live session (TUI-006). The listing halves come from
/// the session's own resource snapshot / context store / extension host; the diagnostics half comes
/// from `AgentSessionServices::startup_diagnostics`, which the builder now retains instead of
/// discarding (`showLoadedResources`, interactive-mode.ts:1519-1690).
fn build_startup_report(session: &AgentSession, verbose: bool) -> cyrup_tui::StartupReport {
    use cyrup_resources::ResourceKind;
    let services = session.services();
    let home = Some(services.home.as_path());
    let snapshot = services.context.snapshot();
    cyrup_tui::StartupReport {
        verbose,
        quiet_startup: services.settings.effective().quiet_startup(),
        // Pi's `[Context]` list is the system-prompt source + appended prompts + the `AGENTS.md`
        // chain, in load order (`:1551-1555`, `{sort: false}`).
        context_files: snapshot
            .context_files
            .iter()
            .map(|f| cyrup_tui::display_path(&f.path, home))
            .collect(),
        skills: services.resources.skills.all().iter().map(|s| s.name.clone()).collect(),
        // Prompt templates list as their slash command (Pi `/${template.name}`, `:1596`).
        prompts: services
            .resources
            .prompts
            .all()
            .iter()
            .map(|p| format!("/{}", p.name))
            .collect(),
        extensions: services.ext_host.loaded_ids().iter().map(|id| id.to_string()).collect(),
        // Built-ins are excluded — Pi lists only themes with a `sourcePath` (`:1615`).
        themes: services
            .resources
            .themes
            .all()
            .iter()
            .filter(|t| t.origin_path.is_some())
            .map(|t| t.data.name.clone())
            .collect(),
        skill_diagnostics: cyrup_tui::resource_diagnostics(
            &services.startup_diagnostics.resources,
            ResourceKind::Skill,
            home,
        ),
        prompt_diagnostics: cyrup_tui::resource_diagnostics(
            &services.startup_diagnostics.resources,
            ResourceKind::Prompt,
            home,
        ),
        // The whole extension vector, Pi-faithfully (`:1660-1665` maps every recorded error into the
        // block). In practice only the NON-fatal entries — the project-trust skips — are reachable
        // here: a genuine load failure is reported and exits 1 at `report_runtime_diagnostics`, well
        // before this panel is built, exactly as Pi's `main.ts:843-849` precedes `InteractiveMode`.
        extension_diagnostics: cyrup_tui::extension_diagnostics(
            &services.startup_diagnostics.extensions,
            home,
        ),
        theme_diagnostics: cyrup_tui::resource_diagnostics(
            &services.startup_diagnostics.resources,
            ResourceKind::Theme,
            home,
        ),
    }
}

/// The interactive front-end: build the TUI over a real `CrosstermBackend<Stdout>`, seed any initial
/// prompt, and run the event loop against the live session. Restores the terminal on exit.
async fn run_interactive(
    runtime: Arc<AgentSessionRuntime>,
    session: Arc<AgentSession>,
    inputs: Inputs,
    // `--verbose` — Pi's `options.verbose`, which overrides `quietStartup` for the startup listing
    // (`cli.rs:818` has always advertised exactly that; TUI-006 makes it true).
    verbose: bool,
    cancel: CancelToken,
    // The detached startup package-update check's answer channel (Pi `interactive-mode.ts:850-856`);
    // `None` when the network policy declined. Handed straight to the run loop.
    package_updates: Option<tokio::sync::mpsc::UnboundedReceiver<Vec<String>>>,
) -> anyhow::Result<()> {
    // Boot the render theme from `settings.theme` + the terminal background/color-depth (feature #4:
    // the `ThemeController`), instead of the hardwired dark boot the audit flagged (theme.rs #4). An
    // unset/`auto` setting resolves against the detected terminal polarity; every role is projected
    // into the detected `ColorMode` (feature #3) so 256-color terminals get indexed colors.
    let theme_setting = session.services().settings.effective().theme_setting();
    let mut controller = ThemeController::boot_from_env(theme_setting.as_deref());
    let mut app = App::into_stdout(controller.theme()).context("initialising the terminal UI")?;
    // TUI-004: now that `into_stdout` has raw mode on — and BEFORE `crossterm_input_stream` spawns
    // the reader thread that would race us for the reply bytes — complete Pi's boot detection by
    // actually ASKING the terminal (OSC 11, and DSR `?996` for an `auto` setting) instead of
    // trusting `COLORFGBG`, which most terminals never set. The probe is hard-bounded at Pi's
    // 100 ms (`theme-controller.ts:41,53`) and consumes nothing when the terminal stays silent; see
    // `cyrup_tui::terminal_query` for the timeout / input-safety contract.
    let colorfgbg = std::env::var("COLORFGBG").unwrap_or_default();
    if let Some(theme) = controller.sync_with_terminal(
        &StdinTerminalProbe,
        std::time::Duration::from_millis(100),
        &colorfgbg,
    ) {
        app.set_theme(theme);
    }
    // Pi persists a HIGH-confidence detection back to `settings.theme` so the next boot skips the
    // query entirely (`theme-controller.ts:57-61`). A low-confidence fallback is never written.
    if let Some(name) = controller.theme_to_persist() {
        let _ = session.persist_setting(
            cyrup_session_svc::SettingsScope::Global,
            "theme",
            serde_json::Value::String(name.to_string()),
        );
    }
    app.detect_image_support();
    seed_footer(&mut app, &runtime, &session).await;
    // Pi shows the package-update notification whenever the detached check settles, which is why the
    // channel — not the answer — is what reaches the loop (`interactive-mode.ts:850-856`).
    app.set_package_update_channel(package_updates);

    // Configurable keybindings (feature #2; Pi `KeybindingsManager.create`, keybindings.ts:348-352):
    // load the user's `~/.cyrup/keybindings.json` and merge it into every live keymap (global/editor/
    // selector/tree). Absent file ⇒ defaults; a malformed file logs to stderr and keeps the defaults.
    let keybindings_path = session.services().agent_dir.join("keybindings.json");
    if let Ok(json) = std::fs::read_to_string(&keybindings_path)
        && let Err(e) = app.load_keybindings_json(&json)
    {
        eprintln!("warning: ignoring {}: {e}", keybindings_path.display());
    }

    // Autocomplete dropdown height (feature #6; Pi `autocompleteMaxVisible`, default 5, clamped 3–20).
    let max_visible = session
        .services()
        .settings
        .effective()
        .autocomplete_max_visible();
    app.set_autocomplete_max_visible(max_visible.clamp(3, 20) as u16);

    // Reserve the idle status band to avoid reflow (feature #9; Pi `terminal.clearOnShrink`,
    // interactive-mode.ts:1638-1642 — an idle status container is cleared only when clearOnShrink is
    // off, so `reserve_status_rows == clearOnShrink`).
    let env_vars = cyrup_session_svc::EnvVars::from_process();
    let reserve = session
        .services()
        .settings
        .effective()
        .clear_on_shrink(&env_vars);
    app.set_reserve_status_rows(reserve);

    // Extension keyboard shortcuts (feature #10; Pi `registerShortcut`): source the registered
    // shortcut key-ids from the session's extension host so a matching press routes to the owning
    // live extension's `execute-shortcut` (refreshed after a session swap inside the run loop).
    app.set_extension_shortcuts(session.services().ext_host.shortcut_keys());

    // Theme hot-reload (feature #1; Pi `ThemeWatcher`, theme.ts watch path): when the active theme
    // resolves to an on-disk file, watch it so `/theme` edits repaint live. The watcher must outlive
    // `app.run`, so it is bound here; a built-in (no `origin_path`) has nothing to watch (`None`).
    let mut _theme_watcher: Option<ThemeWatcher> = None;
    let theme_rx = build_theme_watcher(&session, controller.active_name(), &cancel).map(|w| {
        let rx = w.subscribe();
        _theme_watcher = Some(w);
        rx
    });

    // TUI-006: the startup loaded-resources / diagnostics panel (Pi `showLoadedResources`,
    // interactive-mode.ts:1480-1690, invoked with `showDiagnosticsWhenQuiet: true` at `:1769`).
    // Pushed BEFORE the replay + the first prompt so it heads the scrollback, and before the
    // reader thread starts. `quietStartup` hides the inventory; it never hides a load failure.
    app.push_loaded_resources(&build_startup_report(&session, verbose));

    let input_stream = crossterm_input_stream(cancel.clone());
    let events = session.subscribe();

    // TUI-003: a `--resume`/`--continue` boot starts on an existing branch, so seed the transcript
    // from it before the first frame — Pi's `renderInitialMessages()` (interactive-mode.ts:3548).
    // A fresh session has no messages and replays nothing. `raw_context_messages` keeps the
    // `compactionSummary`/`branchSummary`/`custom`/`bashExecution` roles that `messages()` would
    // have flattened to `user` prose at the LLM boundary (Pi feeds `renderSessionEntries` the same
    // raw projection, interactive-mode.ts:3506-3516).
    let restored = session.raw_context_messages().await;
    if !restored.is_empty() {
        // X11 — WITH the loaded extensions: Pi resolves `getMessageRenderer(message.customType)` on
        // the replay walk (`interactive-mode.ts:3471`) exactly as it does on the live
        // `addMessageToChat` path, so a `--resume`d session keeps the extension rendering it had.
        app.replay_session_with_extensions(&restored, &session.services().ext_host).await;
    }

    if !inputs.is_empty() {
        app.state_mut().transcript.push_user(inputs.initial.clone());
        let _ = session.prompt_accepted(initial_input(&inputs)).await;
        // Queue any follow-up CLI messages into the interactive loop (Pi `initialMessages`,
        // main.ts:816): each becomes a sequential turn after the first.
        for follow_up in &inputs.follow_ups {
            let _ = session
                .prompt_accepted(UserInput::text(follow_up.clone(), InputSource::Cli))
                .await;
        }
    }

    let result = app
        .run(
            input_stream,
            events,
            session.clone(),
            Some(runtime),
            theme_rx,
            cancel,
        )
        .await;
    // `App::run` already drained and restored on its way out (app.rs, `drain_and_restore`). This is
    // the idempotent safety net for the error paths that leave `run` early — restore only, since
    // draining after raw mode is gone accomplishes nothing.
    let _ = app.restore();
    result.map_err(|e| anyhow::anyhow!("tui: {e}"))?;
    Ok(())
}

/// Build a [`ThemeWatcher`] for the active theme when it resolves to an on-disk file (feature #1).
/// Returns `None` for a compiled-in built-in (no `origin_path` — nothing editable to watch) or when
/// the file watcher cannot be spawned (hot-reload simply stays off; never fatal). The watcher's
/// channel seeds with the theme's current [`ThemeData`], so the run loop's `theme_changed` arm fires
/// on every subsequent edit of that file (`/theme` edits + a settings.theme pointed at a file theme).
fn build_theme_watcher(
    session: &AgentSession,
    active_name: &str,
    cancel: &CancelToken,
) -> Option<ThemeWatcher> {
    let theme = session.services().resources.themes.get_name(active_name)?;
    let path = theme.origin_path.clone()?;
    let seed = Arc::new(theme.data.clone());
    match ThemeWatcher::spawn(seed, path.clone(), cancel.clone()) {
        Ok(w) => Some(w),
        Err(e) => {
            eprintln!(
                "warning: theme hot-reload disabled for {}: {e}",
                path.display()
            );
            None
        }
    }
}

/// Seed the footer + editor from the **live session/runtime** before the interactive loop starts
/// (audit #2/#5): the footer's model/provider/cwd/context/reasoning and the editor's thinking-level
/// rule are only ever moved by *change* events (`ModelChanged`/`ThinkingLevelChanged`), which never
/// fire for the initial selection — so without this the footer shows the literal `no-model` and a
/// blank location line all session, and the editor's border ignores the active reasoning level. This
/// is the `FooterDataProvider` the audit calls for: `cyrup-session-svc` → `cyrup-tui` footer data.
async fn seed_footer<B: cyrup_tui::RebuildBackend>(
    app: &mut App<B>,
    runtime: &AgentSessionRuntime,
    session: &AgentSession,
) {
    let model = session.model();
    let provider = model.provider.as_str().to_string();
    let model_id = model.model.as_str().to_string();
    let status = app.status_mut();
    status.set_model(format!("{provider}/{model_id}"));
    status.set_provider(Some(provider.clone()));

    // Reasoning support + provider breadth from the resolved catalog (drives the ` • {level}` suffix
    // and the `(provider)` prefix gate, footer.ts:184-199).
    let catalog = session.model_catalog();
    let reasoning = catalog
        .iter()
        .find(|m| m.provider.as_str() == provider && m.id.as_str() == model_id)
        .map(|m| m.reasoning)
        .unwrap_or(false);
    status.set_reasoning(reasoning);
    let mut providers: Vec<&str> = catalog.iter().map(|m| m.provider.as_str()).collect();
    providers.sort_unstable();
    providers.dedup();
    status.set_provider_count(providers.len());

    // Location line (`cwd (branch) • name`, footer.ts:116-130).
    status.set_cwd(home_relative(runtime.cwd()));
    // …and the `(branch)` half of it, which Pi reads from its `FooterDataProvider`
    // (`footer.ts:117` → `footer-data-provider.ts` `getGitBranch()`). This is the sole production
    // caller: before it existed `StatusLine::set_branch` had only test callers, so the segment could
    // never appear in a real session. Constructed from the RUNTIME's cwd, the same value Pi passes
    // (`new FooterDataProvider(sessionManager.getCwd())`), not the process cwd — a `--resume` of a
    // session recorded elsewhere must show THAT tree's branch.
    let cwd = runtime.cwd().to_path_buf();
    app.set_footer_git_cwd(&cwd);

    // Thinking level → footer suffix + editor rule color (spec/tui/03 §3.3, footer.ts:186-188).
    let level = thinking_level_str(session.thinking_level().await);
    app.status_mut().set_thinking_level(level);
    app.editor_mut().set_thinking_level(level);
}

/// The lowercase footer/editor string for a [`ModelThinkingLevel`] (matches the thinking-selector
/// values + the `theme.thinking_border_style` keys).
fn thinking_level_str(level: cyrup_sdk::core::ModelThinkingLevel) -> &'static str {
    use cyrup_sdk::core::ModelThinkingLevel as L;
    match level {
        L::Off => "off",
        L::Minimal => "minimal",
        L::Low => "low",
        L::Medium => "medium",
        L::High => "high",
        L::Xhigh => "xhigh",
        L::Max => "max",
    }
}

/// Render `path` with the home prefix collapsed to `~` (Pi footer cwd display, footer.ts:120).
fn home_relative(path: &std::path::Path) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::Path::new(&home);
        if let Ok(rel) = path.strip_prefix(home) {
            if rel.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rel.display());
        }
    }
    path.display().to_string()
}

/// Report parse/settings diagnostics to **stderr** (Pi `reportDiagnostics`, main.ts:87-93): warnings
/// prefixed `Warning:`, errors `Error:`. Colour is omitted (no colour dep at the bin boundary).
fn report_diagnostics(diagnostics: &[Diagnostic]) {
    for d in diagnostics {
        match d.level {
            DiagnosticLevel::Error => eprintln!("Error: {}", d.message),
            DiagnosticLevel::Warning => eprintln!("Warning: {}", d.message),
        }
    }
}

/// Pi's SECOND `reportDiagnostics` checkpoint — `reportDiagnostics(runtime.diagnostics)` +
/// `process.exit(1)` on any error (main.ts:843-848). Returns `true` when the caller must exit 1.
///
/// SEAM-S01: `AgentSessionRuntime::diagnostics()` had NO production consumer, which is why a
/// mistyped `--flag` (captured as an extension flag, then owned by no loaded extension) was
/// swallowed with no message and exit 0. Runs in every mode, exactly like Pi's single call site,
/// which sits after runtime creation and before the mode dispatch.
///
/// EXT-S01: extension LOAD failures ride this channel too. Containment (one built-in's failing
/// `init()` no longer aborts the whole build) is Pi's `loader.ts:537-540` `errors.push(...); continue`
/// — but Pi then LIFTS those errors onto `runtime.diagnostics` (`main.ts:735-738`) and exits 1 on
/// them, including Pi's `EXTENSION_LOAD_FAILURE_HINT` (`main.ts:61`, `:844-846`), reproduced below.
/// Routing them to the interactive-only `[Extension issues]` panel alone would leave print/json/rpc
/// silent at exit 0 — and cyrup's natives include the permission gate, so that would be fail-OPEN.
async fn report_runtime_diagnostics(runtime: &AgentSessionRuntime) -> bool {
    let diagnostics = runtime.diagnostics().await;
    let mut fatal = false;
    for d in &diagnostics {
        if d.severity == "error" {
            fatal = true;
            eprintln!("Error: {}", d.message);
        } else {
            eprintln!("Warning: {}", d.message);
        }
    }
    // Pi `main.ts:844-846`: matched on the message text, over ALL diagnostics, not just the errors.
    if fatal && diagnostics.iter().any(|d| d.message.contains(EXTENSION_LOAD_FAILURE_MARKER)) {
        eprintln!("{EXTENSION_LOAD_FAILURE_HINT}");
    }
    fatal
}

/// Pi `main.ts:844` — the substring that selects the extension-load hint.
const EXTENSION_LOAD_FAILURE_MARKER: &str = "Failed to load extension";

/// Pi `EXTENSION_LOAD_FAILURE_HINT` (main.ts:61), rebranded to cyrup's own `-ne` short flag.
const EXTENSION_LOAD_FAILURE_HINT: &str = "Hint: Start without extensions using \"cyrup -ne\".";

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

/// The non-interactive no-models-available exit (Pi main.ts:795-798): print the provider login
/// guidance to stderr and return exit code 1.
fn no_models_available() -> anyhow::Result<i32> {
    eprintln!("{}", format_no_models_available_message());
    Ok(1)
}

/// Initialise `tracing` to **stderr**, honouring `RUST_LOG`. Off by default; `--verbose` raises the
/// floor to `debug`. Idempotent and never fatal.
fn init_tracing(verbose: bool) {
    use tracing_subscriber::{EnvFilter, fmt};
    let default = if verbose { "debug" } else { "warn" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .try_init();
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::{
        DiagnosticLevel, ScopedModel, SessionTarget, format_token_count, fuzzy_match,
        is_fresh_target, pick_scoped_active_model, resolve_scoped_models, scope_diagnostics,
    };

    /// The `--models`/`enabledModels` scope must report Pi's diagnostics, not resolve in silence
    /// (Pi `resolveModelScopeWithDiagnostics` → `resolveModelScope`, model-resolver.ts:270-361;
    /// live path main.ts:741-743). Before the fix `resolve_scope` returned only the matched set and
    /// `apply_post_build` dropped everything else on the floor, so a typo'd pattern was a silent
    /// no-op.
    #[test]
    fn scope_diagnostics_report_no_match_and_invalid_thinking_level_like_pi() {
        let catalog = cyrup::provider::all_available_models(&cyrup_config::ModelFile::default());

        // A pattern that matches nothing warns, in BOTH arms — the glob arm
        // (model-resolver.ts:311-318) and the non-glob arm (:334-341).
        for pattern in ["anthropc/*", "no-such-model-anywhere"] {
            let diags = scope_diagnostics(&catalog, &[pattern.to_string()]);
            assert_eq!(diags.len(), 1, "{pattern}: {diags:?}");
            let only = diags.first().expect("one diagnostic");
            assert_eq!(only.level, DiagnosticLevel::Warning);
            assert_eq!(only.message, format!("No models match pattern \"{pattern}\""));
        }

        // A pattern that DOES match is silent.
        assert!(
            scope_diagnostics(&catalog, &["anthropic/*".to_string()]).is_empty(),
            "a matching pattern emits no diagnostic"
        );

        // An invalid `:level` suffix on a resolving pattern warns with Pi's exact sentence
        // (`parseModelPattern`, model-resolver.ts:243) and does NOT also warn no-match — the model
        // still resolves, at the default thinking level.
        let diags = scope_diagnostics(&catalog, &["claude-opus-4-8:hihg".to_string()]);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(
            diags.first().expect("one diagnostic").message,
            "Invalid thinking level \"hihg\" in pattern \"claude-opus-4-8:hihg\". Using default instead."
        );

        // A VALID `:level` is not a diagnostic at all.
        assert!(
            scope_diagnostics(&catalog, &["claude-opus-4-8:high".to_string()]).is_empty(),
            "a valid thinking level is silent"
        );

        // Both warnings can ride on one pattern list, in pattern order.
        let diags = scope_diagnostics(
            &catalog,
            &["claude-opus-4-8:hihg".to_string(), "anthropc/*".to_string()],
        );
        assert_eq!(diags.len(), 2, "{diags:?}");
        let messages: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert!(messages.first().is_some_and(|m| m.starts_with("Invalid thinking level")));
        assert!(messages.get(1).is_some_and(|m| m.starts_with("No models match pattern")));
    }

    /// The live `--models`/`enabledModels` scope resolution must go through `cyrup-config`'s
    /// `minimatch`-faithful `ModelResolver::resolve_scope`, NOT the removed bespoke `*`-only matcher
    /// (gap-analysis 06 Gap #1; Pi `resolveModelScope`, model-resolver.ts:269-339). These are the
    /// exact divergences the crude matcher got wrong, verified against a real bundled catalog.
    #[test]
    fn resolve_scoped_models_uses_minimatch_semantics_like_pi() {
        let catalog = cyrup::provider::all_available_models(&cyrup_config::ModelFile::default());

        // Path-segment awareness: a 1-segment pattern (`anthropic*`, no `**`) can NEVER match the
        // 2-segment `anthropic/<id>` form under minimatch. The old crude matcher wrongly matched
        // EVERY anthropic model (its single segment anchored via plain substring across the `/`).
        //
        // It CAN match a bare id that genuinely begins "anthropic", and since `amazon-bedrock` was
        // ported that is no longer a hypothetical: Bedrock ids are dotted, e.g.
        // `anthropic.claude-opus-4-7`. So the assertion is no longer "zero matches" — it is that
        // every match is a BARE dotted id and none is the provider-qualified `anthropic/…` form.
        // Asserting emptiness here would have quietly re-encoded "amazon-bedrock is not ported".
        let scoped = resolve_scoped_models(&catalog, &["anthropic*".to_string()]);
        assert!(
            scoped.iter().all(|m| !m.model.id.as_str().contains('/')),
            "`anthropic*` is one segment, so it must never match the 2-segment `anthropic/<id>` \
             form; got {:?}",
            scoped.iter().map(|m| m.model.id.as_str()).collect::<Vec<_>>()
        );
        assert!(
            scoped.iter().all(|m| m.model.id.as_str().starts_with("anthropic")),
            "every match must actually begin with the literal pattern prefix; got {:?}",
            scoped.iter().map(|m| m.model.id.as_str()).collect::<Vec<_>>()
        );

        // Character classes (`[68]`) are real minimatch syntax the crude matcher could not express
        // (it fell through to a literal-substring miss). Pi matches exactly the -6 and -8 opus ids.
        // (This used to read `[08]`; `claude-opus-4-0` was retired upstream in pi `cc2db980` — see
        // cyrup-provider `tests/catalog_data.rs`, PROV-004.)
        let scoped = resolve_scoped_models(&catalog, &["anthropic/claude-opus-4-[68]".to_string()]);
        let ids: Vec<&str> = scoped.iter().map(|s| s.model.id.as_str()).collect();
        assert!(
            ids.contains(&"claude-opus-4-6") && ids.contains(&"claude-opus-4-8"),
            "`anthropic/claude-opus-4-[68]` char-class must scope both opus ids, got {ids:?}"
        );
        assert!(
            scoped.iter().all(|s| s.model.provider.as_str() == "anthropic"),
            "char-class stays path-segment-scoped to the anthropic provider"
        );

        // A bare `provider/*` glob is path-segment-aware: its first segment matches the whole
        // provider segment. Pi matches `minimatch(fullId) || minimatch(id)`, so every scoped model
        // is either an anthropic-provider model (fullId `anthropic/<id>`) or a model whose bare id
        // itself begins `anthropic/` (e.g. openrouter's `anthropic/claude-…`) — never an unrelated
        // provider like `anthropicX/…` (segment boundary, not a substring).
        let scoped = resolve_scoped_models(&catalog, &["anthropic/*".to_string()]);
        assert!(!scoped.is_empty(), "`anthropic/*` scopes a non-empty set");
        assert!(
            scoped.iter().any(|s| s.model.provider.as_str() == "anthropic"),
            "`anthropic/*` includes the anthropic provider's own models"
        );
        assert!(
            scoped.iter().all(|s| s.model.provider.as_str() == "anthropic"
                || s.model.id.as_str().starts_with("anthropic/")),
            "every `anthropic/*` match is anthropic-provider or an `anthropic/`-prefixed id (Pi's \
             `minimatch(fullId) || minimatch(id)`)"
        );
    }

    fn scoped(provider: &str, id: &str) -> ScopedModel {
        // Build a `ScopedModel` from a real catalog entry so the pick exercises real `Model` fields.
        let catalog = cyrup::provider::all_available_models(&cyrup_config::ModelFile::default());
        let model = catalog
            .iter()
            .find(|m| m.provider.as_str() == provider && m.id.as_str() == id)
            .or_else(|| catalog.iter().find(|m| m.provider.as_str() == provider))
            .expect("a catalog model for the provider")
            .clone();
        ScopedModel {
            model,
            thinking_level: None,
        }
    }

    #[test]
    fn scoped_active_model_prefers_saved_default_in_scope_else_first() {
        // Pi `buildSessionOptions` (main.ts:394-414): saved default in scope wins; else scoped[0].
        let a = scoped("anthropic", "");
        let o = scoped("openai", "");
        let scope = vec![a.clone(), o.clone()];

        // Saved default IS in scope (case-insensitive) → it is chosen, even though it is not first.
        let picked = pick_scoped_active_model(
            &scope,
            Some(&o.model.provider.as_str().to_uppercase()),
            Some(&o.model.id.as_str().to_uppercase()),
        )
        .expect("a pick");
        assert_eq!(picked.model.provider, o.model.provider);
        assert_eq!(picked.model.id, o.model.id);

        // Saved default NOT in scope → fall back to the first scoped model.
        let picked =
            pick_scoped_active_model(&scope, Some("together"), Some("nope")).expect("a pick");
        assert_eq!(picked.model.provider, a.model.provider);

        // No saved default → the first scoped model.
        let picked = pick_scoped_active_model(&scope, None, None).expect("a pick");
        assert_eq!(picked.model.provider, a.model.provider);

        // An empty scope yields nothing to pick.
        assert!(pick_scoped_active_model(&[], Some("openai"), Some("gpt-4o")).is_none());
    }

    /// A flat, shared `--session-dir` holding two projects' sessions must list like Pi:
    ///
    /// * the LOCAL listing applies `filterCwd` — `sessionDir !== undefined && dir !==
    ///   getDefaultSessionDirPath(cwd)` → `sessionCwdMatches` (`SessionManager.list`,
    ///   session-manager.ts:1639-1643) — so only THIS cwd's sessions appear as "current project";
    /// * the GLOBAL listing takes Pi's `listAll(sessionDir)` overload — `listSessionsFromDir(
    ///   customSessionDir)` over that one directory, no cross-project walk, no cwd filter
    ///   (session-manager.ts:1654,1660-1665) — so the other project's session is still reachable
    ///   (and reported as "found in a different project", main.ts:227-232).
    ///
    /// Before this was wired, both listing paths passed `None` for the cwd filter and handed the
    /// explicit dir to the cross-project root walk, which scans SUBdirectories: locals leaked the
    /// foreign session and globals came back empty.
    #[test]
    fn shared_session_dir_filters_locals_and_lists_globals_flat_like_pi() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let shared = root.join("shared-sessions");
        let here = root.join("project-here");
        let other = root.join("project-other");
        for d in [&shared, &here, &other] {
            std::fs::create_dir_all(d).unwrap();
        }
        write_session(&shared, "11111111-1111-7111-8111-111111111111", &here);
        write_session(&shared, "22222222-2222-7222-8222-222222222222", &other);

        let dirs = config_dirs(&root, shared.clone(), true, here.clone());
        let (locals, globals) = super::gather_session_refs(&dirs);

        let local_ids: Vec<&str> = locals.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            local_ids,
            vec!["11111111-1111-7111-8111-111111111111"],
            "a shared --session-dir must list only THIS cwd's sessions locally (Pi filterCwd)"
        );

        let mut global_ids: Vec<&str> = globals.iter().map(|s| s.id.as_str()).collect();
        global_ids.sort_unstable();
        assert_eq!(
            global_ids,
            vec![
                "11111111-1111-7111-8111-111111111111",
                "22222222-2222-7222-8222-222222222222"
            ],
            "Pi's listAll(sessionDir) overload scans the custom dir itself, unfiltered"
        );

        // The merged `--resume` listing keeps both, locals first, de-duplicated by path.
        let infos = super::gather_session_infos(&dirs);
        assert_eq!(infos.len(), 2, "merged picker listing de-duplicates by path");
        assert_eq!(
            infos.first().map(|i| i.cwd.clone()),
            Some(here.to_string_lossy().into_owned()),
            "the cwd-filtered locals come first in the merged listing"
        );
    }

    /// The DEFAULT (cwd-encoded) session dir must keep its old behavior: `sessionDir === undefined`
    /// ⇒ `filterCwd` is false and `listAll()` walks every project directory under the root
    /// (session-manager.ts:1640,1667+). The encoded layout already isolates by cwd.
    #[test]
    fn default_session_dir_walks_projects_and_never_filters() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let agent_dir = root.join("agent");
        let here = root.join("project-here");
        let other = root.join("project-other");
        for d in [&here, &other] {
            std::fs::create_dir_all(d).unwrap();
        }
        let sessions_root = agent_dir.join("sessions");
        let here_dir = super::SessionLayout::new(sessions_root.clone(), here.clone()).dir();
        let other_dir = super::SessionLayout::new(sessions_root, other.clone()).dir();
        std::fs::create_dir_all(&here_dir).unwrap();
        std::fs::create_dir_all(&other_dir).unwrap();
        write_session(&here_dir, "33333333-3333-7333-8333-333333333333", &here);
        write_session(&other_dir, "44444444-4444-7444-8444-444444444444", &other);

        let dirs = config_dirs(&root, agent_dir.join("sessions"), false, here.clone());
        let (locals, globals) = super::gather_session_refs(&dirs);
        assert_eq!(locals.len(), 1, "the encoded dir holds only this cwd's session");
        assert_eq!(globals.len(), 2, "listAll() walks every project dir under the root");
    }

    /// One session file: a v3 header line, which is all the listing scanner needs.
    fn write_session(dir: &std::path::Path, id: &str, cwd: &std::path::Path) {
        let line = format!(
            "{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"timestamp\":\"2026-01-01T00:00:00.000Z\",\"cwd\":\"{}\"}}\n",
            cwd.to_string_lossy()
        );
        std::fs::write(dir.join(format!("2026-01-01T00-00-00-000Z_{id}.jsonl")), line).unwrap();
    }

    fn config_dirs(
        root: &std::path::Path,
        session_dir: std::path::PathBuf,
        session_dir_explicit: bool,
        cwd: std::path::PathBuf,
    ) -> cyrup_config::ConfigDirs {
        cyrup_config::ConfigDirs {
            agent_dir: root.join("agent"),
            session_dir,
            session_dir_explicit,
            package_dir: root.join("agent").join("packages"),
            cwd,
            home: root.to_path_buf(),
        }
    }

    #[test]
    fn benchmark_drain_window_matches_pi() {
        // Pi `setTimeout(resolve, 150)` (main.ts:826).
        assert_eq!(super::BENCHMARK_DRAIN_MS, 150);
    }

    #[test]
    fn fresh_target_classifies_new_and_create_with_id() {
        assert!(is_fresh_target(&SessionTarget::New));
        assert!(is_fresh_target(&SessionTarget::CreateWithId("x".into())));
        assert!(!is_fresh_target(&SessionTarget::Continue));
        assert!(!is_fresh_target(&SessionTarget::Resume(
            "/s/a.jsonl".into()
        )));
        assert!(!is_fresh_target(&SessionTarget::Fork {
            source: "/s/a.jsonl".into(),
            id: None
        }));
    }

    #[test]
    fn token_counts_humanise_like_pi() {
        // Pi `formatTokenCount` (list-models.ts:14-24).
        assert_eq!(format_token_count(200_000), "200K");
        assert_eq!(format_token_count(1_000_000), "1M");
        assert_eq!(format_token_count(1_500_000), "1.5M");
        assert_eq!(format_token_count(128_000), "128K");
        assert_eq!(format_token_count(900), "900");
        assert_eq!(format_token_count(8_192), "8.2K");
    }

    #[test]
    fn fuzzy_match_is_token_subsequence() {
        // Each whitespace token must be a case-insensitive subsequence of "provider id".
        assert!(fuzzy_match("anthropic claude-opus", "ant opus"));
        assert!(fuzzy_match("openai gpt-4o", "GPT"));
        assert!(fuzzy_match("together moonshotai/Kimi-K2.6", "kimi"));
        assert!(!fuzzy_match("openai gpt-4o", "claude"));
        // Empty search trivially matches (handled by the caller, but the predicate is total).
        assert!(fuzzy_match("anything", ""));
    }
}
