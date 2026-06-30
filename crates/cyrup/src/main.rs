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
use cyrup_config::{CliConfigOverrides, ConfigDirs, EnvVars, SettingsScope};
use cyrup_sdk::core::CancelToken;
use cyrup_session_svc::{
    AgentSession, AgentSessionRuntime, InputSource, ScopedModel, SessionBuilder, SessionFactory,
    SessionServiceError, UserInput,
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

    // Map CLI → SessionConfig.
    let config = cli.to_session_config(&dirs, mode);
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
        let factory = Arc::new(
            SessionFactory::new(provider, config).settings_store(settings_store.clone()),
        );
        let runtime = Arc::new(
            AgentSessionRuntime::create(factory, target)
                .await
                .context("building agent session runtime")?,
        );
        let session = runtime.session().await;
        apply_post_build(&session, session_name.as_deref(), &cli).await;
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
        timings.print();
        let _signals = spawn_abort_on_signal(session.clone(), cancel.clone());
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
            apply_post_build(&session, session_name.as_deref(), &cli).await;
            timings.print();
            let _signals = spawn_abort_on_signal(session, cancel.clone());
            let reader = tokio::io::BufReader::new(tokio::io::stdin());
            let mut writer = tokio::io::stdout();
            run_rpc_dispatch(&runtime, reader, &mut writer).await?;
            Ok(0)
        }
        AppMode::Print | AppMode::Json => {
            // One-shot modes never swap sessions: build the one `AgentSession` seam (R-11-008).
            let session = match SessionBuilder::new(provider, config)
                .settings_store(settings_store.clone())
                .build()
                .await
            {
                Ok(s) => Arc::new(s),
                // Non-interactive no-models-available guard (Pi main.ts:795-798).
                Err(SessionServiceError::NoModels(_)) => return no_models_available(),
                Err(e) => return Err(anyhow::Error::new(e).context("building agent session")),
            };
            apply_post_build(&session, session_name.as_deref(), &cli).await;
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
async fn apply_post_build(session: &AgentSession, name: Option<&str>, cli: &Cli) {
    if let Some(name) = name {
        let _ = session.set_session_name(name).await;
    }
    if !cli.models.is_empty() {
        let scoped = resolve_scoped_models(session.model_catalog(), &cli.models);
        if !scoped.is_empty() {
            session.set_scoped_models(scoped);
        }
    }
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
    use super::{format_token_count, fuzzy_match};

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
