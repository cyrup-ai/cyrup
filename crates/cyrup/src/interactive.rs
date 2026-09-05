//! The interactive front end: the TTY-bound half of the binary.
//!
//! Everything here drives a real terminal — a `CrosstermBackend<Stdout>`, the theme probe, the
//! startup panel and the run loop — which is why none of it is unit-tested beyond the two pure
//! helpers at the foot of the file. This is the module `main.rs`'s header used to describe as "only
//! the interactive `CrosstermBackend` wiring stays here (it needs a real terminal and is not
//! unit-tested)"; the wiring now lives in one place instead of trailing the startup sequence.
//!
//! Pi's counterpart is `cli/interactive-mode.ts` plus the footer/theme helpers it pulls in.

use std::sync::Arc;

use anyhow::Context;
use cyrup_config::ConfigDirs;
use cyrup_resources::theme::ThemeWatcher;
use cyrup_sdk::core::CancelToken;
use cyrup_session_svc::{AgentSession, AgentSessionRuntime, InputSource, SessionLayout, UserInput};
use cyrup_tui::{App, StdinTerminalProbe, ThemeController, UiTheme, crossterm_input_stream};

use crate::input::Inputs;
use crate::run::initial_input;

/// The startup migrated-credential warning line, or `None` when nothing was migrated — Pi
/// `if (migratedProviders && migratedProviders.length > 0) { this.showWarning(`Migrated credentials
/// to auth.json: ${migratedProviders.join(", ")}`); }` (interactive-mode.ts:874-876 @v0.83.0), with
/// `showWarning`'s own `Warning: ` prefix (`:3885-3889`) folded in, because cyrup's `Entry::Warning`
/// renders its text verbatim. Split out from the call site so the string is pinnable (CFG-051).
fn migrated_credentials_warning(providers: &[String]) -> Option<String> {
    if providers.is_empty() {
        return None;
    }
    Some(format!(
        "Warning: Migrated credentials to auth.json: {}",
        providers.join(", ")
    ))
}

/// Write Pi's exit hint — `To resume this session: cyrup [--session-dir DIR] --session ID` — on the
/// way out of interactive mode (`interactive-mode.ts:3594-3597`, using `formatResumeCommand`,
/// `:231-244`).
///
/// The gates (tty stdout, a persisted session, a session file that exists) live in
/// [`cyrup_tui::format_resume_command`]; this function's whole job is to resolve the four inputs off
/// the live session. `default_session_dir` is Pi's `getDefaultSessionDirPath(cwd)` — the SAME
/// cwd-encoded path [`crate::session_resolve::session_list_cwd_filter`] compares against — so the
/// `--session-dir` argument is printed exactly when the session is not where a bare relaunch would
/// look for it.
pub async fn print_resume_hint(dirs: &ConfigDirs, session: &AgentSession) {
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
    let Some(command) = cyrup_tui::format_resume_command(&target, std::io::stdout().is_tty())
    else {
        return;
    };
    let mut out = std::io::stdout();
    let _ = out.write_all(cyrup_tui::resume_hint_line(&command).as_bytes());
    let _ = out.flush();
}

/// The terminal-query drain window for the startup benchmark (Pi `setTimeout(resolve, 150)`,
/// main.ts:826): the brief pause that lets the TUI's stdin handler consume the terminal's query
/// replies (Kitty keyboard protocol, device attributes, cell size) before the terminal is restored.
const BENCHMARK_DRAIN_MS: u64 = 150;

/// The `PI_STARTUP_BENCHMARK` interactive teardown (Pi main.ts:819-835): initialise the TUI over the
/// real terminal (Pi `interactiveMode.init()`), give the stdin handler [`BENCHMARK_DRAIN_MS`] to drain
/// the terminal's query replies, then stop + restore — measuring cold startup without running the
/// event loop. TTY-bound (it owns a real `CrosstermBackend`), so it is not unit-tested.
pub async fn run_interactive_benchmark() -> anyhow::Result<()> {
    let mut app = App::into_stdout(UiTheme::default()).context("initialising the terminal UI")?;
    app.detect_image_support();
    tokio::time::sleep(std::time::Duration::from_millis(BENCHMARK_DRAIN_MS)).await;
    let _ = app.restore();
    Ok(())
}

