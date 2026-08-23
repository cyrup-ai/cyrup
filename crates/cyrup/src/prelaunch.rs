//! Pre-launch resolution: **which** session this run attaches to, and under **what** project
//! trust — everything decided before [`cyrup_session_svc::AgentSessionRuntime`] exists.
//!
//! Three concerns that only look separate: pi resolves all of them between `parseArgs` and
//! `createAgentSessionRuntime`, and each can terminate the run on its own (a not-found ref, a
//! declined fork, a cancelled picker). They are grouped here because they share that contract —
//! every entry point answers `Some(exit_code)` for "the run is over" and `None` for "a target was
//! written onto `config`, proceed".
//!
//! The split against the two modules either side of it:
//!
//! * [`crate::session_resolve`] is the pure algebra — prefix matching, the cross-project search,
//!   the listings — with no terminal and no `SessionConfig`;
//! * [`crate::startup_ui`] owns the selector primitives (`run_resume_picker`, `run_trust_prompt`,
//!   `run_missing_cwd_prompt`) and the pure row/label builders;
//! * this module is the orchestration that runs them in pi's order and feeds the results back.

use std::io;
use std::sync::Arc;

use cyrup_config::{ConfigDirs, DefaultProjectTrust, SettingsManager};
use cyrup_session_svc::{AppMode, SessionConfig, SessionTarget};

use crate::cli::Cli;
use crate::session_resolve::{
    Outcome, SessionFlags, gather_session_refs, gather_session_scopes, resolve_session_target,
};
use crate::startup::file_settings_store;

