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
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use cyrup::{
    apply_arg_leniency, build_inputs, file_settings_store, format_no_models_available_message,
    initial_input, migrations, normalize_short_aliases, partition_extension_flags, render_help,
    resolve_app_mode, run_json_dispatch, run_print_dispatch, run_rpc_dispatch, select_provider,
    should_take_over_stdout, spawn_abort_on_signal, subcommands, timings, AppMode, Cli, Diagnostic,
    DiagnosticLevel, Inputs,
};
use cyrup::cli::{qualified_matches, split_model_level};
use cyrup::session_resolve::{resolve_session_target, Outcome, SessionFlags, SessionRef};
use cyrup_config::{
    CliConfigOverrides, ConfigDirs, DefaultProjectTrust, EnvVars, Settings, SettingsManager,
    SettingsScope,
};
use cyrup_sdk::core::CancelToken;
use cyrup_session_svc::{
    list_all, list_in_dir, AgentSession, AgentSessionRuntime, InputSource, ScopedModel,
    SessionBuilder, SessionConfig, SessionFactory, SessionInfo, SessionLayout, SessionServiceError,
    SessionTarget, SessionsRoot, UserInput,
};
use cyrup_tui::{crossterm_input_stream, App, UiTheme};

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

    // Package/config subcommand pre-dispatch (Pi main.ts:486, before arg parsing). Resolve dirs with
    // no CLI overrides for the subcommand's package/project roots.
    if subcommands::first_subcommand(&argv).is_some() {
        let env = EnvVars::from_process();
        let dirs = ConfigDirs::resolve(&CliConfigOverrides::default(), &env)
            .context("resolving config directories")?;
        let trust_override = subcommands::trust_override(&argv);
        if let Some(code) =
            subcommands::dispatch(&argv, &dirs, trust_override).await?
        {
            return Ok(code);
        }
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
    init_tracing(cli.verbose);
    timings.mark("parseArgs");

    // Report parse diagnostics (Pi main.ts:504-512): warnings + errors to stderr, any error exits 1.
    report_diagnostics(&parse_diagnostics);
    if parse_diagnostics.iter().any(|d| d.level == DiagnosticLevel::Error) {
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

    // Stdout-takeover decision (Pi main.ts:535). The streams the bin owns are already disciplined
    // (tracing → stderr), so the takeover itself is inert; the Pi-faithful decision is computed.
    let _guard_stdout = should_take_over_stdout(&cli, mode);

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

    // Surface settings load/parse errors as warnings (Pi `collectSettingsDiagnostics` over the
    // `startupSettingsManager`, main.ts:552-553).
    report_diagnostics(&collect_settings_diagnostics(
        file_settings_store(&dirs),
        "startup session lookup",
    ));

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
    // `--list-models` enumerates the FULL multi-provider registry (Pi `modelRegistry.getAvailable()`,
    // list-models.ts:35) — independent of `--provider`/`--model`, and resolved BEFORE provider
    // selection (so a `--provider <unknown>` does not gate the listing, matching Pi).
    if let Some(search) = &cli.list_models {
        return list_models(&cyrup::provider::all_available_models(), search);
    }
    let provider = select_provider(cli.provider.as_deref(), cli.model.as_deref(), cli.api_key.as_deref())?;

    // Unknown-model diagnostic (Pi `resolveCliModel`, main.ts:377-378 / model-resolver.ts:494-500):
    // a `--model` on a *known* provider whose id is not in the catalog warns (the build still proceeds
    // with a custom-id model). An *unknown provider* already errored in `select_provider` above.
    if let Some(warning) = cyrup::unknown_model_warning(
        cli.provider.as_deref(),
        cli.model.as_deref(),
        &cyrup::provider::all_available_models(),
    ) {
        report_diagnostics(&[Diagnostic::warning(warning)]);
    }

    // Map CLI → SessionConfig.
    let mut config = cli.to_session_config(&dirs, mode);

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
    let cancel = CancelToken::new();

    // Interactive mode drives the **multi-session** `AgentSessionRuntime` (arch-11 §3.4) so the
    // session-swap commands rebuild the active session in place and the TUI re-binds to it. The
    // one-shot/RPC modes keep the single fixed `AgentSession` seam unchanged.
    if let AppMode::Interactive = mode {
        // `PI_STARTUP_BENCHMARK` only supports interactive mode (Pi main.ts:800-804) — here it is
        // satisfied (interactive). In the one-shot/RPC arms below it is an error.
        let target = config.target.clone();
        let fresh = is_fresh_target(&target);
        let factory = Arc::new(
            SessionFactory::new(provider, config).settings_store(settings_store.clone()),
        );
        let runtime = Arc::new(
            AgentSessionRuntime::create(factory, target)
                .await
                .context("building agent session runtime")?,
        );
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
        let inputs = build_inputs(&cli, &dirs.cwd).await?;
        run_interactive(runtime, session, inputs, cancel).await?;
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
            let factory = Arc::new(
                SessionFactory::new(provider, config).settings_store(settings_store.clone()),
            );
            let runtime = match AgentSessionRuntime::create(factory, target).await {
                Ok(r) => Arc::new(r),
                // Non-interactive no-models-available guard (Pi main.ts:795-798): print the provider
                // login guidance + exit 1 instead of a generic build error.
                Err(SessionServiceError::NoModels(_)) => return no_models_available(),
                Err(e) => {
                    return Err(anyhow::Error::new(e).context("building agent session runtime"))
                }
            };
            let session = runtime.session().await;
            apply_post_build(&session, session_name.as_deref(), &cli, fresh).await;
            timings.print();
            let _signals = spawn_abort_on_signal(session, cancel.clone());
            let reader = tokio::io::BufReader::new(tokio::io::stdin());
            let mut writer = tokio::io::stdout();
            run_rpc_dispatch(&runtime, reader, &mut writer).await?;
            Ok(0)
        }
        AppMode::Print | AppMode::Json => {
            // One-shot modes never swap sessions: build the one `AgentSession` seam (R-11-008).
            let fresh = is_fresh_target(&config.target);
            let session = match SessionBuilder::new(provider, config)
                .settings_store(settings_store.clone())
                .build()
                .await
            {
                // Bind the self-handle (via `into_shared`) so the post-run loop — auto-retry,
                // post-run auto-compaction, queued continuations — fires for one-shot print/json runs.
                Ok(s) => s.into_shared(),
                // Non-interactive no-models-available guard (Pi main.ts:795-798).
                Err(SessionServiceError::NoModels(_)) => return no_models_available(),
                Err(e) => return Err(anyhow::Error::new(e).context("building agent session")),
            };
            apply_post_build(&session, session_name.as_deref(), &cli, fresh).await;
            timings.print();
            let _signals = spawn_abort_on_signal(session.clone(), cancel.clone());
            let inputs = build_inputs(&cli, &dirs.cwd).await?;
            ensure_prompt(&inputs)?;
            let mut out = io::stdout();
            if let AppMode::Json = mode {
                run_json_dispatch(&session, &inputs, &mut out).await
            } else {
                run_print_dispatch(&session, &inputs, &mut out).await
            }
        }
        AppMode::Interactive => unreachable!("interactive mode is handled before this match"),
    }
}