/// The interactive front-end: build the TUI over a real `CrosstermBackend<Stdout>`, seed any initial
/// prompt, and run the event loop against the live session. Restores the terminal on exit.
// The interactive entry point wires eight independently-owned collaborators; bundling them
// into a struct would only move the arity to the caller, which constructs each one separately.
#[allow(clippy::too_many_arguments)]
pub async fn run_interactive(
    runtime: Arc<AgentSessionRuntime>,
    session: Arc<AgentSession>,
    inputs: Inputs,
    // `--verbose` — Pi's `options.verbose`, which overrides `quietStartup` for the startup listing
    // (`cli/help.rs` has always advertised exactly that; TUI-006 makes it true).
    verbose: bool,
    cancel: CancelToken,
    // The detached startup package-update check's answer channel (Pi `interactive-mode.ts:850-856`);
    // `None` when the network policy declined. Handed straight to the run loop.
    package_updates: Option<tokio::sync::mpsc::UnboundedReceiver<Vec<String>>>,
    // Pi `InteractiveModeOptions.migratedProviders` (interactive-mode.ts:308), threaded from
    // `runMigrations` through `main.ts:607`. CFG-051.
    migrated_providers: Vec<String>,
    // `--tui-mode` (pi `cli/args.ts:180-193`, threaded to the composition root at `main.ts:935`
    // and read at `interactive-mode.ts:345-352`). `None` when the flag was omitted, in which case
    // the `tuiMode` SETTING decides — the precedence ADR-0005 §B-14 fixes and `cli/enums.rs`
    // documents: the flag wins when given, else the setting, else `regular`.
    tui_mode: Option<cyrup_config::settings::TuiMode>,
    // TUI-037 — pi's `InteractiveModeOptions.autoTrustOnReloadCwd` (`interactive-mode.ts:344`
    // @v0.84.4), computed at the composition root (`main.ts:701-704`) and handed in here exactly
    // as `migratedProviders` is: the session cwd when its trust was granted implicitly at boot.
    auto_trust_on_reload_cwd: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    // Boot the render theme from `settings.theme` + the terminal background/color-depth (feature #4:
    // the `ThemeController`), instead of the hardwired dark boot the audit flagged (theme.rs #4). An
    // unset/`auto` setting resolves against the detected terminal polarity; every role is projected
    // into the detected `ColorMode` (feature #3) so 256-color terminals get indexed colors.
    let theme_setting = session.services().settings.effective().theme_setting();
    let mut controller = ThemeController::boot_from_env(theme_setting.as_deref());
    let mut app = App::into_stdout(controller.theme()).context("initialising the terminal UI")?;

    // ADR-0005 §B-14 — select the renderer before anything paints. The flag wins when supplied,
    // otherwise the `tuiMode` setting (§A-3), otherwise `regular`. `switch_tui_mode` is a no-op
    // returning `Unchanged` for `Regular`, so the common path costs one comparison.
    //
    // A refusal is NOT fatal: `ModeSwitch::RendererUnavailable` means the alternate screen could
    // not be entered (a terminal that rejected the escape, a backend rebuild that failed), and the
    // session continues inline rather than dying — upstream's own posture, and the reason
    // `install_renderer` returns a bool rather than propagating.
    let effective_tui_mode =
        tui_mode.unwrap_or_else(|| session.services().settings.effective().tui_mode());
    if effective_tui_mode == cyrup_config::settings::TuiMode::Fullscreen {
        let outcome = app.switch_tui_mode(
            cyrup_tui::TuiRenderMode::Fullscreen,
            cyrup_tui::ModeSwitchOptions::default(),
        );
        if let Some(status) = outcome.refusal_status() {
            tracing::warn!(target: "cyrup::tui", "--tui-mode fullscreen declined: {status}");
        }
        // ADR-0005 §A-3 → §B-5. Applied only once the renderer is live, and only in fullscreen:
        // pi documents the `/settings` row as having "no effect in regular mode", and the setter is
        // a no-op inline for the same reason.
        app.set_fullscreen_scrollbar(
            match session
                .services()
                .settings
                .effective()
                .fullscreen_scrollbar()
            {
                cyrup_config::settings::FullscreenScrollbar::Always => {
                    cyrup_tui::ScrollbarMode::Always
                }
                cyrup_config::settings::FullscreenScrollbar::Hidden => {
                    cyrup_tui::ScrollbarMode::Hidden
                }
                cyrup_config::settings::FullscreenScrollbar::Auto => cyrup_tui::ScrollbarMode::Auto,
            },
        );
    }
    // CFG-078 — the two v0.84.4 alt-screen keys. Applied UNCONDITIONALLY, not inside the
    // fullscreen branch above: pi seeds `copyOnSelect` into every `createInteractiveTui`
    // (`interactive-mode.ts:378`, fed at `:586`), including the one a later `/settings` switch
    // builds, and evaluates `getFullscreenExitOutput()` at `stop()` whatever renderer is live
    // (`:6556`). Seeding them only when the session BOOTS fullscreen would leave a session that
    // switched in mid-run running on the renderer defaults instead of the user's settings.
    {
        let eff = session.services().settings.effective();
        app.set_fullscreen_exit_output(match eff.fullscreen_exit_output() {
            cyrup_config::settings::FullscreenExitOutput::ResumeHint => {
                cyrup_tui::FullscreenExitOutput::ResumeHint
            }
            cyrup_config::settings::FullscreenExitOutput::Transcript => {
                cyrup_tui::FullscreenExitOutput::Transcript
            }
        });
        app.set_fullscreen_copy_on_select(eff.fullscreen_copy_on_select());
    }
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
        let _ = session
            .persist_setting(
                cyrup_session_svc::SettingsScope::Global,
                "theme",
                serde_json::Value::String(name.to_string()),
            )
            .await;
    }
    // TUI-004 — hand the settled controller to the app so the run loop's `session_swapped` arm can
    // re-run pi's `applyFromSettings` on every session replacement. Upstream's controller is a field
    // of the interactive mode (`interactive-mode.ts:960` @v0.84.4) and its `setRebindSession` hook
    // calls straight into it (`:576-579`); cyrup's lived only in this stack frame, which is why
    // `/reload` re-read five other settings rows and never the theme. Cloned rather than moved: the
    // theme file watcher below still binds against `controller.active_name()`.
    app.set_theme_controller(controller.clone());
    app.detect_image_support();
    seed_footer(&mut app, &runtime, &session).await;
    // Pi shows the package-update notification whenever the detached check settles, which is why the
    // channel — not the answer — is what reaches the loop (`interactive-mode.ts:850-856`).
    app.set_package_update_channel(package_updates);

    // Configurable keybindings (feature #2; Pi `KeybindingsManager.create`, keybindings.ts:348-352):
    // load the user's `~/.cyrup/keybindings.json` and merge it into every live keymap (global/editor/
    // selector/tree). Absent file ⇒ defaults; a malformed file logs to stderr and keeps the defaults.
    let keybindings_path = session.services().agent_dir.join("keybindings.json");
    if let Ok(json) = std::fs::read_to_string(&keybindings_path) {
        // CFG-038 — `load_keybindings_json` no longer aborts on the first bad entry, so the two
        // outcomes are now genuinely different and are reported differently. `Err` really does mean
        // the whole document was ignored (unparseable JSON or a non-object top level, Pi's
        // `loadRawConfig` → `undefined`, `core/keybindings.ts:328-336` @v0.83.0). `Ok(issues)` means
        // everything else applied and these specific ids did not — the old code printed
        // "ignoring <path>" for that case too, which was false: the file had already been
        // half-applied in an iteration order the user cannot see.
        match app.load_keybindings_json(&json) {
            Err(e) => eprintln!("warning: ignoring {}: {e}", keybindings_path.display()),
            Ok(issues) => {
                for issue in issues {
                    eprintln!(
                        "warning: {}: ignoring {}",
                        keybindings_path.display(),
                        issue
                    );
                }
            }
        }
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
    //
    // EXT-040 — the installed specs are `(key, description)`, not bare key-ids. pi stores an
    // `ExtensionShortcut {shortcut, description?, handler, extensionPath}`
    // (`extensions/types.ts:1547-1552`, stored at `:1524-1529`) and `/hotkeys` renders the
    // DESCRIPTION. `shortcut_keys()` is the bare `Vec<String>`, so the description an extension
    // registered was dropped one call from the renderer and `/hotkeys` printed the key id as its
    // own label.
    //
    // EXT-039 — and they are RESOLVED against the live keybindings first, which is why this runs
    // after the `keybindings.json` merge above. pi's `setupExtensionShortcuts` opens with
    // `extensionRunner.getShortcuts(this.keybindings.getEffectiveConfig())`
    // (`modes/interactive/interactive-mode.ts:2079` @v0.84.4): a shortcut on a reserved key is
    // dropped with a warning instead of being installed dead, and the warnings land in the
    // `[Extension issues]` panel `StartupReport::from_session` builds below (`:1884-1886`).
    app.install_extension_shortcuts(&session.services().ext_host);
    // TUI-037 — arm the implicit-trust save `/reload` performs (pi stores the option at
    // `interactive-mode.ts:572` @v0.84.4; the consumer is `App::maybe_save_implicit_project_trust`).
    app.set_auto_trust_on_reload_cwd(auto_trust_on_reload_cwd);

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
    // TUI-N02 — `StartupReport::from_session` used to be this module's private
    // `build_startup_report`. It moved into `cyrup-tui` when the panel gained its SECOND call site:
    // upstream emits it from every session rebind and again from `/reload`
    // (`interactive-mode.ts:1982`, `:5991-5994` @v0.84.4), and that second site is the run loop's
    // `session_swapped` arm, which lives in that crate. `set_verbose_startup` arms the
    // `options.verbose` half of `showListing` for it (`:1702`).
    app.set_verbose_startup(verbose);
    app.push_session_loaded_resources(&session);

    // Pi's startup-warning block (interactive-mode.ts:871-885 @v0.83.0), in pi's order. Both lines
    // go in the TRANSCRIPT, not on stderr, because that is the only place a first-run user will
    // still see them once the alternate screen is up. Pushed after `showLoadedResources` (which pi
    // runs from `init()`, ahead of `run()`'s startup-warning block) and before the replay.
    //
    // The `Warning: ` prefix belongs to pi's `showWarning` itself — `new Text(theme.fg("warning",
    // `Warning: ${warningMessage}`), 1, 0)`, interactive-mode.ts:3885-3889 @v0.83.0 — and cyrup
    // renders `Entry::Warning` verbatim, so every caller supplies it (app.rs:3626, :7821).

    // FIRST: the migrated-credential notice (`:874-876`). It tells the user their OAuth tokens and
    // API keys were relocated out of `oauth.json`/`settings.json` into `auth.json` — a change that
    // silently invalidates any backup or tooling pointing at the old files — and on stderr it lived
    // exactly one frame before the paint that erased it. CFG-051.
    if let Some(line) = migrated_credentials_warning(&migrated_providers) {
        app.state_mut().transcript.push_warning(line);
    }
    // THEN the `modelFallbackMessage` warning (`:883-885`). Reading it is the whole point: on a
    // credential-less start it is `formatNoModelsAvailableMessage()`, i.e. "No models available.
    // Use /login …" (auth-guidance.ts:14-16), the instruction that turns a modelless launch
    // (SEAM-075) into a working session. The `Warning: ` prefix was missing at this call site.
    if let Some(msg) = runtime.model_fallback_message().await {
        app.state_mut()
            .transcript
            .push_warning(format!("Warning: {msg}"));
    }

    let input_stream = crossterm_input_stream(cancel.clone());
    let events = session.subscribe();

    // TUI-003: a `--resume`/`--continue` boot starts on an existing branch, so seed the transcript
    // from it before the first frame — Pi's `renderInitialMessages()` (interactive-mode.ts:3548).
    // A fresh session has no messages and replays nothing. The raw projection keeps the
    // `compactionSummary`/`branchSummary`/`custom`/`bashExecution` roles that `messages()` would
    // have flattened to `user` prose at the LLM boundary (Pi feeds `renderSessionEntries` the same
    // raw projection, interactive-mode.ts:3506-3516).
    // The replay walk holds messages, not a session, so seed the has-a-definition registry it reads
    // first (Pi's `definitionRegistry`, agent-session.ts:2659-2676, consulted per tool-execution
    // component as `hasRendererDefinition()`, tool-execution.ts:103-105). Without it a `--resume`d
    // MCP/extension tool call replays through `formatToolExecution`'s full argument dump.
    app.refresh_known_tool_definitions(&session);
    // `showCacheMissNotices` gates the derived notices below, and `App::seed_session_ui` — which
    // caches it — does not run until `App::run` takes over, so seed it here or a `--resume` would
    // replay with the boot default rather than the persisted value.
    app.state_mut().show_cache_miss_notices = session
        .services()
        .settings
        .effective()
        .show_cache_miss_notices();
    // `replay_items` is `raw_context_messages` plus the cache-miss and compaction-cost notices pi
    // re-derives on every rebuild (`interactive-mode.ts:3694-3696`, `:3788-3794`); neither is
    // persisted, so this is the only way a resumed transcript carries them.
    let restored = session.replay_items().await;
    if !restored.is_empty() {
        // X11 — WITH the loaded extensions: Pi resolves `getMessageRenderer(message.customType)` on
        // the replay walk (`interactive-mode.ts:3471`) exactly as it does on the live
        // `addMessageToChat` path, so a `--resume`d session keeps the extension rendering it had.
        app.replay_items_with_extensions(&restored, &session.services().ext_host)
            .await;
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
/// channel seeds with the theme's current [`cyrup_resources::ThemeData`], so the run loop's
/// `theme_changed` arm fires
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
    // pi's footer reads the OPTIONAL `state.model`: the model cell is
    // `state.model?.id || "no-model"` (footer.ts:169) and the `(provider)` prefix is gated on
    // `state.model` being present (footer.ts:192-193). A modelless session (SEAM-075) therefore
    // seeds an empty model — which `cyrup_tui::status` already renders as `no-model` (status.rs:394)
    // — and no provider, instead of a fabricated `provider/model` pair.
    let model = session.model();
    let provider = model
        .as_ref()
        .map(|m| m.provider.as_str().to_string())
        .unwrap_or_default();
    let model_id = model
        .as_ref()
        .map(|m| m.model.as_str().to_string())
        .unwrap_or_default();
    let status = app.status_mut();
    status.set_model(match model.as_ref() {
        Some(_) => format!("{provider}/{model_id}"),
        None => String::new(),
    });
    status.set_provider(model.as_ref().map(|_| provider.clone()));

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

/// The lowercase footer/editor string for a [`cyrup_sdk::core::ModelThinkingLevel`] (matches the
/// thinking-selector
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::migrated_credentials_warning;

    /// CFG-051 — the notice that a user's OAuth tokens and API keys were relocated out of
    /// `oauth.json`/`settings.json` into `auth.json`. pi renders it INSIDE the running UI
    /// (`this.showWarning(...)`, interactive-mode.ts:874-876 @v0.83.0, whose copy carries the
    /// `Warning: ` prefix at `:3885-3889`); cyrup wrote it to stderr on the pre-TUI path, one frame
    /// ahead of the paint that erased it. It is now a transcript entry beside the
    /// `modelFallbackMessage` warning, in pi's order (`:874` before `:884`).
    ///
    /// RED before this pass: there was no function — the text was an `eprintln!` in `main`.
    #[test]
    fn the_migrated_credential_notice_is_pis_line_and_is_absent_when_nothing_moved() {
        assert_eq!(migrated_credentials_warning(&[]), None);
        assert_eq!(
            migrated_credentials_warning(&["anthropic".to_string()]).as_deref(),
            Some("Warning: Migrated credentials to auth.json: anthropic")
        );
        // `migratedProviders.join(", ")` — comma-space, and every provider named.
        assert_eq!(
            migrated_credentials_warning(&["anthropic".to_string(), "openai".to_string()])
                .as_deref(),
            Some("Warning: Migrated credentials to auth.json: anthropic, openai")
        );
    }

    #[test]
    fn benchmark_drain_window_matches_pi() {
        // Pi `setTimeout(resolve, 150)` (main.ts:826).
        assert_eq!(super::BENCHMARK_DRAIN_MS, 150);
    }
}