/// Resolve the session target with Pi's full non-interactive depth (Pi `createSessionManager`,
/// main.ts:254-350) and write it onto `config`. Returns `Some(exit_code)` when the resolution itself
/// terminates the run (a not-found ref / id-collision → 1, a declined fork → 0); `None` when it set a
/// target to build. The session listings are scanned only here, behind the caller's ref-present guard.
pub fn resolve_session(
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
        crate::output_guard::emit_stray_line(line);
    }
    for line in &resolution.stderr {
        eprintln!("{line}");
    }

    // Interactive missing-session-cwd Continue/Cancel prompt (Pi `promptForMissingSessionCwd`,
    // main.ts:575-580): a resumed session whose stored cwd is gone is offered a continuation against
    // the current cwd, or cancels to exit 0. The non-interactive arm already errored above.
    if let Some(issue) = resolution.missing_cwd {
        // SEAM-066/067: pi's `createStartupTui` resolves the theme AND installs the user's
        // keybindings before mounting any pre-launch selector (startup-ui.ts:78-83).
        let theme = crate::startup_theme(dirs);
        let (select_keymap, _) = crate::startup_keymaps(dirs);
        let body =
            crate::format_missing_session_cwd_prompt(&issue.session_cwd, &issue.fallback_cwd);
        return match crate::run_missing_cwd_prompt(&theme, &select_keymap, &body, &issue.fallback_cwd)? {
            crate::MissingCwdChoice::Continue => {
                // Reopen the session against the chosen (fallback) cwd (Pi `SessionManager.open(
                // sessionFile, sessionDir, selectedCwd)`, main.ts:578).
                config.target = SessionTarget::Resume(issue.session_file);
                config.cwd_override = Some(issue.fallback_cwd);
                config.persist = !cli.no_session;
                Ok(None)
            }
            // Pi `if (!selectedCwd) process.exit(0)` (main.ts:576-577).
            crate::MissingCwdChoice::Cancel => Ok(Some(0)),
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

/// The plain-stdin fork-into-cwd confirmation (Pi `promptConfirm`, main.ts:191-203): a cooked-mode
/// `[y/N]` readline (NOT the TUI dialog host), run before any terminal takeover. Defaults to `no`.
///
/// The prompt itself routes through the stdout guard: Pi's `promptConfirm` writes it via readline to
/// `process.stdout`, which the stdout takeover redirects to stderr, so under a non-interactive
/// `--mode json`/`--mode rpc` run the `[y/N]` prompt lands on stderr and cannot corrupt the protocol
/// stream on stdout (the answer is still read from stdin).
fn prompt_fork_confirm() -> bool {
    crate::output_guard::emit_stray("Fork this session into current directory? [y/N] ");
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    let answer = line.trim().to_ascii_lowercase();
    answer == "y" || answer == "yes"
}

/// The pre-launch startup-UI orchestration (Pi `cli/startup-ui.ts` + `cli/session-picker.ts` +
/// `cli/project-trust.ts`): run the interactive `--resume` picker and the project-trust prompt over
/// the cyrup-tui selectors and feed their results back into `config` before the runtime is built.
/// Returns `Some(0)` when the resume picker is cancelled (Pi `No session selected` + exit 0), else
/// `None` (proceed). Interactive-only — the caller gates on the mode so the one-shot/RPC live path is
/// never touched. TTY-bound (it drives real terminals), so it is not unit-tested; the row/label/
/// decision builders it composes are unit-tested in [`crate::startup_ui`].
pub fn resolve_startup_ui(
    cli: &Cli,
    dirs: &ConfigDirs,
    mode: AppMode,
    config: &mut SessionConfig,
) -> anyhow::Result<Option<i32>> {
    // SEAM-066/067 — pi's `createStartupTui` does four settings-derived things before mounting
    // anything (startup-ui.ts:77-85); the two that reach these selectors are the resolved theme
    // (`initTheme(resolveThemeSetting(...))`, :79-80) and the user's keybindings
    // (`setKeybindings(KeybindingsManager.create())`, :81).
    let theme = crate::startup_theme(dirs);
    let keymaps = crate::startup_keymaps(dirs);

    // --resume (#1): mount the `SessionSelector` over the merged local+global session listing and
    // resume the chosen session (Pi `selectSession`, session-picker.ts:15-55). A bare `--resume`
    // mapped to `New` in `to_session_config`; the picker resolves the real target here.
    if cli.resume && matches!(config.target, SessionTarget::New) {
        // SEAM-061: the picker takes pi's two loaders, not one merged list — `Tab` swaps between
        // them (`session-picker.ts:15-19` hands `selectSession` both).
        let (current_sessions, all_sessions) = gather_session_scopes(dirs);
        let (choice, status) =
            crate::run_resume_picker(&theme, &keymaps, &current_sessions, &all_sessions, None)?;
        // Pi renders these inside the picker header with a 2 s / 3 s dwell
        // (session-selector.ts:847,851); cyrup's selector has no status channel yet (area 07), so
        // they are printed after the alternate screen is torn down rather than dropped. SEAM-063.
        for line in &status {
            eprintln!("{line}");
        }
        match choice {
            crate::ResumeChoice::Selected(path) => {
                // Pi runs `getMissingSessionCwdIssue(sessionManager, cwd)` UNCONDITIONALLY after
                // `createSessionManager` — which handles `--resume` by returning the opened manager
                // (main.ts:321-332,573-585). So a `--resume`-selected session whose stored cwd is gone
                // must still get the interactive Continue/Cancel prompt, exactly as the
                // `--session`/`--session-id` open paths do via `resolve_session`. The picked session's
                // stored cwd comes from its `SessionInfo` listing (Pi `sessionManager.getCwd()`).
                // Searched across BOTH scopes: with the `Tab` toggle live the chosen row may have
                // come from the all-projects set, and a cross-project session is exactly the one
                // whose stored cwd is most likely to be gone.
                let stored_cwd = current_sessions
                    .iter()
                    .chain(all_sessions.iter())
                    .find(|s| s.path == path)
                    .map(|s| s.cwd.clone())
                    .unwrap_or_default();
                if crate::session_cwd_is_missing(&stored_cwd) {
                    let body =
                        crate::format_missing_session_cwd_prompt(&stored_cwd, &dirs.cwd);
                    match crate::run_missing_cwd_prompt(&theme, &keymaps.0, &body, &dirs.cwd)? {
                        // Reopen the session against the current cwd (Pi `SessionManager.open(
                        // sessionFile, sessionDir, selectedCwd)`, main.ts:580).
                        crate::MissingCwdChoice::Continue => {
                            config.target = SessionTarget::Resume(path);
                            config.cwd_override = Some(dirs.cwd.clone());
                            config.persist = !cli.no_session;
                        }
                        // Pi `if (!selectedCwd) process.exit(0)` (main.ts:577-578).
                        crate::MissingCwdChoice::Cancel => return Ok(Some(0)),
                    }
                } else {
                    config.target = SessionTarget::Resume(path);
                    config.persist = !cli.no_session;
                }
            }
            // Pi `console.log(chalk.dim("No session selected")); process.exit(0)` (main.ts:329).
            crate::ResumeChoice::Cancelled => {
                println!("No session selected");
                return Ok(Some(0));
            }
        }
    }

    // Project trust is NOT resolved here — SEAM-065. pi reaches its prompt from *inside*
    // `createAgentSessionServices`' `resolveProjectTrust` callback (`main.ts:687-706` @v0.83.0), so
    // by then `extensionsResult` exists and `resolveProjectTrusted` can run its tiers in pi's order:
    // `trustOverride` (project-trust.ts:47) → no-trust-requiring-resources (`:50`) →
    // **`emitProjectTrustEvent`** (`:54-70`) → the store (`:72-75`) → the default policy (`:77-84`)
    // → `hasUI` (`:86-88`) → `selectProjectTrustOption` (`:90-94`). Resolving it out here inverted
    // that: an answered prompt became a `trust_override`, which short-circuited the builder's
    // `pre_trust_extension_verdict` and killed the `on-project-trust` hook on the one path it
    // matters. The prompt is now the builder's `trust_prompt` callback (`SessionFactory::
    // trust_prompt`, wired in `run` for the interactive host only), invoked only on
    // `TrustOutcome::NeedsPrompt`.
    let _ = mode;

    Ok(None)
}

/// The bin's half of pi's `resolveProjectTrust` callback (`main.ts:687-706` @v0.83.0): render the
/// pre-launch `TrustSelector` and persist the chosen option, for the builder to invoke *after* the
/// extension `project_trust` verdict and the trust store have had their say. SEAM-065.
///
/// pi supplies this callback only where it has a UI (`hasUI`, project-trust.ts:86-88), which is what
/// the interactive-only wiring in `run` (`main.rs`) reproduces; every other host leaves it unset
/// and the builder falls through to untrusted, exactly as pi's `if (!hasUI) return false;` does.
pub fn trust_prompt_callback(dirs: &ConfigDirs) -> cyrup_session_svc::TrustPromptFn {
    let cwd = dirs.cwd.clone();
    let store = trust_store_for(dirs);
    // SEAM-066/067: the same two settings-derived inputs every other pre-launch selector takes.
    let theme = crate::startup_theme(dirs);
    let (keymap, _) = crate::startup_keymaps(dirs);
    Arc::new(move |options, saved| {
        let (theme, keymap, cwd, store) = (theme.clone(), keymap.clone(), cwd.clone(), store.clone());
        Box::pin(async move {
            match crate::run_trust_prompt(&theme, &keymap, &cwd, options, saved, &store).await {
                Ok(choice) => choice,
                Err(e) => {
                    eprintln!("Error: project trust prompt failed: {e}");
                    None
                }
            }
        })
    })
}

/// The project-trust store the builder reads its saved-decision tier from and the prompt persists
/// into (Pi `getProjectTrustStore()`, project-trust.ts:72/93). SEAM-065.
pub(crate) fn trust_store_for(dirs: &ConfigDirs) -> Arc<cyrup_config::trust::TrustStore> {
    Arc::new(cyrup_config::trust::TrustStore::new(
        dirs.agent_dir.join("trust.json"),
    ))
}

/// The global-only `defaultProjectTrust` policy (Pi `getDefaultProjectTrust`), read from the file
/// settings store with the project scope untrusted (matching the startup settings manager).
///
/// Unused since SEAM-065 moved trust resolution INSIDE the build, where `SessionBuilder` reads the
/// same policy off its own startup settings manager (`builder.rs`, step 1) — which is pi's own
/// arrangement (`resolveProjectTrusted` reads `getDefaultProjectTrust()` at
/// `core/project-trust.ts:77-84`, inside the callback). Kept because it is the bin's only
/// expression of pi's "global scope, project untrusted" read and the next pre-launch consumer will
/// want it verbatim.
#[allow(dead_code)]
fn default_project_trust(dirs: &ConfigDirs) -> DefaultProjectTrust {
    let mgr = SettingsManager::load(file_settings_store(dirs), false);
    mgr.effective().default_project_trust()
}