/// Apply the per-run, post-build session knobs that have no `SessionConfig` slot: the trimmed
/// `--name` display name (Pi `appendSessionInfo`, main.ts:586) and the `--models` Ctrl+P scope (Pi
/// `resolveModelScope`/`scopedModels`, main.ts:685).
///
/// `fresh` is whether this is a brand-new session (Pi `!hasExistingSession`, main.ts:394): a resumed
/// session keeps its own restored model, so the saved-default-in-scope active-model pick only fires
/// for a fresh session.
async fn apply_post_build(session: &AgentSession, name: Option<&str>, cli: &Cli, fresh: bool) {
    if let Some(name) = name {
        let _ = session.set_session_name(name).await;
    }
    if !cli.models.is_empty() {
        let scoped = resolve_scoped_models(session.model_catalog(), &cli.models);
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

/// Resolve the `--models` patterns to a [`ScopedModel`] set against the live catalog (Pi
/// `resolveModelScope`): each pattern (optionally `:level`-suffixed) selects the catalog models whose
/// `provider/id` matches it; duplicates are de-duplicated in first-seen order.
fn resolve_scoped_models(
    catalog: &[cyrup_provider::Model],
    patterns: &[String],
) -> Vec<ScopedModel> {
    let mut out: Vec<ScopedModel> = Vec::new();
    for pattern in patterns {
        let (base, level) = split_model_level(pattern);
        for model in catalog {
            let qualified = format!("{}/{}", model.provider.as_str(), model.id.as_str());
            if qualified_matches(&qualified, &base)
                && !out.iter().any(|s| {
                    s.model.provider == model.provider && s.model.id == model.id
                })
            {
                out.push(ScopedModel { model: model.clone(), thinking_level: level.map(|l| l.to_level()) });
            }
        }
    }
    out
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
    // prefix (the messages are pre-composed, e.g. `No session found matching '<arg>'`).
    for line in &resolution.stdout {
        println!("{line}");
    }
    for line in &resolution.stderr {
        eprintln!("{line}");
    }

    // Interactive missing-session-cwd Continue/Cancel prompt (Pi `promptForMissingSessionCwd`,
    // main.ts:575-580): a resumed session whose stored cwd is gone is offered a continuation against
    // the current cwd, or cancels to exit 0. The non-interactive arm already errored above.
    if let Some(issue) = resolution.missing_cwd {
        let theme = UiTheme::default();
        let body = cyrup::format_missing_session_cwd_prompt(&issue.session_cwd, &issue.fallback_cwd);
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
                config.target = SessionTarget::Resume(path);
                config.persist = !cli.no_session;
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
        let trust_store =
            cyrup_config::trust::TrustStore::new(dirs.agent_dir.join("trust.json"));
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

/// Scan the cwd's local session listing and the global cross-project listing into a merged
/// [`SessionInfo`] vector (locals first, globals de-duplicated by path) for the `--resume` picker (Pi
/// `selectSession`'s `current`/`all` `SessionsLoader`s, session-picker.ts:23-25).
fn gather_session_infos(dirs: &ConfigDirs) -> Vec<SessionInfo> {
    let root = dirs.session_dir.clone();
    let layout = SessionLayout::new(root.clone(), dirs.cwd.clone());
    let mut sessions = list_in_dir(&layout.dir(), None, None);
    for global in list_all(&SessionsRoot(root)) {
        if !sessions.iter().any(|s| s.path == global.path) {
            sessions.push(global);
        }
    }
    sessions
}

/// Scan the cwd's session listing and the global cross-project listing into [`SessionRef`]s (Pi
/// `SessionManager.list(cwd, sessionDir)` + `SessionManager.listAll(sessionDir)`, main.ts:169,179).
fn gather_session_refs(dirs: &ConfigDirs) -> (Vec<SessionRef>, Vec<SessionRef>) {
    let root = dirs.session_dir.clone();
    let layout = SessionLayout::new(root.clone(), dirs.cwd.clone());
    let locals: Vec<SessionRef> =
        list_in_dir(&layout.dir(), None, None).iter().map(SessionRef::from).collect();
    let globals: Vec<SessionRef> =
        list_all(&SessionsRoot(root)).iter().map(SessionRef::from).collect();
    (locals, globals)
}

/// The plain-stdin fork-into-cwd confirmation (Pi `promptConfirm`, main.ts:191-203): a cooked-mode
/// `[y/N]` readline (NOT the TUI dialog host), run before any terminal takeover. Defaults to `no`.
fn prompt_fork_confirm() -> bool {
    use std::io::Write;
    print!("Fork this session into current directory? [y/N] ");
    let _ = io::stdout().flush();
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
        token.to_ascii_lowercase().chars().all(|c| hay_chars.any(|h| h == c))
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
            .filter(|m| fuzzy_match(&format!("{} {}", m.provider.as_str(), m.id.as_str()), search))
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
            images: if m.input.contains(&Modality::Image) { "yes" } else { "no" }.to_string(),
        })
        .collect();

    let hdr = ("provider", "model", "context", "max-out", "thinking", "images");
    let w_provider = rows.iter().map(|r| r.provider.len()).chain([hdr.0.len()]).max().unwrap_or(0);
    let w_model = rows.iter().map(|r| r.model.len()).chain([hdr.1.len()]).max().unwrap_or(0);
    let w_context = rows.iter().map(|r| r.context.len()).chain([hdr.2.len()]).max().unwrap_or(0);
    let w_max = rows.iter().map(|r| r.max_out.len()).chain([hdr.3.len()]).max().unwrap_or(0);
    let w_think = rows.iter().map(|r| r.thinking.len()).chain([hdr.4.len()]).max().unwrap_or(0);
    let w_img = rows.iter().map(|r| r.images.len()).chain([hdr.5.len()]).max().unwrap_or(0);

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

/// Require a non-empty prompt for the one-shot modes (a message, an `@file`, or piped stdin).
fn ensure_prompt(inputs: &Inputs) -> anyhow::Result<()> {
    if inputs.is_empty() {
        anyhow::bail!(
            "no prompt provided: pass a message, an @file reference, or pipe text on stdin"
        );
    }
    Ok(())
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

/// The interactive front-end: build the TUI over a real `CrosstermBackend<Stdout>`, seed any initial
/// prompt, and run the event loop against the live session. Restores the terminal on exit.
async fn run_interactive(
    runtime: Arc<AgentSessionRuntime>,
    session: Arc<AgentSession>,
    inputs: Inputs,
    cancel: CancelToken,
) -> anyhow::Result<()> {
    let mut app = App::into_stdout(UiTheme::default()).context("initialising the terminal UI")?;
    app.detect_image_support();
    let input_stream = crossterm_input_stream(cancel.clone());
    let events = session.subscribe();

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

    let result =
        app.run(input_stream, events, session.clone(), Some(runtime), None, cancel).await;
    let _ = app.restore();
    result.map_err(|e| anyhow::anyhow!("tui: {e}"))?;
    Ok(())
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

/// Drain settings load/parse errors into warning diagnostics (Pi `collectSettingsDiagnostics`,
/// main.ts:77-85): `(<context>, <scope> settings) <message>`. Builds a throwaway `SettingsManager`
/// over the file store (project untrusted, so only global is read — matching the startup manager).
fn collect_settings_diagnostics(
    store: std::sync::Arc<dyn cyrup_config::SettingsStore>,
    context: &str,
) -> Vec<Diagnostic> {
    let mut mgr = cyrup_config::SettingsManager::load(store, cyrup_config::Settings::new(), false);
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
    use tracing_subscriber::{fmt, EnvFilter};
    let default = if verbose { "debug" } else { "warn" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = fmt().with_env_filter(filter).with_writer(io::stderr).try_init();
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::{
        format_token_count, fuzzy_match, is_fresh_target, pick_scoped_active_model, ScopedModel,
        SessionTarget,
    };

    fn scoped(provider: &str, id: &str) -> ScopedModel {
        // Build a `ScopedModel` from a real catalog entry so the pick exercises real `Model` fields.
        let catalog = cyrup::provider::all_available_models();
        let model = catalog
            .iter()
            .find(|m| m.provider.as_str() == provider && m.id.as_str() == id)
            .or_else(|| catalog.iter().find(|m| m.provider.as_str() == provider))
            .expect("a catalog model for the provider")
            .clone();
        ScopedModel { model, thinking_level: None }
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
        let picked = pick_scoped_active_model(&scope, Some("together"), Some("nope")).expect("a pick");
        assert_eq!(picked.model.provider, a.model.provider);

        // No saved default → the first scoped model.
        let picked = pick_scoped_active_model(&scope, None, None).expect("a pick");
        assert_eq!(picked.model.provider, a.model.provider);

        // An empty scope yields nothing to pick.
        assert!(pick_scoped_active_model(&[], Some("openai"), Some("gpt-4o")).is_none());
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
        assert!(!is_fresh_target(&SessionTarget::Resume("/s/a.jsonl".into())));
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
